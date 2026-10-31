---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for SessionDocumentMachine.

CONSTANTS BooleanValues, DurableHeadRelationValues, DurableTailRecoveryClassValues, DurableTailRunCursorValues, DurableTailStopReasonValues, InstructionDeferReasonValues, InstructionEffectValues, InstructionFoldSourceValues, InstructionLoweringValues, InstructionRunStartValues, InstructionTargetOwnerValues, InstructionWriteOutcomeValues, InstructionWriteRefusalValues, LiveAssistantPlaybackTerminalDispositionValues, LiveAssistantPlaybackTerminalObservationValues, LiveContextCommittedRowDispositionValues, LiveContextCommittedRowKindValues, LiveContextCommittedTextProvenanceValues, LiveSessionAuthorityKindValues, LiveSessionAuthorityReasonValues, LiveTranscriptReconciliationValues, NatValues, ObservedSessionTailKindValues, PendingContinuationDispositionValues, PendingContinuationPublicTerminalValues, RealtimeTranscriptLaneKindValues, RealtimeTranscriptMaterializeDecisionValues, RealtimeTranscriptRoleKindValues, RealtimeTranscriptStopReasonKindValues, RealtimeUserContentBlobFinalizeDispositionValues, RealtimeUserContentBlobRecoveryDispositionValues, RealtimeUserContentBlobStageDispositionValues, RealtimeUserContentIdentityDispositionValues, RecoveryCandidateIdValues, ResumeOverrideRejectionValues, ResumeProviderSelectionValues, ResumeSelfHostedSelectionValues, RunIdCardinalityValues, RuntimeCheckpointProjectionDispositionValues, SessionArchiveDispositionValues, SessionArchiveRuntimeObservationValues, SessionDocumentLifecycleMergeValues, SessionDocumentLifecycleValues, SessionFirstTurnPhaseValues, SessionIdValues, SessionInitialPromptStageDecisionValues, StringValues, TranscriptEditKindValues, InputSamples_ClassifyDurableTail_dangling_tool_use_count, InputSamples_ClassifyDurableTail_orphan_tool_result_count, InputSamples_RecoverSessionInstructionState_target_generation, InputSamples_RecoverSessionInstructionState_entry_sequence, InputSamples_RecoverSessionInstructionEffective_generation, InputSamples_RecoverSessionInstructionEffective_last_entry_sequence, InputSamples_SetSessionInstruction_body_bytes

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

MapSessionIdBoolValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in SessionIdValues, v \in BOOLEAN }
MapSessionIdInstructionTargetOwnerValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in SessionIdValues, v \in InstructionTargetOwnerValues }
MapSessionIdLiveAssistantPlaybackTerminalObservationValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in SessionIdValues, v \in LiveAssistantPlaybackTerminalObservationValues }
MapSessionIdLiveTranscriptReconciliationValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in SessionIdValues, v \in LiveTranscriptReconciliationValues }
MapSessionIdSessionDocumentLifecycleValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in SessionIdValues, v \in SessionDocumentLifecycleValues }
MapSessionIdSessionFirstTurnPhaseValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in SessionIdValues, v \in SessionFirstTurnPhaseValues }
MapSessionIdStringValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in SessionIdValues, v \in StringValues }
MapSessionIdU64Values == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in SessionIdValues, v \in NatValues }
MapStringInstructionEffectValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in StringValues, v \in InstructionEffectValues }
MapStringStringValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in StringValues, v \in StringValues }
MapStringU64Values == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in StringValues, v \in NatValues }
OptionStringValues == {None} \cup {Some(x) : x \in StringValues}
OptionU64Values == {None} \cup {Some(x) : x \in NatValues}
MapSessionIdMapStringInstructionEffectValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in SessionIdValues, v \in MapStringInstructionEffectValues }
MapSessionIdMapStringStringValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in SessionIdValues, v \in MapStringStringValues }
MapSessionIdMapStringU64Values == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in SessionIdValues, v \in MapStringU64Values }

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

VARIABLES phase, model_step_count, session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal, session_live_channel_id, session_live_interaction_id, session_live_transcript_reconciliation, session_live_provisional_transcript_present, session_live_assistant_playback_response_id, session_live_assistant_playback_item_id, session_live_assistant_playback_content_index, session_live_assistant_final_chars, session_live_assistant_final_digest, session_live_assistant_terminal_observation, session_live_assistant_terminal_prefix_chars, session_live_assistant_terminal_prefix_digest, session_instruction_owner, session_instruction_owner_mob, session_instruction_target_generation, session_instruction_target_revision, session_instruction_target_effect, session_instruction_target_digest, session_instruction_target_bytes, session_instruction_effective_revision, session_instruction_effective_effect, session_instruction_effective_digest, session_instruction_effective_bytes, session_instruction_effective_generation, session_instruction_entry_sequence

vars == << phase, model_step_count, session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal, session_live_channel_id, session_live_interaction_id, session_live_transcript_reconciliation, session_live_provisional_transcript_present, session_live_assistant_playback_response_id, session_live_assistant_playback_item_id, session_live_assistant_playback_content_index, session_live_assistant_final_chars, session_live_assistant_final_digest, session_live_assistant_terminal_observation, session_live_assistant_terminal_prefix_chars, session_live_assistant_terminal_prefix_digest, session_instruction_owner, session_instruction_owner_mob, session_instruction_target_generation, session_instruction_target_revision, session_instruction_target_effect, session_instruction_target_digest, session_instruction_target_bytes, session_instruction_effective_revision, session_instruction_effective_effect, session_instruction_effective_digest, session_instruction_effective_bytes, session_instruction_effective_generation, session_instruction_entry_sequence >>

instruction_entry_lowers(lowering, transcript_is_system_prefix) == (IF (lowering = "MidConversation") THEN TRUE ELSE ((lowering = "LeadingPrefixOnly") /\ transcript_is_system_prefix))
instruction_change_pending(target_revision, target_effect, target_digest, effective_revision, effective_effect, effective_digest) == (IF (\E slot \in DOMAIN target_revision : (IF (((IF "value" \in DOMAIN (IF (slot \in DOMAIN target_effect) THEN Some((IF slot \in DOMAIN target_effect THEN target_effect[slot] ELSE "None")) ELSE None) THEN (IF (slot \in DOMAIN target_effect) THEN Some((IF slot \in DOMAIN target_effect THEN target_effect[slot] ELSE "None")) ELSE None)["value"] ELSE None) = "Active") /\ (IF ((slot \in DOMAIN effective_effect) = FALSE) THEN TRUE ELSE (IF ((IF "value" \in DOMAIN (IF (slot \in DOMAIN effective_effect) THEN Some((IF slot \in DOMAIN effective_effect THEN effective_effect[slot] ELSE "None")) ELSE None) THEN (IF (slot \in DOMAIN effective_effect) THEN Some((IF slot \in DOMAIN effective_effect THEN effective_effect[slot] ELSE "None")) ELSE None)["value"] ELSE None) # "Active") THEN TRUE ELSE (IF ((IF "value" \in DOMAIN (IF (slot \in DOMAIN effective_revision) THEN Some((IF slot \in DOMAIN effective_revision THEN effective_revision[slot] ELSE 0)) ELSE None) THEN (IF (slot \in DOMAIN effective_revision) THEN Some((IF slot \in DOMAIN effective_revision THEN effective_revision[slot] ELSE 0)) ELSE None)["value"] ELSE None) # (IF "value" \in DOMAIN (IF (slot \in DOMAIN target_revision) THEN Some((IF slot \in DOMAIN target_revision THEN target_revision[slot] ELSE 0)) ELSE None) THEN (IF (slot \in DOMAIN target_revision) THEN Some((IF slot \in DOMAIN target_revision THEN target_revision[slot] ELSE 0)) ELSE None)["value"] ELSE None)) THEN TRUE ELSE ((IF "value" \in DOMAIN (IF (slot \in DOMAIN effective_digest) THEN Some((IF slot \in DOMAIN effective_digest THEN effective_digest[slot] ELSE "None")) ELSE None) THEN (IF (slot \in DOMAIN effective_digest) THEN Some((IF slot \in DOMAIN effective_digest THEN effective_digest[slot] ELSE "None")) ELSE None)["value"] ELSE None) # (IF "value" \in DOMAIN (IF (slot \in DOMAIN target_digest) THEN Some((IF slot \in DOMAIN target_digest THEN target_digest[slot] ELSE "None")) ELSE None) THEN (IF (slot \in DOMAIN target_digest) THEN Some((IF slot \in DOMAIN target_digest THEN target_digest[slot] ELSE "None")) ELSE None)["value"] ELSE None)))))) THEN TRUE ELSE (((IF "value" \in DOMAIN (IF (slot \in DOMAIN target_effect) THEN Some((IF slot \in DOMAIN target_effect THEN target_effect[slot] ELSE "None")) ELSE None) THEN (IF (slot \in DOMAIN target_effect) THEN Some((IF slot \in DOMAIN target_effect THEN target_effect[slot] ELSE "None")) ELSE None)["value"] ELSE None) = "Absent") /\ (slot \in DOMAIN effective_effect) /\ ((IF "value" \in DOMAIN (IF (slot \in DOMAIN effective_effect) THEN Some((IF slot \in DOMAIN effective_effect THEN effective_effect[slot] ELSE "None")) ELSE None) THEN (IF (slot \in DOMAIN effective_effect) THEN Some((IF slot \in DOMAIN effective_effect THEN effective_effect[slot] ELSE "None")) ELSE None)["value"] ELSE None) = "Active")))) THEN TRUE ELSE (\E slot \in DOMAIN effective_effect : (((IF "value" \in DOMAIN (IF (slot \in DOMAIN effective_effect) THEN Some((IF slot \in DOMAIN effective_effect THEN effective_effect[slot] ELSE "None")) ELSE None) THEN (IF (slot \in DOMAIN effective_effect) THEN Some((IF slot \in DOMAIN effective_effect THEN effective_effect[slot] ELSE "None")) ELSE None)["value"] ELSE None) = "Active") /\ ((slot \in DOMAIN target_revision) = FALSE))))
instruction_slot_set_empty(revision) == (Cardinality(DOMAIN revision) = 0)
instruction_slot_count_within_limit(revision) == (Cardinality(DOMAIN revision) <= 64)
instruction_slot_maps_consistent(revision, effect, digest, bytes) == ((DOMAIN revision = DOMAIN effect) /\ (DOMAIN revision = DOMAIN digest) /\ (DOMAIN revision = DOMAIN bytes) /\ (\A slot \in DOMAIN revision : (((IF "value" \in DOMAIN (IF (slot \in DOMAIN revision) THEN Some((IF slot \in DOMAIN revision THEN revision[slot] ELSE 0)) ELSE None) THEN (IF (slot \in DOMAIN revision) THEN Some((IF slot \in DOMAIN revision THEN revision[slot] ELSE 0)) ELSE None)["value"] ELSE None) >= 1) /\ (IF (((IF "value" \in DOMAIN (IF (slot \in DOMAIN effect) THEN Some((IF slot \in DOMAIN effect THEN effect[slot] ELSE "None")) ELSE None) THEN (IF (slot \in DOMAIN effect) THEN Some((IF slot \in DOMAIN effect THEN effect[slot] ELSE "None")) ELSE None)["value"] ELSE None) = "Active") /\ ((IF "value" \in DOMAIN (IF (slot \in DOMAIN digest) THEN Some((IF slot \in DOMAIN digest THEN digest[slot] ELSE "None")) ELSE None) THEN (IF (slot \in DOMAIN digest) THEN Some((IF slot \in DOMAIN digest THEN digest[slot] ELSE "None")) ELSE None)["value"] ELSE None) # "")) THEN TRUE ELSE (((IF "value" \in DOMAIN (IF (slot \in DOMAIN effect) THEN Some((IF slot \in DOMAIN effect THEN effect[slot] ELSE "None")) ELSE None) THEN (IF (slot \in DOMAIN effect) THEN Some((IF slot \in DOMAIN effect THEN effect[slot] ELSE "None")) ELSE None)["value"] ELSE None) = "Absent") /\ ((IF "value" \in DOMAIN (IF (slot \in DOMAIN digest) THEN Some((IF slot \in DOMAIN digest THEN digest[slot] ELSE "None")) ELSE None) THEN (IF (slot \in DOMAIN digest) THEN Some((IF slot \in DOMAIN digest THEN digest[slot] ELSE "None")) ELSE None)["value"] ELSE None) = "") /\ ((IF "value" \in DOMAIN (IF (slot \in DOMAIN bytes) THEN Some((IF slot \in DOMAIN bytes THEN bytes[slot] ELSE 0)) ELSE None) THEN (IF (slot \in DOMAIN bytes) THEN Some((IF slot \in DOMAIN bytes THEN bytes[slot] ELSE 0)) ELSE None)["value"] ELSE None) = 0))))))
archive_should_retire_runtime(runtime_backed, durable_document_present, runtime_observation) == (runtime_backed /\ (runtime_observation # "QuiescentTerminal") /\ (IF durable_document_present THEN TRUE ELSE (runtime_observation = "RetirementRequired")))
store_projection_can_recover_authority(has_metadata, has_build_state, runtime_projection_quarantined) == (IF has_metadata THEN TRUE ELSE (IF has_build_state THEN TRUE ELSE runtime_projection_quarantined))
resume_provider_recompute_from_model(model_override_present, provider_override_present) == (model_override_present /\ (provider_override_present = FALSE))
resume_reject_provider_requires_model(provider_override_present, model_override_present) == (provider_override_present /\ (model_override_present = FALSE))
tail_has_pending_boundary(session_tail) == (IF (session_tail = "User") THEN TRUE ELSE (session_tail = "ToolResults"))
realtime_stop_reason_records_completion(stop_reason) == (stop_reason = "Other")
realtime_stop_reason_removes_completion(stop_reason) == (stop_reason = "ToolUse")
realtime_stop_reason_discards(stop_reason) == (IF (stop_reason = "Cancelled") THEN TRUE ELSE (stop_reason = "Failed"))
realtime_should_mark_ready_after_write(response_completed, text_after_write_present) == (response_completed /\ text_after_write_present)
realtime_lane_accepts(item_has_text, current_lane, requested_lane) == (IF (current_lane = requested_lane) THEN TRUE ELSE (item_has_text = FALSE))
realtime_delta_is_duplicate(delta_id_present, delta_id_seen) == (delta_id_present /\ delta_id_seen)
should_store_initial_prompt(arg_phase, prompt_has_content) == ((arg_phase = "Pending") /\ prompt_has_content)
phase_allows_initial_turn_overrides(arg_phase) == (arg_phase = "Pending")
resume_reject_build_only_after_first_turn(has_build_only_overrides, first_turn_phase) == (has_build_only_overrides /\ (phase_allows_initial_turn_overrides(first_turn_phase) = FALSE))
has_effective_pending_boundary(session_tail, staged_tool_result_count) == (IF tail_has_pending_boundary(session_tail) THEN TRUE ELSE (staged_tool_result_count > 0))
resume_overrides_admissible(provider_override_present, model_override_present, has_build_only_overrides, first_turn_phase) == ((resume_reject_provider_requires_model(provider_override_present, model_override_present) = FALSE) /\ (resume_reject_build_only_after_first_turn(has_build_only_overrides, first_turn_phase) = FALSE))
session_document_instruction_slot_after_set(current, slot, value) ==
    MapSet(current, slot, value)
session_document_instruction_active_bytes(slot_effect, slot_bytes) ==
    LET RECURSIVE SessionDocumentInstructionActiveBytesOver(_)
        SessionDocumentInstructionActiveBytesOver(instruction_slots) ==
            IF instruction_slots = {} THEN 0
            ELSE LET instruction_slot == CHOOSE candidate_slot \in instruction_slots : TRUE
                 IN (IF slot_effect[instruction_slot] = "Active" /\ instruction_slot \in DOMAIN slot_bytes
                     THEN slot_bytes[instruction_slot] ELSE 0)
                    + SessionDocumentInstructionActiveBytesOver(instruction_slots \ {instruction_slot})
    IN SessionDocumentInstructionActiveBytesOver(DOMAIN slot_effect)
session_document_instruction_active_slots(slot_effect, slot_values) ==
    [instruction_slot \in {candidate_slot \in DOMAIN slot_values :
            candidate_slot \in DOMAIN slot_effect /\ slot_effect[candidate_slot] = "Active"}
        |-> slot_values[instruction_slot]]

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
    /\ session_live_assistant_final_chars = [x \in {} |-> None]
    /\ session_live_assistant_final_digest = [x \in {} |-> None]
    /\ session_live_assistant_terminal_observation = [x \in {} |-> None]
    /\ session_live_assistant_terminal_prefix_chars = [x \in {} |-> None]
    /\ session_live_assistant_terminal_prefix_digest = [x \in {} |-> None]
    /\ session_instruction_owner = [x \in {} |-> None]
    /\ session_instruction_owner_mob = [x \in {} |-> None]
    /\ session_instruction_target_generation = [x \in {} |-> None]
    /\ session_instruction_target_revision = [x \in {} |-> None]
    /\ session_instruction_target_effect = [x \in {} |-> None]
    /\ session_instruction_target_digest = [x \in {} |-> None]
    /\ session_instruction_target_bytes = [x \in {} |-> None]
    /\ session_instruction_effective_revision = [x \in {} |-> None]
    /\ session_instruction_effective_effect = [x \in {} |-> None]
    /\ session_instruction_effective_digest = [x \in {} |-> None]
    /\ session_instruction_effective_bytes = [x \in {} |-> None]
    /\ session_instruction_effective_generation = [x \in {} |-> None]
    /\ session_instruction_entry_sequence = [x \in {} |-> None]

\* Named UNCHANGED frames. One definition per distinct frame; every action
\* that leaves those variables unchanged references the definition by name.
UnchangedFrame_0711ab9608104053 == UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_live_channel_id, session_live_interaction_id, session_live_transcript_reconciliation, session_live_provisional_transcript_present, session_live_assistant_playback_response_id, session_live_assistant_playback_item_id, session_live_assistant_playback_content_index, session_live_assistant_final_chars, session_live_assistant_final_digest, session_live_assistant_terminal_observation, session_live_assistant_terminal_prefix_chars, session_live_assistant_terminal_prefix_digest, session_instruction_owner, session_instruction_owner_mob, session_instruction_target_generation, session_instruction_target_revision, session_instruction_target_effect, session_instruction_target_digest, session_instruction_target_bytes, session_instruction_effective_revision, session_instruction_effective_effect, session_instruction_effective_digest, session_instruction_effective_bytes, session_instruction_effective_generation, session_instruction_entry_sequence >>
UnchangedFrame_138f3e22fdc07ade == UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal, session_live_assistant_final_chars, session_live_assistant_final_digest, session_live_assistant_terminal_observation, session_live_assistant_terminal_prefix_chars, session_live_assistant_terminal_prefix_digest, session_instruction_owner, session_instruction_owner_mob, session_instruction_target_generation, session_instruction_target_revision, session_instruction_target_effect, session_instruction_target_digest, session_instruction_target_bytes, session_instruction_effective_revision, session_instruction_effective_effect, session_instruction_effective_digest, session_instruction_effective_bytes, session_instruction_effective_generation, session_instruction_entry_sequence >>
UnchangedFrame_2999b03e64d8844d == UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal, session_live_channel_id, session_live_interaction_id, session_live_transcript_reconciliation, session_live_provisional_transcript_present, session_live_assistant_playback_response_id, session_live_assistant_playback_item_id, session_live_assistant_playback_content_index, session_live_assistant_final_chars, session_live_assistant_final_digest, session_live_assistant_terminal_observation, session_live_assistant_terminal_prefix_chars, session_live_assistant_terminal_prefix_digest, session_instruction_effective_revision, session_instruction_effective_effect, session_instruction_effective_digest, session_instruction_effective_bytes, session_instruction_effective_generation, session_instruction_entry_sequence >>
UnchangedFrame_2b67ec16fd8d9aca == UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal, session_live_channel_id, session_live_interaction_id, session_live_transcript_reconciliation, session_live_provisional_transcript_present, session_live_assistant_playback_response_id, session_live_assistant_playback_item_id, session_live_assistant_playback_content_index, session_live_assistant_final_chars, session_live_assistant_final_digest, session_live_assistant_terminal_observation, session_live_assistant_terminal_prefix_chars, session_live_assistant_terminal_prefix_digest, session_instruction_owner, session_instruction_owner_mob, session_instruction_target_generation, session_instruction_target_revision, session_instruction_target_effect, session_instruction_target_digest, session_instruction_target_bytes >>
UnchangedFrame_2e043f788505248c == UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal, session_instruction_owner, session_instruction_owner_mob, session_instruction_target_generation, session_instruction_target_revision, session_instruction_target_effect, session_instruction_target_digest, session_instruction_target_bytes, session_instruction_effective_revision, session_instruction_effective_effect, session_instruction_effective_digest, session_instruction_effective_bytes, session_instruction_effective_generation, session_instruction_entry_sequence >>
UnchangedFrame_3a7a9ba51b225cf4 == UNCHANGED << session_lifecycle_terminal, session_live_channel_id, session_live_interaction_id, session_live_transcript_reconciliation, session_live_provisional_transcript_present, session_live_assistant_playback_response_id, session_live_assistant_playback_item_id, session_live_assistant_playback_content_index, session_live_assistant_final_chars, session_live_assistant_final_digest, session_live_assistant_terminal_observation, session_live_assistant_terminal_prefix_chars, session_live_assistant_terminal_prefix_digest, session_instruction_owner, session_instruction_owner_mob, session_instruction_target_generation, session_instruction_target_revision, session_instruction_target_effect, session_instruction_target_digest, session_instruction_target_bytes, session_instruction_effective_revision, session_instruction_effective_effect, session_instruction_effective_digest, session_instruction_effective_bytes, session_instruction_effective_generation, session_instruction_entry_sequence >>
UnchangedFrame_433d7a9f27015ca1 == UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal, session_live_channel_id, session_live_interaction_id, session_live_provisional_transcript_present, session_live_assistant_playback_response_id, session_live_assistant_playback_item_id, session_live_assistant_playback_content_index, session_live_assistant_final_chars, session_live_assistant_final_digest, session_live_assistant_terminal_observation, session_live_assistant_terminal_prefix_chars, session_live_assistant_terminal_prefix_digest, session_instruction_owner, session_instruction_owner_mob, session_instruction_target_generation, session_instruction_target_revision, session_instruction_target_effect, session_instruction_target_digest, session_instruction_target_bytes, session_instruction_effective_revision, session_instruction_effective_effect, session_instruction_effective_digest, session_instruction_effective_bytes, session_instruction_effective_generation, session_instruction_entry_sequence >>
UnchangedFrame_44bcbd08434cac9f == UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal, session_live_channel_id, session_live_interaction_id, session_live_transcript_reconciliation, session_live_provisional_transcript_present, session_live_assistant_playback_response_id, session_live_assistant_playback_item_id, session_live_assistant_playback_content_index, session_live_assistant_final_chars, session_live_assistant_final_digest, session_live_assistant_terminal_observation, session_live_assistant_terminal_prefix_chars, session_live_assistant_terminal_prefix_digest, session_instruction_owner, session_instruction_owner_mob, session_instruction_target_generation, session_instruction_target_revision, session_instruction_target_effect, session_instruction_target_digest, session_instruction_target_bytes, session_instruction_effective_revision, session_instruction_effective_effect, session_instruction_effective_digest, session_instruction_effective_bytes, session_instruction_effective_generation, session_instruction_entry_sequence >>
UnchangedFrame_47174268e34aef7b == UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal, session_live_channel_id, session_live_interaction_id, session_live_transcript_reconciliation, session_live_provisional_transcript_present, session_live_assistant_terminal_observation, session_live_assistant_terminal_prefix_chars, session_live_assistant_terminal_prefix_digest, session_instruction_owner, session_instruction_owner_mob, session_instruction_target_generation, session_instruction_target_revision, session_instruction_target_effect, session_instruction_target_digest, session_instruction_target_bytes, session_instruction_effective_revision, session_instruction_effective_effect, session_instruction_effective_digest, session_instruction_effective_bytes, session_instruction_effective_generation, session_instruction_entry_sequence >>
UnchangedFrame_4f5d9ea655f1ee99 == UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_lifecycle_terminal, session_live_channel_id, session_live_interaction_id, session_live_transcript_reconciliation, session_live_provisional_transcript_present, session_live_assistant_playback_response_id, session_live_assistant_playback_item_id, session_live_assistant_playback_content_index, session_live_assistant_final_chars, session_live_assistant_final_digest, session_live_assistant_terminal_observation, session_live_assistant_terminal_prefix_chars, session_live_assistant_terminal_prefix_digest, session_instruction_owner, session_instruction_owner_mob, session_instruction_target_generation, session_instruction_target_revision, session_instruction_target_effect, session_instruction_target_digest, session_instruction_target_bytes, session_instruction_effective_revision, session_instruction_effective_effect, session_instruction_effective_digest, session_instruction_effective_bytes, session_instruction_effective_generation, session_instruction_entry_sequence >>
UnchangedFrame_596d257c703034bc == UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal, session_live_channel_id, session_live_interaction_id, session_live_transcript_reconciliation, session_live_provisional_transcript_present, session_live_assistant_playback_response_id, session_live_assistant_playback_item_id, session_live_assistant_playback_content_index, session_live_assistant_terminal_observation, session_live_assistant_terminal_prefix_chars, session_live_assistant_terminal_prefix_digest, session_instruction_owner, session_instruction_owner_mob, session_instruction_target_generation, session_instruction_target_revision, session_instruction_target_effect, session_instruction_target_digest, session_instruction_target_bytes, session_instruction_effective_revision, session_instruction_effective_effect, session_instruction_effective_digest, session_instruction_effective_bytes, session_instruction_effective_generation, session_instruction_entry_sequence >>
UnchangedFrame_5e494352ab2e7e45 == UNCHANGED << session_first_turn_phase, session_pending_tool_results_count, session_lifecycle_terminal, session_live_channel_id, session_live_interaction_id, session_live_transcript_reconciliation, session_live_provisional_transcript_present, session_live_assistant_playback_response_id, session_live_assistant_playback_item_id, session_live_assistant_playback_content_index, session_live_assistant_final_chars, session_live_assistant_final_digest, session_live_assistant_terminal_observation, session_live_assistant_terminal_prefix_chars, session_live_assistant_terminal_prefix_digest, session_instruction_owner, session_instruction_owner_mob, session_instruction_target_generation, session_instruction_target_revision, session_instruction_target_effect, session_instruction_target_digest, session_instruction_target_bytes, session_instruction_effective_revision, session_instruction_effective_effect, session_instruction_effective_digest, session_instruction_effective_bytes, session_instruction_effective_generation, session_instruction_entry_sequence >>
UnchangedFrame_69701730652d9513 == UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal, session_live_channel_id, session_live_interaction_id, session_live_transcript_reconciliation, session_live_provisional_transcript_present, session_live_assistant_playback_response_id, session_live_assistant_playback_item_id, session_live_assistant_playback_content_index, session_live_assistant_final_chars, session_live_assistant_final_digest, session_live_assistant_terminal_observation, session_live_assistant_terminal_prefix_chars, session_live_assistant_terminal_prefix_digest, session_instruction_owner_mob, session_instruction_effective_revision, session_instruction_effective_effect, session_instruction_effective_digest, session_instruction_effective_bytes, session_instruction_effective_generation, session_instruction_entry_sequence >>
UnchangedFrame_7fb5fd2297000ba9 == UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal, session_live_channel_id, session_live_interaction_id, session_live_transcript_reconciliation, session_live_provisional_transcript_present, session_live_assistant_final_chars, session_live_assistant_final_digest, session_instruction_owner, session_instruction_owner_mob, session_instruction_target_generation, session_instruction_target_revision, session_instruction_target_effect, session_instruction_target_digest, session_instruction_target_bytes, session_instruction_effective_revision, session_instruction_effective_effect, session_instruction_effective_digest, session_instruction_effective_bytes, session_instruction_effective_generation, session_instruction_entry_sequence >>
UnchangedFrame_8f85d92b50112077 == UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal, session_live_assistant_playback_response_id, session_live_assistant_playback_item_id, session_live_assistant_playback_content_index, session_live_assistant_final_chars, session_live_assistant_final_digest, session_live_assistant_terminal_observation, session_live_assistant_terminal_prefix_chars, session_live_assistant_terminal_prefix_digest, session_instruction_owner, session_instruction_owner_mob, session_instruction_target_generation, session_instruction_target_revision, session_instruction_target_effect, session_instruction_target_digest, session_instruction_target_bytes, session_instruction_effective_revision, session_instruction_effective_effect, session_instruction_effective_digest, session_instruction_effective_bytes, session_instruction_effective_generation, session_instruction_entry_sequence >>
UnchangedFrame_99c0c7c5a3f3ae36 == UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal, session_live_channel_id, session_live_interaction_id, session_live_transcript_reconciliation, session_live_provisional_transcript_present, session_live_assistant_final_chars, session_live_assistant_final_digest, session_live_assistant_terminal_observation, session_live_assistant_terminal_prefix_chars, session_live_assistant_terminal_prefix_digest, session_instruction_owner, session_instruction_owner_mob, session_instruction_target_generation, session_instruction_target_revision, session_instruction_target_effect, session_instruction_target_digest, session_instruction_target_bytes, session_instruction_effective_revision, session_instruction_effective_effect, session_instruction_effective_digest, session_instruction_effective_bytes, session_instruction_effective_generation, session_instruction_entry_sequence >>
UnchangedFrame_a26768aa2b7e30dc == UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal, session_live_channel_id, session_live_interaction_id, session_live_transcript_reconciliation, session_live_provisional_transcript_present, session_live_assistant_playback_response_id, session_live_assistant_playback_item_id, session_live_assistant_playback_content_index, session_live_assistant_final_chars, session_live_assistant_final_digest, session_live_assistant_terminal_observation, session_live_assistant_terminal_prefix_chars, session_live_assistant_terminal_prefix_digest, session_instruction_target_revision, session_instruction_target_effect, session_instruction_target_digest, session_instruction_target_bytes, session_instruction_effective_revision, session_instruction_effective_effect, session_instruction_effective_digest, session_instruction_effective_bytes, session_instruction_effective_generation, session_instruction_entry_sequence >>
UnchangedFrame_a6146a1f3b9f1b60 == UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal, session_live_channel_id, session_live_interaction_id, session_live_transcript_reconciliation, session_live_provisional_transcript_present, session_live_assistant_playback_response_id, session_live_assistant_playback_item_id, session_live_assistant_playback_content_index, session_live_assistant_final_chars, session_live_assistant_final_digest, session_live_assistant_terminal_observation, session_live_assistant_terminal_prefix_chars, session_live_assistant_terminal_prefix_digest, session_instruction_owner, session_instruction_owner_mob, session_instruction_target_generation, session_instruction_effective_revision, session_instruction_effective_effect, session_instruction_effective_digest, session_instruction_effective_bytes, session_instruction_effective_generation, session_instruction_entry_sequence >>
UnchangedFrame_afcdf325669eb17c == UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal, session_live_channel_id, session_live_interaction_id, session_live_transcript_reconciliation, session_live_provisional_transcript_present, session_live_assistant_playback_response_id, session_live_assistant_playback_item_id, session_live_assistant_playback_content_index, session_live_assistant_final_chars, session_live_assistant_final_digest, session_live_assistant_terminal_observation, session_live_assistant_terminal_prefix_chars, session_live_assistant_terminal_prefix_digest >>
UnchangedFrame_c525efbe4aa2156c == UNCHANGED << session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal, session_live_channel_id, session_live_interaction_id, session_live_transcript_reconciliation, session_live_provisional_transcript_present, session_live_assistant_playback_response_id, session_live_assistant_playback_item_id, session_live_assistant_playback_content_index, session_live_assistant_final_chars, session_live_assistant_final_digest, session_live_assistant_terminal_observation, session_live_assistant_terminal_prefix_chars, session_live_assistant_terminal_prefix_digest, session_instruction_owner, session_instruction_owner_mob, session_instruction_target_generation, session_instruction_target_revision, session_instruction_target_effect, session_instruction_target_digest, session_instruction_target_bytes, session_instruction_effective_revision, session_instruction_effective_effect, session_instruction_effective_digest, session_instruction_effective_bytes, session_instruction_effective_generation, session_instruction_entry_sequence >>
UnchangedFrame_efa9360e0c81d6cf == UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal, session_live_channel_id, session_live_interaction_id, session_live_transcript_reconciliation, session_live_provisional_transcript_present, session_live_assistant_playback_response_id, session_live_assistant_playback_item_id, session_live_assistant_playback_content_index, session_live_assistant_final_chars, session_live_assistant_final_digest, session_live_assistant_terminal_observation, session_live_assistant_terminal_prefix_chars, session_live_assistant_terminal_prefix_digest, session_instruction_owner, session_instruction_owner_mob, session_instruction_target_generation, session_instruction_target_revision, session_instruction_target_effect, session_instruction_target_digest, session_instruction_target_bytes, session_instruction_effective_revision, session_instruction_effective_effect, session_instruction_effective_digest, session_instruction_effective_bytes, session_instruction_effective_generation >>
UnchangedFrame_efd8609eaa54492a == UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal, session_live_channel_id, session_live_interaction_id, session_live_transcript_reconciliation, session_live_assistant_playback_response_id, session_live_assistant_playback_item_id, session_live_assistant_playback_content_index, session_live_assistant_final_chars, session_live_assistant_final_digest, session_live_assistant_terminal_observation, session_live_assistant_terminal_prefix_chars, session_live_assistant_terminal_prefix_digest, session_instruction_owner, session_instruction_owner_mob, session_instruction_target_generation, session_instruction_target_revision, session_instruction_target_effect, session_instruction_target_digest, session_instruction_target_bytes, session_instruction_effective_revision, session_instruction_effective_effect, session_instruction_effective_digest, session_instruction_effective_bytes, session_instruction_effective_generation, session_instruction_entry_sequence >>
UnchangedFrame_f76c28a808303d50 == UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal, session_live_channel_id, session_live_interaction_id, session_live_transcript_reconciliation, session_live_provisional_transcript_present, session_live_assistant_playback_response_id, session_live_assistant_playback_item_id, session_live_assistant_playback_content_index, session_live_assistant_final_chars, session_live_assistant_final_digest, session_instruction_owner, session_instruction_owner_mob, session_instruction_target_generation, session_instruction_target_revision, session_instruction_target_effect, session_instruction_target_digest, session_instruction_target_bytes, session_instruction_effective_revision, session_instruction_effective_effect, session_instruction_effective_digest, session_instruction_effective_bytes, session_instruction_effective_generation, session_instruction_entry_sequence >>
UnchangedFrame_ff1eba536507aee7 == UNCHANGED << session_first_turn_phase, session_lifecycle_terminal, session_live_channel_id, session_live_interaction_id, session_live_transcript_reconciliation, session_live_provisional_transcript_present, session_live_assistant_playback_response_id, session_live_assistant_playback_item_id, session_live_assistant_playback_content_index, session_live_assistant_final_chars, session_live_assistant_final_digest, session_live_assistant_terminal_observation, session_live_assistant_terminal_prefix_chars, session_live_assistant_terminal_prefix_digest, session_instruction_owner, session_instruction_owner_mob, session_instruction_target_generation, session_instruction_target_revision, session_instruction_target_effect, session_instruction_target_digest, session_instruction_target_bytes, session_instruction_effective_revision, session_instruction_effective_effect, session_instruction_effective_digest, session_instruction_effective_bytes, session_instruction_effective_generation, session_instruction_entry_sequence >>

MarkSessionInitialTurnPendingInactiveOrPending(session_id) ==
    /\ phase = "Ready"
    /\ (((session_id \in DOMAIN session_first_turn_phase) /\ (IF ((session_first_turn_phase)[session_id] = "Inactive") THEN TRUE ELSE (session_id \in DOMAIN session_first_turn_phase))) /\ (IF ((session_first_turn_phase)[session_id] = "Inactive") THEN TRUE ELSE ((session_first_turn_phase)[session_id] = "Pending")))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_first_turn_phase' = MapSet(session_first_turn_phase, session_id, "Pending")
    /\ UnchangedFrame_c525efbe4aa2156c


MarkSessionInitialTurnPendingConsumed(session_id) ==
    /\ phase = "Ready"
    /\ ((session_id \in DOMAIN session_first_turn_phase) /\ ((session_first_turn_phase)[session_id] = "Consumed"))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


StartSessionInitialTurnPending(session_id) ==
    /\ phase = "Ready"
    /\ ((session_id \in DOMAIN session_first_turn_phase) /\ ((session_first_turn_phase)[session_id] = "Pending"))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_first_turn_phase' = MapSet(session_first_turn_phase, session_id, "Consumed")
    /\ UnchangedFrame_c525efbe4aa2156c


StartSessionInitialTurnInactive(session_id) ==
    /\ phase = "Ready"
    /\ ((session_id \in DOMAIN session_first_turn_phase) /\ ((session_first_turn_phase)[session_id] = "Inactive"))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


StartSessionInitialTurnConsumed(session_id) ==
    /\ phase = "Ready"
    /\ ((session_id \in DOMAIN session_first_turn_phase) /\ ((session_first_turn_phase)[session_id] = "Consumed"))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ResolveSessionFirstTurnOverridesAllowed(session_id) ==
    /\ phase = "Ready"
    /\ ((session_id \in DOMAIN session_first_turn_phase) /\ phase_allows_initial_turn_overrides((session_first_turn_phase)[session_id]))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ResolveSessionFirstTurnOverridesDenied(session_id) ==
    /\ phase = "Ready"
    /\ ((session_id \in DOMAIN session_first_turn_phase) /\ (phase_allows_initial_turn_overrides((session_first_turn_phase)[session_id]) = FALSE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


StageSessionInitialPromptStore(session_id, prompt_has_content) ==
    /\ phase = "Ready"
    /\ ((session_id \in DOMAIN session_first_turn_phase) /\ should_store_initial_prompt((session_first_turn_phase)[session_id], prompt_has_content))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_pending_initial_prompt_present' = MapSet(session_pending_initial_prompt_present, session_id, TRUE)
    /\ UnchangedFrame_5e494352ab2e7e45


StageSessionInitialPromptClear(session_id, prompt_has_content) ==
    /\ phase = "Ready"
    /\ ((session_id \in DOMAIN session_first_turn_phase) /\ (should_store_initial_prompt((session_first_turn_phase)[session_id], prompt_has_content) = FALSE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_pending_initial_prompt_present' = MapSet(session_pending_initial_prompt_present, session_id, FALSE)
    /\ UnchangedFrame_5e494352ab2e7e45


StageSessionToolResults(session_id, result_count) ==
    /\ phase = "Ready"
    /\ (((session_id \in DOMAIN session_first_turn_phase) /\ (IF ((session_first_turn_phase)[session_id] = "Inactive") THEN TRUE ELSE ((session_id \in DOMAIN session_first_turn_phase) /\ (IF ((session_first_turn_phase)[session_id] = "Pending") THEN TRUE ELSE (session_id \in DOMAIN session_first_turn_phase))))) /\ (IF ((session_first_turn_phase)[session_id] = "Inactive") THEN TRUE ELSE (IF ((session_first_turn_phase)[session_id] = "Pending") THEN TRUE ELSE ((session_first_turn_phase)[session_id] = "Consumed"))))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_pending_tool_results_count' = MapSet(session_pending_tool_results_count, session_id, result_count)
    /\ UnchangedFrame_4f5d9ea655f1ee99


ConsumeSessionDeferredInputsPending(session_id) ==
    /\ phase = "Ready"
    /\ ((session_id \in DOMAIN session_first_turn_phase) /\ ((session_first_turn_phase)[session_id] = "Pending"))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_first_turn_phase' = MapSet(session_first_turn_phase, session_id, "Consumed")
    /\ session_pending_initial_prompt_present' = MapSet(session_pending_initial_prompt_present, session_id, FALSE)
    /\ session_pending_tool_results_count' = MapSet(session_pending_tool_results_count, session_id, 0)
    /\ UnchangedFrame_3a7a9ba51b225cf4


ConsumeSessionDeferredInputsInactive(session_id) ==
    /\ phase = "Ready"
    /\ ((session_id \in DOMAIN session_first_turn_phase) /\ ((session_first_turn_phase)[session_id] = "Inactive"))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_pending_initial_prompt_present' = MapSet(session_pending_initial_prompt_present, session_id, FALSE)
    /\ session_pending_tool_results_count' = MapSet(session_pending_tool_results_count, session_id, 0)
    /\ UnchangedFrame_ff1eba536507aee7


ConsumeSessionDeferredInputsConsumed(session_id) ==
    /\ phase = "Ready"
    /\ ((session_id \in DOMAIN session_first_turn_phase) /\ ((session_first_turn_phase)[session_id] = "Consumed"))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_pending_initial_prompt_present' = MapSet(session_pending_initial_prompt_present, session_id, FALSE)
    /\ session_pending_tool_results_count' = MapSet(session_pending_tool_results_count, session_id, 0)
    /\ UnchangedFrame_ff1eba536507aee7


RestoreSessionConsumedInputs(session_id, restore_first_turn_pending, pending_initial_prompt_present, pending_tool_result_message_count) ==
    /\ phase = "Ready"
    /\ restore_first_turn_pending
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_first_turn_phase' = MapSet(session_first_turn_phase, session_id, "Pending")
    /\ session_pending_initial_prompt_present' = MapSet(session_pending_initial_prompt_present, session_id, pending_initial_prompt_present)
    /\ session_pending_tool_results_count' = MapSet(session_pending_tool_results_count, session_id, pending_tool_result_message_count)
    /\ UnchangedFrame_3a7a9ba51b225cf4


RestoreSessionConsumedInputsNoPhaseRollback(session_id, restore_first_turn_pending, pending_initial_prompt_present, pending_tool_result_message_count) ==
    /\ phase = "Ready"
    /\ (restore_first_turn_pending = FALSE)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_pending_initial_prompt_present' = MapSet(session_pending_initial_prompt_present, session_id, pending_initial_prompt_present)
    /\ session_pending_tool_results_count' = MapSet(session_pending_tool_results_count, session_id, pending_tool_result_message_count)
    /\ UnchangedFrame_ff1eba536507aee7


RecoverSessionFirstTurnPhase(session_id, arg_phase, pending_initial_prompt_present, pending_tool_result_message_count) ==
    /\ phase = "Ready"
    /\ (IF (arg_phase = "Inactive") THEN TRUE ELSE (IF (arg_phase = "Pending") THEN TRUE ELSE (arg_phase = "Consumed")))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_first_turn_phase' = MapSet(session_first_turn_phase, session_id, arg_phase)
    /\ session_pending_initial_prompt_present' = MapSet(session_pending_initial_prompt_present, session_id, pending_initial_prompt_present)
    /\ session_pending_tool_results_count' = MapSet(session_pending_tool_results_count, session_id, pending_tool_result_message_count)
    /\ UnchangedFrame_3a7a9ba51b225cf4


ResolveRealtimeItemObservedDiscardedAssistant(role, response_discarded) ==
    /\ phase = "Ready"
    /\ ((role = "Assistant") /\ response_discarded)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ResolveRealtimeItemObservedPresent(role, response_discarded) ==
    /\ phase = "Ready"
    /\ (IF (role # "Assistant") THEN TRUE ELSE (response_discarded = FALSE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ResolveRealtimeItemSkipped ==
    /\ phase = "Ready"
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ResolveRealtimeUserTranscriptFinalEmpty(text_present, segment_empty, segment_matches) ==
    /\ phase = "Ready"
    /\ (text_present = FALSE)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ResolveRealtimeUserTranscriptFinalStore(text_present, segment_empty, segment_matches) ==
    /\ phase = "Ready"
    /\ (text_present /\ segment_empty)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ResolveRealtimeUserTranscriptFinalReplayOrConflict(text_present, segment_empty, segment_matches) ==
    /\ phase = "Ready"
    /\ (text_present /\ (segment_empty = FALSE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ResolveRealtimeUserContentIdentityInvalid(identity_fields_valid, key_tombstoned, predecessor_materialized, existing_identity_present, existing_payload_matches, target_item_id_available, reducer_commit_proof_required, reducer_commit_proof_present) ==
    /\ phase = "Ready"
    /\ (IF (identity_fields_valid = FALSE) THEN TRUE ELSE ((key_tombstoned = FALSE) /\ predecessor_materialized /\ (existing_identity_present = FALSE) /\ target_item_id_available /\ reducer_commit_proof_required /\ (reducer_commit_proof_present = FALSE)))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ResolveRealtimeUserContentIdentityUnmaterializedPredecessor(identity_fields_valid, key_tombstoned, predecessor_materialized, existing_identity_present, existing_payload_matches, target_item_id_available, reducer_commit_proof_required, reducer_commit_proof_present) ==
    /\ phase = "Ready"
    /\ (identity_fields_valid /\ (key_tombstoned = FALSE) /\ (predecessor_materialized = FALSE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ResolveRealtimeUserContentIdentityConflict(identity_fields_valid, key_tombstoned, predecessor_materialized, existing_identity_present, existing_payload_matches, target_item_id_available, reducer_commit_proof_required, reducer_commit_proof_present) ==
    /\ phase = "Ready"
    /\ (identity_fields_valid /\ (IF key_tombstoned THEN TRUE ELSE (predecessor_materialized /\ (IF (existing_identity_present /\ (existing_payload_matches = FALSE)) THEN TRUE ELSE ((existing_identity_present = FALSE) /\ (target_item_id_available = FALSE))))))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ResolveRealtimeUserContentIdentityReplay(identity_fields_valid, key_tombstoned, predecessor_materialized, existing_identity_present, existing_payload_matches, target_item_id_available, reducer_commit_proof_required, reducer_commit_proof_present) ==
    /\ phase = "Ready"
    /\ (identity_fields_valid /\ (key_tombstoned = FALSE) /\ predecessor_materialized /\ existing_identity_present /\ existing_payload_matches)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ResolveRealtimeUserContentIdentityCommitNew(identity_fields_valid, key_tombstoned, predecessor_materialized, existing_identity_present, existing_payload_matches, target_item_id_available, reducer_commit_proof_required, reducer_commit_proof_present) ==
    /\ phase = "Ready"
    /\ (identity_fields_valid /\ (key_tombstoned = FALSE) /\ predecessor_materialized /\ (existing_identity_present = FALSE) /\ target_item_id_available /\ (IF (reducer_commit_proof_required = FALSE) THEN TRUE ELSE reducer_commit_proof_present))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ResolveRealtimeUserContentBlobStageNew(pending_present, pending_matches_request) ==
    /\ phase = "Ready"
    /\ (pending_present = FALSE)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ResolveRealtimeUserContentBlobStageReuseExact(pending_present, pending_matches_request) ==
    /\ phase = "Ready"
    /\ (pending_present /\ pending_matches_request)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ResolveRealtimeUserContentBlobStageRejectOccupied(pending_present, pending_matches_request) ==
    /\ phase = "Ready"
    /\ (pending_present /\ (pending_matches_request = FALSE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ResolveRealtimeUserContentBlobRecoveryNone(pending_present, request_matches_pending, pending_blob_valid) ==
    /\ phase = "Ready"
    /\ (pending_present = FALSE)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ResolveRealtimeUserContentBlobRecoveryExact(pending_present, request_matches_pending, pending_blob_valid) ==
    /\ phase = "Ready"
    /\ (pending_present /\ request_matches_pending)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ResolveRealtimeUserContentBlobRecoveryCommitVerified(pending_present, request_matches_pending, pending_blob_valid) ==
    /\ phase = "Ready"
    /\ (pending_present /\ (request_matches_pending = FALSE) /\ pending_blob_valid)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ResolveRealtimeUserContentBlobRecoveryClearInvalid(pending_present, request_matches_pending, pending_blob_valid) ==
    /\ phase = "Ready"
    /\ (pending_present /\ (request_matches_pending = FALSE) /\ (pending_blob_valid = FALSE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ResolveRealtimeUserContentBlobFinalizeNone(pending_present, pending_matches_committed) ==
    /\ phase = "Ready"
    /\ (pending_present = FALSE)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ResolveRealtimeUserContentBlobFinalizeClearCommitted(pending_present, pending_matches_committed) ==
    /\ phase = "Ready"
    /\ (pending_present /\ pending_matches_committed)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ResolveRealtimeUserContentBlobFinalizeRejectMismatch(pending_present, pending_matches_committed) ==
    /\ phase = "Ready"
    /\ (pending_present /\ (pending_matches_committed = FALSE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ResolveRealtimeUserContentFinalEmpty(content_present, segment_empty, segment_matches) ==
    /\ phase = "Ready"
    /\ (content_present = FALSE)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ResolveRealtimeUserContentFinalStore(content_present, segment_empty, segment_matches) ==
    /\ phase = "Ready"
    /\ (content_present /\ segment_empty)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ResolveRealtimeUserContentFinalReplayOrConflict(content_present, segment_empty, segment_matches) ==
    /\ phase = "Ready"
    /\ (content_present /\ (segment_empty = FALSE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ResolveRealtimeAssistantDeltaInvalidOrDuplicate(response_id_valid, response_discarded, delta_id_present, delta_id_seen, item_has_text, current_lane, requested_lane, response_completed, text_after_write_present) ==
    /\ phase = "Ready"
    /\ (IF (response_id_valid = FALSE) THEN TRUE ELSE realtime_delta_is_duplicate(delta_id_present, delta_id_seen))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ResolveRealtimeAssistantDeltaDiscarded(response_id_valid, response_discarded, delta_id_present, delta_id_seen, item_has_text, current_lane, requested_lane, response_completed, text_after_write_present) ==
    /\ phase = "Ready"
    /\ (response_id_valid /\ response_discarded)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ResolveRealtimeAssistantDeltaLaneConflict(response_id_valid, response_discarded, delta_id_present, delta_id_seen, item_has_text, current_lane, requested_lane, response_completed, text_after_write_present) ==
    /\ phase = "Ready"
    /\ (response_id_valid /\ (response_discarded = FALSE) /\ (realtime_delta_is_duplicate(delta_id_present, delta_id_seen) = FALSE) /\ (realtime_lane_accepts(item_has_text, current_lane, requested_lane) = FALSE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ResolveRealtimeAssistantDeltaAccepted(response_id_valid, response_discarded, delta_id_present, delta_id_seen, item_has_text, current_lane, requested_lane, response_completed, text_after_write_present) ==
    /\ phase = "Ready"
    /\ (response_id_valid /\ (response_discarded = FALSE) /\ (realtime_delta_is_duplicate(delta_id_present, delta_id_seen) = FALSE) /\ realtime_lane_accepts(item_has_text, current_lane, requested_lane))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ResolveRealtimeAssistantReplacementInvalid(response_id_valid, response_discarded, item_materialized, item_has_text, current_lane, requested_lane, response_completed, text_after_replace_present) ==
    /\ phase = "Ready"
    /\ (response_id_valid = FALSE)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ResolveRealtimeAssistantReplacementDiscarded(response_id_valid, response_discarded, item_materialized, item_has_text, current_lane, requested_lane, response_completed, text_after_replace_present) ==
    /\ phase = "Ready"
    /\ (response_id_valid /\ response_discarded)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ResolveRealtimeAssistantReplacementLocked(response_id_valid, response_discarded, item_materialized, item_has_text, current_lane, requested_lane, response_completed, text_after_replace_present) ==
    /\ phase = "Ready"
    /\ (response_id_valid /\ (response_discarded = FALSE) /\ item_materialized)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ResolveRealtimeAssistantReplacementLaneConflict(response_id_valid, response_discarded, item_materialized, item_has_text, current_lane, requested_lane, response_completed, text_after_replace_present) ==
    /\ phase = "Ready"
    /\ (response_id_valid /\ (response_discarded = FALSE) /\ (item_materialized = FALSE) /\ (realtime_lane_accepts(item_has_text, current_lane, requested_lane) = FALSE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ResolveRealtimeAssistantReplacementAccepted(response_id_valid, response_discarded, item_materialized, item_has_text, current_lane, requested_lane, response_completed, text_after_replace_present) ==
    /\ phase = "Ready"
    /\ (response_id_valid /\ (response_discarded = FALSE) /\ (item_materialized = FALSE) /\ realtime_lane_accepts(item_has_text, current_lane, requested_lane))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ResolveRealtimeAssistantTurnCompletedInvalid(response_id_valid, response_discarded, stop_reason) ==
    /\ phase = "Ready"
    /\ (response_id_valid = FALSE)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ResolveRealtimeAssistantTurnCompletedDiscard(response_id_valid, response_discarded, stop_reason) ==
    /\ phase = "Ready"
    /\ (response_id_valid /\ (IF response_discarded THEN TRUE ELSE realtime_stop_reason_discards(stop_reason)))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ResolveRealtimeAssistantTurnCompletedToolUse(response_id_valid, response_discarded, stop_reason) ==
    /\ phase = "Ready"
    /\ (response_id_valid /\ (response_discarded = FALSE) /\ realtime_stop_reason_removes_completion(stop_reason))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ResolveRealtimeAssistantTurnCompletedRecord(response_id_valid, response_discarded, stop_reason) ==
    /\ phase = "Ready"
    /\ (response_id_valid /\ (response_discarded = FALSE) /\ realtime_stop_reason_records_completion(stop_reason))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ResolveRealtimeAssistantPlaybackSnapshot(target_matches, snapshot_present, response_discarded, item_materialized, requested_lane) ==
    /\ phase = "Ready"
    /\ (target_matches /\ snapshot_present /\ (response_discarded = FALSE) /\ (item_materialized = FALSE) /\ (IF (requested_lane = "Spoken") THEN TRUE ELSE (requested_lane = "SpokenUnmeasured")))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ResolveRealtimeAssistantTurnInterruptedInvalid(response_id_valid) ==
    /\ phase = "Ready"
    /\ (response_id_valid = FALSE)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ResolveRealtimeAssistantTurnInterruptedValid(response_id_valid) ==
    /\ phase = "Ready"
    /\ response_id_valid
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ResolveRealtimeMaterializeAlreadyDone(item_materialized, predecessor_materialized, item_skipped, item_ready, item_text_present, role, response_id_present, completion_present, completion_usage_consumed) ==
    /\ phase = "Ready"
    /\ item_materialized
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ResolveRealtimeMaterializeWaitForPredecessor(item_materialized, predecessor_materialized, item_skipped, item_ready, item_text_present, role, response_id_present, completion_present, completion_usage_consumed) ==
    /\ phase = "Ready"
    /\ ((item_materialized = FALSE) /\ (predecessor_materialized = FALSE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ResolveRealtimeMaterializeSkipped(item_materialized, predecessor_materialized, item_skipped, item_ready, item_text_present, role, response_id_present, completion_present, completion_usage_consumed) ==
    /\ phase = "Ready"
    /\ ((item_materialized = FALSE) /\ predecessor_materialized /\ item_skipped)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ResolveRealtimeMaterializeWaitForReadyText(item_materialized, predecessor_materialized, item_skipped, item_ready, item_text_present, role, response_id_present, completion_present, completion_usage_consumed) ==
    /\ phase = "Ready"
    /\ ((item_materialized = FALSE) /\ predecessor_materialized /\ (item_skipped = FALSE) /\ (IF (item_ready = FALSE) THEN TRUE ELSE (item_text_present = FALSE)))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ResolveRealtimeMaterializeUser(item_materialized, predecessor_materialized, item_skipped, item_ready, item_text_present, role, response_id_present, completion_present, completion_usage_consumed) ==
    /\ phase = "Ready"
    /\ ((item_materialized = FALSE) /\ predecessor_materialized /\ (item_skipped = FALSE) /\ item_ready /\ item_text_present /\ (role = "User"))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ResolveRealtimeMaterializeAssistant(item_materialized, predecessor_materialized, item_skipped, item_ready, item_text_present, role, response_id_present, completion_present, completion_usage_consumed) ==
    /\ phase = "Ready"
    /\ ((item_materialized = FALSE) /\ predecessor_materialized /\ (item_skipped = FALSE) /\ item_ready /\ item_text_present /\ (role = "Assistant") /\ response_id_present /\ completion_present)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ResolveRealtimeMaterializeAssistantMissingCompletion(item_materialized, predecessor_materialized, item_skipped, item_ready, item_text_present, role, response_id_present, completion_present, completion_usage_consumed) ==
    /\ phase = "Ready"
    /\ ((item_materialized = FALSE) /\ predecessor_materialized /\ (item_skipped = FALSE) /\ item_ready /\ item_text_present /\ (role = "Assistant") /\ (IF (response_id_present = FALSE) THEN TRUE ELSE (completion_present = FALSE)))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


AuthorizeRestoreRealtimeTranscriptState(item_count, first_seen_count, first_seen_unique_count, every_item_has_order_entry, every_order_entry_has_item, all_materialized_predecessor_references_exist, no_self_predecessor_references, causal_graph_acyclic, all_materialized_items_have_materialized_ancestry, all_identity_fields_valid, all_user_content_identity_keys_match, all_user_content_identity_fields_valid, all_user_content_identity_item_ids_unique, all_user_content_identities_reference_materialized_user_items, all_user_content_tombstones_valid, user_content_identities_and_tombstones_disjoint, pending_user_content_blob_fields_valid, pending_user_content_blob_uncommitted, all_delta_ids_valid, all_completion_response_ids_valid, all_discarded_response_ids_valid, all_materialized_items_were_ready_or_skipped, all_assistant_items_have_response_unless_skipped, all_ready_assistant_items_have_completion_or_are_skipped, all_materialized_assistant_completions_consumed, all_completed_assistant_text_items_are_ready_or_materialized_or_skipped, all_discarded_assistant_items_are_skipped_or_materialized) ==
    /\ phase = "Ready"
    /\ ((item_count = first_seen_count) /\ (first_seen_count = first_seen_unique_count) /\ every_item_has_order_entry /\ every_order_entry_has_item /\ all_materialized_predecessor_references_exist /\ no_self_predecessor_references /\ causal_graph_acyclic /\ all_materialized_items_have_materialized_ancestry /\ all_identity_fields_valid /\ all_user_content_identity_keys_match /\ all_user_content_identity_fields_valid /\ all_user_content_identity_item_ids_unique /\ all_user_content_identities_reference_materialized_user_items /\ all_user_content_tombstones_valid /\ user_content_identities_and_tombstones_disjoint /\ pending_user_content_blob_fields_valid /\ pending_user_content_blob_uncommitted /\ all_delta_ids_valid /\ all_completion_response_ids_valid /\ all_discarded_response_ids_valid /\ all_materialized_items_were_ready_or_skipped /\ all_assistant_items_have_response_unless_skipped /\ all_ready_assistant_items_have_completion_or_are_skipped /\ all_materialized_assistant_completions_consumed /\ all_completed_assistant_text_items_are_ready_or_materialized_or_skipped /\ all_discarded_assistant_items_are_skipped_or_materialized)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


AdmitLiveInteractionTranscript(session_id, channel_id, interaction_id) ==
    /\ phase = "Ready"
    /\ ((channel_id # "") /\ (interaction_id # "") /\ ~((session_id \in DOMAIN session_live_interaction_id)))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_live_channel_id' = MapSet(session_live_channel_id, session_id, channel_id)
    /\ session_live_interaction_id' = MapSet(session_live_interaction_id, session_id, interaction_id)
    /\ session_live_transcript_reconciliation' = MapSet(session_live_transcript_reconciliation, session_id, "Provisional")
    /\ session_live_provisional_transcript_present' = MapSet(session_live_provisional_transcript_present, session_id, FALSE)
    /\ UnchangedFrame_8f85d92b50112077


StageLiveProvisionalUserTranscript(session_id, channel_id, interaction_id, provisional_has_content) ==
    /\ phase = "Ready"
    /\ ((provisional_has_content = TRUE) /\ ((IF (session_id \in DOMAIN session_live_channel_id) THEN Some((IF session_id \in DOMAIN session_live_channel_id THEN session_live_channel_id[session_id] ELSE "None")) ELSE None) = Some(channel_id)) /\ ((IF (session_id \in DOMAIN session_live_interaction_id) THEN Some((IF session_id \in DOMAIN session_live_interaction_id THEN session_live_interaction_id[session_id] ELSE "None")) ELSE None) = Some(interaction_id)) /\ ((IF (session_id \in DOMAIN session_live_transcript_reconciliation) THEN Some((IF session_id \in DOMAIN session_live_transcript_reconciliation THEN session_live_transcript_reconciliation[session_id] ELSE "None")) ELSE None) = Some("Provisional")) /\ ((IF (session_id \in DOMAIN session_live_provisional_transcript_present) THEN Some((IF session_id \in DOMAIN session_live_provisional_transcript_present THEN session_live_provisional_transcript_present[session_id] ELSE FALSE)) ELSE None) = Some(FALSE)))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_live_provisional_transcript_present' = MapSet(session_live_provisional_transcript_present, session_id, TRUE)
    /\ UnchangedFrame_efd8609eaa54492a


ReconcileLiveFinalUserTranscript(session_id, channel_id, interaction_id, reconciliation) ==
    /\ phase = "Ready"
    /\ ((IF (reconciliation = "Committed") THEN TRUE ELSE (reconciliation = "Missing")) /\ ((IF (session_id \in DOMAIN session_live_channel_id) THEN Some((IF session_id \in DOMAIN session_live_channel_id THEN session_live_channel_id[session_id] ELSE "None")) ELSE None) = Some(channel_id)) /\ ((IF (session_id \in DOMAIN session_live_interaction_id) THEN Some((IF session_id \in DOMAIN session_live_interaction_id THEN session_live_interaction_id[session_id] ELSE "None")) ELSE None) = Some(interaction_id)) /\ ((IF (session_id \in DOMAIN session_live_transcript_reconciliation) THEN Some((IF session_id \in DOMAIN session_live_transcript_reconciliation THEN session_live_transcript_reconciliation[session_id] ELSE "None")) ELSE None) = Some("Provisional")) /\ (IF ((reconciliation = "Committed") /\ ((IF (session_id \in DOMAIN session_live_provisional_transcript_present) THEN Some((IF session_id \in DOMAIN session_live_provisional_transcript_present THEN session_live_provisional_transcript_present[session_id] ELSE FALSE)) ELSE None) = Some(TRUE))) THEN TRUE ELSE (reconciliation # "Committed")))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_live_transcript_reconciliation' = MapSet(session_live_transcript_reconciliation, session_id, reconciliation)
    /\ UnchangedFrame_433d7a9f27015ca1


CompleteLiveInteractionTranscript(session_id, channel_id, interaction_id) ==
    /\ phase = "Ready"
    /\ (((IF (session_id \in DOMAIN session_live_channel_id) THEN Some((IF session_id \in DOMAIN session_live_channel_id THEN session_live_channel_id[session_id] ELSE "None")) ELSE None) = Some(channel_id)) /\ ((IF (session_id \in DOMAIN session_live_interaction_id) THEN Some((IF session_id \in DOMAIN session_live_interaction_id THEN session_live_interaction_id[session_id] ELSE "None")) ELSE None) = Some(interaction_id)) /\ (IF ((IF (session_id \in DOMAIN session_live_transcript_reconciliation) THEN Some((IF session_id \in DOMAIN session_live_transcript_reconciliation THEN session_live_transcript_reconciliation[session_id] ELSE "None")) ELSE None) # Some("Provisional")) THEN TRUE ELSE ((IF (session_id \in DOMAIN session_live_provisional_transcript_present) THEN Some((IF session_id \in DOMAIN session_live_provisional_transcript_present THEN session_live_provisional_transcript_present[session_id] ELSE FALSE)) ELSE None) = Some(FALSE))) /\ ~((session_id \in DOMAIN session_live_assistant_playback_response_id)))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_live_channel_id' = MapRemove(session_live_channel_id, session_id)
    /\ session_live_interaction_id' = MapRemove(session_live_interaction_id, session_id)
    /\ session_live_transcript_reconciliation' = MapRemove(session_live_transcript_reconciliation, session_id)
    /\ session_live_provisional_transcript_present' = MapRemove(session_live_provisional_transcript_present, session_id)
    /\ UnchangedFrame_8f85d92b50112077


AdmitLiveAssistantPlaybackTarget(session_id, channel_id, interaction_id, response_id, item_id, content_index) ==
    /\ phase = "Ready"
    /\ ((response_id # "") /\ (item_id # "") /\ ((IF (session_id \in DOMAIN session_live_channel_id) THEN Some((IF session_id \in DOMAIN session_live_channel_id THEN session_live_channel_id[session_id] ELSE "None")) ELSE None) = Some(channel_id)) /\ ((IF (session_id \in DOMAIN session_live_interaction_id) THEN Some((IF session_id \in DOMAIN session_live_interaction_id THEN session_live_interaction_id[session_id] ELSE "None")) ELSE None) = Some(interaction_id)) /\ ~((session_id \in DOMAIN session_live_assistant_playback_response_id)))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_live_assistant_playback_response_id' = MapSet(session_live_assistant_playback_response_id, session_id, response_id)
    /\ session_live_assistant_playback_item_id' = MapSet(session_live_assistant_playback_item_id, session_id, item_id)
    /\ session_live_assistant_playback_content_index' = MapSet(session_live_assistant_playback_content_index, session_id, content_index)
    /\ UnchangedFrame_99c0c7c5a3f3ae36


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
    /\ UnchangedFrame_138f3e22fdc07ade


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
    /\ session_live_assistant_final_chars' = MapRemove(session_live_assistant_final_chars, session_id)
    /\ session_live_assistant_final_digest' = MapRemove(session_live_assistant_final_digest, session_id)
    /\ session_live_assistant_terminal_observation' = MapRemove(session_live_assistant_terminal_observation, session_id)
    /\ session_live_assistant_terminal_prefix_chars' = MapRemove(session_live_assistant_terminal_prefix_chars, session_id)
    /\ session_live_assistant_terminal_prefix_digest' = MapRemove(session_live_assistant_terminal_prefix_digest, session_id)
    /\ UnchangedFrame_2e043f788505248c


ObserveLiveAssistantPlaybackSnapshot(session_id, channel_id, interaction_id, response_id, item_id, content_index, snapshot_chars, snapshot_digest, canonical_chars, canonical_digest, prefix_matches_snapshot, observation_only) ==
    /\ phase = "Ready"
    /\ ((snapshot_chars > 0) /\ (snapshot_digest # "") /\ (IF (observation_only /\ (canonical_chars = 0) /\ (canonical_digest = "") /\ ~(prefix_matches_snapshot)) THEN TRUE ELSE (~(observation_only) /\ prefix_matches_snapshot /\ (canonical_chars <= snapshot_chars) /\ (canonical_digest # ""))) /\ ((IF (session_id \in DOMAIN session_live_channel_id) THEN Some((IF session_id \in DOMAIN session_live_channel_id THEN session_live_channel_id[session_id] ELSE "None")) ELSE None) = Some(channel_id)) /\ ((IF (session_id \in DOMAIN session_live_interaction_id) THEN Some((IF session_id \in DOMAIN session_live_interaction_id THEN session_live_interaction_id[session_id] ELSE "None")) ELSE None) = Some(interaction_id)) /\ ((IF (session_id \in DOMAIN session_live_assistant_playback_response_id) THEN Some((IF session_id \in DOMAIN session_live_assistant_playback_response_id THEN session_live_assistant_playback_response_id[session_id] ELSE "None")) ELSE None) = Some(response_id)) /\ ((IF (session_id \in DOMAIN session_live_assistant_playback_item_id) THEN Some((IF session_id \in DOMAIN session_live_assistant_playback_item_id THEN session_live_assistant_playback_item_id[session_id] ELSE "None")) ELSE None) = Some(item_id)) /\ ((IF (session_id \in DOMAIN session_live_assistant_playback_content_index) THEN Some((IF session_id \in DOMAIN session_live_assistant_playback_content_index THEN session_live_assistant_playback_content_index[session_id] ELSE 0)) ELSE None) = Some(content_index)) /\ ~((session_id \in DOMAIN session_live_assistant_terminal_observation)))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_live_assistant_playback_response_id' = MapRemove(session_live_assistant_playback_response_id, session_id)
    /\ session_live_assistant_playback_item_id' = MapRemove(session_live_assistant_playback_item_id, session_id)
    /\ session_live_assistant_playback_content_index' = MapRemove(session_live_assistant_playback_content_index, session_id)
    /\ session_live_assistant_final_chars' = MapRemove(session_live_assistant_final_chars, session_id)
    /\ session_live_assistant_final_digest' = MapRemove(session_live_assistant_final_digest, session_id)
    /\ UnchangedFrame_47174268e34aef7b


ObserveLiveAssistantPlaybackFinalPendingTerminal(session_id, channel_id, interaction_id, response_id, item_id, content_index, authoritative_assistant_chars, authoritative_text_digest, pending_terminal_observation, pending_reported_prefix_chars, pending_reported_prefix_digest, reported_prefix_matches_authoritative) ==
    /\ phase = "Ready"
    /\ ((authoritative_assistant_chars > 0) /\ (authoritative_text_digest # "") /\ (pending_terminal_observation = "Unmeasured") /\ (pending_reported_prefix_chars = 0) /\ (pending_reported_prefix_digest = "") /\ (reported_prefix_matches_authoritative = FALSE) /\ ((IF (session_id \in DOMAIN session_live_channel_id) THEN Some((IF session_id \in DOMAIN session_live_channel_id THEN session_live_channel_id[session_id] ELSE "None")) ELSE None) = Some(channel_id)) /\ ((IF (session_id \in DOMAIN session_live_interaction_id) THEN Some((IF session_id \in DOMAIN session_live_interaction_id THEN session_live_interaction_id[session_id] ELSE "None")) ELSE None) = Some(interaction_id)) /\ ((IF (session_id \in DOMAIN session_live_assistant_playback_response_id) THEN Some((IF session_id \in DOMAIN session_live_assistant_playback_response_id THEN session_live_assistant_playback_response_id[session_id] ELSE "None")) ELSE None) = Some(response_id)) /\ ((IF (session_id \in DOMAIN session_live_assistant_playback_item_id) THEN Some((IF session_id \in DOMAIN session_live_assistant_playback_item_id THEN session_live_assistant_playback_item_id[session_id] ELSE "None")) ELSE None) = Some(item_id)) /\ ((IF (session_id \in DOMAIN session_live_assistant_playback_content_index) THEN Some((IF session_id \in DOMAIN session_live_assistant_playback_content_index THEN session_live_assistant_playback_content_index[session_id] ELSE 0)) ELSE None) = Some(content_index)) /\ ~((session_id \in DOMAIN session_live_assistant_final_digest)) /\ ~((session_id \in DOMAIN session_live_assistant_terminal_observation)))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_live_assistant_final_chars' = MapSet(session_live_assistant_final_chars, session_id, authoritative_assistant_chars)
    /\ session_live_assistant_final_digest' = MapSet(session_live_assistant_final_digest, session_id, authoritative_text_digest)
    /\ UnchangedFrame_596d257c703034bc


RecoverLiveAssistantPlaybackFinal(session_id, channel_id, interaction_id, response_id, item_id, content_index, authoritative_assistant_chars, authoritative_text_digest) ==
    /\ phase = "Ready"
    /\ ((authoritative_assistant_chars > 0) /\ (authoritative_text_digest # "") /\ ((IF (session_id \in DOMAIN session_live_channel_id) THEN Some((IF session_id \in DOMAIN session_live_channel_id THEN session_live_channel_id[session_id] ELSE "None")) ELSE None) = Some(channel_id)) /\ ((IF (session_id \in DOMAIN session_live_interaction_id) THEN Some((IF session_id \in DOMAIN session_live_interaction_id THEN session_live_interaction_id[session_id] ELSE "None")) ELSE None) = Some(interaction_id)) /\ ((IF (session_id \in DOMAIN session_live_assistant_playback_response_id) THEN Some((IF session_id \in DOMAIN session_live_assistant_playback_response_id THEN session_live_assistant_playback_response_id[session_id] ELSE "None")) ELSE None) = Some(response_id)) /\ ((IF (session_id \in DOMAIN session_live_assistant_playback_item_id) THEN Some((IF session_id \in DOMAIN session_live_assistant_playback_item_id THEN session_live_assistant_playback_item_id[session_id] ELSE "None")) ELSE None) = Some(item_id)) /\ ((IF (session_id \in DOMAIN session_live_assistant_playback_content_index) THEN Some((IF session_id \in DOMAIN session_live_assistant_playback_content_index THEN session_live_assistant_playback_content_index[session_id] ELSE 0)) ELSE None) = Some(content_index)) /\ ~((session_id \in DOMAIN session_live_assistant_final_digest)))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_live_assistant_final_chars' = MapSet(session_live_assistant_final_chars, session_id, authoritative_assistant_chars)
    /\ session_live_assistant_final_digest' = MapSet(session_live_assistant_final_digest, session_id, authoritative_text_digest)
    /\ UnchangedFrame_596d257c703034bc


ObserveLiveAssistantPlaybackTerminalPendingFinal(session_id, channel_id, interaction_id, response_id, item_id, content_index, observation, reported_prefix_chars, reported_prefix_digest, authoritative_assistant_chars, authoritative_text_digest, authoritative_assistant_final, reported_prefix_matches_authoritative) ==
    /\ phase = "Ready"
    /\ ((IF (observation = "PlaybackComplete") THEN TRUE ELSE (observation = "ReportedPrefix")) /\ (IF ((observation = "PlaybackComplete") /\ (reported_prefix_chars = 0) /\ (reported_prefix_digest = "")) THEN TRUE ELSE ((observation = "ReportedPrefix") /\ (reported_prefix_digest # ""))) /\ (reported_prefix_matches_authoritative = FALSE) /\ (authoritative_assistant_chars = 0) /\ (authoritative_text_digest = "") /\ (authoritative_assistant_final = FALSE) /\ ((IF (session_id \in DOMAIN session_live_channel_id) THEN Some((IF session_id \in DOMAIN session_live_channel_id THEN session_live_channel_id[session_id] ELSE "None")) ELSE None) = Some(channel_id)) /\ ((IF (session_id \in DOMAIN session_live_interaction_id) THEN Some((IF session_id \in DOMAIN session_live_interaction_id THEN session_live_interaction_id[session_id] ELSE "None")) ELSE None) = Some(interaction_id)) /\ ((IF (session_id \in DOMAIN session_live_assistant_playback_response_id) THEN Some((IF session_id \in DOMAIN session_live_assistant_playback_response_id THEN session_live_assistant_playback_response_id[session_id] ELSE "None")) ELSE None) = Some(response_id)) /\ ((IF (session_id \in DOMAIN session_live_assistant_playback_item_id) THEN Some((IF session_id \in DOMAIN session_live_assistant_playback_item_id THEN session_live_assistant_playback_item_id[session_id] ELSE "None")) ELSE None) = Some(item_id)) /\ ((IF (session_id \in DOMAIN session_live_assistant_playback_content_index) THEN Some((IF session_id \in DOMAIN session_live_assistant_playback_content_index THEN session_live_assistant_playback_content_index[session_id] ELSE 0)) ELSE None) = Some(content_index)) /\ ~((session_id \in DOMAIN session_live_assistant_final_digest)) /\ ~((session_id \in DOMAIN session_live_assistant_terminal_observation)))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_live_assistant_terminal_observation' = MapSet(session_live_assistant_terminal_observation, session_id, observation)
    /\ session_live_assistant_terminal_prefix_chars' = MapSet(session_live_assistant_terminal_prefix_chars, session_id, reported_prefix_chars)
    /\ session_live_assistant_terminal_prefix_digest' = MapSet(session_live_assistant_terminal_prefix_digest, session_id, reported_prefix_digest)
    /\ UnchangedFrame_f76c28a808303d50


RecoverLiveAssistantPlaybackTerminal(session_id, channel_id, interaction_id, response_id, item_id, content_index, observation, reported_prefix_chars, reported_prefix_digest) ==
    /\ phase = "Ready"
    /\ ((IF (observation = "PlaybackComplete") THEN TRUE ELSE (observation = "ReportedPrefix")) /\ (IF ((observation = "PlaybackComplete") /\ (reported_prefix_chars = 0) /\ (reported_prefix_digest = "")) THEN TRUE ELSE ((observation = "ReportedPrefix") /\ (reported_prefix_digest # ""))) /\ ((IF (session_id \in DOMAIN session_live_channel_id) THEN Some((IF session_id \in DOMAIN session_live_channel_id THEN session_live_channel_id[session_id] ELSE "None")) ELSE None) = Some(channel_id)) /\ ((IF (session_id \in DOMAIN session_live_interaction_id) THEN Some((IF session_id \in DOMAIN session_live_interaction_id THEN session_live_interaction_id[session_id] ELSE "None")) ELSE None) = Some(interaction_id)) /\ ((IF (session_id \in DOMAIN session_live_assistant_playback_response_id) THEN Some((IF session_id \in DOMAIN session_live_assistant_playback_response_id THEN session_live_assistant_playback_response_id[session_id] ELSE "None")) ELSE None) = Some(response_id)) /\ ((IF (session_id \in DOMAIN session_live_assistant_playback_item_id) THEN Some((IF session_id \in DOMAIN session_live_assistant_playback_item_id THEN session_live_assistant_playback_item_id[session_id] ELSE "None")) ELSE None) = Some(item_id)) /\ ((IF (session_id \in DOMAIN session_live_assistant_playback_content_index) THEN Some((IF session_id \in DOMAIN session_live_assistant_playback_content_index THEN session_live_assistant_playback_content_index[session_id] ELSE 0)) ELSE None) = Some(content_index)) /\ ~((session_id \in DOMAIN session_live_assistant_terminal_observation)))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_live_assistant_terminal_observation' = MapSet(session_live_assistant_terminal_observation, session_id, observation)
    /\ session_live_assistant_terminal_prefix_chars' = MapSet(session_live_assistant_terminal_prefix_chars, session_id, reported_prefix_chars)
    /\ session_live_assistant_terminal_prefix_digest' = MapSet(session_live_assistant_terminal_prefix_digest, session_id, reported_prefix_digest)
    /\ UnchangedFrame_f76c28a808303d50


ObserveLiveAssistantPlaybackFinalJoinsComplete(session_id, channel_id, interaction_id, response_id, item_id, content_index, authoritative_assistant_chars, authoritative_text_digest, pending_terminal_observation, pending_reported_prefix_chars, pending_reported_prefix_digest, reported_prefix_matches_authoritative) ==
    /\ phase = "Ready"
    /\ ((authoritative_assistant_chars > 0) /\ (authoritative_text_digest # "") /\ (reported_prefix_matches_authoritative = FALSE) /\ (pending_terminal_observation = "PlaybackComplete") /\ (pending_reported_prefix_chars = 0) /\ (pending_reported_prefix_digest = "") /\ ((IF (session_id \in DOMAIN session_live_assistant_terminal_observation) THEN Some((IF session_id \in DOMAIN session_live_assistant_terminal_observation THEN session_live_assistant_terminal_observation[session_id] ELSE "None")) ELSE None) = Some("PlaybackComplete")) /\ ((IF (session_id \in DOMAIN session_live_channel_id) THEN Some((IF session_id \in DOMAIN session_live_channel_id THEN session_live_channel_id[session_id] ELSE "None")) ELSE None) = Some(channel_id)) /\ ((IF (session_id \in DOMAIN session_live_interaction_id) THEN Some((IF session_id \in DOMAIN session_live_interaction_id THEN session_live_interaction_id[session_id] ELSE "None")) ELSE None) = Some(interaction_id)) /\ ((IF (session_id \in DOMAIN session_live_assistant_playback_response_id) THEN Some((IF session_id \in DOMAIN session_live_assistant_playback_response_id THEN session_live_assistant_playback_response_id[session_id] ELSE "None")) ELSE None) = Some(response_id)) /\ ((IF (session_id \in DOMAIN session_live_assistant_playback_item_id) THEN Some((IF session_id \in DOMAIN session_live_assistant_playback_item_id THEN session_live_assistant_playback_item_id[session_id] ELSE "None")) ELSE None) = Some(item_id)) /\ ((IF (session_id \in DOMAIN session_live_assistant_playback_content_index) THEN Some((IF session_id \in DOMAIN session_live_assistant_playback_content_index THEN session_live_assistant_playback_content_index[session_id] ELSE 0)) ELSE None) = Some(content_index)) /\ ~((session_id \in DOMAIN session_live_assistant_final_digest)))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_live_assistant_playback_response_id' = MapRemove(session_live_assistant_playback_response_id, session_id)
    /\ session_live_assistant_playback_item_id' = MapRemove(session_live_assistant_playback_item_id, session_id)
    /\ session_live_assistant_playback_content_index' = MapRemove(session_live_assistant_playback_content_index, session_id)
    /\ session_live_assistant_terminal_observation' = MapRemove(session_live_assistant_terminal_observation, session_id)
    /\ session_live_assistant_terminal_prefix_chars' = MapRemove(session_live_assistant_terminal_prefix_chars, session_id)
    /\ session_live_assistant_terminal_prefix_digest' = MapRemove(session_live_assistant_terminal_prefix_digest, session_id)
    /\ UnchangedFrame_7fb5fd2297000ba9


ObserveLiveAssistantPlaybackFinalJoinsPrefix(session_id, channel_id, interaction_id, response_id, item_id, content_index, authoritative_assistant_chars, authoritative_text_digest, pending_terminal_observation, pending_reported_prefix_chars, pending_reported_prefix_digest, reported_prefix_matches_authoritative) ==
    /\ phase = "Ready"
    /\ ((authoritative_assistant_chars > 0) /\ (authoritative_text_digest # "") /\ (reported_prefix_matches_authoritative = TRUE) /\ (pending_terminal_observation = "ReportedPrefix") /\ (pending_reported_prefix_digest # "") /\ (pending_reported_prefix_chars <= authoritative_assistant_chars) /\ ((IF (session_id \in DOMAIN session_live_assistant_terminal_observation) THEN Some((IF session_id \in DOMAIN session_live_assistant_terminal_observation THEN session_live_assistant_terminal_observation[session_id] ELSE "None")) ELSE None) = Some("ReportedPrefix")) /\ ((IF (session_id \in DOMAIN session_live_assistant_terminal_prefix_digest) THEN Some((IF session_id \in DOMAIN session_live_assistant_terminal_prefix_digest THEN session_live_assistant_terminal_prefix_digest[session_id] ELSE "None")) ELSE None) = Some(pending_reported_prefix_digest)) /\ ((IF (session_id \in DOMAIN session_live_assistant_terminal_prefix_chars) THEN Some((IF session_id \in DOMAIN session_live_assistant_terminal_prefix_chars THEN session_live_assistant_terminal_prefix_chars[session_id] ELSE 0)) ELSE None) = Some(pending_reported_prefix_chars)) /\ ((IF (session_id \in DOMAIN session_live_channel_id) THEN Some((IF session_id \in DOMAIN session_live_channel_id THEN session_live_channel_id[session_id] ELSE "None")) ELSE None) = Some(channel_id)) /\ ((IF (session_id \in DOMAIN session_live_interaction_id) THEN Some((IF session_id \in DOMAIN session_live_interaction_id THEN session_live_interaction_id[session_id] ELSE "None")) ELSE None) = Some(interaction_id)) /\ ((IF (session_id \in DOMAIN session_live_assistant_playback_response_id) THEN Some((IF session_id \in DOMAIN session_live_assistant_playback_response_id THEN session_live_assistant_playback_response_id[session_id] ELSE "None")) ELSE None) = Some(response_id)) /\ ((IF (session_id \in DOMAIN session_live_assistant_playback_item_id) THEN Some((IF session_id \in DOMAIN session_live_assistant_playback_item_id THEN session_live_assistant_playback_item_id[session_id] ELSE "None")) ELSE None) = Some(item_id)) /\ ((IF (session_id \in DOMAIN session_live_assistant_playback_content_index) THEN Some((IF session_id \in DOMAIN session_live_assistant_playback_content_index THEN session_live_assistant_playback_content_index[session_id] ELSE 0)) ELSE None) = Some(content_index)) /\ ~((session_id \in DOMAIN session_live_assistant_final_digest)))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_live_assistant_playback_response_id' = MapRemove(session_live_assistant_playback_response_id, session_id)
    /\ session_live_assistant_playback_item_id' = MapRemove(session_live_assistant_playback_item_id, session_id)
    /\ session_live_assistant_playback_content_index' = MapRemove(session_live_assistant_playback_content_index, session_id)
    /\ session_live_assistant_terminal_observation' = MapRemove(session_live_assistant_terminal_observation, session_id)
    /\ session_live_assistant_terminal_prefix_chars' = MapRemove(session_live_assistant_terminal_prefix_chars, session_id)
    /\ session_live_assistant_terminal_prefix_digest' = MapRemove(session_live_assistant_terminal_prefix_digest, session_id)
    /\ UnchangedFrame_7fb5fd2297000ba9


ObserveLiveAssistantPlaybackTerminalJoinsComplete(session_id, channel_id, interaction_id, response_id, item_id, content_index, observation, reported_prefix_chars, reported_prefix_digest, authoritative_assistant_chars, authoritative_text_digest, authoritative_assistant_final, reported_prefix_matches_authoritative) ==
    /\ phase = "Ready"
    /\ ((observation = "PlaybackComplete") /\ (reported_prefix_chars = 0) /\ (reported_prefix_digest = "") /\ (reported_prefix_matches_authoritative = FALSE) /\ (authoritative_assistant_final = TRUE) /\ (authoritative_assistant_chars > 0) /\ (authoritative_text_digest # "") /\ ((IF (session_id \in DOMAIN session_live_assistant_final_chars) THEN Some((IF session_id \in DOMAIN session_live_assistant_final_chars THEN session_live_assistant_final_chars[session_id] ELSE 0)) ELSE None) = Some(authoritative_assistant_chars)) /\ ((IF (session_id \in DOMAIN session_live_assistant_final_digest) THEN Some((IF session_id \in DOMAIN session_live_assistant_final_digest THEN session_live_assistant_final_digest[session_id] ELSE "None")) ELSE None) = Some(authoritative_text_digest)) /\ ((IF (session_id \in DOMAIN session_live_channel_id) THEN Some((IF session_id \in DOMAIN session_live_channel_id THEN session_live_channel_id[session_id] ELSE "None")) ELSE None) = Some(channel_id)) /\ ((IF (session_id \in DOMAIN session_live_interaction_id) THEN Some((IF session_id \in DOMAIN session_live_interaction_id THEN session_live_interaction_id[session_id] ELSE "None")) ELSE None) = Some(interaction_id)) /\ ((IF (session_id \in DOMAIN session_live_assistant_playback_response_id) THEN Some((IF session_id \in DOMAIN session_live_assistant_playback_response_id THEN session_live_assistant_playback_response_id[session_id] ELSE "None")) ELSE None) = Some(response_id)) /\ ((IF (session_id \in DOMAIN session_live_assistant_playback_item_id) THEN Some((IF session_id \in DOMAIN session_live_assistant_playback_item_id THEN session_live_assistant_playback_item_id[session_id] ELSE "None")) ELSE None) = Some(item_id)) /\ ((IF (session_id \in DOMAIN session_live_assistant_playback_content_index) THEN Some((IF session_id \in DOMAIN session_live_assistant_playback_content_index THEN session_live_assistant_playback_content_index[session_id] ELSE 0)) ELSE None) = Some(content_index)) /\ ~((session_id \in DOMAIN session_live_assistant_terminal_observation)))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_live_assistant_playback_response_id' = MapRemove(session_live_assistant_playback_response_id, session_id)
    /\ session_live_assistant_playback_item_id' = MapRemove(session_live_assistant_playback_item_id, session_id)
    /\ session_live_assistant_playback_content_index' = MapRemove(session_live_assistant_playback_content_index, session_id)
    /\ session_live_assistant_final_chars' = MapRemove(session_live_assistant_final_chars, session_id)
    /\ session_live_assistant_final_digest' = MapRemove(session_live_assistant_final_digest, session_id)
    /\ UnchangedFrame_47174268e34aef7b


ObserveLiveAssistantPlaybackTerminalJoinsPrefix(session_id, channel_id, interaction_id, response_id, item_id, content_index, observation, reported_prefix_chars, reported_prefix_digest, authoritative_assistant_chars, authoritative_text_digest, authoritative_assistant_final, reported_prefix_matches_authoritative) ==
    /\ phase = "Ready"
    /\ ((observation = "ReportedPrefix") /\ (reported_prefix_digest # "") /\ (reported_prefix_matches_authoritative = TRUE) /\ (authoritative_assistant_final = TRUE) /\ (authoritative_text_digest # "") /\ (reported_prefix_chars <= authoritative_assistant_chars) /\ ((IF (session_id \in DOMAIN session_live_assistant_final_chars) THEN Some((IF session_id \in DOMAIN session_live_assistant_final_chars THEN session_live_assistant_final_chars[session_id] ELSE 0)) ELSE None) = Some(authoritative_assistant_chars)) /\ ((IF (session_id \in DOMAIN session_live_assistant_final_digest) THEN Some((IF session_id \in DOMAIN session_live_assistant_final_digest THEN session_live_assistant_final_digest[session_id] ELSE "None")) ELSE None) = Some(authoritative_text_digest)) /\ ((IF (session_id \in DOMAIN session_live_channel_id) THEN Some((IF session_id \in DOMAIN session_live_channel_id THEN session_live_channel_id[session_id] ELSE "None")) ELSE None) = Some(channel_id)) /\ ((IF (session_id \in DOMAIN session_live_interaction_id) THEN Some((IF session_id \in DOMAIN session_live_interaction_id THEN session_live_interaction_id[session_id] ELSE "None")) ELSE None) = Some(interaction_id)) /\ ((IF (session_id \in DOMAIN session_live_assistant_playback_response_id) THEN Some((IF session_id \in DOMAIN session_live_assistant_playback_response_id THEN session_live_assistant_playback_response_id[session_id] ELSE "None")) ELSE None) = Some(response_id)) /\ ((IF (session_id \in DOMAIN session_live_assistant_playback_item_id) THEN Some((IF session_id \in DOMAIN session_live_assistant_playback_item_id THEN session_live_assistant_playback_item_id[session_id] ELSE "None")) ELSE None) = Some(item_id)) /\ ((IF (session_id \in DOMAIN session_live_assistant_playback_content_index) THEN Some((IF session_id \in DOMAIN session_live_assistant_playback_content_index THEN session_live_assistant_playback_content_index[session_id] ELSE 0)) ELSE None) = Some(content_index)) /\ ~((session_id \in DOMAIN session_live_assistant_terminal_observation)))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_live_assistant_playback_response_id' = MapRemove(session_live_assistant_playback_response_id, session_id)
    /\ session_live_assistant_playback_item_id' = MapRemove(session_live_assistant_playback_item_id, session_id)
    /\ session_live_assistant_playback_content_index' = MapRemove(session_live_assistant_playback_content_index, session_id)
    /\ session_live_assistant_final_chars' = MapRemove(session_live_assistant_final_chars, session_id)
    /\ session_live_assistant_final_digest' = MapRemove(session_live_assistant_final_digest, session_id)
    /\ UnchangedFrame_47174268e34aef7b


ObserveLiveAssistantPlaybackUnmeasured(session_id, channel_id, interaction_id, response_id, item_id, content_index, observation, reported_prefix_chars, reported_prefix_digest, authoritative_assistant_chars, authoritative_text_digest, authoritative_assistant_final, reported_prefix_matches_authoritative) ==
    /\ phase = "Ready"
    /\ ((observation = "Unmeasured") /\ (reported_prefix_chars = 0) /\ (reported_prefix_digest = "") /\ (reported_prefix_matches_authoritative = FALSE) /\ (IF ((authoritative_assistant_final = FALSE) /\ (authoritative_assistant_chars = 0) /\ (authoritative_text_digest = "") /\ ~((session_id \in DOMAIN session_live_assistant_final_digest))) THEN TRUE ELSE ((authoritative_assistant_final = TRUE) /\ (authoritative_assistant_chars > 0) /\ (authoritative_text_digest # "") /\ ((IF (session_id \in DOMAIN session_live_assistant_final_chars) THEN Some((IF session_id \in DOMAIN session_live_assistant_final_chars THEN session_live_assistant_final_chars[session_id] ELSE 0)) ELSE None) = Some(authoritative_assistant_chars)) /\ ((IF (session_id \in DOMAIN session_live_assistant_final_digest) THEN Some((IF session_id \in DOMAIN session_live_assistant_final_digest THEN session_live_assistant_final_digest[session_id] ELSE "None")) ELSE None) = Some(authoritative_text_digest)))) /\ ((IF (session_id \in DOMAIN session_live_channel_id) THEN Some((IF session_id \in DOMAIN session_live_channel_id THEN session_live_channel_id[session_id] ELSE "None")) ELSE None) = Some(channel_id)) /\ ((IF (session_id \in DOMAIN session_live_interaction_id) THEN Some((IF session_id \in DOMAIN session_live_interaction_id THEN session_live_interaction_id[session_id] ELSE "None")) ELSE None) = Some(interaction_id)) /\ ((IF (session_id \in DOMAIN session_live_assistant_playback_response_id) THEN Some((IF session_id \in DOMAIN session_live_assistant_playback_response_id THEN session_live_assistant_playback_response_id[session_id] ELSE "None")) ELSE None) = Some(response_id)) /\ ((IF (session_id \in DOMAIN session_live_assistant_playback_item_id) THEN Some((IF session_id \in DOMAIN session_live_assistant_playback_item_id THEN session_live_assistant_playback_item_id[session_id] ELSE "None")) ELSE None) = Some(item_id)) /\ ((IF (session_id \in DOMAIN session_live_assistant_playback_content_index) THEN Some((IF session_id \in DOMAIN session_live_assistant_playback_content_index THEN session_live_assistant_playback_content_index[session_id] ELSE 0)) ELSE None) = Some(content_index)) /\ ~((session_id \in DOMAIN session_live_assistant_terminal_observation)))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_live_assistant_playback_response_id' = MapRemove(session_live_assistant_playback_response_id, session_id)
    /\ session_live_assistant_playback_item_id' = MapRemove(session_live_assistant_playback_item_id, session_id)
    /\ session_live_assistant_playback_content_index' = MapRemove(session_live_assistant_playback_content_index, session_id)
    /\ session_live_assistant_final_chars' = MapRemove(session_live_assistant_final_chars, session_id)
    /\ session_live_assistant_final_digest' = MapRemove(session_live_assistant_final_digest, session_id)
    /\ UnchangedFrame_47174268e34aef7b


ClassifyLiveContextCommittedRow(session_id, canonical_row_sequence, row_kind, provenance, content_digest, store_commit_authority) ==
    /\ phase = "Ready"
    /\ ((canonical_row_sequence > 0) /\ (content_digest # "") /\ (store_commit_authority # ""))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


AuthorizeSessionMetadataPersist(schema_version, model_present) ==
    /\ phase = "Ready"
    /\ ((schema_version > 0) /\ (model_present = TRUE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


AuthorizeSessionBuildStatePersist(mob_tool_authority_context_present, mob_tool_authority_context_generated) ==
    /\ phase = "Ready"
    /\ (IF (mob_tool_authority_context_present = FALSE) THEN TRUE ELSE (mob_tool_authority_context_generated = TRUE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


RestoreSessionBuildState ==
    /\ phase = "Ready"
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ResolvePendingContinuationWithBoundary(session_tail, staged_tool_result_count) ==
    /\ phase = "Ready"
    /\ has_effective_pending_boundary(session_tail, staged_tool_result_count)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ResolvePendingContinuationWithoutBoundary(session_tail, staged_tool_result_count) ==
    /\ phase = "Ready"
    /\ (has_effective_pending_boundary(session_tail, staged_tool_result_count) = FALSE)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


AuthorizeSessionResumeOverridesRejectProviderRequiresModel(provider_override_present, model_override_present, self_hosted_server_override_present, has_build_only_overrides, first_turn_phase) ==
    /\ phase = "Ready"
    /\ resume_reject_provider_requires_model(provider_override_present, model_override_present)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


AuthorizeSessionResumeOverridesRejectBuildOnlyAfterFirstTurn(provider_override_present, model_override_present, self_hosted_server_override_present, has_build_only_overrides, first_turn_phase) ==
    /\ phase = "Ready"
    /\ ((resume_reject_provider_requires_model(provider_override_present, model_override_present) = FALSE) /\ resume_reject_build_only_after_first_turn(has_build_only_overrides, first_turn_phase))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


AuthorizeSessionResumeOverridesAcceptRecomputeProvider(provider_override_present, model_override_present, self_hosted_server_override_present, has_build_only_overrides, first_turn_phase) ==
    /\ phase = "Ready"
    /\ (resume_overrides_admissible(provider_override_present, model_override_present, has_build_only_overrides, first_turn_phase) /\ resume_provider_recompute_from_model(model_override_present, provider_override_present) /\ (self_hosted_server_override_present = FALSE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


AuthorizeSessionResumeOverridesAcceptRecomputeProviderWithSelfHostedOverride(provider_override_present, model_override_present, self_hosted_server_override_present, has_build_only_overrides, first_turn_phase) ==
    /\ phase = "Ready"
    /\ (resume_overrides_admissible(provider_override_present, model_override_present, has_build_only_overrides, first_turn_phase) /\ resume_provider_recompute_from_model(model_override_present, provider_override_present) /\ self_hosted_server_override_present)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


AuthorizeSessionResumeOverridesAcceptUseOverride(provider_override_present, model_override_present, self_hosted_server_override_present, has_build_only_overrides, first_turn_phase) ==
    /\ phase = "Ready"
    /\ (resume_overrides_admissible(provider_override_present, model_override_present, has_build_only_overrides, first_turn_phase) /\ (resume_provider_recompute_from_model(model_override_present, provider_override_present) = FALSE) /\ provider_override_present /\ (self_hosted_server_override_present = FALSE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


AuthorizeSessionResumeOverridesAcceptUseOverrideWithSelfHostedOverride(provider_override_present, model_override_present, self_hosted_server_override_present, has_build_only_overrides, first_turn_phase) ==
    /\ phase = "Ready"
    /\ (resume_overrides_admissible(provider_override_present, model_override_present, has_build_only_overrides, first_turn_phase) /\ (resume_provider_recompute_from_model(model_override_present, provider_override_present) = FALSE) /\ provider_override_present /\ self_hosted_server_override_present)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


AuthorizeSessionResumeOverridesAcceptRetainStored(provider_override_present, model_override_present, self_hosted_server_override_present, has_build_only_overrides, first_turn_phase) ==
    /\ phase = "Ready"
    /\ (resume_overrides_admissible(provider_override_present, model_override_present, has_build_only_overrides, first_turn_phase) /\ (resume_provider_recompute_from_model(model_override_present, provider_override_present) = FALSE) /\ (provider_override_present = FALSE) /\ (self_hosted_server_override_present = FALSE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


AuthorizeSessionResumeOverridesAcceptRetainStoredWithSelfHostedOverride(provider_override_present, model_override_present, self_hosted_server_override_present, has_build_only_overrides, first_turn_phase) ==
    /\ phase = "Ready"
    /\ (resume_overrides_admissible(provider_override_present, model_override_present, has_build_only_overrides, first_turn_phase) /\ (resume_provider_recompute_from_model(model_override_present, provider_override_present) = FALSE) /\ (provider_override_present = FALSE) /\ self_hosted_server_override_present)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ClassifyLiveSessionAuthorityLive(stored_transcript_diverged, live_has_uncommitted_transcript, stored_is_archived) ==
    /\ phase = "Ready"
    /\ ((stored_transcript_diverged = FALSE) /\ (live_has_uncommitted_transcript = FALSE) /\ (stored_is_archived = FALSE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ClassifyLiveSessionAuthorityDurableArchived(stored_transcript_diverged, live_has_uncommitted_transcript, stored_is_archived) ==
    /\ phase = "Ready"
    /\ (stored_is_archived = TRUE)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ClassifyLiveSessionAuthorityDurableUncommitted(stored_transcript_diverged, live_has_uncommitted_transcript, stored_is_archived) ==
    /\ phase = "Ready"
    /\ ((stored_is_archived = FALSE) /\ (live_has_uncommitted_transcript = TRUE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ClassifyLiveSessionAuthorityDurableRevision(stored_transcript_diverged, live_has_uncommitted_transcript, stored_is_archived) ==
    /\ phase = "Ready"
    /\ ((stored_is_archived = FALSE) /\ (live_has_uncommitted_transcript = FALSE) /\ (stored_transcript_diverged = TRUE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


RecoverSessionFromStoreAuthorized(session_id, has_metadata, has_build_state, runtime_projection_quarantined) ==
    /\ phase = "Ready"
    /\ store_projection_can_recover_authority(has_metadata, has_build_state, runtime_projection_quarantined)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


RecoverSessionFromStoreUnrecoverable(session_id, has_metadata, has_build_state, runtime_projection_quarantined) ==
    /\ phase = "Ready"
    /\ (store_projection_can_recover_authority(has_metadata, has_build_state, runtime_projection_quarantined) = FALSE)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ClassifyDurableTailCompleted(session_id, candidate_id, relation, run_id_cardinality, terminal_stop_reason, dangling_tool_use_count, orphan_tool_result_count, messages_after_terminal, run_cursor, cursor_inputs_open) ==
    /\ phase = "Ready"
    /\ ((IF (relation = "VerifiedStrictDescendant") THEN TRUE ELSE (relation = "VerifiedRewriteDescendant")) /\ (run_id_cardinality = "SingleRunId") /\ (terminal_stop_reason = "EndTurn") /\ (dangling_tool_use_count = 0) /\ (orphan_tool_result_count = 0) /\ (messages_after_terminal = FALSE) /\ (run_cursor = "Absent"))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ClassifyDurableTailRepairable(session_id, candidate_id, relation, run_id_cardinality, terminal_stop_reason, dangling_tool_use_count, orphan_tool_result_count, messages_after_terminal, run_cursor, cursor_inputs_open) ==
    /\ phase = "Ready"
    /\ ((IF (relation = "VerifiedStrictDescendant") THEN TRUE ELSE (relation = "VerifiedRewriteDescendant")) /\ (IF (run_id_cardinality = "SingleRunId") THEN TRUE ELSE ((run_id_cardinality = "NoRunRows") /\ (terminal_stop_reason = "Absent") /\ (IF (relation = "VerifiedStrictDescendant") THEN TRUE ELSE (run_cursor = "Agrees")))) /\ (dangling_tool_use_count = 0) /\ (orphan_tool_result_count = 0) /\ (messages_after_terminal = FALSE) /\ (IF (terminal_stop_reason = "ToolUse") THEN TRUE ELSE (terminal_stop_reason = "Absent")) /\ (IF (run_cursor = "Absent") THEN TRUE ELSE ((run_cursor = "Agrees") /\ (cursor_inputs_open = FALSE))))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ClassifyDurableTailContinuable(session_id, candidate_id, relation, run_id_cardinality, terminal_stop_reason, dangling_tool_use_count, orphan_tool_result_count, messages_after_terminal, run_cursor, cursor_inputs_open) ==
    /\ phase = "Ready"
    /\ ((IF (relation = "VerifiedStrictDescendant") THEN TRUE ELSE (relation = "VerifiedRewriteDescendant")) /\ (IF (run_id_cardinality = "SingleRunId") THEN TRUE ELSE ((run_id_cardinality = "NoRunRows") /\ (terminal_stop_reason = "Absent") /\ (IF (relation = "VerifiedStrictDescendant") THEN TRUE ELSE (run_cursor = "Agrees")))) /\ (dangling_tool_use_count = 0) /\ (orphan_tool_result_count = 0) /\ (messages_after_terminal = FALSE) /\ (IF (terminal_stop_reason = "ToolUse") THEN TRUE ELSE (terminal_stop_reason = "Absent")) /\ (run_cursor = "Agrees") /\ (cursor_inputs_open = TRUE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ClassifyDurableTailAmbiguous(session_id, candidate_id, relation, run_id_cardinality, terminal_stop_reason, dangling_tool_use_count, orphan_tool_result_count, messages_after_terminal, run_cursor, cursor_inputs_open) ==
    /\ phase = "Ready"
    /\ (IF ((relation # "VerifiedStrictDescendant") /\ (relation # "VerifiedRewriteDescendant")) THEN TRUE ELSE (IF ~((IF (run_id_cardinality = "SingleRunId") THEN TRUE ELSE ((run_id_cardinality = "NoRunRows") /\ (terminal_stop_reason = "Absent") /\ (IF (relation = "VerifiedStrictDescendant") THEN TRUE ELSE (run_cursor = "Agrees"))))) THEN TRUE ELSE (IF (orphan_tool_result_count # 0) THEN TRUE ELSE (IF (messages_after_terminal = TRUE) THEN TRUE ELSE (IF (terminal_stop_reason = "Other") THEN TRUE ELSE (IF (dangling_tool_use_count # 0) THEN TRUE ELSE (IF (run_cursor = "Disagrees") THEN TRUE ELSE ((terminal_stop_reason = "EndTurn") /\ (run_cursor # "Absent")))))))))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ResolveRuntimeCheckpointProjectionActive(session_id) ==
    /\ phase = "Ready"
    /\ ((session_id \in DOMAIN session_lifecycle_terminal) /\ ((session_lifecycle_terminal)[session_id] = "Active"))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ResolveRuntimeCheckpointProjectionArchived(session_id) ==
    /\ phase = "Ready"
    /\ ((session_id \in DOMAIN session_lifecycle_terminal) /\ ((session_lifecycle_terminal)[session_id] = "Archived"))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ResolveSessionDocumentLifecycleMergeArchivedAbsorbing(session_id, authority_archived, candidate_archived) ==
    /\ phase = "Ready"
    /\ (IF (authority_archived = TRUE) THEN TRUE ELSE (candidate_archived = TRUE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ResolveSessionDocumentLifecycleMergeAuthority(session_id, authority_archived, candidate_archived) ==
    /\ phase = "Ready"
    /\ ((authority_archived = FALSE) /\ (candidate_archived = FALSE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ApplyPendingToolResults(session_id, result_count) ==
    /\ phase = "Ready"
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


TranscriptEditFork(session_id, fork_or_rewrite_directive) ==
    /\ phase = "Ready"
    /\ (fork_or_rewrite_directive = "Fork")
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


TranscriptEditRewrite(session_id, fork_or_rewrite_directive) ==
    /\ phase = "Ready"
    /\ (fork_or_rewrite_directive = "Rewrite")
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


RecoverSessionLifecycleTerminal(session_id, terminal) ==
    /\ phase = "Ready"
    /\ (IF (terminal = "Active") THEN TRUE ELSE (terminal = "Archived"))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_lifecycle_terminal' = MapSet(session_lifecycle_terminal, session_id, terminal)
    /\ UnchangedFrame_0711ab9608104053


ReviveArchivedSessionDocument(session_id) ==
    /\ phase = "Ready"
    /\ ((session_id \in DOMAIN session_lifecycle_terminal) /\ ((session_lifecycle_terminal)[session_id] = "Archived"))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_lifecycle_terminal' = MapSet(session_lifecycle_terminal, session_id, "Active")
    /\ UnchangedFrame_0711ab9608104053


ArchiveSessionDocumentActive(session_id, runtime_backed, durable_document_present, runtime_observation) ==
    /\ phase = "Ready"
    /\ ((session_id \in DOMAIN session_lifecycle_terminal) /\ ((session_lifecycle_terminal)[session_id] = "Active"))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_lifecycle_terminal' = MapSet(session_lifecycle_terminal, session_id, "Archived")
    /\ UnchangedFrame_0711ab9608104053


ArchiveSessionDocumentAlreadyArchived(session_id, runtime_backed, durable_document_present, runtime_observation) ==
    /\ phase = "Ready"
    /\ ((session_id \in DOMAIN session_lifecycle_terminal) /\ ((session_lifecycle_terminal)[session_id] = "Archived"))
    /\ (runtime_observation # "RetirementRequired")
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ArchiveSessionDocumentCompleteRetire(session_id, runtime_backed, durable_document_present, runtime_observation) ==
    /\ phase = "Ready"
    /\ ((session_id \in DOMAIN session_lifecycle_terminal) /\ ((session_lifecycle_terminal)[session_id] = "Archived"))
    /\ (runtime_observation = "RetirementRequired")
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


RecoverSessionInstructionState(session_id, owner, owner_mob, target_generation, entry_sequence) ==
    /\ phase = "Ready"
    /\ ((IF (owner # "Unowned") THEN TRUE ELSE (target_generation = 0)) /\ (IF ((owner = "Mob") /\ (owner_mob # "")) THEN TRUE ELSE ((owner # "Mob") /\ (owner_mob = ""))))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_instruction_owner' = MapSet(session_instruction_owner, session_id, owner)
    /\ session_instruction_owner_mob' = MapSet(session_instruction_owner_mob, session_id, owner_mob)
    /\ session_instruction_target_generation' = MapSet(session_instruction_target_generation, session_id, target_generation)
    /\ session_instruction_target_revision' = MapSet(session_instruction_target_revision, session_id, [x \in {} |-> None])
    /\ session_instruction_target_effect' = MapSet(session_instruction_target_effect, session_id, [x \in {} |-> None])
    /\ session_instruction_target_digest' = MapSet(session_instruction_target_digest, session_id, [x \in {} |-> None])
    /\ session_instruction_target_bytes' = MapSet(session_instruction_target_bytes, session_id, [x \in {} |-> None])
    /\ session_instruction_effective_revision' = MapSet(session_instruction_effective_revision, session_id, [x \in {} |-> None])
    /\ session_instruction_effective_effect' = MapSet(session_instruction_effective_effect, session_id, [x \in {} |-> None])
    /\ session_instruction_effective_digest' = MapSet(session_instruction_effective_digest, session_id, [x \in {} |-> None])
    /\ session_instruction_effective_bytes' = MapSet(session_instruction_effective_bytes, session_id, [x \in {} |-> None])
    /\ session_instruction_effective_generation' = MapSet(session_instruction_effective_generation, session_id, 0)
    /\ session_instruction_entry_sequence' = MapSet(session_instruction_entry_sequence, session_id, entry_sequence)
    /\ UnchangedFrame_afcdf325669eb17c


RecoverSessionInstructionTarget(session_id, revision, effect, digest, bytes) ==
    /\ phase = "Ready"
    /\ ((session_id \in DOMAIN session_instruction_owner) /\ ((IF ((session_instruction_owner)[session_id] # "Unowned") THEN TRUE ELSE instruction_slot_set_empty(revision)) /\ instruction_slot_maps_consistent(revision, effect, digest, bytes) /\ instruction_slot_count_within_limit(revision) /\ (1048576 >= session_document_instruction_active_bytes(effect, bytes))))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_instruction_target_revision' = MapSet(session_instruction_target_revision, session_id, revision)
    /\ session_instruction_target_effect' = MapSet(session_instruction_target_effect, session_id, effect)
    /\ session_instruction_target_digest' = MapSet(session_instruction_target_digest, session_id, digest)
    /\ session_instruction_target_bytes' = MapSet(session_instruction_target_bytes, session_id, bytes)
    /\ UnchangedFrame_a6146a1f3b9f1b60


RecoverSessionInstructionEffective(session_id, revision, effect, digest, bytes, generation, last_entry_sequence) ==
    /\ phase = "Ready"
    /\ ((IF ~((session_id \in DOMAIN session_instruction_entry_sequence)) THEN TRUE ELSE (IF (last_entry_sequence >= 1) THEN TRUE ELSE (session_id \in DOMAIN session_instruction_entry_sequence))) /\ ((session_id \in DOMAIN session_instruction_entry_sequence) /\ (IF (last_entry_sequence >= 1) THEN TRUE ELSE (IF ((session_instruction_entry_sequence)[session_id] >= 1) THEN TRUE ELSE (\A slot \in DOMAIN effect : ((IF "value" \in DOMAIN (IF (slot \in DOMAIN effect) THEN Some((IF slot \in DOMAIN effect THEN effect[slot] ELSE "None")) ELSE None) THEN (IF (slot \in DOMAIN effect) THEN Some((IF slot \in DOMAIN effect THEN effect[slot] ELSE "None")) ELSE None)["value"] ELSE None) # "Active")))) /\ instruction_slot_maps_consistent(revision, effect, digest, bytes) /\ (\A slot \in DOMAIN effect : ((IF "value" \in DOMAIN (IF (slot \in DOMAIN effect) THEN Some((IF slot \in DOMAIN effect THEN effect[slot] ELSE "None")) ELSE None) THEN (IF (slot \in DOMAIN effect) THEN Some((IF slot \in DOMAIN effect THEN effect[slot] ELSE "None")) ELSE None)["value"] ELSE None) = "Active")) /\ (1048576 >= session_document_instruction_active_bytes(effect, bytes))))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_instruction_effective_revision' = MapSet(session_instruction_effective_revision, session_id, revision)
    /\ session_instruction_effective_effect' = MapSet(session_instruction_effective_effect, session_id, effect)
    /\ session_instruction_effective_digest' = MapSet(session_instruction_effective_digest, session_id, digest)
    /\ session_instruction_effective_bytes' = MapSet(session_instruction_effective_bytes, session_id, bytes)
    /\ session_instruction_effective_generation' = MapSet(session_instruction_effective_generation, session_id, generation)
    /\ session_instruction_entry_sequence' = MapSet(session_instruction_entry_sequence, session_id, (IF (last_entry_sequence > (IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_instruction_entry_sequence) THEN Some((IF session_id \in DOMAIN session_instruction_entry_sequence THEN session_instruction_entry_sequence[session_id] ELSE 0)) ELSE None) THEN (IF (session_id \in DOMAIN session_instruction_entry_sequence) THEN Some((IF session_id \in DOMAIN session_instruction_entry_sequence THEN session_instruction_entry_sequence[session_id] ELSE 0)) ELSE None)["value"] ELSE None)) THEN last_entry_sequence ELSE (IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_instruction_entry_sequence) THEN Some((IF session_id \in DOMAIN session_instruction_entry_sequence THEN session_instruction_entry_sequence[session_id] ELSE 0)) ELSE None) THEN (IF (session_id \in DOMAIN session_instruction_entry_sequence) THEN Some((IF session_id \in DOMAIN session_instruction_entry_sequence THEN session_instruction_entry_sequence[session_id] ELSE 0)) ELSE None)["value"] ELSE None)))
    /\ UnchangedFrame_2b67ec16fd8d9aca


RecordInstructionTargetAdvance(session_id, mob_id, generation, revision, effect, digest, bytes) ==
    /\ phase = "Ready"
    /\ ((IF ~((mob_id # "")) THEN TRUE ELSE (session_id \in DOMAIN session_instruction_target_generation)) /\ ((mob_id # "") /\ (generation > (session_instruction_target_generation)[session_id]) /\ instruction_slot_maps_consistent(revision, effect, digest, bytes) /\ instruction_slot_count_within_limit(revision)))
    /\ (1048576 >= session_document_instruction_active_bytes(effect, bytes))
    /\ (((session_id \in DOMAIN session_instruction_owner) /\ (IF ((session_instruction_owner)[session_id] = "Unowned") THEN TRUE ELSE ((session_id \in DOMAIN session_instruction_owner) /\ (IF ~(((session_instruction_owner)[session_id] = "Mob")) THEN TRUE ELSE (session_id \in DOMAIN session_instruction_owner_mob))))) /\ (IF ((session_instruction_owner)[session_id] = "Unowned") THEN TRUE ELSE (((session_instruction_owner)[session_id] = "Mob") /\ ((session_instruction_owner_mob)[session_id] = mob_id))))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_instruction_owner' = MapSet(session_instruction_owner, session_id, "Mob")
    /\ session_instruction_owner_mob' = MapSet(session_instruction_owner_mob, session_id, mob_id)
    /\ session_instruction_target_generation' = MapSet(session_instruction_target_generation, session_id, generation)
    /\ session_instruction_target_revision' = MapSet(session_instruction_target_revision, session_id, revision)
    /\ session_instruction_target_effect' = MapSet(session_instruction_target_effect, session_id, effect)
    /\ session_instruction_target_digest' = MapSet(session_instruction_target_digest, session_id, digest)
    /\ session_instruction_target_bytes' = MapSet(session_instruction_target_bytes, session_id, bytes)
    /\ UnchangedFrame_2999b03e64d8844d


RecordInstructionTargetActiveBudgetExceeded(session_id, mob_id, generation, revision, effect, digest, bytes) ==
    /\ phase = "Ready"
    /\ ((IF ~((mob_id # "")) THEN TRUE ELSE ((session_id \in DOMAIN session_instruction_target_generation) /\ (IF ~((generation > (session_instruction_target_generation)[session_id])) THEN TRUE ELSE (IF ~(instruction_slot_maps_consistent(revision, effect, digest, bytes)) THEN TRUE ELSE (IF ~(instruction_slot_count_within_limit(revision)) THEN TRUE ELSE (IF ~((session_document_instruction_active_bytes(effect, bytes) > 1048576)) THEN TRUE ELSE ((session_id \in DOMAIN session_instruction_owner) /\ (IF ((session_instruction_owner)[session_id] = "Unowned") THEN TRUE ELSE ((session_id \in DOMAIN session_instruction_owner) /\ (IF ~(((session_instruction_owner)[session_id] = "Mob")) THEN TRUE ELSE (session_id \in DOMAIN session_instruction_owner_mob))))))))))) /\ ((mob_id # "") /\ (generation > (session_instruction_target_generation)[session_id]) /\ instruction_slot_maps_consistent(revision, effect, digest, bytes) /\ instruction_slot_count_within_limit(revision) /\ (session_document_instruction_active_bytes(effect, bytes) > 1048576) /\ (IF ((session_instruction_owner)[session_id] = "Unowned") THEN TRUE ELSE (((session_instruction_owner)[session_id] = "Mob") /\ ((session_instruction_owner_mob)[session_id] = mob_id)))))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


RecordInstructionTargetSlotLimitExceeded(session_id, mob_id, generation, revision, effect, digest, bytes) ==
    /\ phase = "Ready"
    /\ ((IF ~((mob_id # "")) THEN TRUE ELSE ((session_id \in DOMAIN session_instruction_target_generation) /\ (IF ~((generation > (session_instruction_target_generation)[session_id])) THEN TRUE ELSE (IF ~(instruction_slot_maps_consistent(revision, effect, digest, bytes)) THEN TRUE ELSE (IF ~((instruction_slot_count_within_limit(revision) = FALSE)) THEN TRUE ELSE ((session_id \in DOMAIN session_instruction_owner) /\ (IF ((session_instruction_owner)[session_id] = "Unowned") THEN TRUE ELSE ((session_id \in DOMAIN session_instruction_owner) /\ (IF ~(((session_instruction_owner)[session_id] = "Mob")) THEN TRUE ELSE (session_id \in DOMAIN session_instruction_owner_mob)))))))))) /\ ((mob_id # "") /\ (generation > (session_instruction_target_generation)[session_id]) /\ instruction_slot_maps_consistent(revision, effect, digest, bytes) /\ (instruction_slot_count_within_limit(revision) = FALSE) /\ (IF ((session_instruction_owner)[session_id] = "Unowned") THEN TRUE ELSE (((session_instruction_owner)[session_id] = "Mob") /\ ((session_instruction_owner_mob)[session_id] = mob_id)))))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


RecordInstructionTargetAlreadyRecorded(session_id, mob_id, generation, revision, effect, digest, bytes) ==
    /\ phase = "Ready"
    /\ (((session_id \in DOMAIN session_instruction_owner) /\ (IF ~(((session_instruction_owner)[session_id] = "Mob")) THEN TRUE ELSE ((session_id \in DOMAIN session_instruction_owner_mob) /\ (IF ~(((session_instruction_owner_mob)[session_id] = mob_id)) THEN TRUE ELSE (session_id \in DOMAIN session_instruction_target_generation))))) /\ (((session_instruction_owner)[session_id] = "Mob") /\ ((session_instruction_owner_mob)[session_id] = mob_id) /\ (generation <= (session_instruction_target_generation)[session_id])))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


RecordInstructionTargetOwnedBySession(session_id, mob_id, generation, revision, effect, digest, bytes) ==
    /\ phase = "Ready"
    /\ ((session_id \in DOMAIN session_instruction_owner) /\ ((session_instruction_owner)[session_id] = "Session"))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


RecordInstructionTargetOwnedByOtherMob(session_id, mob_id, generation, revision, effect, digest, bytes) ==
    /\ phase = "Ready"
    /\ (((session_id \in DOMAIN session_instruction_owner) /\ (IF ~(((session_instruction_owner)[session_id] = "Mob")) THEN TRUE ELSE (session_id \in DOMAIN session_instruction_owner_mob))) /\ (((session_instruction_owner)[session_id] = "Mob") /\ ((session_instruction_owner_mob)[session_id] # mob_id)))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


SetSessionInstructionNew(session_id, slot, revision, effect, digest, body_bytes) ==
    /\ phase = "Ready"
    /\ (((session_id \in DOMAIN session_instruction_owner) /\ (IF ~(((session_instruction_owner)[session_id] # "Mob")) THEN TRUE ELSE ((session_id \in DOMAIN session_instruction_target_revision) /\ (IF ~(((slot \in DOMAIN (session_instruction_target_revision)[session_id]) = FALSE)) THEN TRUE ELSE (IF ~((effect = "Active")) THEN TRUE ELSE (IF ~((digest # "")) THEN TRUE ELSE (IF ~((revision >= 1)) THEN TRUE ELSE (session_id \in DOMAIN session_instruction_target_revision)))))))) /\ (((session_instruction_owner)[session_id] # "Mob") /\ ((slot \in DOMAIN (session_instruction_target_revision)[session_id]) = FALSE) /\ (effect = "Active") /\ (digest # "") /\ (revision >= 1) /\ (64 > Cardinality(DOMAIN (session_instruction_target_revision)[session_id]))))
    /\ (((session_id \in DOMAIN session_instruction_target_effect) /\ (session_id \in DOMAIN session_instruction_target_bytes)) /\ (1048576 >= session_document_instruction_active_bytes(session_document_instruction_slot_after_set((session_instruction_target_effect)[session_id], slot, effect), session_document_instruction_slot_after_set((session_instruction_target_bytes)[session_id], slot, body_bytes))))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_instruction_owner' = MapSet(session_instruction_owner, session_id, "Session")
    /\ session_instruction_target_generation' = MapSet(session_instruction_target_generation, session_id, ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_instruction_target_generation) THEN Some((IF session_id \in DOMAIN session_instruction_target_generation THEN session_instruction_target_generation[session_id] ELSE 0)) ELSE None) THEN (IF (session_id \in DOMAIN session_instruction_target_generation) THEN Some((IF session_id \in DOMAIN session_instruction_target_generation THEN session_instruction_target_generation[session_id] ELSE 0)) ELSE None)["value"] ELSE None) + 1))
    /\ session_instruction_target_revision' = MapSet(session_instruction_target_revision, session_id, session_document_instruction_slot_after_set((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_instruction_target_revision) THEN Some((IF session_id \in DOMAIN session_instruction_target_revision THEN session_instruction_target_revision[session_id] ELSE [x \in {} |-> None])) ELSE None) THEN (IF (session_id \in DOMAIN session_instruction_target_revision) THEN Some((IF session_id \in DOMAIN session_instruction_target_revision THEN session_instruction_target_revision[session_id] ELSE [x \in {} |-> None])) ELSE None)["value"] ELSE None), slot, revision))
    /\ session_instruction_target_effect' = MapSet(session_instruction_target_effect, session_id, session_document_instruction_slot_after_set((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_instruction_target_effect) THEN Some((IF session_id \in DOMAIN session_instruction_target_effect THEN session_instruction_target_effect[session_id] ELSE [x \in {} |-> None])) ELSE None) THEN (IF (session_id \in DOMAIN session_instruction_target_effect) THEN Some((IF session_id \in DOMAIN session_instruction_target_effect THEN session_instruction_target_effect[session_id] ELSE [x \in {} |-> None])) ELSE None)["value"] ELSE None), slot, effect))
    /\ session_instruction_target_digest' = MapSet(session_instruction_target_digest, session_id, session_document_instruction_slot_after_set((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_instruction_target_digest) THEN Some((IF session_id \in DOMAIN session_instruction_target_digest THEN session_instruction_target_digest[session_id] ELSE [x \in {} |-> None])) ELSE None) THEN (IF (session_id \in DOMAIN session_instruction_target_digest) THEN Some((IF session_id \in DOMAIN session_instruction_target_digest THEN session_instruction_target_digest[session_id] ELSE [x \in {} |-> None])) ELSE None)["value"] ELSE None), slot, digest))
    /\ session_instruction_target_bytes' = MapSet(session_instruction_target_bytes, session_id, session_document_instruction_slot_after_set((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_instruction_target_bytes) THEN Some((IF session_id \in DOMAIN session_instruction_target_bytes THEN session_instruction_target_bytes[session_id] ELSE [x \in {} |-> None])) ELSE None) THEN (IF (session_id \in DOMAIN session_instruction_target_bytes) THEN Some((IF session_id \in DOMAIN session_instruction_target_bytes THEN session_instruction_target_bytes[session_id] ELSE [x \in {} |-> None])) ELSE None)["value"] ELSE None), slot, body_bytes))
    /\ UnchangedFrame_69701730652d9513


SetSessionInstructionAdvance(session_id, slot, revision, effect, digest, body_bytes) ==
    /\ phase = "Ready"
    /\ (((session_id \in DOMAIN session_instruction_owner) /\ (IF ~(((session_instruction_owner)[session_id] # "Mob")) THEN TRUE ELSE (session_id \in DOMAIN session_instruction_target_revision))) /\ (((session_instruction_owner)[session_id] # "Mob") /\ (slot \in DOMAIN (session_instruction_target_revision)[session_id]) /\ (IF ((effect = "Active") /\ (digest # "")) THEN TRUE ELSE ((effect = "Absent") /\ (digest = "") /\ (body_bytes = 0)))))
    /\ (((session_id \in DOMAIN session_instruction_target_effect) /\ (session_id \in DOMAIN session_instruction_target_bytes)) /\ (1048576 >= session_document_instruction_active_bytes(session_document_instruction_slot_after_set((session_instruction_target_effect)[session_id], slot, effect), session_document_instruction_slot_after_set((session_instruction_target_bytes)[session_id], slot, body_bytes))))
    /\ ((session_id \in DOMAIN session_instruction_target_revision) /\ (revision > (IF "value" \in DOMAIN (IF (slot \in DOMAIN (session_instruction_target_revision)[session_id]) THEN Some((IF slot \in DOMAIN (session_instruction_target_revision)[session_id] THEN (session_instruction_target_revision)[session_id][slot] ELSE None)) ELSE None) THEN (IF (slot \in DOMAIN (session_instruction_target_revision)[session_id]) THEN Some((IF slot \in DOMAIN (session_instruction_target_revision)[session_id] THEN (session_instruction_target_revision)[session_id][slot] ELSE None)) ELSE None)["value"] ELSE None)))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_instruction_owner' = MapSet(session_instruction_owner, session_id, "Session")
    /\ session_instruction_target_generation' = MapSet(session_instruction_target_generation, session_id, ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_instruction_target_generation) THEN Some((IF session_id \in DOMAIN session_instruction_target_generation THEN session_instruction_target_generation[session_id] ELSE 0)) ELSE None) THEN (IF (session_id \in DOMAIN session_instruction_target_generation) THEN Some((IF session_id \in DOMAIN session_instruction_target_generation THEN session_instruction_target_generation[session_id] ELSE 0)) ELSE None)["value"] ELSE None) + 1))
    /\ session_instruction_target_revision' = MapSet(session_instruction_target_revision, session_id, session_document_instruction_slot_after_set((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_instruction_target_revision) THEN Some((IF session_id \in DOMAIN session_instruction_target_revision THEN session_instruction_target_revision[session_id] ELSE [x \in {} |-> None])) ELSE None) THEN (IF (session_id \in DOMAIN session_instruction_target_revision) THEN Some((IF session_id \in DOMAIN session_instruction_target_revision THEN session_instruction_target_revision[session_id] ELSE [x \in {} |-> None])) ELSE None)["value"] ELSE None), slot, revision))
    /\ session_instruction_target_effect' = MapSet(session_instruction_target_effect, session_id, session_document_instruction_slot_after_set((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_instruction_target_effect) THEN Some((IF session_id \in DOMAIN session_instruction_target_effect THEN session_instruction_target_effect[session_id] ELSE [x \in {} |-> None])) ELSE None) THEN (IF (session_id \in DOMAIN session_instruction_target_effect) THEN Some((IF session_id \in DOMAIN session_instruction_target_effect THEN session_instruction_target_effect[session_id] ELSE [x \in {} |-> None])) ELSE None)["value"] ELSE None), slot, effect))
    /\ session_instruction_target_digest' = MapSet(session_instruction_target_digest, session_id, session_document_instruction_slot_after_set((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_instruction_target_digest) THEN Some((IF session_id \in DOMAIN session_instruction_target_digest THEN session_instruction_target_digest[session_id] ELSE [x \in {} |-> None])) ELSE None) THEN (IF (session_id \in DOMAIN session_instruction_target_digest) THEN Some((IF session_id \in DOMAIN session_instruction_target_digest THEN session_instruction_target_digest[session_id] ELSE [x \in {} |-> None])) ELSE None)["value"] ELSE None), slot, digest))
    /\ session_instruction_target_bytes' = MapSet(session_instruction_target_bytes, session_id, session_document_instruction_slot_after_set((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_instruction_target_bytes) THEN Some((IF session_id \in DOMAIN session_instruction_target_bytes THEN session_instruction_target_bytes[session_id] ELSE [x \in {} |-> None])) ELSE None) THEN (IF (session_id \in DOMAIN session_instruction_target_bytes) THEN Some((IF session_id \in DOMAIN session_instruction_target_bytes THEN session_instruction_target_bytes[session_id] ELSE [x \in {} |-> None])) ELSE None)["value"] ELSE None), slot, body_bytes))
    /\ UnchangedFrame_69701730652d9513


SetSessionInstructionActiveBudgetExceeded(session_id, slot, revision, effect, digest, body_bytes) ==
    /\ phase = "Ready"
    /\ (((session_id \in DOMAIN session_instruction_owner) /\ (IF ~(((session_instruction_owner)[session_id] # "Mob")) THEN TRUE ELSE (IF ~((effect = "Active")) THEN TRUE ELSE (IF ~((digest # "")) THEN TRUE ELSE ((((session_id \in DOMAIN session_instruction_target_revision) /\ (IF ~(((slot \in DOMAIN (session_instruction_target_revision)[session_id]) = FALSE)) THEN TRUE ELSE (IF ~((revision >= 1)) THEN TRUE ELSE (session_id \in DOMAIN session_instruction_target_revision)))) /\ (IF (((slot \in DOMAIN (session_instruction_target_revision)[session_id]) = FALSE) /\ (revision >= 1) /\ (64 > Cardinality(DOMAIN (session_instruction_target_revision)[session_id]))) THEN TRUE ELSE ((session_id \in DOMAIN session_instruction_target_revision) /\ (IF ~((slot \in DOMAIN (session_instruction_target_revision)[session_id])) THEN TRUE ELSE (session_id \in DOMAIN session_instruction_target_revision))))) /\ (IF ~((IF (((slot \in DOMAIN (session_instruction_target_revision)[session_id]) = FALSE) /\ (revision >= 1) /\ (64 > Cardinality(DOMAIN (session_instruction_target_revision)[session_id]))) THEN TRUE ELSE ((slot \in DOMAIN (session_instruction_target_revision)[session_id]) /\ (revision > (IF "value" \in DOMAIN (IF (slot \in DOMAIN (session_instruction_target_revision)[session_id]) THEN Some((IF slot \in DOMAIN (session_instruction_target_revision)[session_id] THEN (session_instruction_target_revision)[session_id][slot] ELSE None)) ELSE None) THEN (IF (slot \in DOMAIN (session_instruction_target_revision)[session_id]) THEN Some((IF slot \in DOMAIN (session_instruction_target_revision)[session_id] THEN (session_instruction_target_revision)[session_id][slot] ELSE None)) ELSE None)["value"] ELSE None))))) THEN TRUE ELSE ((session_id \in DOMAIN session_instruction_target_effect) /\ (session_id \in DOMAIN session_instruction_target_bytes)))))))) /\ (((session_instruction_owner)[session_id] # "Mob") /\ (effect = "Active") /\ (digest # "") /\ (IF (((slot \in DOMAIN (session_instruction_target_revision)[session_id]) = FALSE) /\ (revision >= 1) /\ (64 > Cardinality(DOMAIN (session_instruction_target_revision)[session_id]))) THEN TRUE ELSE ((slot \in DOMAIN (session_instruction_target_revision)[session_id]) /\ (revision > (IF "value" \in DOMAIN (IF (slot \in DOMAIN (session_instruction_target_revision)[session_id]) THEN Some((IF slot \in DOMAIN (session_instruction_target_revision)[session_id] THEN (session_instruction_target_revision)[session_id][slot] ELSE None)) ELSE None) THEN (IF (slot \in DOMAIN (session_instruction_target_revision)[session_id]) THEN Some((IF slot \in DOMAIN (session_instruction_target_revision)[session_id] THEN (session_instruction_target_revision)[session_id][slot] ELSE None)) ELSE None)["value"] ELSE None)))) /\ (session_document_instruction_active_bytes(session_document_instruction_slot_after_set((session_instruction_target_effect)[session_id], slot, effect), session_document_instruction_slot_after_set((session_instruction_target_bytes)[session_id], slot, body_bytes)) > 1048576)))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


SetSessionInstructionDuplicate(session_id, slot, revision, effect, digest, body_bytes) ==
    /\ phase = "Ready"
    /\ (((session_id \in DOMAIN session_instruction_owner) /\ (IF ~(((session_instruction_owner)[session_id] # "Mob")) THEN TRUE ELSE ((session_id \in DOMAIN session_instruction_target_revision) /\ (IF ~((slot \in DOMAIN (session_instruction_target_revision)[session_id])) THEN TRUE ELSE ((session_id \in DOMAIN session_instruction_target_revision) /\ (IF ~((revision = (IF "value" \in DOMAIN (IF (slot \in DOMAIN (session_instruction_target_revision)[session_id]) THEN Some((IF slot \in DOMAIN (session_instruction_target_revision)[session_id] THEN (session_instruction_target_revision)[session_id][slot] ELSE None)) ELSE None) THEN (IF (slot \in DOMAIN (session_instruction_target_revision)[session_id]) THEN Some((IF slot \in DOMAIN (session_instruction_target_revision)[session_id] THEN (session_instruction_target_revision)[session_id][slot] ELSE None)) ELSE None)["value"] ELSE None))) THEN TRUE ELSE ((session_id \in DOMAIN session_instruction_target_effect) /\ (IF ~(((IF "value" \in DOMAIN (IF (slot \in DOMAIN (session_instruction_target_effect)[session_id]) THEN Some((IF slot \in DOMAIN (session_instruction_target_effect)[session_id] THEN (session_instruction_target_effect)[session_id][slot] ELSE None)) ELSE None) THEN (IF (slot \in DOMAIN (session_instruction_target_effect)[session_id]) THEN Some((IF slot \in DOMAIN (session_instruction_target_effect)[session_id] THEN (session_instruction_target_effect)[session_id][slot] ELSE None)) ELSE None)["value"] ELSE None) = effect)) THEN TRUE ELSE (session_id \in DOMAIN session_instruction_target_digest))))))))) /\ (((session_instruction_owner)[session_id] # "Mob") /\ (slot \in DOMAIN (session_instruction_target_revision)[session_id]) /\ (revision = (IF "value" \in DOMAIN (IF (slot \in DOMAIN (session_instruction_target_revision)[session_id]) THEN Some((IF slot \in DOMAIN (session_instruction_target_revision)[session_id] THEN (session_instruction_target_revision)[session_id][slot] ELSE None)) ELSE None) THEN (IF (slot \in DOMAIN (session_instruction_target_revision)[session_id]) THEN Some((IF slot \in DOMAIN (session_instruction_target_revision)[session_id] THEN (session_instruction_target_revision)[session_id][slot] ELSE None)) ELSE None)["value"] ELSE None)) /\ ((IF "value" \in DOMAIN (IF (slot \in DOMAIN (session_instruction_target_effect)[session_id]) THEN Some((IF slot \in DOMAIN (session_instruction_target_effect)[session_id] THEN (session_instruction_target_effect)[session_id][slot] ELSE None)) ELSE None) THEN (IF (slot \in DOMAIN (session_instruction_target_effect)[session_id]) THEN Some((IF slot \in DOMAIN (session_instruction_target_effect)[session_id] THEN (session_instruction_target_effect)[session_id][slot] ELSE None)) ELSE None)["value"] ELSE None) = effect) /\ ((IF "value" \in DOMAIN (IF (slot \in DOMAIN (session_instruction_target_digest)[session_id]) THEN Some((IF slot \in DOMAIN (session_instruction_target_digest)[session_id] THEN (session_instruction_target_digest)[session_id][slot] ELSE None)) ELSE None) THEN (IF (slot \in DOMAIN (session_instruction_target_digest)[session_id]) THEN Some((IF slot \in DOMAIN (session_instruction_target_digest)[session_id] THEN (session_instruction_target_digest)[session_id][slot] ELSE None)) ELSE None)["value"] ELSE None) = digest)))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


SetSessionInstructionConflict(session_id, slot, revision, effect, digest, body_bytes) ==
    /\ phase = "Ready"
    /\ (((session_id \in DOMAIN session_instruction_owner) /\ (IF ~(((session_instruction_owner)[session_id] # "Mob")) THEN TRUE ELSE ((session_id \in DOMAIN session_instruction_target_revision) /\ (IF ~((slot \in DOMAIN (session_instruction_target_revision)[session_id])) THEN TRUE ELSE ((session_id \in DOMAIN session_instruction_target_revision) /\ (IF ~((revision = (IF "value" \in DOMAIN (IF (slot \in DOMAIN (session_instruction_target_revision)[session_id]) THEN Some((IF slot \in DOMAIN (session_instruction_target_revision)[session_id] THEN (session_instruction_target_revision)[session_id][slot] ELSE None)) ELSE None) THEN (IF (slot \in DOMAIN (session_instruction_target_revision)[session_id]) THEN Some((IF slot \in DOMAIN (session_instruction_target_revision)[session_id] THEN (session_instruction_target_revision)[session_id][slot] ELSE None)) ELSE None)["value"] ELSE None))) THEN TRUE ELSE ((session_id \in DOMAIN session_instruction_target_effect) /\ (IF ((IF "value" \in DOMAIN (IF (slot \in DOMAIN (session_instruction_target_effect)[session_id]) THEN Some((IF slot \in DOMAIN (session_instruction_target_effect)[session_id] THEN (session_instruction_target_effect)[session_id][slot] ELSE None)) ELSE None) THEN (IF (slot \in DOMAIN (session_instruction_target_effect)[session_id]) THEN Some((IF slot \in DOMAIN (session_instruction_target_effect)[session_id] THEN (session_instruction_target_effect)[session_id][slot] ELSE None)) ELSE None)["value"] ELSE None) # effect) THEN TRUE ELSE (session_id \in DOMAIN session_instruction_target_digest))))))))) /\ (((session_instruction_owner)[session_id] # "Mob") /\ (slot \in DOMAIN (session_instruction_target_revision)[session_id]) /\ (revision = (IF "value" \in DOMAIN (IF (slot \in DOMAIN (session_instruction_target_revision)[session_id]) THEN Some((IF slot \in DOMAIN (session_instruction_target_revision)[session_id] THEN (session_instruction_target_revision)[session_id][slot] ELSE None)) ELSE None) THEN (IF (slot \in DOMAIN (session_instruction_target_revision)[session_id]) THEN Some((IF slot \in DOMAIN (session_instruction_target_revision)[session_id] THEN (session_instruction_target_revision)[session_id][slot] ELSE None)) ELSE None)["value"] ELSE None)) /\ (IF ((IF "value" \in DOMAIN (IF (slot \in DOMAIN (session_instruction_target_effect)[session_id]) THEN Some((IF slot \in DOMAIN (session_instruction_target_effect)[session_id] THEN (session_instruction_target_effect)[session_id][slot] ELSE None)) ELSE None) THEN (IF (slot \in DOMAIN (session_instruction_target_effect)[session_id]) THEN Some((IF slot \in DOMAIN (session_instruction_target_effect)[session_id] THEN (session_instruction_target_effect)[session_id][slot] ELSE None)) ELSE None)["value"] ELSE None) # effect) THEN TRUE ELSE ((IF "value" \in DOMAIN (IF (slot \in DOMAIN (session_instruction_target_digest)[session_id]) THEN Some((IF slot \in DOMAIN (session_instruction_target_digest)[session_id] THEN (session_instruction_target_digest)[session_id][slot] ELSE None)) ELSE None) THEN (IF (slot \in DOMAIN (session_instruction_target_digest)[session_id]) THEN Some((IF slot \in DOMAIN (session_instruction_target_digest)[session_id] THEN (session_instruction_target_digest)[session_id][slot] ELSE None)) ELSE None)["value"] ELSE None) # digest))))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


SetSessionInstructionStale(session_id, slot, revision, effect, digest, body_bytes) ==
    /\ phase = "Ready"
    /\ (((session_id \in DOMAIN session_instruction_owner) /\ (IF ~(((session_instruction_owner)[session_id] # "Mob")) THEN TRUE ELSE ((session_id \in DOMAIN session_instruction_target_revision) /\ (IF ~((slot \in DOMAIN (session_instruction_target_revision)[session_id])) THEN TRUE ELSE (session_id \in DOMAIN session_instruction_target_revision))))) /\ (((session_instruction_owner)[session_id] # "Mob") /\ (slot \in DOMAIN (session_instruction_target_revision)[session_id]) /\ (revision < (IF "value" \in DOMAIN (IF (slot \in DOMAIN (session_instruction_target_revision)[session_id]) THEN Some((IF slot \in DOMAIN (session_instruction_target_revision)[session_id] THEN (session_instruction_target_revision)[session_id][slot] ELSE None)) ELSE None) THEN (IF (slot \in DOMAIN (session_instruction_target_revision)[session_id]) THEN Some((IF slot \in DOMAIN (session_instruction_target_revision)[session_id] THEN (session_instruction_target_revision)[session_id][slot] ELSE None)) ELSE None)["value"] ELSE None))))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


SetSessionInstructionOwnedByMob(session_id, slot, revision, effect, digest, body_bytes) ==
    /\ phase = "Ready"
    /\ ((session_id \in DOMAIN session_instruction_owner) /\ ((session_instruction_owner)[session_id] = "Mob"))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


SetSessionInstructionSlotLimitExceeded(session_id, slot, revision, effect, digest, body_bytes) ==
    /\ phase = "Ready"
    /\ (((session_id \in DOMAIN session_instruction_owner) /\ (IF ~(((session_instruction_owner)[session_id] # "Mob")) THEN TRUE ELSE ((session_id \in DOMAIN session_instruction_target_revision) /\ (IF ~(((slot \in DOMAIN (session_instruction_target_revision)[session_id]) = FALSE)) THEN TRUE ELSE (IF ~((effect = "Active")) THEN TRUE ELSE (session_id \in DOMAIN session_instruction_target_revision)))))) /\ (((session_instruction_owner)[session_id] # "Mob") /\ ((slot \in DOMAIN (session_instruction_target_revision)[session_id]) = FALSE) /\ (effect = "Active") /\ (Cardinality(DOMAIN (session_instruction_target_revision)[session_id]) >= 64)))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


SetSessionInstructionUnknownSlot(session_id, slot, revision, effect, digest, body_bytes) ==
    /\ phase = "Ready"
    /\ (((session_id \in DOMAIN session_instruction_owner) /\ (IF ~(((session_instruction_owner)[session_id] # "Mob")) THEN TRUE ELSE (session_id \in DOMAIN session_instruction_target_revision))) /\ (((session_instruction_owner)[session_id] # "Mob") /\ ((slot \in DOMAIN (session_instruction_target_revision)[session_id]) = FALSE) /\ (effect = "Absent")))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


RebindInstructionOwner(session_id, owner, mob_id) ==
    /\ phase = "Ready"
    /\ ((IF ~((owner = "Mob")) THEN TRUE ELSE (IF ~((mob_id # "")) THEN TRUE ELSE (session_id \in DOMAIN session_instruction_effective_revision))) /\ ((owner = "Mob") /\ (mob_id # "") /\ (Cardinality(DOMAIN (session_instruction_effective_revision)[session_id]) <= 64)))
    /\ ((session_id \in DOMAIN session_instruction_owner) /\ ((session_instruction_owner)[session_id] = "Session"))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_instruction_owner' = MapSet(session_instruction_owner, session_id, owner)
    /\ session_instruction_owner_mob' = MapSet(session_instruction_owner_mob, session_id, mob_id)
    /\ session_instruction_target_generation' = MapSet(session_instruction_target_generation, session_id, 0)
    /\ session_instruction_target_revision' = MapSet(session_instruction_target_revision, session_id, (IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_instruction_effective_revision) THEN Some((IF session_id \in DOMAIN session_instruction_effective_revision THEN session_instruction_effective_revision[session_id] ELSE [x \in {} |-> None])) ELSE None) THEN (IF (session_id \in DOMAIN session_instruction_effective_revision) THEN Some((IF session_id \in DOMAIN session_instruction_effective_revision THEN session_instruction_effective_revision[session_id] ELSE [x \in {} |-> None])) ELSE None)["value"] ELSE None))
    /\ session_instruction_target_effect' = MapSet(session_instruction_target_effect, session_id, (IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_instruction_effective_effect) THEN Some((IF session_id \in DOMAIN session_instruction_effective_effect THEN session_instruction_effective_effect[session_id] ELSE [x \in {} |-> None])) ELSE None) THEN (IF (session_id \in DOMAIN session_instruction_effective_effect) THEN Some((IF session_id \in DOMAIN session_instruction_effective_effect THEN session_instruction_effective_effect[session_id] ELSE [x \in {} |-> None])) ELSE None)["value"] ELSE None))
    /\ session_instruction_target_digest' = MapSet(session_instruction_target_digest, session_id, (IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_instruction_effective_digest) THEN Some((IF session_id \in DOMAIN session_instruction_effective_digest THEN session_instruction_effective_digest[session_id] ELSE [x \in {} |-> None])) ELSE None) THEN (IF (session_id \in DOMAIN session_instruction_effective_digest) THEN Some((IF session_id \in DOMAIN session_instruction_effective_digest THEN session_instruction_effective_digest[session_id] ELSE [x \in {} |-> None])) ELSE None)["value"] ELSE None))
    /\ session_instruction_target_bytes' = MapSet(session_instruction_target_bytes, session_id, (IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_instruction_effective_bytes) THEN Some((IF session_id \in DOMAIN session_instruction_effective_bytes THEN session_instruction_effective_bytes[session_id] ELSE [x \in {} |-> None])) ELSE None) THEN (IF (session_id \in DOMAIN session_instruction_effective_bytes) THEN Some((IF session_id \in DOMAIN session_instruction_effective_bytes THEN session_instruction_effective_bytes[session_id] ELSE [x \in {} |-> None])) ELSE None)["value"] ELSE None))
    /\ UnchangedFrame_2999b03e64d8844d


RebindInstructionOwnerUnowned(session_id, owner, mob_id) ==
    /\ phase = "Ready"
    /\ (IF ((owner = "Session") /\ (mob_id = "")) THEN TRUE ELSE ((owner = "Mob") /\ (mob_id # "")))
    /\ ((session_id \in DOMAIN session_instruction_owner) /\ ((session_instruction_owner)[session_id] = "Unowned"))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_instruction_owner' = MapSet(session_instruction_owner, session_id, owner)
    /\ session_instruction_owner_mob' = MapSet(session_instruction_owner_mob, session_id, mob_id)
    /\ session_instruction_target_generation' = MapSet(session_instruction_target_generation, session_id, 0)
    /\ UnchangedFrame_a26768aa2b7e30dc


ReleaseInstructionTarget(session_id, mob_id) ==
    /\ phase = "Ready"
    /\ (((session_id \in DOMAIN session_instruction_owner) /\ (IF ~(((session_instruction_owner)[session_id] = "Mob")) THEN TRUE ELSE (session_id \in DOMAIN session_instruction_owner_mob))) /\ (((session_instruction_owner)[session_id] = "Mob") /\ ((session_instruction_owner_mob)[session_id] = mob_id)))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_instruction_owner' = MapSet(session_instruction_owner, session_id, "Unowned")
    /\ session_instruction_owner_mob' = MapSet(session_instruction_owner_mob, session_id, "")
    /\ session_instruction_target_generation' = MapSet(session_instruction_target_generation, session_id, 0)
    /\ session_instruction_target_revision' = MapSet(session_instruction_target_revision, session_id, [x \in {} |-> None])
    /\ session_instruction_target_effect' = MapSet(session_instruction_target_effect, session_id, [x \in {} |-> None])
    /\ session_instruction_target_digest' = MapSet(session_instruction_target_digest, session_id, [x \in {} |-> None])
    /\ session_instruction_target_bytes' = MapSet(session_instruction_target_bytes, session_id, [x \in {} |-> None])
    /\ UnchangedFrame_2999b03e64d8844d


ReleaseInstructionTargetAlreadyUnowned(session_id, mob_id) ==
    /\ phase = "Ready"
    /\ ((session_id \in DOMAIN session_instruction_owner) /\ ((session_instruction_owner)[session_id] = "Unowned"))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ReleaseInstructionTargetOwnedByOtherMob(session_id, mob_id) ==
    /\ phase = "Ready"
    /\ (((session_id \in DOMAIN session_instruction_owner) /\ (IF ~(((session_instruction_owner)[session_id] = "Mob")) THEN TRUE ELSE (session_id \in DOMAIN session_instruction_owner_mob))) /\ (((session_instruction_owner)[session_id] = "Mob") /\ ((session_instruction_owner_mob)[session_id] # mob_id)))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ReleaseInstructionTargetOwnedBySession(session_id, mob_id) ==
    /\ phase = "Ready"
    /\ ((session_id \in DOMAIN session_instruction_owner) /\ ((session_instruction_owner)[session_id] = "Session"))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ResolveRunStartInstructionsApply(session_id, run_start, lowering, transcript_is_system_prefix) ==
    /\ phase = "Ready"
    /\ (((((((session_id \in DOMAIN session_instruction_target_revision) /\ (session_id \in DOMAIN session_instruction_target_effect)) /\ (session_id \in DOMAIN session_instruction_target_digest)) /\ (session_id \in DOMAIN session_instruction_effective_revision)) /\ (session_id \in DOMAIN session_instruction_effective_effect)) /\ (session_id \in DOMAIN session_instruction_effective_digest)) /\ instruction_change_pending((session_instruction_target_revision)[session_id], (session_instruction_target_effect)[session_id], (session_instruction_target_digest)[session_id], (session_instruction_effective_revision)[session_id], (session_instruction_effective_effect)[session_id], (session_instruction_effective_digest)[session_id]))
    /\ (run_start = "FreshRun")
    /\ instruction_entry_lowers(lowering, transcript_is_system_prefix)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_instruction_effective_revision' = MapSet(session_instruction_effective_revision, session_id, session_document_instruction_active_slots((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_instruction_target_effect) THEN Some((IF session_id \in DOMAIN session_instruction_target_effect THEN session_instruction_target_effect[session_id] ELSE [x \in {} |-> None])) ELSE None) THEN (IF (session_id \in DOMAIN session_instruction_target_effect) THEN Some((IF session_id \in DOMAIN session_instruction_target_effect THEN session_instruction_target_effect[session_id] ELSE [x \in {} |-> None])) ELSE None)["value"] ELSE None), (IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_instruction_target_revision) THEN Some((IF session_id \in DOMAIN session_instruction_target_revision THEN session_instruction_target_revision[session_id] ELSE [x \in {} |-> None])) ELSE None) THEN (IF (session_id \in DOMAIN session_instruction_target_revision) THEN Some((IF session_id \in DOMAIN session_instruction_target_revision THEN session_instruction_target_revision[session_id] ELSE [x \in {} |-> None])) ELSE None)["value"] ELSE None)))
    /\ session_instruction_effective_effect' = MapSet(session_instruction_effective_effect, session_id, session_document_instruction_active_slots((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_instruction_target_effect) THEN Some((IF session_id \in DOMAIN session_instruction_target_effect THEN session_instruction_target_effect[session_id] ELSE [x \in {} |-> None])) ELSE None) THEN (IF (session_id \in DOMAIN session_instruction_target_effect) THEN Some((IF session_id \in DOMAIN session_instruction_target_effect THEN session_instruction_target_effect[session_id] ELSE [x \in {} |-> None])) ELSE None)["value"] ELSE None), (IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_instruction_target_effect) THEN Some((IF session_id \in DOMAIN session_instruction_target_effect THEN session_instruction_target_effect[session_id] ELSE [x \in {} |-> None])) ELSE None) THEN (IF (session_id \in DOMAIN session_instruction_target_effect) THEN Some((IF session_id \in DOMAIN session_instruction_target_effect THEN session_instruction_target_effect[session_id] ELSE [x \in {} |-> None])) ELSE None)["value"] ELSE None)))
    /\ session_instruction_effective_digest' = MapSet(session_instruction_effective_digest, session_id, session_document_instruction_active_slots((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_instruction_target_effect) THEN Some((IF session_id \in DOMAIN session_instruction_target_effect THEN session_instruction_target_effect[session_id] ELSE [x \in {} |-> None])) ELSE None) THEN (IF (session_id \in DOMAIN session_instruction_target_effect) THEN Some((IF session_id \in DOMAIN session_instruction_target_effect THEN session_instruction_target_effect[session_id] ELSE [x \in {} |-> None])) ELSE None)["value"] ELSE None), (IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_instruction_target_digest) THEN Some((IF session_id \in DOMAIN session_instruction_target_digest THEN session_instruction_target_digest[session_id] ELSE [x \in {} |-> None])) ELSE None) THEN (IF (session_id \in DOMAIN session_instruction_target_digest) THEN Some((IF session_id \in DOMAIN session_instruction_target_digest THEN session_instruction_target_digest[session_id] ELSE [x \in {} |-> None])) ELSE None)["value"] ELSE None)))
    /\ session_instruction_effective_bytes' = MapSet(session_instruction_effective_bytes, session_id, session_document_instruction_active_slots((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_instruction_target_effect) THEN Some((IF session_id \in DOMAIN session_instruction_target_effect THEN session_instruction_target_effect[session_id] ELSE [x \in {} |-> None])) ELSE None) THEN (IF (session_id \in DOMAIN session_instruction_target_effect) THEN Some((IF session_id \in DOMAIN session_instruction_target_effect THEN session_instruction_target_effect[session_id] ELSE [x \in {} |-> None])) ELSE None)["value"] ELSE None), (IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_instruction_target_bytes) THEN Some((IF session_id \in DOMAIN session_instruction_target_bytes THEN session_instruction_target_bytes[session_id] ELSE [x \in {} |-> None])) ELSE None) THEN (IF (session_id \in DOMAIN session_instruction_target_bytes) THEN Some((IF session_id \in DOMAIN session_instruction_target_bytes THEN session_instruction_target_bytes[session_id] ELSE [x \in {} |-> None])) ELSE None)["value"] ELSE None)))
    /\ session_instruction_effective_generation' = MapSet(session_instruction_effective_generation, session_id, (IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_instruction_target_generation) THEN Some((IF session_id \in DOMAIN session_instruction_target_generation THEN session_instruction_target_generation[session_id] ELSE 0)) ELSE None) THEN (IF (session_id \in DOMAIN session_instruction_target_generation) THEN Some((IF session_id \in DOMAIN session_instruction_target_generation THEN session_instruction_target_generation[session_id] ELSE 0)) ELSE None)["value"] ELSE None))
    /\ session_instruction_entry_sequence' = MapSet(session_instruction_entry_sequence, session_id, ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_instruction_entry_sequence) THEN Some((IF session_id \in DOMAIN session_instruction_entry_sequence THEN session_instruction_entry_sequence[session_id] ELSE 0)) ELSE None) THEN (IF (session_id \in DOMAIN session_instruction_entry_sequence) THEN Some((IF session_id \in DOMAIN session_instruction_entry_sequence THEN session_instruction_entry_sequence[session_id] ELSE 0)) ELSE None)["value"] ELSE None) + 1))
    /\ UnchangedFrame_2b67ec16fd8d9aca


ResolveRunStartInstructionsNoChange(session_id, run_start, lowering, transcript_is_system_prefix) ==
    /\ phase = "Ready"
    /\ ((IF (run_start # "FreshRun") THEN TRUE ELSE ((((((session_id \in DOMAIN session_instruction_target_revision) /\ (session_id \in DOMAIN session_instruction_target_effect)) /\ (session_id \in DOMAIN session_instruction_target_digest)) /\ (session_id \in DOMAIN session_instruction_effective_revision)) /\ (session_id \in DOMAIN session_instruction_effective_effect)) /\ (session_id \in DOMAIN session_instruction_effective_digest))) /\ (IF (run_start # "FreshRun") THEN TRUE ELSE (instruction_change_pending((session_instruction_target_revision)[session_id], (session_instruction_target_effect)[session_id], (session_instruction_target_digest)[session_id], (session_instruction_effective_revision)[session_id], (session_instruction_effective_effect)[session_id], (session_instruction_effective_digest)[session_id]) = FALSE)))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ResolveRunStartInstructionsDeferredLeadingPrefix(session_id, run_start, lowering, transcript_is_system_prefix) ==
    /\ phase = "Ready"
    /\ ((IF ~((run_start = "FreshRun")) THEN TRUE ELSE ((((((session_id \in DOMAIN session_instruction_target_revision) /\ (session_id \in DOMAIN session_instruction_target_effect)) /\ (session_id \in DOMAIN session_instruction_target_digest)) /\ (session_id \in DOMAIN session_instruction_effective_revision)) /\ (session_id \in DOMAIN session_instruction_effective_effect)) /\ (session_id \in DOMAIN session_instruction_effective_digest))) /\ ((run_start = "FreshRun") /\ instruction_change_pending((session_instruction_target_revision)[session_id], (session_instruction_target_effect)[session_id], (session_instruction_target_digest)[session_id], (session_instruction_effective_revision)[session_id], (session_instruction_effective_effect)[session_id], (session_instruction_effective_digest)[session_id]) /\ (lowering = "LeadingPrefixOnly") /\ (transcript_is_system_prefix = FALSE)))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


ResolveRunStartInstructionsDeferredUnavailable(session_id, run_start, lowering, transcript_is_system_prefix) ==
    /\ phase = "Ready"
    /\ ((IF ~((run_start = "FreshRun")) THEN TRUE ELSE ((((((session_id \in DOMAIN session_instruction_target_revision) /\ (session_id \in DOMAIN session_instruction_target_effect)) /\ (session_id \in DOMAIN session_instruction_target_digest)) /\ (session_id \in DOMAIN session_instruction_effective_revision)) /\ (session_id \in DOMAIN session_instruction_effective_effect)) /\ (session_id \in DOMAIN session_instruction_effective_digest))) /\ ((run_start = "FreshRun") /\ instruction_change_pending((session_instruction_target_revision)[session_id], (session_instruction_target_effect)[session_id], (session_instruction_target_digest)[session_id], (session_instruction_effective_revision)[session_id], (session_instruction_effective_effect)[session_id], (session_instruction_effective_digest)[session_id]) /\ (lowering = "Unavailable")))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44bcbd08434cac9f


AuthorizeInstructionFoldAtRunStart(session_id, at_run_start, lowering) ==
    /\ phase = "Ready"
    /\ (session_id \in DOMAIN session_instruction_entry_sequence)
    /\ (at_run_start = TRUE)
    /\ (lowering # "Unavailable")
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_instruction_effective_revision' = MapSet(session_instruction_effective_revision, session_id, session_document_instruction_active_slots((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_instruction_target_effect) THEN Some((IF session_id \in DOMAIN session_instruction_target_effect THEN session_instruction_target_effect[session_id] ELSE [x \in {} |-> None])) ELSE None) THEN (IF (session_id \in DOMAIN session_instruction_target_effect) THEN Some((IF session_id \in DOMAIN session_instruction_target_effect THEN session_instruction_target_effect[session_id] ELSE [x \in {} |-> None])) ELSE None)["value"] ELSE None), (IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_instruction_target_revision) THEN Some((IF session_id \in DOMAIN session_instruction_target_revision THEN session_instruction_target_revision[session_id] ELSE [x \in {} |-> None])) ELSE None) THEN (IF (session_id \in DOMAIN session_instruction_target_revision) THEN Some((IF session_id \in DOMAIN session_instruction_target_revision THEN session_instruction_target_revision[session_id] ELSE [x \in {} |-> None])) ELSE None)["value"] ELSE None)))
    /\ session_instruction_effective_effect' = MapSet(session_instruction_effective_effect, session_id, session_document_instruction_active_slots((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_instruction_target_effect) THEN Some((IF session_id \in DOMAIN session_instruction_target_effect THEN session_instruction_target_effect[session_id] ELSE [x \in {} |-> None])) ELSE None) THEN (IF (session_id \in DOMAIN session_instruction_target_effect) THEN Some((IF session_id \in DOMAIN session_instruction_target_effect THEN session_instruction_target_effect[session_id] ELSE [x \in {} |-> None])) ELSE None)["value"] ELSE None), (IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_instruction_target_effect) THEN Some((IF session_id \in DOMAIN session_instruction_target_effect THEN session_instruction_target_effect[session_id] ELSE [x \in {} |-> None])) ELSE None) THEN (IF (session_id \in DOMAIN session_instruction_target_effect) THEN Some((IF session_id \in DOMAIN session_instruction_target_effect THEN session_instruction_target_effect[session_id] ELSE [x \in {} |-> None])) ELSE None)["value"] ELSE None)))
    /\ session_instruction_effective_digest' = MapSet(session_instruction_effective_digest, session_id, session_document_instruction_active_slots((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_instruction_target_effect) THEN Some((IF session_id \in DOMAIN session_instruction_target_effect THEN session_instruction_target_effect[session_id] ELSE [x \in {} |-> None])) ELSE None) THEN (IF (session_id \in DOMAIN session_instruction_target_effect) THEN Some((IF session_id \in DOMAIN session_instruction_target_effect THEN session_instruction_target_effect[session_id] ELSE [x \in {} |-> None])) ELSE None)["value"] ELSE None), (IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_instruction_target_digest) THEN Some((IF session_id \in DOMAIN session_instruction_target_digest THEN session_instruction_target_digest[session_id] ELSE [x \in {} |-> None])) ELSE None) THEN (IF (session_id \in DOMAIN session_instruction_target_digest) THEN Some((IF session_id \in DOMAIN session_instruction_target_digest THEN session_instruction_target_digest[session_id] ELSE [x \in {} |-> None])) ELSE None)["value"] ELSE None)))
    /\ session_instruction_effective_bytes' = MapSet(session_instruction_effective_bytes, session_id, session_document_instruction_active_slots((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_instruction_target_effect) THEN Some((IF session_id \in DOMAIN session_instruction_target_effect THEN session_instruction_target_effect[session_id] ELSE [x \in {} |-> None])) ELSE None) THEN (IF (session_id \in DOMAIN session_instruction_target_effect) THEN Some((IF session_id \in DOMAIN session_instruction_target_effect THEN session_instruction_target_effect[session_id] ELSE [x \in {} |-> None])) ELSE None)["value"] ELSE None), (IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_instruction_target_bytes) THEN Some((IF session_id \in DOMAIN session_instruction_target_bytes THEN session_instruction_target_bytes[session_id] ELSE [x \in {} |-> None])) ELSE None) THEN (IF (session_id \in DOMAIN session_instruction_target_bytes) THEN Some((IF session_id \in DOMAIN session_instruction_target_bytes THEN session_instruction_target_bytes[session_id] ELSE [x \in {} |-> None])) ELSE None)["value"] ELSE None)))
    /\ session_instruction_effective_generation' = MapSet(session_instruction_effective_generation, session_id, (IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_instruction_target_generation) THEN Some((IF session_id \in DOMAIN session_instruction_target_generation THEN session_instruction_target_generation[session_id] ELSE 0)) ELSE None) THEN (IF (session_id \in DOMAIN session_instruction_target_generation) THEN Some((IF session_id \in DOMAIN session_instruction_target_generation THEN session_instruction_target_generation[session_id] ELSE 0)) ELSE None)["value"] ELSE None))
    /\ session_instruction_entry_sequence' = MapSet(session_instruction_entry_sequence, session_id, ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_instruction_entry_sequence) THEN Some((IF session_id \in DOMAIN session_instruction_entry_sequence THEN session_instruction_entry_sequence[session_id] ELSE 0)) ELSE None) THEN (IF (session_id \in DOMAIN session_instruction_entry_sequence) THEN Some((IF session_id \in DOMAIN session_instruction_entry_sequence THEN session_instruction_entry_sequence[session_id] ELSE 0)) ELSE None)["value"] ELSE None) + 1))
    /\ UnchangedFrame_2b67ec16fd8d9aca


AuthorizeInstructionFoldEffective(session_id, at_run_start, lowering) ==
    /\ phase = "Ready"
    /\ ((session_id \in DOMAIN session_instruction_entry_sequence) /\ (IF (at_run_start = FALSE) THEN TRUE ELSE (lowering = "Unavailable")))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_instruction_entry_sequence' = MapSet(session_instruction_entry_sequence, session_id, ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_instruction_entry_sequence) THEN Some((IF session_id \in DOMAIN session_instruction_entry_sequence THEN session_instruction_entry_sequence[session_id] ELSE 0)) ELSE None) THEN (IF (session_id \in DOMAIN session_instruction_entry_sequence) THEN Some((IF session_id \in DOMAIN session_instruction_entry_sequence THEN session_instruction_entry_sequence[session_id] ELSE 0)) ELSE None)["value"] ELSE None) + 1))
    /\ UnchangedFrame_efa9360e0c81d6cf


Next ==
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : MarkSessionInitialTurnPendingInactiveOrPending(session_id)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : MarkSessionInitialTurnPendingConsumed(session_id)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : StartSessionInitialTurnPending(session_id)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : StartSessionInitialTurnInactive(session_id)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : StartSessionInitialTurnConsumed(session_id)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : ResolveSessionFirstTurnOverridesAllowed(session_id)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : ResolveSessionFirstTurnOverridesDenied(session_id)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : \E prompt_has_content \in BOOLEAN : StageSessionInitialPromptStore(session_id, prompt_has_content)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : \E prompt_has_content \in BOOLEAN : StageSessionInitialPromptClear(session_id, prompt_has_content)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : \E result_count \in 0..2 : StageSessionToolResults(session_id, result_count)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : ConsumeSessionDeferredInputsPending(session_id)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : ConsumeSessionDeferredInputsInactive(session_id)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : ConsumeSessionDeferredInputsConsumed(session_id)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : \E pending_initial_prompt_present \in BOOLEAN : \E pending_tool_result_message_count \in 0..2 : RestoreSessionConsumedInputs(session_id, TRUE, pending_initial_prompt_present, pending_tool_result_message_count)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : \E pending_initial_prompt_present \in BOOLEAN : \E pending_tool_result_message_count \in 0..2 : RestoreSessionConsumedInputsNoPhaseRollback(session_id, FALSE, pending_initial_prompt_present, pending_tool_result_message_count)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : \E arg_phase \in SessionFirstTurnPhaseValues : \E pending_initial_prompt_present \in BOOLEAN : \E pending_tool_result_message_count \in 0..2 : RecoverSessionFirstTurnPhase(session_id, arg_phase, pending_initial_prompt_present, pending_tool_result_message_count)
    \/ (phase = "Ready") /\ \E role \in RealtimeTranscriptRoleKindValues : ResolveRealtimeItemObservedDiscardedAssistant(role, TRUE)
    \/ (phase = "Ready") /\ \E role \in RealtimeTranscriptRoleKindValues : \E response_discarded \in BOOLEAN : ResolveRealtimeItemObservedPresent(role, response_discarded)
    \/ ResolveRealtimeItemSkipped
    \/ (phase = "Ready") /\ \E segment_empty \in BOOLEAN : \E segment_matches \in BOOLEAN : ResolveRealtimeUserTranscriptFinalEmpty(FALSE, segment_empty, segment_matches)
    \/ (phase = "Ready") /\ \E segment_matches \in BOOLEAN : ResolveRealtimeUserTranscriptFinalStore(TRUE, TRUE, segment_matches)
    \/ (phase = "Ready") /\ \E segment_matches \in BOOLEAN : ResolveRealtimeUserTranscriptFinalReplayOrConflict(TRUE, FALSE, segment_matches)
    \/ (phase = "Ready") /\ \E identity_fields_valid \in BOOLEAN : \E key_tombstoned \in BOOLEAN : \E predecessor_materialized \in BOOLEAN : \E existing_identity_present \in BOOLEAN : \E existing_payload_matches \in BOOLEAN : \E target_item_id_available \in BOOLEAN : \E reducer_commit_proof_required \in BOOLEAN : \E reducer_commit_proof_present \in BOOLEAN : ResolveRealtimeUserContentIdentityInvalid(identity_fields_valid, key_tombstoned, predecessor_materialized, existing_identity_present, existing_payload_matches, target_item_id_available, reducer_commit_proof_required, reducer_commit_proof_present)
    \/ (phase = "Ready") /\ \E existing_identity_present \in BOOLEAN : \E existing_payload_matches \in BOOLEAN : \E target_item_id_available \in BOOLEAN : \E reducer_commit_proof_required \in BOOLEAN : \E reducer_commit_proof_present \in BOOLEAN : ResolveRealtimeUserContentIdentityUnmaterializedPredecessor(TRUE, FALSE, FALSE, existing_identity_present, existing_payload_matches, target_item_id_available, reducer_commit_proof_required, reducer_commit_proof_present)
    \/ (phase = "Ready") /\ \E key_tombstoned \in BOOLEAN : \E predecessor_materialized \in BOOLEAN : \E existing_identity_present \in BOOLEAN : \E existing_payload_matches \in BOOLEAN : \E target_item_id_available \in BOOLEAN : \E reducer_commit_proof_required \in BOOLEAN : \E reducer_commit_proof_present \in BOOLEAN : ResolveRealtimeUserContentIdentityConflict(TRUE, key_tombstoned, predecessor_materialized, existing_identity_present, existing_payload_matches, target_item_id_available, reducer_commit_proof_required, reducer_commit_proof_present)
    \/ (phase = "Ready") /\ \E target_item_id_available \in BOOLEAN : \E reducer_commit_proof_required \in BOOLEAN : \E reducer_commit_proof_present \in BOOLEAN : ResolveRealtimeUserContentIdentityReplay(TRUE, FALSE, TRUE, TRUE, TRUE, target_item_id_available, reducer_commit_proof_required, reducer_commit_proof_present)
    \/ (phase = "Ready") /\ \E existing_payload_matches \in BOOLEAN : \E reducer_commit_proof_required \in BOOLEAN : \E reducer_commit_proof_present \in BOOLEAN : ResolveRealtimeUserContentIdentityCommitNew(TRUE, FALSE, TRUE, FALSE, existing_payload_matches, TRUE, reducer_commit_proof_required, reducer_commit_proof_present)
    \/ (phase = "Ready") /\ \E pending_matches_request \in BOOLEAN : ResolveRealtimeUserContentBlobStageNew(FALSE, pending_matches_request)
    \/ ResolveRealtimeUserContentBlobStageReuseExact(TRUE, TRUE)
    \/ ResolveRealtimeUserContentBlobStageRejectOccupied(TRUE, FALSE)
    \/ (phase = "Ready") /\ \E request_matches_pending \in BOOLEAN : \E pending_blob_valid \in BOOLEAN : ResolveRealtimeUserContentBlobRecoveryNone(FALSE, request_matches_pending, pending_blob_valid)
    \/ (phase = "Ready") /\ \E pending_blob_valid \in BOOLEAN : ResolveRealtimeUserContentBlobRecoveryExact(TRUE, TRUE, pending_blob_valid)
    \/ ResolveRealtimeUserContentBlobRecoveryCommitVerified(TRUE, FALSE, TRUE)
    \/ ResolveRealtimeUserContentBlobRecoveryClearInvalid(TRUE, FALSE, FALSE)
    \/ (phase = "Ready") /\ \E pending_matches_committed \in BOOLEAN : ResolveRealtimeUserContentBlobFinalizeNone(FALSE, pending_matches_committed)
    \/ ResolveRealtimeUserContentBlobFinalizeClearCommitted(TRUE, TRUE)
    \/ ResolveRealtimeUserContentBlobFinalizeRejectMismatch(TRUE, FALSE)
    \/ (phase = "Ready") /\ \E segment_empty \in BOOLEAN : \E segment_matches \in BOOLEAN : ResolveRealtimeUserContentFinalEmpty(FALSE, segment_empty, segment_matches)
    \/ (phase = "Ready") /\ \E segment_matches \in BOOLEAN : ResolveRealtimeUserContentFinalStore(TRUE, TRUE, segment_matches)
    \/ (phase = "Ready") /\ \E segment_matches \in BOOLEAN : ResolveRealtimeUserContentFinalReplayOrConflict(TRUE, FALSE, segment_matches)
    \/ (phase = "Ready") /\ \E response_id_valid \in BOOLEAN : \E response_discarded \in BOOLEAN : \E delta_id_present \in BOOLEAN : \E delta_id_seen \in BOOLEAN : \E item_has_text \in BOOLEAN : \E current_lane \in RealtimeTranscriptLaneKindValues : \E requested_lane \in RealtimeTranscriptLaneKindValues : \E response_completed \in BOOLEAN : \E text_after_write_present \in BOOLEAN : ResolveRealtimeAssistantDeltaInvalidOrDuplicate(response_id_valid, response_discarded, delta_id_present, delta_id_seen, item_has_text, current_lane, requested_lane, response_completed, text_after_write_present)
    \/ (phase = "Ready") /\ \E delta_id_present \in BOOLEAN : \E delta_id_seen \in BOOLEAN : \E item_has_text \in BOOLEAN : \E current_lane \in RealtimeTranscriptLaneKindValues : \E requested_lane \in RealtimeTranscriptLaneKindValues : \E response_completed \in BOOLEAN : \E text_after_write_present \in BOOLEAN : ResolveRealtimeAssistantDeltaDiscarded(TRUE, TRUE, delta_id_present, delta_id_seen, item_has_text, current_lane, requested_lane, response_completed, text_after_write_present)
    \/ (phase = "Ready") /\ \E delta_id_present \in BOOLEAN : \E delta_id_seen \in BOOLEAN : \E item_has_text \in BOOLEAN : \E current_lane \in RealtimeTranscriptLaneKindValues : \E requested_lane \in RealtimeTranscriptLaneKindValues : \E response_completed \in BOOLEAN : \E text_after_write_present \in BOOLEAN : ResolveRealtimeAssistantDeltaLaneConflict(TRUE, FALSE, delta_id_present, delta_id_seen, item_has_text, current_lane, requested_lane, response_completed, text_after_write_present)
    \/ (phase = "Ready") /\ \E delta_id_present \in BOOLEAN : \E delta_id_seen \in BOOLEAN : \E item_has_text \in BOOLEAN : \E current_lane \in RealtimeTranscriptLaneKindValues : \E requested_lane \in RealtimeTranscriptLaneKindValues : \E response_completed \in BOOLEAN : \E text_after_write_present \in BOOLEAN : ResolveRealtimeAssistantDeltaAccepted(TRUE, FALSE, delta_id_present, delta_id_seen, item_has_text, current_lane, requested_lane, response_completed, text_after_write_present)
    \/ (phase = "Ready") /\ \E response_discarded \in BOOLEAN : \E item_materialized \in BOOLEAN : \E item_has_text \in BOOLEAN : \E current_lane \in RealtimeTranscriptLaneKindValues : \E requested_lane \in RealtimeTranscriptLaneKindValues : \E response_completed \in BOOLEAN : \E text_after_replace_present \in BOOLEAN : ResolveRealtimeAssistantReplacementInvalid(FALSE, response_discarded, item_materialized, item_has_text, current_lane, requested_lane, response_completed, text_after_replace_present)
    \/ (phase = "Ready") /\ \E item_materialized \in BOOLEAN : \E item_has_text \in BOOLEAN : \E current_lane \in RealtimeTranscriptLaneKindValues : \E requested_lane \in RealtimeTranscriptLaneKindValues : \E response_completed \in BOOLEAN : \E text_after_replace_present \in BOOLEAN : ResolveRealtimeAssistantReplacementDiscarded(TRUE, TRUE, item_materialized, item_has_text, current_lane, requested_lane, response_completed, text_after_replace_present)
    \/ (phase = "Ready") /\ \E item_has_text \in BOOLEAN : \E current_lane \in RealtimeTranscriptLaneKindValues : \E requested_lane \in RealtimeTranscriptLaneKindValues : \E response_completed \in BOOLEAN : \E text_after_replace_present \in BOOLEAN : ResolveRealtimeAssistantReplacementLocked(TRUE, FALSE, TRUE, item_has_text, current_lane, requested_lane, response_completed, text_after_replace_present)
    \/ (phase = "Ready") /\ \E item_has_text \in BOOLEAN : \E current_lane \in RealtimeTranscriptLaneKindValues : \E requested_lane \in RealtimeTranscriptLaneKindValues : \E response_completed \in BOOLEAN : \E text_after_replace_present \in BOOLEAN : ResolveRealtimeAssistantReplacementLaneConflict(TRUE, FALSE, FALSE, item_has_text, current_lane, requested_lane, response_completed, text_after_replace_present)
    \/ (phase = "Ready") /\ \E item_has_text \in BOOLEAN : \E current_lane \in RealtimeTranscriptLaneKindValues : \E requested_lane \in RealtimeTranscriptLaneKindValues : \E response_completed \in BOOLEAN : \E text_after_replace_present \in BOOLEAN : ResolveRealtimeAssistantReplacementAccepted(TRUE, FALSE, FALSE, item_has_text, current_lane, requested_lane, response_completed, text_after_replace_present)
    \/ (phase = "Ready") /\ \E response_discarded \in BOOLEAN : \E stop_reason \in RealtimeTranscriptStopReasonKindValues : ResolveRealtimeAssistantTurnCompletedInvalid(FALSE, response_discarded, stop_reason)
    \/ (phase = "Ready") /\ \E response_discarded \in BOOLEAN : \E stop_reason \in RealtimeTranscriptStopReasonKindValues : ResolveRealtimeAssistantTurnCompletedDiscard(TRUE, response_discarded, stop_reason)
    \/ (phase = "Ready") /\ \E stop_reason \in RealtimeTranscriptStopReasonKindValues : ResolveRealtimeAssistantTurnCompletedToolUse(TRUE, FALSE, stop_reason)
    \/ (phase = "Ready") /\ \E stop_reason \in RealtimeTranscriptStopReasonKindValues : ResolveRealtimeAssistantTurnCompletedRecord(TRUE, FALSE, stop_reason)
    \/ (phase = "Ready") /\ \E requested_lane \in RealtimeTranscriptLaneKindValues : ResolveRealtimeAssistantPlaybackSnapshot(TRUE, TRUE, FALSE, FALSE, requested_lane)
    \/ ResolveRealtimeAssistantTurnInterruptedInvalid(FALSE)
    \/ ResolveRealtimeAssistantTurnInterruptedValid(TRUE)
    \/ (phase = "Ready") /\ \E predecessor_materialized \in BOOLEAN : \E item_skipped \in BOOLEAN : \E item_ready \in BOOLEAN : \E item_text_present \in BOOLEAN : \E role \in RealtimeTranscriptRoleKindValues : \E response_id_present \in BOOLEAN : \E completion_present \in BOOLEAN : \E completion_usage_consumed \in BOOLEAN : ResolveRealtimeMaterializeAlreadyDone(TRUE, predecessor_materialized, item_skipped, item_ready, item_text_present, role, response_id_present, completion_present, completion_usage_consumed)
    \/ (phase = "Ready") /\ \E item_skipped \in BOOLEAN : \E item_ready \in BOOLEAN : \E item_text_present \in BOOLEAN : \E role \in RealtimeTranscriptRoleKindValues : \E response_id_present \in BOOLEAN : \E completion_present \in BOOLEAN : \E completion_usage_consumed \in BOOLEAN : ResolveRealtimeMaterializeWaitForPredecessor(FALSE, FALSE, item_skipped, item_ready, item_text_present, role, response_id_present, completion_present, completion_usage_consumed)
    \/ (phase = "Ready") /\ \E item_ready \in BOOLEAN : \E item_text_present \in BOOLEAN : \E role \in RealtimeTranscriptRoleKindValues : \E response_id_present \in BOOLEAN : \E completion_present \in BOOLEAN : \E completion_usage_consumed \in BOOLEAN : ResolveRealtimeMaterializeSkipped(FALSE, TRUE, TRUE, item_ready, item_text_present, role, response_id_present, completion_present, completion_usage_consumed)
    \/ (phase = "Ready") /\ \E item_ready \in BOOLEAN : \E item_text_present \in BOOLEAN : \E role \in RealtimeTranscriptRoleKindValues : \E response_id_present \in BOOLEAN : \E completion_present \in BOOLEAN : \E completion_usage_consumed \in BOOLEAN : ResolveRealtimeMaterializeWaitForReadyText(FALSE, TRUE, FALSE, item_ready, item_text_present, role, response_id_present, completion_present, completion_usage_consumed)
    \/ (phase = "Ready") /\ \E role \in RealtimeTranscriptRoleKindValues : \E response_id_present \in BOOLEAN : \E completion_present \in BOOLEAN : \E completion_usage_consumed \in BOOLEAN : ResolveRealtimeMaterializeUser(FALSE, TRUE, FALSE, TRUE, TRUE, role, response_id_present, completion_present, completion_usage_consumed)
    \/ (phase = "Ready") /\ \E role \in RealtimeTranscriptRoleKindValues : \E completion_usage_consumed \in BOOLEAN : ResolveRealtimeMaterializeAssistant(FALSE, TRUE, FALSE, TRUE, TRUE, role, TRUE, TRUE, completion_usage_consumed)
    \/ (phase = "Ready") /\ \E role \in RealtimeTranscriptRoleKindValues : \E response_id_present \in BOOLEAN : \E completion_present \in BOOLEAN : \E completion_usage_consumed \in BOOLEAN : ResolveRealtimeMaterializeAssistantMissingCompletion(FALSE, TRUE, FALSE, TRUE, TRUE, role, response_id_present, completion_present, completion_usage_consumed)
    \/ (phase = "Ready") /\ \E item_count \in 0..2 : \E first_seen_count \in 0..2 : \E first_seen_unique_count \in 0..2 : AuthorizeRestoreRealtimeTranscriptState(item_count, first_seen_count, first_seen_unique_count, TRUE, TRUE, TRUE, TRUE, TRUE, TRUE, TRUE, TRUE, TRUE, TRUE, TRUE, TRUE, TRUE, TRUE, TRUE, TRUE, TRUE, TRUE, TRUE, TRUE, TRUE, TRUE, TRUE, TRUE)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : \E channel_id \in StringValues : \E interaction_id \in StringValues : AdmitLiveInteractionTranscript(session_id, channel_id, interaction_id)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : \E channel_id \in StringValues : \E interaction_id \in StringValues : StageLiveProvisionalUserTranscript(session_id, channel_id, interaction_id, TRUE)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : \E channel_id \in StringValues : \E interaction_id \in StringValues : \E reconciliation \in LiveTranscriptReconciliationValues : ReconcileLiveFinalUserTranscript(session_id, channel_id, interaction_id, reconciliation)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : \E channel_id \in StringValues : \E interaction_id \in StringValues : CompleteLiveInteractionTranscript(session_id, channel_id, interaction_id)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : \E channel_id \in StringValues : \E interaction_id \in StringValues : \E response_id \in StringValues : \E item_id \in StringValues : \E content_index \in 0..2 : AdmitLiveAssistantPlaybackTarget(session_id, channel_id, interaction_id, response_id, item_id, content_index)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : \E channel_id \in StringValues : \E interaction_id \in StringValues : \E response_id \in StringValues : \E item_id \in StringValues : \E content_index \in 0..2 : RecoverLiveAssistantPlaybackTarget(session_id, channel_id, interaction_id, response_id, item_id, content_index)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : \E channel_id \in StringValues : \E interaction_id \in StringValues : \E response_id \in StringValues : \E item_id \in StringValues : \E content_index \in 0..2 : ResolveLiveAssistantPlaybackOnChannelClose(session_id, channel_id, interaction_id, response_id, item_id, content_index)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : \E channel_id \in StringValues : \E interaction_id \in StringValues : \E response_id \in StringValues : \E item_id \in StringValues : \E content_index \in 0..2 : \E snapshot_chars \in 0..2 : \E snapshot_digest \in StringValues : \E canonical_chars \in 0..2 : \E canonical_digest \in StringValues : \E prefix_matches_snapshot \in BOOLEAN : \E observation_only \in BOOLEAN : ObserveLiveAssistantPlaybackSnapshot(session_id, channel_id, interaction_id, response_id, item_id, content_index, snapshot_chars, snapshot_digest, canonical_chars, canonical_digest, prefix_matches_snapshot, observation_only)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : \E channel_id \in StringValues : \E interaction_id \in StringValues : \E response_id \in StringValues : \E item_id \in StringValues : \E content_index \in 0..2 : \E authoritative_assistant_chars \in 0..2 : \E authoritative_text_digest \in StringValues : \E pending_terminal_observation \in LiveAssistantPlaybackTerminalObservationValues : \E pending_reported_prefix_chars \in 0..2 : \E pending_reported_prefix_digest \in StringValues : ObserveLiveAssistantPlaybackFinalPendingTerminal(session_id, channel_id, interaction_id, response_id, item_id, content_index, authoritative_assistant_chars, authoritative_text_digest, pending_terminal_observation, pending_reported_prefix_chars, pending_reported_prefix_digest, FALSE)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : \E channel_id \in StringValues : \E interaction_id \in StringValues : \E response_id \in StringValues : \E item_id \in StringValues : \E content_index \in 0..2 : \E authoritative_assistant_chars \in 0..2 : \E authoritative_text_digest \in StringValues : RecoverLiveAssistantPlaybackFinal(session_id, channel_id, interaction_id, response_id, item_id, content_index, authoritative_assistant_chars, authoritative_text_digest)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : \E channel_id \in StringValues : \E interaction_id \in StringValues : \E response_id \in StringValues : \E item_id \in StringValues : \E content_index \in 0..2 : \E observation \in LiveAssistantPlaybackTerminalObservationValues : \E reported_prefix_chars \in 0..2 : \E reported_prefix_digest \in StringValues : \E authoritative_assistant_chars \in 0..2 : \E authoritative_text_digest \in StringValues : ObserveLiveAssistantPlaybackTerminalPendingFinal(session_id, channel_id, interaction_id, response_id, item_id, content_index, observation, reported_prefix_chars, reported_prefix_digest, authoritative_assistant_chars, authoritative_text_digest, FALSE, FALSE)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : \E channel_id \in StringValues : \E interaction_id \in StringValues : \E response_id \in StringValues : \E item_id \in StringValues : \E content_index \in 0..2 : \E observation \in LiveAssistantPlaybackTerminalObservationValues : \E reported_prefix_chars \in 0..2 : \E reported_prefix_digest \in StringValues : RecoverLiveAssistantPlaybackTerminal(session_id, channel_id, interaction_id, response_id, item_id, content_index, observation, reported_prefix_chars, reported_prefix_digest)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : \E channel_id \in StringValues : \E interaction_id \in StringValues : \E response_id \in StringValues : \E item_id \in StringValues : \E content_index \in 0..2 : \E authoritative_assistant_chars \in 0..2 : \E authoritative_text_digest \in StringValues : \E pending_terminal_observation \in LiveAssistantPlaybackTerminalObservationValues : \E pending_reported_prefix_chars \in 0..2 : \E pending_reported_prefix_digest \in StringValues : ObserveLiveAssistantPlaybackFinalJoinsComplete(session_id, channel_id, interaction_id, response_id, item_id, content_index, authoritative_assistant_chars, authoritative_text_digest, pending_terminal_observation, pending_reported_prefix_chars, pending_reported_prefix_digest, FALSE)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : \E channel_id \in StringValues : \E interaction_id \in StringValues : \E response_id \in StringValues : \E item_id \in StringValues : \E content_index \in 0..2 : \E authoritative_assistant_chars \in 0..2 : \E authoritative_text_digest \in StringValues : \E pending_terminal_observation \in LiveAssistantPlaybackTerminalObservationValues : \E pending_reported_prefix_chars \in 0..2 : \E pending_reported_prefix_digest \in StringValues : ObserveLiveAssistantPlaybackFinalJoinsPrefix(session_id, channel_id, interaction_id, response_id, item_id, content_index, authoritative_assistant_chars, authoritative_text_digest, pending_terminal_observation, pending_reported_prefix_chars, pending_reported_prefix_digest, TRUE)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : \E channel_id \in StringValues : \E interaction_id \in StringValues : \E response_id \in StringValues : \E item_id \in StringValues : \E content_index \in 0..2 : \E observation \in LiveAssistantPlaybackTerminalObservationValues : \E reported_prefix_chars \in 0..2 : \E reported_prefix_digest \in StringValues : \E authoritative_assistant_chars \in 0..2 : \E authoritative_text_digest \in StringValues : ObserveLiveAssistantPlaybackTerminalJoinsComplete(session_id, channel_id, interaction_id, response_id, item_id, content_index, observation, reported_prefix_chars, reported_prefix_digest, authoritative_assistant_chars, authoritative_text_digest, TRUE, FALSE)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : \E channel_id \in StringValues : \E interaction_id \in StringValues : \E response_id \in StringValues : \E item_id \in StringValues : \E content_index \in 0..2 : \E observation \in LiveAssistantPlaybackTerminalObservationValues : \E reported_prefix_chars \in 0..2 : \E reported_prefix_digest \in StringValues : \E authoritative_assistant_chars \in 0..2 : \E authoritative_text_digest \in StringValues : ObserveLiveAssistantPlaybackTerminalJoinsPrefix(session_id, channel_id, interaction_id, response_id, item_id, content_index, observation, reported_prefix_chars, reported_prefix_digest, authoritative_assistant_chars, authoritative_text_digest, TRUE, TRUE)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : \E channel_id \in StringValues : \E interaction_id \in StringValues : \E response_id \in StringValues : \E item_id \in StringValues : \E content_index \in 0..2 : \E observation \in LiveAssistantPlaybackTerminalObservationValues : \E reported_prefix_chars \in 0..2 : \E reported_prefix_digest \in StringValues : \E authoritative_assistant_chars \in 0..2 : \E authoritative_text_digest \in StringValues : \E authoritative_assistant_final \in BOOLEAN : ObserveLiveAssistantPlaybackUnmeasured(session_id, channel_id, interaction_id, response_id, item_id, content_index, observation, reported_prefix_chars, reported_prefix_digest, authoritative_assistant_chars, authoritative_text_digest, authoritative_assistant_final, FALSE)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : \E canonical_row_sequence \in 0..2 : \E row_kind \in LiveContextCommittedRowKindValues : \E provenance \in LiveContextCommittedTextProvenanceValues : \E content_digest \in StringValues : \E store_commit_authority \in StringValues : ClassifyLiveContextCommittedRow(session_id, canonical_row_sequence, row_kind, provenance, content_digest, store_commit_authority)
    \/ (phase = "Ready") /\ \E schema_version \in 0..2 : AuthorizeSessionMetadataPersist(schema_version, TRUE)
    \/ (phase = "Ready") /\ \E mob_tool_authority_context_present \in BOOLEAN : \E mob_tool_authority_context_generated \in BOOLEAN : AuthorizeSessionBuildStatePersist(mob_tool_authority_context_present, mob_tool_authority_context_generated)
    \/ RestoreSessionBuildState
    \/ (phase = "Ready") /\ \E session_tail \in ObservedSessionTailKindValues : \E staged_tool_result_count \in 0..2 : ResolvePendingContinuationWithBoundary(session_tail, staged_tool_result_count)
    \/ (phase = "Ready") /\ \E session_tail \in ObservedSessionTailKindValues : \E staged_tool_result_count \in 0..2 : ResolvePendingContinuationWithoutBoundary(session_tail, staged_tool_result_count)
    \/ (phase = "Ready") /\ \E provider_override_present \in BOOLEAN : \E model_override_present \in BOOLEAN : \E self_hosted_server_override_present \in BOOLEAN : \E has_build_only_overrides \in BOOLEAN : \E first_turn_phase \in SessionFirstTurnPhaseValues : AuthorizeSessionResumeOverridesRejectProviderRequiresModel(provider_override_present, model_override_present, self_hosted_server_override_present, has_build_only_overrides, first_turn_phase)
    \/ (phase = "Ready") /\ \E provider_override_present \in BOOLEAN : \E model_override_present \in BOOLEAN : \E self_hosted_server_override_present \in BOOLEAN : \E has_build_only_overrides \in BOOLEAN : \E first_turn_phase \in SessionFirstTurnPhaseValues : AuthorizeSessionResumeOverridesRejectBuildOnlyAfterFirstTurn(provider_override_present, model_override_present, self_hosted_server_override_present, has_build_only_overrides, first_turn_phase)
    \/ (phase = "Ready") /\ \E provider_override_present \in BOOLEAN : \E model_override_present \in BOOLEAN : \E has_build_only_overrides \in BOOLEAN : \E first_turn_phase \in SessionFirstTurnPhaseValues : AuthorizeSessionResumeOverridesAcceptRecomputeProvider(provider_override_present, model_override_present, FALSE, has_build_only_overrides, first_turn_phase)
    \/ (phase = "Ready") /\ \E provider_override_present \in BOOLEAN : \E model_override_present \in BOOLEAN : \E has_build_only_overrides \in BOOLEAN : \E first_turn_phase \in SessionFirstTurnPhaseValues : AuthorizeSessionResumeOverridesAcceptRecomputeProviderWithSelfHostedOverride(provider_override_present, model_override_present, TRUE, has_build_only_overrides, first_turn_phase)
    \/ (phase = "Ready") /\ \E model_override_present \in BOOLEAN : \E has_build_only_overrides \in BOOLEAN : \E first_turn_phase \in SessionFirstTurnPhaseValues : AuthorizeSessionResumeOverridesAcceptUseOverride(TRUE, model_override_present, FALSE, has_build_only_overrides, first_turn_phase)
    \/ (phase = "Ready") /\ \E model_override_present \in BOOLEAN : \E has_build_only_overrides \in BOOLEAN : \E first_turn_phase \in SessionFirstTurnPhaseValues : AuthorizeSessionResumeOverridesAcceptUseOverrideWithSelfHostedOverride(TRUE, model_override_present, TRUE, has_build_only_overrides, first_turn_phase)
    \/ (phase = "Ready") /\ \E model_override_present \in BOOLEAN : \E has_build_only_overrides \in BOOLEAN : \E first_turn_phase \in SessionFirstTurnPhaseValues : AuthorizeSessionResumeOverridesAcceptRetainStored(FALSE, model_override_present, FALSE, has_build_only_overrides, first_turn_phase)
    \/ (phase = "Ready") /\ \E model_override_present \in BOOLEAN : \E has_build_only_overrides \in BOOLEAN : \E first_turn_phase \in SessionFirstTurnPhaseValues : AuthorizeSessionResumeOverridesAcceptRetainStoredWithSelfHostedOverride(FALSE, model_override_present, TRUE, has_build_only_overrides, first_turn_phase)
    \/ ClassifyLiveSessionAuthorityLive(FALSE, FALSE, FALSE)
    \/ (phase = "Ready") /\ \E stored_transcript_diverged \in BOOLEAN : \E live_has_uncommitted_transcript \in BOOLEAN : ClassifyLiveSessionAuthorityDurableArchived(stored_transcript_diverged, live_has_uncommitted_transcript, TRUE)
    \/ (phase = "Ready") /\ \E stored_transcript_diverged \in BOOLEAN : ClassifyLiveSessionAuthorityDurableUncommitted(stored_transcript_diverged, TRUE, FALSE)
    \/ ClassifyLiveSessionAuthorityDurableRevision(TRUE, FALSE, FALSE)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : \E has_metadata \in BOOLEAN : \E has_build_state \in BOOLEAN : \E runtime_projection_quarantined \in BOOLEAN : RecoverSessionFromStoreAuthorized(session_id, has_metadata, has_build_state, runtime_projection_quarantined)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : \E has_metadata \in BOOLEAN : \E has_build_state \in BOOLEAN : \E runtime_projection_quarantined \in BOOLEAN : RecoverSessionFromStoreUnrecoverable(session_id, has_metadata, has_build_state, runtime_projection_quarantined)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : \E candidate_id \in RecoveryCandidateIdValues : \E relation \in DurableHeadRelationValues : \E run_id_cardinality \in RunIdCardinalityValues : \E terminal_stop_reason \in DurableTailStopReasonValues : \E dangling_tool_use_count \in InputSamples_ClassifyDurableTail_dangling_tool_use_count : \E orphan_tool_result_count \in InputSamples_ClassifyDurableTail_orphan_tool_result_count : \E run_cursor \in DurableTailRunCursorValues : \E cursor_inputs_open \in BOOLEAN : ClassifyDurableTailCompleted(session_id, candidate_id, relation, run_id_cardinality, terminal_stop_reason, dangling_tool_use_count, orphan_tool_result_count, FALSE, run_cursor, cursor_inputs_open)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : \E candidate_id \in RecoveryCandidateIdValues : \E relation \in DurableHeadRelationValues : \E run_id_cardinality \in RunIdCardinalityValues : \E terminal_stop_reason \in DurableTailStopReasonValues : \E dangling_tool_use_count \in InputSamples_ClassifyDurableTail_dangling_tool_use_count : \E orphan_tool_result_count \in InputSamples_ClassifyDurableTail_orphan_tool_result_count : \E run_cursor \in DurableTailRunCursorValues : \E cursor_inputs_open \in BOOLEAN : ClassifyDurableTailRepairable(session_id, candidate_id, relation, run_id_cardinality, terminal_stop_reason, dangling_tool_use_count, orphan_tool_result_count, FALSE, run_cursor, cursor_inputs_open)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : \E candidate_id \in RecoveryCandidateIdValues : \E relation \in DurableHeadRelationValues : \E run_id_cardinality \in RunIdCardinalityValues : \E terminal_stop_reason \in DurableTailStopReasonValues : \E dangling_tool_use_count \in InputSamples_ClassifyDurableTail_dangling_tool_use_count : \E orphan_tool_result_count \in InputSamples_ClassifyDurableTail_orphan_tool_result_count : \E run_cursor \in DurableTailRunCursorValues : ClassifyDurableTailContinuable(session_id, candidate_id, relation, run_id_cardinality, terminal_stop_reason, dangling_tool_use_count, orphan_tool_result_count, FALSE, run_cursor, TRUE)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : \E candidate_id \in RecoveryCandidateIdValues : \E relation \in DurableHeadRelationValues : \E run_id_cardinality \in RunIdCardinalityValues : \E terminal_stop_reason \in DurableTailStopReasonValues : \E dangling_tool_use_count \in InputSamples_ClassifyDurableTail_dangling_tool_use_count : \E orphan_tool_result_count \in InputSamples_ClassifyDurableTail_orphan_tool_result_count : \E messages_after_terminal \in BOOLEAN : \E run_cursor \in DurableTailRunCursorValues : \E cursor_inputs_open \in BOOLEAN : ClassifyDurableTailAmbiguous(session_id, candidate_id, relation, run_id_cardinality, terminal_stop_reason, dangling_tool_use_count, orphan_tool_result_count, messages_after_terminal, run_cursor, cursor_inputs_open)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : ResolveRuntimeCheckpointProjectionActive(session_id)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : ResolveRuntimeCheckpointProjectionArchived(session_id)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : \E authority_archived \in BOOLEAN : \E candidate_archived \in BOOLEAN : ResolveSessionDocumentLifecycleMergeArchivedAbsorbing(session_id, authority_archived, candidate_archived)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : ResolveSessionDocumentLifecycleMergeAuthority(session_id, FALSE, FALSE)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : \E result_count \in 0..2 : ApplyPendingToolResults(session_id, result_count)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : \E fork_or_rewrite_directive \in TranscriptEditKindValues : TranscriptEditFork(session_id, fork_or_rewrite_directive)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : \E fork_or_rewrite_directive \in TranscriptEditKindValues : TranscriptEditRewrite(session_id, fork_or_rewrite_directive)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : \E terminal \in SessionDocumentLifecycleValues : RecoverSessionLifecycleTerminal(session_id, terminal)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : ReviveArchivedSessionDocument(session_id)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : \E runtime_backed \in BOOLEAN : \E durable_document_present \in BOOLEAN : \E runtime_observation \in SessionArchiveRuntimeObservationValues : ArchiveSessionDocumentActive(session_id, runtime_backed, durable_document_present, runtime_observation)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : \E runtime_backed \in BOOLEAN : \E durable_document_present \in BOOLEAN : \E runtime_observation \in SessionArchiveRuntimeObservationValues : ArchiveSessionDocumentAlreadyArchived(session_id, runtime_backed, durable_document_present, runtime_observation)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : \E runtime_backed \in BOOLEAN : \E durable_document_present \in BOOLEAN : \E runtime_observation \in SessionArchiveRuntimeObservationValues : ArchiveSessionDocumentCompleteRetire(session_id, runtime_backed, durable_document_present, runtime_observation)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : \E owner \in InstructionTargetOwnerValues : \E owner_mob \in StringValues : \E target_generation \in InputSamples_RecoverSessionInstructionState_target_generation : \E entry_sequence \in InputSamples_RecoverSessionInstructionState_entry_sequence : RecoverSessionInstructionState(session_id, owner, owner_mob, target_generation, entry_sequence)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : \E revision \in MapStringU64Values : \E effect \in MapStringInstructionEffectValues : \E digest \in MapStringStringValues : \E bytes \in MapStringU64Values : RecoverSessionInstructionTarget(session_id, revision, effect, digest, bytes)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : \E revision \in MapStringU64Values : \E effect \in MapStringInstructionEffectValues : \E digest \in MapStringStringValues : \E bytes \in MapStringU64Values : \E generation \in InputSamples_RecoverSessionInstructionEffective_generation : \E last_entry_sequence \in InputSamples_RecoverSessionInstructionEffective_last_entry_sequence : RecoverSessionInstructionEffective(session_id, revision, effect, digest, bytes, generation, last_entry_sequence)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : \E mob_id \in StringValues : \E generation \in 0..2 : \E revision \in MapStringU64Values : \E effect \in MapStringInstructionEffectValues : \E digest \in MapStringStringValues : \E bytes \in MapStringU64Values : RecordInstructionTargetAdvance(session_id, mob_id, generation, revision, effect, digest, bytes)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : \E mob_id \in StringValues : \E generation \in 0..2 : \E revision \in MapStringU64Values : \E effect \in MapStringInstructionEffectValues : \E digest \in MapStringStringValues : \E bytes \in MapStringU64Values : RecordInstructionTargetActiveBudgetExceeded(session_id, mob_id, generation, revision, effect, digest, bytes)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : \E mob_id \in StringValues : \E generation \in 0..2 : \E revision \in MapStringU64Values : \E effect \in MapStringInstructionEffectValues : \E digest \in MapStringStringValues : \E bytes \in MapStringU64Values : RecordInstructionTargetSlotLimitExceeded(session_id, mob_id, generation, revision, effect, digest, bytes)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : \E mob_id \in StringValues : \E generation \in 0..2 : \E revision \in MapStringU64Values : \E effect \in MapStringInstructionEffectValues : \E digest \in MapStringStringValues : \E bytes \in MapStringU64Values : RecordInstructionTargetAlreadyRecorded(session_id, mob_id, generation, revision, effect, digest, bytes)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : \E mob_id \in StringValues : \E generation \in 0..2 : \E revision \in MapStringU64Values : \E effect \in MapStringInstructionEffectValues : \E digest \in MapStringStringValues : \E bytes \in MapStringU64Values : RecordInstructionTargetOwnedBySession(session_id, mob_id, generation, revision, effect, digest, bytes)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : \E mob_id \in StringValues : \E generation \in 0..2 : \E revision \in MapStringU64Values : \E effect \in MapStringInstructionEffectValues : \E digest \in MapStringStringValues : \E bytes \in MapStringU64Values : RecordInstructionTargetOwnedByOtherMob(session_id, mob_id, generation, revision, effect, digest, bytes)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : \E slot \in StringValues : \E revision \in 0..2 : \E effect \in InstructionEffectValues : \E digest \in StringValues : \E body_bytes \in InputSamples_SetSessionInstruction_body_bytes : SetSessionInstructionNew(session_id, slot, revision, effect, digest, body_bytes)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : \E slot \in StringValues : \E revision \in 0..2 : \E effect \in InstructionEffectValues : \E digest \in StringValues : \E body_bytes \in InputSamples_SetSessionInstruction_body_bytes : SetSessionInstructionAdvance(session_id, slot, revision, effect, digest, body_bytes)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : \E slot \in StringValues : \E revision \in 0..2 : \E effect \in InstructionEffectValues : \E digest \in StringValues : \E body_bytes \in InputSamples_SetSessionInstruction_body_bytes : SetSessionInstructionActiveBudgetExceeded(session_id, slot, revision, effect, digest, body_bytes)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : \E slot \in StringValues : \E revision \in 0..2 : \E effect \in InstructionEffectValues : \E digest \in StringValues : \E body_bytes \in InputSamples_SetSessionInstruction_body_bytes : SetSessionInstructionDuplicate(session_id, slot, revision, effect, digest, body_bytes)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : \E slot \in StringValues : \E revision \in 0..2 : \E effect \in InstructionEffectValues : \E digest \in StringValues : \E body_bytes \in InputSamples_SetSessionInstruction_body_bytes : SetSessionInstructionConflict(session_id, slot, revision, effect, digest, body_bytes)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : \E slot \in StringValues : \E revision \in 0..2 : \E effect \in InstructionEffectValues : \E digest \in StringValues : \E body_bytes \in InputSamples_SetSessionInstruction_body_bytes : SetSessionInstructionStale(session_id, slot, revision, effect, digest, body_bytes)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : \E slot \in StringValues : \E revision \in 0..2 : \E effect \in InstructionEffectValues : \E digest \in StringValues : \E body_bytes \in InputSamples_SetSessionInstruction_body_bytes : SetSessionInstructionOwnedByMob(session_id, slot, revision, effect, digest, body_bytes)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : \E slot \in StringValues : \E revision \in 0..2 : \E effect \in InstructionEffectValues : \E digest \in StringValues : \E body_bytes \in InputSamples_SetSessionInstruction_body_bytes : SetSessionInstructionSlotLimitExceeded(session_id, slot, revision, effect, digest, body_bytes)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : \E slot \in StringValues : \E revision \in 0..2 : \E effect \in InstructionEffectValues : \E digest \in StringValues : \E body_bytes \in InputSamples_SetSessionInstruction_body_bytes : SetSessionInstructionUnknownSlot(session_id, slot, revision, effect, digest, body_bytes)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : \E owner \in InstructionTargetOwnerValues : \E mob_id \in StringValues : RebindInstructionOwner(session_id, owner, mob_id)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : \E owner \in InstructionTargetOwnerValues : \E mob_id \in StringValues : RebindInstructionOwnerUnowned(session_id, owner, mob_id)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : \E mob_id \in StringValues : ReleaseInstructionTarget(session_id, mob_id)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : \E mob_id \in StringValues : ReleaseInstructionTargetAlreadyUnowned(session_id, mob_id)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : \E mob_id \in StringValues : ReleaseInstructionTargetOwnedByOtherMob(session_id, mob_id)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : \E mob_id \in StringValues : ReleaseInstructionTargetOwnedBySession(session_id, mob_id)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : \E run_start \in InstructionRunStartValues : \E lowering \in InstructionLoweringValues : \E transcript_is_system_prefix \in BOOLEAN : ResolveRunStartInstructionsApply(session_id, run_start, lowering, transcript_is_system_prefix)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : \E run_start \in InstructionRunStartValues : \E lowering \in InstructionLoweringValues : \E transcript_is_system_prefix \in BOOLEAN : ResolveRunStartInstructionsNoChange(session_id, run_start, lowering, transcript_is_system_prefix)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : \E run_start \in InstructionRunStartValues : \E lowering \in InstructionLoweringValues : ResolveRunStartInstructionsDeferredLeadingPrefix(session_id, run_start, lowering, FALSE)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : \E run_start \in InstructionRunStartValues : \E lowering \in InstructionLoweringValues : \E transcript_is_system_prefix \in BOOLEAN : ResolveRunStartInstructionsDeferredUnavailable(session_id, run_start, lowering, transcript_is_system_prefix)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : \E lowering \in InstructionLoweringValues : AuthorizeInstructionFoldAtRunStart(session_id, TRUE, lowering)
    \/ (phase = "Ready") /\ \E session_id \in SessionIdValues : \E at_run_start \in BOOLEAN : \E lowering \in InstructionLoweringValues : AuthorizeInstructionFoldEffective(session_id, at_run_start, lowering)

live_interaction_transcript_binding_is_complete == ((DOMAIN session_live_channel_id = DOMAIN session_live_interaction_id) /\ (DOMAIN session_live_channel_id = DOMAIN session_live_transcript_reconciliation) /\ (DOMAIN session_live_channel_id = DOMAIN session_live_provisional_transcript_present))
committed_live_transcript_had_provisional_admission == (\A session_id \in DOMAIN session_live_transcript_reconciliation : (IF ((IF (session_id \in DOMAIN session_live_transcript_reconciliation) THEN Some((IF session_id \in DOMAIN session_live_transcript_reconciliation THEN session_live_transcript_reconciliation[session_id] ELSE "None")) ELSE None) # Some("Committed")) THEN TRUE ELSE ((IF (session_id \in DOMAIN session_live_provisional_transcript_present) THEN Some((IF session_id \in DOMAIN session_live_provisional_transcript_present THEN session_live_provisional_transcript_present[session_id] ELSE FALSE)) ELSE None) = Some(TRUE))))
live_assistant_playback_target_is_complete_and_interaction_bound == ((DOMAIN session_live_assistant_playback_response_id = DOMAIN session_live_assistant_playback_item_id) /\ (DOMAIN session_live_assistant_playback_response_id = DOMAIN session_live_assistant_playback_content_index) /\ (DOMAIN session_live_assistant_final_chars = DOMAIN session_live_assistant_final_digest) /\ (DOMAIN session_live_assistant_terminal_observation = DOMAIN session_live_assistant_terminal_prefix_chars) /\ (DOMAIN session_live_assistant_terminal_observation = DOMAIN session_live_assistant_terminal_prefix_digest) /\ (\A session_id \in DOMAIN session_live_assistant_final_digest : (session_id \in DOMAIN session_live_assistant_playback_response_id)) /\ (\A session_id \in DOMAIN session_live_assistant_terminal_observation : (session_id \in DOMAIN session_live_assistant_playback_response_id)) /\ (\A session_id \in DOMAIN session_live_assistant_playback_response_id : ((session_id \in DOMAIN session_live_channel_id) /\ (session_id \in DOMAIN session_live_interaction_id))))
instruction_state_maps_lockstep == ((DOMAIN session_instruction_owner = DOMAIN session_instruction_owner_mob) /\ (DOMAIN session_instruction_owner = DOMAIN session_instruction_target_generation) /\ (DOMAIN session_instruction_owner = DOMAIN session_instruction_target_revision) /\ (DOMAIN session_instruction_owner = DOMAIN session_instruction_target_effect) /\ (DOMAIN session_instruction_owner = DOMAIN session_instruction_target_digest) /\ (DOMAIN session_instruction_owner = DOMAIN session_instruction_target_bytes) /\ (DOMAIN session_instruction_owner = DOMAIN session_instruction_effective_revision) /\ (DOMAIN session_instruction_owner = DOMAIN session_instruction_effective_effect) /\ (DOMAIN session_instruction_owner = DOMAIN session_instruction_effective_digest) /\ (DOMAIN session_instruction_owner = DOMAIN session_instruction_effective_bytes) /\ (DOMAIN session_instruction_owner = DOMAIN session_instruction_effective_generation) /\ (DOMAIN session_instruction_owner = DOMAIN session_instruction_entry_sequence) /\ (\A session_id \in DOMAIN session_instruction_owner : (instruction_slot_maps_consistent((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_instruction_target_revision) THEN Some((IF session_id \in DOMAIN session_instruction_target_revision THEN session_instruction_target_revision[session_id] ELSE [x \in {} |-> None])) ELSE None) THEN (IF (session_id \in DOMAIN session_instruction_target_revision) THEN Some((IF session_id \in DOMAIN session_instruction_target_revision THEN session_instruction_target_revision[session_id] ELSE [x \in {} |-> None])) ELSE None)["value"] ELSE None), (IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_instruction_target_effect) THEN Some((IF session_id \in DOMAIN session_instruction_target_effect THEN session_instruction_target_effect[session_id] ELSE [x \in {} |-> None])) ELSE None) THEN (IF (session_id \in DOMAIN session_instruction_target_effect) THEN Some((IF session_id \in DOMAIN session_instruction_target_effect THEN session_instruction_target_effect[session_id] ELSE [x \in {} |-> None])) ELSE None)["value"] ELSE None), (IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_instruction_target_digest) THEN Some((IF session_id \in DOMAIN session_instruction_target_digest THEN session_instruction_target_digest[session_id] ELSE [x \in {} |-> None])) ELSE None) THEN (IF (session_id \in DOMAIN session_instruction_target_digest) THEN Some((IF session_id \in DOMAIN session_instruction_target_digest THEN session_instruction_target_digest[session_id] ELSE [x \in {} |-> None])) ELSE None)["value"] ELSE None), (IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_instruction_target_bytes) THEN Some((IF session_id \in DOMAIN session_instruction_target_bytes THEN session_instruction_target_bytes[session_id] ELSE [x \in {} |-> None])) ELSE None) THEN (IF (session_id \in DOMAIN session_instruction_target_bytes) THEN Some((IF session_id \in DOMAIN session_instruction_target_bytes THEN session_instruction_target_bytes[session_id] ELSE [x \in {} |-> None])) ELSE None)["value"] ELSE None)) /\ instruction_slot_maps_consistent((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_instruction_effective_revision) THEN Some((IF session_id \in DOMAIN session_instruction_effective_revision THEN session_instruction_effective_revision[session_id] ELSE [x \in {} |-> None])) ELSE None) THEN (IF (session_id \in DOMAIN session_instruction_effective_revision) THEN Some((IF session_id \in DOMAIN session_instruction_effective_revision THEN session_instruction_effective_revision[session_id] ELSE [x \in {} |-> None])) ELSE None)["value"] ELSE None), (IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_instruction_effective_effect) THEN Some((IF session_id \in DOMAIN session_instruction_effective_effect THEN session_instruction_effective_effect[session_id] ELSE [x \in {} |-> None])) ELSE None) THEN (IF (session_id \in DOMAIN session_instruction_effective_effect) THEN Some((IF session_id \in DOMAIN session_instruction_effective_effect THEN session_instruction_effective_effect[session_id] ELSE [x \in {} |-> None])) ELSE None)["value"] ELSE None), (IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_instruction_effective_digest) THEN Some((IF session_id \in DOMAIN session_instruction_effective_digest THEN session_instruction_effective_digest[session_id] ELSE [x \in {} |-> None])) ELSE None) THEN (IF (session_id \in DOMAIN session_instruction_effective_digest) THEN Some((IF session_id \in DOMAIN session_instruction_effective_digest THEN session_instruction_effective_digest[session_id] ELSE [x \in {} |-> None])) ELSE None)["value"] ELSE None), (IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_instruction_effective_bytes) THEN Some((IF session_id \in DOMAIN session_instruction_effective_bytes THEN session_instruction_effective_bytes[session_id] ELSE [x \in {} |-> None])) ELSE None) THEN (IF (session_id \in DOMAIN session_instruction_effective_bytes) THEN Some((IF session_id \in DOMAIN session_instruction_effective_bytes THEN session_instruction_effective_bytes[session_id] ELSE [x \in {} |-> None])) ELSE None)["value"] ELSE None)))))
instruction_owner_present_with_target == (\A session_id \in DOMAIN session_instruction_owner : (IF ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_instruction_owner) THEN Some((IF session_id \in DOMAIN session_instruction_owner THEN session_instruction_owner[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_instruction_owner) THEN Some((IF session_id \in DOMAIN session_instruction_owner THEN session_instruction_owner[session_id] ELSE "None")) ELSE None)["value"] ELSE None) # "Unowned") THEN TRUE ELSE (((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_instruction_target_generation) THEN Some((IF session_id \in DOMAIN session_instruction_target_generation THEN session_instruction_target_generation[session_id] ELSE 0)) ELSE None) THEN (IF (session_id \in DOMAIN session_instruction_target_generation) THEN Some((IF session_id \in DOMAIN session_instruction_target_generation THEN session_instruction_target_generation[session_id] ELSE 0)) ELSE None)["value"] ELSE None) = 0) /\ instruction_slot_set_empty((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_instruction_target_revision) THEN Some((IF session_id \in DOMAIN session_instruction_target_revision THEN session_instruction_target_revision[session_id] ELSE [x \in {} |-> None])) ELSE None) THEN (IF (session_id \in DOMAIN session_instruction_target_revision) THEN Some((IF session_id \in DOMAIN session_instruction_target_revision THEN session_instruction_target_revision[session_id] ELSE [x \in {} |-> None])) ELSE None)["value"] ELSE None)))))
instruction_slot_count_bounded == (\A session_id \in DOMAIN session_instruction_target_revision : instruction_slot_count_within_limit((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_instruction_target_revision) THEN Some((IF session_id \in DOMAIN session_instruction_target_revision THEN session_instruction_target_revision[session_id] ELSE [x \in {} |-> None])) ELSE None) THEN (IF (session_id \in DOMAIN session_instruction_target_revision) THEN Some((IF session_id \in DOMAIN session_instruction_target_revision THEN session_instruction_target_revision[session_id] ELSE [x \in {} |-> None])) ELSE None)["value"] ELSE None)))
instruction_active_bytes_bounded == (\A session_id \in DOMAIN session_instruction_target_effect : ((1048576 >= session_document_instruction_active_bytes((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_instruction_target_effect) THEN Some((IF session_id \in DOMAIN session_instruction_target_effect THEN session_instruction_target_effect[session_id] ELSE [x \in {} |-> None])) ELSE None) THEN (IF (session_id \in DOMAIN session_instruction_target_effect) THEN Some((IF session_id \in DOMAIN session_instruction_target_effect THEN session_instruction_target_effect[session_id] ELSE [x \in {} |-> None])) ELSE None)["value"] ELSE None), (IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_instruction_target_bytes) THEN Some((IF session_id \in DOMAIN session_instruction_target_bytes THEN session_instruction_target_bytes[session_id] ELSE [x \in {} |-> None])) ELSE None) THEN (IF (session_id \in DOMAIN session_instruction_target_bytes) THEN Some((IF session_id \in DOMAIN session_instruction_target_bytes THEN session_instruction_target_bytes[session_id] ELSE [x \in {} |-> None])) ELSE None)["value"] ELSE None))) /\ (1048576 >= session_document_instruction_active_bytes((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_instruction_effective_effect) THEN Some((IF session_id \in DOMAIN session_instruction_effective_effect THEN session_instruction_effective_effect[session_id] ELSE [x \in {} |-> None])) ELSE None) THEN (IF (session_id \in DOMAIN session_instruction_effective_effect) THEN Some((IF session_id \in DOMAIN session_instruction_effective_effect THEN session_instruction_effective_effect[session_id] ELSE [x \in {} |-> None])) ELSE None)["value"] ELSE None), (IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_instruction_effective_bytes) THEN Some((IF session_id \in DOMAIN session_instruction_effective_bytes THEN session_instruction_effective_bytes[session_id] ELSE [x \in {} |-> None])) ELSE None) THEN (IF (session_id \in DOMAIN session_instruction_effective_bytes) THEN Some((IF session_id \in DOMAIN session_instruction_effective_bytes THEN session_instruction_effective_bytes[session_id] ELSE [x \in {} |-> None])) ELSE None)["value"] ELSE None)))))
instruction_entry_sequence_covers_effective == (\A session_id \in DOMAIN session_instruction_effective_effect : (IF ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_instruction_entry_sequence) THEN Some((IF session_id \in DOMAIN session_instruction_entry_sequence THEN session_instruction_entry_sequence[session_id] ELSE 0)) ELSE None) THEN (IF (session_id \in DOMAIN session_instruction_entry_sequence) THEN Some((IF session_id \in DOMAIN session_instruction_entry_sequence THEN session_instruction_entry_sequence[session_id] ELSE 0)) ELSE None)["value"] ELSE None) >= 1) THEN TRUE ELSE (\A slot \in DOMAIN (IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_instruction_effective_effect) THEN Some((IF session_id \in DOMAIN session_instruction_effective_effect THEN session_instruction_effective_effect[session_id] ELSE [x \in {} |-> None])) ELSE None) THEN (IF (session_id \in DOMAIN session_instruction_effective_effect) THEN Some((IF session_id \in DOMAIN session_instruction_effective_effect THEN session_instruction_effective_effect[session_id] ELSE [x \in {} |-> None])) ELSE None)["value"] ELSE None) : ((IF "value" \in DOMAIN (IF (slot \in DOMAIN (IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_instruction_effective_effect) THEN Some((IF session_id \in DOMAIN session_instruction_effective_effect THEN session_instruction_effective_effect[session_id] ELSE [x \in {} |-> None])) ELSE None) THEN (IF (session_id \in DOMAIN session_instruction_effective_effect) THEN Some((IF session_id \in DOMAIN session_instruction_effective_effect THEN session_instruction_effective_effect[session_id] ELSE [x \in {} |-> None])) ELSE None)["value"] ELSE None)) THEN Some((IF slot \in DOMAIN (IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_instruction_effective_effect) THEN Some((IF session_id \in DOMAIN session_instruction_effective_effect THEN session_instruction_effective_effect[session_id] ELSE [x \in {} |-> None])) ELSE None) THEN (IF (session_id \in DOMAIN session_instruction_effective_effect) THEN Some((IF session_id \in DOMAIN session_instruction_effective_effect THEN session_instruction_effective_effect[session_id] ELSE [x \in {} |-> None])) ELSE None)["value"] ELSE None) THEN (IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_instruction_effective_effect) THEN Some((IF session_id \in DOMAIN session_instruction_effective_effect THEN session_instruction_effective_effect[session_id] ELSE [x \in {} |-> None])) ELSE None) THEN (IF (session_id \in DOMAIN session_instruction_effective_effect) THEN Some((IF session_id \in DOMAIN session_instruction_effective_effect THEN session_instruction_effective_effect[session_id] ELSE [x \in {} |-> None])) ELSE None)["value"] ELSE None)[slot] ELSE None)) ELSE None) THEN (IF (slot \in DOMAIN (IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_instruction_effective_effect) THEN Some((IF session_id \in DOMAIN session_instruction_effective_effect THEN session_instruction_effective_effect[session_id] ELSE [x \in {} |-> None])) ELSE None) THEN (IF (session_id \in DOMAIN session_instruction_effective_effect) THEN Some((IF session_id \in DOMAIN session_instruction_effective_effect THEN session_instruction_effective_effect[session_id] ELSE [x \in {} |-> None])) ELSE None)["value"] ELSE None)) THEN Some((IF slot \in DOMAIN (IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_instruction_effective_effect) THEN Some((IF session_id \in DOMAIN session_instruction_effective_effect THEN session_instruction_effective_effect[session_id] ELSE [x \in {} |-> None])) ELSE None) THEN (IF (session_id \in DOMAIN session_instruction_effective_effect) THEN Some((IF session_id \in DOMAIN session_instruction_effective_effect THEN session_instruction_effective_effect[session_id] ELSE [x \in {} |-> None])) ELSE None)["value"] ELSE None) THEN (IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_instruction_effective_effect) THEN Some((IF session_id \in DOMAIN session_instruction_effective_effect THEN session_instruction_effective_effect[session_id] ELSE [x \in {} |-> None])) ELSE None) THEN (IF (session_id \in DOMAIN session_instruction_effective_effect) THEN Some((IF session_id \in DOMAIN session_instruction_effective_effect THEN session_instruction_effective_effect[session_id] ELSE [x \in {} |-> None])) ELSE None)["value"] ELSE None)[slot] ELSE None)) ELSE None)["value"] ELSE None) # "Active"))))
instruction_mob_owner_names_its_mob == (\A session_id \in DOMAIN session_instruction_owner : (IF (((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_instruction_owner) THEN Some((IF session_id \in DOMAIN session_instruction_owner THEN session_instruction_owner[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_instruction_owner) THEN Some((IF session_id \in DOMAIN session_instruction_owner THEN session_instruction_owner[session_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Mob") /\ ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_instruction_owner_mob) THEN Some((IF session_id \in DOMAIN session_instruction_owner_mob THEN session_instruction_owner_mob[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_instruction_owner_mob) THEN Some((IF session_id \in DOMAIN session_instruction_owner_mob THEN session_instruction_owner_mob[session_id] ELSE "None")) ELSE None)["value"] ELSE None) # "")) THEN TRUE ELSE (((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_instruction_owner) THEN Some((IF session_id \in DOMAIN session_instruction_owner THEN session_instruction_owner[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_instruction_owner) THEN Some((IF session_id \in DOMAIN session_instruction_owner THEN session_instruction_owner[session_id] ELSE "None")) ELSE None)["value"] ELSE None) # "Mob") /\ ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_instruction_owner_mob) THEN Some((IF session_id \in DOMAIN session_instruction_owner_mob THEN session_instruction_owner_mob[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_instruction_owner_mob) THEN Some((IF session_id \in DOMAIN session_instruction_owner_mob THEN session_instruction_owner_mob[session_id] ELSE "None")) ELSE None)["value"] ELSE None) = ""))))

CiStateConstraint == /\ model_step_count <= 5 /\ Cardinality(DOMAIN session_first_turn_phase) <= 1 /\ Cardinality(DOMAIN session_pending_initial_prompt_present) <= 1 /\ Cardinality(DOMAIN session_pending_tool_results_count) <= 1 /\ Cardinality(DOMAIN session_lifecycle_terminal) <= 1 /\ Cardinality(DOMAIN session_live_channel_id) <= 1 /\ Cardinality(DOMAIN session_live_interaction_id) <= 1 /\ Cardinality(DOMAIN session_live_transcript_reconciliation) <= 1 /\ Cardinality(DOMAIN session_live_provisional_transcript_present) <= 1 /\ Cardinality(DOMAIN session_live_assistant_playback_response_id) <= 1 /\ Cardinality(DOMAIN session_live_assistant_playback_item_id) <= 1 /\ Cardinality(DOMAIN session_live_assistant_playback_content_index) <= 1 /\ Cardinality(DOMAIN session_live_assistant_final_chars) <= 1 /\ Cardinality(DOMAIN session_live_assistant_final_digest) <= 1 /\ Cardinality(DOMAIN session_live_assistant_terminal_observation) <= 1 /\ Cardinality(DOMAIN session_live_assistant_terminal_prefix_chars) <= 1 /\ Cardinality(DOMAIN session_live_assistant_terminal_prefix_digest) <= 1 /\ Cardinality(DOMAIN session_instruction_owner) <= 1 /\ Cardinality(DOMAIN session_instruction_owner_mob) <= 1 /\ Cardinality(DOMAIN session_instruction_target_generation) <= 1 /\ Cardinality(DOMAIN session_instruction_target_revision) <= 1 /\ Cardinality(DOMAIN session_instruction_target_effect) <= 1 /\ Cardinality(DOMAIN session_instruction_target_digest) <= 1 /\ Cardinality(DOMAIN session_instruction_target_bytes) <= 1 /\ Cardinality(DOMAIN session_instruction_effective_revision) <= 1 /\ Cardinality(DOMAIN session_instruction_effective_effect) <= 1 /\ Cardinality(DOMAIN session_instruction_effective_digest) <= 1 /\ Cardinality(DOMAIN session_instruction_effective_bytes) <= 1 /\ Cardinality(DOMAIN session_instruction_effective_generation) <= 1 /\ Cardinality(DOMAIN session_instruction_entry_sequence) <= 1
DeepStateConstraint == /\ model_step_count <= 8 /\ Cardinality(DOMAIN session_first_turn_phase) <= 2 /\ Cardinality(DOMAIN session_pending_initial_prompt_present) <= 2 /\ Cardinality(DOMAIN session_pending_tool_results_count) <= 2 /\ Cardinality(DOMAIN session_lifecycle_terminal) <= 2 /\ Cardinality(DOMAIN session_live_channel_id) <= 2 /\ Cardinality(DOMAIN session_live_interaction_id) <= 2 /\ Cardinality(DOMAIN session_live_transcript_reconciliation) <= 2 /\ Cardinality(DOMAIN session_live_provisional_transcript_present) <= 2 /\ Cardinality(DOMAIN session_live_assistant_playback_response_id) <= 2 /\ Cardinality(DOMAIN session_live_assistant_playback_item_id) <= 2 /\ Cardinality(DOMAIN session_live_assistant_playback_content_index) <= 2 /\ Cardinality(DOMAIN session_live_assistant_final_chars) <= 2 /\ Cardinality(DOMAIN session_live_assistant_final_digest) <= 2 /\ Cardinality(DOMAIN session_live_assistant_terminal_observation) <= 2 /\ Cardinality(DOMAIN session_live_assistant_terminal_prefix_chars) <= 2 /\ Cardinality(DOMAIN session_live_assistant_terminal_prefix_digest) <= 2 /\ Cardinality(DOMAIN session_instruction_owner) <= 2 /\ Cardinality(DOMAIN session_instruction_owner_mob) <= 2 /\ Cardinality(DOMAIN session_instruction_target_generation) <= 2 /\ Cardinality(DOMAIN session_instruction_target_revision) <= 2 /\ Cardinality(DOMAIN session_instruction_target_effect) <= 2 /\ Cardinality(DOMAIN session_instruction_target_digest) <= 2 /\ Cardinality(DOMAIN session_instruction_target_bytes) <= 2 /\ Cardinality(DOMAIN session_instruction_effective_revision) <= 2 /\ Cardinality(DOMAIN session_instruction_effective_effect) <= 2 /\ Cardinality(DOMAIN session_instruction_effective_digest) <= 2 /\ Cardinality(DOMAIN session_instruction_effective_bytes) <= 2 /\ Cardinality(DOMAIN session_instruction_effective_generation) <= 2 /\ Cardinality(DOMAIN session_instruction_entry_sequence) <= 2

Spec == Init /\ [][Next]_vars

THEOREM Spec => []live_interaction_transcript_binding_is_complete
THEOREM Spec => []committed_live_transcript_had_provisional_admission
THEOREM Spec => []live_assistant_playback_target_is_complete_and_interaction_bound
THEOREM Spec => []instruction_state_maps_lockstep
THEOREM Spec => []instruction_owner_present_with_target
THEOREM Spec => []instruction_slot_count_bounded
THEOREM Spec => []instruction_active_bytes_bounded
THEOREM Spec => []instruction_entry_sequence_covers_effective
THEOREM Spec => []instruction_mob_owner_names_its_mob

=============================================================================
