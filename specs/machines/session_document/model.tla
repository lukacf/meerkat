---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for SessionDocumentMachine.

CONSTANTS BooleanValues, LiveSessionAuthorityKindValues, LiveSessionAuthorityReasonValues, NatValues, ObservedSessionTailKindValues, PendingContinuationDispositionValues, PendingContinuationPublicTerminalValues, RealtimeTranscriptLaneKindValues, RealtimeTranscriptMaterializeDecisionValues, RealtimeTranscriptRoleKindValues, RealtimeTranscriptStopReasonKindValues, ResumeOverrideRejectionValues, ResumeProviderSelectionValues, ResumeSelfHostedSelectionValues, SessionFirstTurnPhaseValues, SessionIdValues, SessionInitialPromptStageDecisionValues, SessionSystemPromptSourceValues, SystemContextAppendDecisionValues, SystemContextPersistAppendAdmissionValues, SystemContextSourceValues, TranscriptEditKindValues

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

MapSessionIdBoolValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in SessionIdValues, v \in BOOLEAN }
MapSessionIdSessionFirstTurnPhaseValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in SessionIdValues, v \in SessionFirstTurnPhaseValues }
MapSessionIdU64Values == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in SessionIdValues, v \in NatValues }

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

VARIABLES phase, model_step_count, session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count

vars == << phase, model_step_count, session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>

store_projection_can_recover_authority(has_metadata, has_build_state) == (IF has_metadata THEN TRUE ELSE has_build_state)
resume_provider_recompute_from_model(model_override_present, provider_override_present) == (model_override_present /\ (provider_override_present = FALSE))
resume_reject_provider_requires_model(provider_override_present, model_override_present) == (provider_override_present /\ (model_override_present = FALSE))
tail_has_pending_boundary(session_tail) == (IF (session_tail = "User") THEN TRUE ELSE (session_tail = "ToolResults"))
realtime_stop_reason_records_completion(stop_reason) == (stop_reason = "Other")
realtime_stop_reason_removes_completion(stop_reason) == (stop_reason = "ToolUse")
realtime_stop_reason_discards(stop_reason) == (stop_reason = "Cancelled")
realtime_should_mark_ready_after_write(response_completed, text_after_write_present) == (response_completed /\ text_after_write_present)
realtime_lane_accepts(item_has_text, current_lane, requested_lane) == (IF (current_lane = requested_lane) THEN TRUE ELSE (item_has_text = FALSE))
realtime_delta_is_duplicate(delta_id_present, delta_id_seen) == (delta_id_present /\ delta_id_seen)
persist_append_is_admissible(has_previous, content_identical, content_extends_previous, appended_starts_with_separator, incoming_is_runtime_context_append) == (IF (has_previous /\ content_identical) THEN TRUE ELSE (IF (has_previous /\ content_extends_previous /\ appended_starts_with_separator /\ incoming_is_runtime_context_append) THEN TRUE ELSE ((has_previous = FALSE) /\ incoming_is_runtime_context_append)))
append_is_new(idempotency_key_present, existing_key_matches, existing_key_conflicts) == (IF (idempotency_key_present = FALSE) THEN TRUE ELSE ((existing_key_matches = FALSE) /\ (existing_key_conflicts = FALSE)))
append_is_duplicate(idempotency_key_present, existing_key_matches, existing_key_conflicts) == (idempotency_key_present /\ existing_key_matches /\ (existing_key_conflicts = FALSE))
append_is_conflict(idempotency_key_present, existing_key_conflicts) == (idempotency_key_present /\ existing_key_conflicts)
append_is_empty(trimmed_text_byte_count) == (trimmed_text_byte_count = 0)
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

MarkSessionInitialTurnPendingInactiveOrPending(session_id) ==
    /\ phase = "Ready"
    /\ (IF ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Inactive") THEN TRUE ELSE ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Pending"))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_first_turn_phase' = MapSet(session_first_turn_phase, session_id, "Pending")
    /\ UNCHANGED << session_pending_initial_prompt_present, session_pending_tool_results_count >>


MarkSessionInitialTurnPendingConsumed(session_id) ==
    /\ phase = "Ready"
    /\ ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Consumed")
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


StartSessionInitialTurnPending(session_id) ==
    /\ phase = "Ready"
    /\ ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Pending")
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_first_turn_phase' = MapSet(session_first_turn_phase, session_id, "Consumed")
    /\ UNCHANGED << session_pending_initial_prompt_present, session_pending_tool_results_count >>


StartSessionInitialTurnInactive(session_id) ==
    /\ phase = "Ready"
    /\ ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Inactive")
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


StartSessionInitialTurnConsumed(session_id) ==
    /\ phase = "Ready"
    /\ ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Consumed")
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


ResolveSessionFirstTurnOverridesAllowed(session_id) ==
    /\ phase = "Ready"
    /\ phase_allows_initial_turn_overrides((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


ResolveSessionFirstTurnOverridesDenied(session_id) ==
    /\ phase = "Ready"
    /\ (phase_allows_initial_turn_overrides((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None)) = FALSE)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


StageSessionInitialPromptStore(session_id, prompt_has_content) ==
    /\ phase = "Ready"
    /\ should_store_initial_prompt((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None), prompt_has_content)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_pending_initial_prompt_present' = MapSet(session_pending_initial_prompt_present, session_id, TRUE)
    /\ UNCHANGED << session_first_turn_phase, session_pending_tool_results_count >>


StageSessionInitialPromptClear(session_id, prompt_has_content) ==
    /\ phase = "Ready"
    /\ (should_store_initial_prompt((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None), prompt_has_content) = FALSE)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_pending_initial_prompt_present' = MapSet(session_pending_initial_prompt_present, session_id, FALSE)
    /\ UNCHANGED << session_first_turn_phase, session_pending_tool_results_count >>


StageSessionToolResults(session_id, result_count) ==
    /\ phase = "Ready"
    /\ (IF ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Inactive") THEN TRUE ELSE (IF ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Pending") THEN TRUE ELSE ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Consumed")))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_pending_tool_results_count' = MapSet(session_pending_tool_results_count, session_id, result_count)
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present >>


ConsumeSessionDeferredInputsPending(session_id) ==
    /\ phase = "Ready"
    /\ ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Pending")
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_first_turn_phase' = MapSet(session_first_turn_phase, session_id, "Consumed")
    /\ session_pending_initial_prompt_present' = MapSet(session_pending_initial_prompt_present, session_id, FALSE)
    /\ session_pending_tool_results_count' = MapSet(session_pending_tool_results_count, session_id, 0)


ConsumeSessionDeferredInputsInactive(session_id) ==
    /\ phase = "Ready"
    /\ ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Inactive")
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_pending_initial_prompt_present' = MapSet(session_pending_initial_prompt_present, session_id, FALSE)
    /\ session_pending_tool_results_count' = MapSet(session_pending_tool_results_count, session_id, 0)
    /\ UNCHANGED << session_first_turn_phase >>


ConsumeSessionDeferredInputsConsumed(session_id) ==
    /\ phase = "Ready"
    /\ ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Consumed")
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_pending_initial_prompt_present' = MapSet(session_pending_initial_prompt_present, session_id, FALSE)
    /\ session_pending_tool_results_count' = MapSet(session_pending_tool_results_count, session_id, 0)
    /\ UNCHANGED << session_first_turn_phase >>


RestoreSessionConsumedInputs(session_id, restore_first_turn_pending, pending_initial_prompt_present, pending_tool_result_message_count) ==
    /\ phase = "Ready"
    /\ restore_first_turn_pending
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_first_turn_phase' = MapSet(session_first_turn_phase, session_id, "Pending")
    /\ session_pending_initial_prompt_present' = MapSet(session_pending_initial_prompt_present, session_id, pending_initial_prompt_present)
    /\ session_pending_tool_results_count' = MapSet(session_pending_tool_results_count, session_id, pending_tool_result_message_count)


RestoreSessionConsumedInputsNoPhaseRollback(session_id, restore_first_turn_pending, pending_initial_prompt_present, pending_tool_result_message_count) ==
    /\ phase = "Ready"
    /\ (restore_first_turn_pending = FALSE)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_pending_initial_prompt_present' = MapSet(session_pending_initial_prompt_present, session_id, pending_initial_prompt_present)
    /\ session_pending_tool_results_count' = MapSet(session_pending_tool_results_count, session_id, pending_tool_result_message_count)
    /\ UNCHANGED << session_first_turn_phase >>


RecoverSessionFirstTurnPhase(session_id, arg_phase, pending_initial_prompt_present, pending_tool_result_message_count) ==
    /\ phase = "Ready"
    /\ (IF (arg_phase = "Inactive") THEN TRUE ELSE (IF (arg_phase = "Pending") THEN TRUE ELSE (arg_phase = "Consumed")))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_first_turn_phase' = MapSet(session_first_turn_phase, session_id, arg_phase)
    /\ session_pending_initial_prompt_present' = MapSet(session_pending_initial_prompt_present, session_id, pending_initial_prompt_present)
    /\ session_pending_tool_results_count' = MapSet(session_pending_tool_results_count, session_id, pending_tool_result_message_count)


ResolveSystemContextAppendEmpty(trimmed_text_byte_count, idempotency_key_present, existing_key_matches, existing_key_conflicts, active_turn_scoped) ==
    /\ phase = "Ready"
    /\ append_is_empty(trimmed_text_byte_count)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


ResolveSystemContextAppendConflict(trimmed_text_byte_count, idempotency_key_present, existing_key_matches, existing_key_conflicts, active_turn_scoped) ==
    /\ phase = "Ready"
    /\ ((append_is_empty(trimmed_text_byte_count) = FALSE) /\ append_is_conflict(idempotency_key_present, existing_key_conflicts))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


ResolveSystemContextAppendDuplicate(trimmed_text_byte_count, idempotency_key_present, existing_key_matches, existing_key_conflicts, active_turn_scoped) ==
    /\ phase = "Ready"
    /\ ((append_is_empty(trimmed_text_byte_count) = FALSE) /\ append_is_duplicate(idempotency_key_present, existing_key_matches, existing_key_conflicts))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


ResolveSystemContextAppendNew(trimmed_text_byte_count, idempotency_key_present, existing_key_matches, existing_key_conflicts, active_turn_scoped) ==
    /\ phase = "Ready"
    /\ ((append_is_empty(trimmed_text_byte_count) = FALSE) /\ append_is_new(idempotency_key_present, existing_key_matches, existing_key_conflicts))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


ResolveSystemContextPersistAppendAdmissionAdmit(has_previous, content_identical, content_extends_previous, appended_starts_with_separator, incoming_is_runtime_context_append) ==
    /\ phase = "Ready"
    /\ persist_append_is_admissible(has_previous, content_identical, content_extends_previous, appended_starts_with_separator, incoming_is_runtime_context_append)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


ResolveSystemContextPersistAppendAdmissionReject(has_previous, content_identical, content_extends_previous, appended_starts_with_separator, incoming_is_runtime_context_append) ==
    /\ phase = "Ready"
    /\ (persist_append_is_admissible(has_previous, content_identical, content_extends_previous, appended_starts_with_separator, incoming_is_runtime_context_append) = FALSE)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


ResolveSystemContextPendingApplyItemRuntimeSteer(source_kind) ==
    /\ phase = "Ready"
    /\ (source_kind = "RuntimeSteer")
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


ResolveSystemContextPendingApplyItemNormal(source_kind) ==
    /\ phase = "Ready"
    /\ (source_kind = "Normal")
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


ResolveSystemContextSteerCleanupItemRuntimeSteer(source_kind) ==
    /\ phase = "Ready"
    /\ (source_kind = "RuntimeSteer")
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


ResolveSystemContextSteerCleanupItemNormal(source_kind) ==
    /\ phase = "Ready"
    /\ (source_kind = "Normal")
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


RestoreSystemContextSnapshot(active_keys_have_known_pending_or_seen, seen_keys_match_known_appends) ==
    /\ phase = "Ready"
    /\ (active_keys_have_known_pending_or_seen /\ seen_keys_match_known_appends)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


ResolveRealtimeItemObservedDiscardedAssistant(role, response_discarded) ==
    /\ phase = "Ready"
    /\ ((role = "Assistant") /\ response_discarded)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


ResolveRealtimeItemObservedPresent(role, response_discarded) ==
    /\ phase = "Ready"
    /\ (IF (role # "Assistant") THEN TRUE ELSE (response_discarded = FALSE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


ResolveRealtimeItemSkipped ==
    /\ phase = "Ready"
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


ResolveRealtimeUserTranscriptFinalEmpty(text_present, segment_empty, segment_matches) ==
    /\ phase = "Ready"
    /\ (text_present = FALSE)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


ResolveRealtimeUserTranscriptFinalStore(text_present, segment_empty, segment_matches) ==
    /\ phase = "Ready"
    /\ (text_present /\ segment_empty)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


ResolveRealtimeUserTranscriptFinalReplayOrConflict(text_present, segment_empty, segment_matches) ==
    /\ phase = "Ready"
    /\ (text_present /\ (segment_empty = FALSE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


ResolveRealtimeAssistantDeltaInvalidOrDuplicate(response_id_valid, response_discarded, delta_id_present, delta_id_seen, item_has_text, current_lane, requested_lane, response_completed, text_after_write_present) ==
    /\ phase = "Ready"
    /\ (IF (response_id_valid = FALSE) THEN TRUE ELSE realtime_delta_is_duplicate(delta_id_present, delta_id_seen))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


ResolveRealtimeAssistantDeltaDiscarded(response_id_valid, response_discarded, delta_id_present, delta_id_seen, item_has_text, current_lane, requested_lane, response_completed, text_after_write_present) ==
    /\ phase = "Ready"
    /\ (response_id_valid /\ response_discarded)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


ResolveRealtimeAssistantDeltaLaneConflict(response_id_valid, response_discarded, delta_id_present, delta_id_seen, item_has_text, current_lane, requested_lane, response_completed, text_after_write_present) ==
    /\ phase = "Ready"
    /\ (response_id_valid /\ (response_discarded = FALSE) /\ (realtime_delta_is_duplicate(delta_id_present, delta_id_seen) = FALSE) /\ (realtime_lane_accepts(item_has_text, current_lane, requested_lane) = FALSE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


ResolveRealtimeAssistantDeltaAccepted(response_id_valid, response_discarded, delta_id_present, delta_id_seen, item_has_text, current_lane, requested_lane, response_completed, text_after_write_present) ==
    /\ phase = "Ready"
    /\ (response_id_valid /\ (response_discarded = FALSE) /\ (realtime_delta_is_duplicate(delta_id_present, delta_id_seen) = FALSE) /\ realtime_lane_accepts(item_has_text, current_lane, requested_lane))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


ResolveRealtimeAssistantReplacementInvalid(response_id_valid, response_discarded, item_materialized, item_has_text, current_lane, requested_lane, response_completed, text_after_replace_present) ==
    /\ phase = "Ready"
    /\ (response_id_valid = FALSE)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


ResolveRealtimeAssistantReplacementDiscarded(response_id_valid, response_discarded, item_materialized, item_has_text, current_lane, requested_lane, response_completed, text_after_replace_present) ==
    /\ phase = "Ready"
    /\ (response_id_valid /\ response_discarded)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


ResolveRealtimeAssistantReplacementLocked(response_id_valid, response_discarded, item_materialized, item_has_text, current_lane, requested_lane, response_completed, text_after_replace_present) ==
    /\ phase = "Ready"
    /\ (response_id_valid /\ (response_discarded = FALSE) /\ item_materialized)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


ResolveRealtimeAssistantReplacementLaneConflict(response_id_valid, response_discarded, item_materialized, item_has_text, current_lane, requested_lane, response_completed, text_after_replace_present) ==
    /\ phase = "Ready"
    /\ (response_id_valid /\ (response_discarded = FALSE) /\ (item_materialized = FALSE) /\ (realtime_lane_accepts(item_has_text, current_lane, requested_lane) = FALSE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


ResolveRealtimeAssistantReplacementAccepted(response_id_valid, response_discarded, item_materialized, item_has_text, current_lane, requested_lane, response_completed, text_after_replace_present) ==
    /\ phase = "Ready"
    /\ (response_id_valid /\ (response_discarded = FALSE) /\ (item_materialized = FALSE) /\ realtime_lane_accepts(item_has_text, current_lane, requested_lane))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


ResolveRealtimeAssistantTurnCompletedInvalid(response_id_valid, response_discarded, stop_reason) ==
    /\ phase = "Ready"
    /\ (response_id_valid = FALSE)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


ResolveRealtimeAssistantTurnCompletedDiscard(response_id_valid, response_discarded, stop_reason) ==
    /\ phase = "Ready"
    /\ (response_id_valid /\ (IF response_discarded THEN TRUE ELSE realtime_stop_reason_discards(stop_reason)))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


ResolveRealtimeAssistantTurnCompletedToolUse(response_id_valid, response_discarded, stop_reason) ==
    /\ phase = "Ready"
    /\ (response_id_valid /\ (response_discarded = FALSE) /\ realtime_stop_reason_removes_completion(stop_reason))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


ResolveRealtimeAssistantTurnCompletedRecord(response_id_valid, response_discarded, stop_reason) ==
    /\ phase = "Ready"
    /\ (response_id_valid /\ (response_discarded = FALSE) /\ realtime_stop_reason_records_completion(stop_reason))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


ResolveRealtimeAssistantTurnInterruptedInvalid(response_id_valid) ==
    /\ phase = "Ready"
    /\ (response_id_valid = FALSE)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


ResolveRealtimeAssistantTurnInterruptedValid(response_id_valid) ==
    /\ phase = "Ready"
    /\ response_id_valid
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


ResolveRealtimeMaterializeAlreadyDone(item_materialized, predecessor_materialized, item_skipped, item_ready, item_text_present, role, response_id_present, completion_present, completion_usage_consumed) ==
    /\ phase = "Ready"
    /\ item_materialized
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


ResolveRealtimeMaterializeWaitForPredecessor(item_materialized, predecessor_materialized, item_skipped, item_ready, item_text_present, role, response_id_present, completion_present, completion_usage_consumed) ==
    /\ phase = "Ready"
    /\ ((item_materialized = FALSE) /\ (predecessor_materialized = FALSE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


ResolveRealtimeMaterializeSkipped(item_materialized, predecessor_materialized, item_skipped, item_ready, item_text_present, role, response_id_present, completion_present, completion_usage_consumed) ==
    /\ phase = "Ready"
    /\ ((item_materialized = FALSE) /\ predecessor_materialized /\ item_skipped)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


ResolveRealtimeMaterializeWaitForReadyText(item_materialized, predecessor_materialized, item_skipped, item_ready, item_text_present, role, response_id_present, completion_present, completion_usage_consumed) ==
    /\ phase = "Ready"
    /\ ((item_materialized = FALSE) /\ predecessor_materialized /\ (item_skipped = FALSE) /\ (IF (item_ready = FALSE) THEN TRUE ELSE (item_text_present = FALSE)))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


ResolveRealtimeMaterializeUser(item_materialized, predecessor_materialized, item_skipped, item_ready, item_text_present, role, response_id_present, completion_present, completion_usage_consumed) ==
    /\ phase = "Ready"
    /\ ((item_materialized = FALSE) /\ predecessor_materialized /\ (item_skipped = FALSE) /\ item_ready /\ item_text_present /\ (role = "User"))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


ResolveRealtimeMaterializeAssistant(item_materialized, predecessor_materialized, item_skipped, item_ready, item_text_present, role, response_id_present, completion_present, completion_usage_consumed) ==
    /\ phase = "Ready"
    /\ ((item_materialized = FALSE) /\ predecessor_materialized /\ (item_skipped = FALSE) /\ item_ready /\ item_text_present /\ (role = "Assistant") /\ response_id_present /\ completion_present)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


ResolveRealtimeMaterializeAssistantMissingCompletion(item_materialized, predecessor_materialized, item_skipped, item_ready, item_text_present, role, response_id_present, completion_present, completion_usage_consumed) ==
    /\ phase = "Ready"
    /\ ((item_materialized = FALSE) /\ predecessor_materialized /\ (item_skipped = FALSE) /\ item_ready /\ item_text_present /\ (role = "Assistant") /\ (IF (response_id_present = FALSE) THEN TRUE ELSE (completion_present = FALSE)))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


AuthorizeRestoreRealtimeTranscriptState(item_count, first_seen_count, first_seen_unique_count, every_item_has_order_entry, every_order_entry_has_item, all_identity_fields_valid, all_delta_ids_valid, all_completion_response_ids_valid, all_discarded_response_ids_valid, all_materialized_items_were_ready_or_skipped, all_assistant_items_have_response_unless_skipped, all_ready_assistant_items_have_completion_or_are_skipped, all_materialized_assistant_completions_consumed, all_completed_assistant_text_items_are_ready_or_materialized_or_skipped, all_discarded_assistant_items_are_skipped_or_materialized) ==
    /\ phase = "Ready"
    /\ ((item_count = first_seen_count) /\ (first_seen_count = first_seen_unique_count) /\ every_item_has_order_entry /\ every_order_entry_has_item /\ all_identity_fields_valid /\ all_delta_ids_valid /\ all_completion_response_ids_valid /\ all_discarded_response_ids_valid /\ all_materialized_items_were_ready_or_skipped /\ all_assistant_items_have_response_unless_skipped /\ all_ready_assistant_items_have_completion_or_are_skipped /\ all_materialized_assistant_completions_consumed /\ all_completed_assistant_text_items_are_ready_or_materialized_or_skipped /\ all_discarded_assistant_items_are_skipped_or_materialized)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


AuthorizeSessionMetadataPersist(schema_version, model_present) ==
    /\ phase = "Ready"
    /\ ((schema_version > 0) /\ (model_present = TRUE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


AuthorizeSessionBuildStatePersist(mob_tool_authority_context_present, mob_tool_authority_context_generated) ==
    /\ phase = "Ready"
    /\ (IF (mob_tool_authority_context_present = FALSE) THEN TRUE ELSE (mob_tool_authority_context_generated = TRUE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


RestoreSessionBuildState ==
    /\ phase = "Ready"
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


AuthorizeSystemPromptMutation(source, prompt_present, prompt_byte_count, replacing_existing) ==
    /\ phase = "Ready"
    /\ (IF (prompt_present = TRUE) THEN TRUE ELSE (prompt_byte_count = 0))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


ResolvePendingContinuationWithBoundary(session_tail, staged_tool_result_count) ==
    /\ phase = "Ready"
    /\ has_effective_pending_boundary(session_tail, staged_tool_result_count)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


ResolvePendingContinuationWithoutBoundary(session_tail, staged_tool_result_count) ==
    /\ phase = "Ready"
    /\ (has_effective_pending_boundary(session_tail, staged_tool_result_count) = FALSE)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


AuthorizeSessionResumeOverridesRejectProviderRequiresModel(provider_override_present, model_override_present, has_build_only_overrides, first_turn_phase) ==
    /\ phase = "Ready"
    /\ resume_reject_provider_requires_model(provider_override_present, model_override_present)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


AuthorizeSessionResumeOverridesRejectBuildOnlyAfterFirstTurn(provider_override_present, model_override_present, has_build_only_overrides, first_turn_phase) ==
    /\ phase = "Ready"
    /\ ((resume_reject_provider_requires_model(provider_override_present, model_override_present) = FALSE) /\ resume_reject_build_only_after_first_turn(has_build_only_overrides, first_turn_phase))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


AuthorizeSessionResumeOverridesAcceptRecomputeProvider(provider_override_present, model_override_present, has_build_only_overrides, first_turn_phase) ==
    /\ phase = "Ready"
    /\ (resume_overrides_admissible(provider_override_present, model_override_present, has_build_only_overrides, first_turn_phase) /\ resume_provider_recompute_from_model(model_override_present, provider_override_present))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


AuthorizeSessionResumeOverridesAcceptUseOverride(provider_override_present, model_override_present, has_build_only_overrides, first_turn_phase) ==
    /\ phase = "Ready"
    /\ (resume_overrides_admissible(provider_override_present, model_override_present, has_build_only_overrides, first_turn_phase) /\ (resume_provider_recompute_from_model(model_override_present, provider_override_present) = FALSE) /\ provider_override_present)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


AuthorizeSessionResumeOverridesAcceptRetainStored(provider_override_present, model_override_present, has_build_only_overrides, first_turn_phase) ==
    /\ phase = "Ready"
    /\ (resume_overrides_admissible(provider_override_present, model_override_present, has_build_only_overrides, first_turn_phase) /\ (resume_provider_recompute_from_model(model_override_present, provider_override_present) = FALSE) /\ (provider_override_present = FALSE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


ClassifyLiveSessionAuthorityLive(stored_transcript_diverged, live_has_uncommitted_transcript, runtime_system_context_diverged, stored_is_archived) ==
    /\ phase = "Ready"
    /\ ((stored_transcript_diverged = FALSE) /\ (live_has_uncommitted_transcript = FALSE) /\ (runtime_system_context_diverged = FALSE) /\ (stored_is_archived = FALSE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


ClassifyLiveSessionAuthorityDurableArchived(stored_transcript_diverged, live_has_uncommitted_transcript, runtime_system_context_diverged, stored_is_archived) ==
    /\ phase = "Ready"
    /\ (stored_is_archived = TRUE)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


ClassifyLiveSessionAuthorityDurableUncommitted(stored_transcript_diverged, live_has_uncommitted_transcript, runtime_system_context_diverged, stored_is_archived) ==
    /\ phase = "Ready"
    /\ ((stored_is_archived = FALSE) /\ (live_has_uncommitted_transcript = TRUE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


ClassifyLiveSessionAuthorityDurableSystemContext(stored_transcript_diverged, live_has_uncommitted_transcript, runtime_system_context_diverged, stored_is_archived) ==
    /\ phase = "Ready"
    /\ ((stored_is_archived = FALSE) /\ (live_has_uncommitted_transcript = FALSE) /\ (runtime_system_context_diverged = TRUE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


ClassifyLiveSessionAuthorityDurableRevision(stored_transcript_diverged, live_has_uncommitted_transcript, runtime_system_context_diverged, stored_is_archived) ==
    /\ phase = "Ready"
    /\ ((stored_is_archived = FALSE) /\ (live_has_uncommitted_transcript = FALSE) /\ (runtime_system_context_diverged = FALSE) /\ (stored_transcript_diverged = TRUE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


RecoverSessionFromStoreAuthorized(session_id, has_metadata, has_build_state) ==
    /\ phase = "Ready"
    /\ store_projection_can_recover_authority(has_metadata, has_build_state)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


RecoverSessionFromStoreUnrecoverable(session_id, has_metadata, has_build_state) ==
    /\ phase = "Ready"
    /\ (store_projection_can_recover_authority(has_metadata, has_build_state) = FALSE)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


ApplyPendingToolResults(session_id, result_count) ==
    /\ phase = "Ready"
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


TranscriptEditFork(session_id, fork_or_rewrite_directive) ==
    /\ phase = "Ready"
    /\ (fork_or_rewrite_directive = "Fork")
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


TranscriptEditRewrite(session_id, fork_or_rewrite_directive) ==
    /\ phase = "Ready"
    /\ (fork_or_rewrite_directive = "Rewrite")
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


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
    \/ \E session_id \in SessionIdValues : \E restore_first_turn_pending \in BOOLEAN : \E pending_initial_prompt_present \in BOOLEAN : \E pending_tool_result_message_count \in 0..2 : RestoreSessionConsumedInputs(session_id, restore_first_turn_pending, pending_initial_prompt_present, pending_tool_result_message_count)
    \/ \E session_id \in SessionIdValues : \E restore_first_turn_pending \in BOOLEAN : \E pending_initial_prompt_present \in BOOLEAN : \E pending_tool_result_message_count \in 0..2 : RestoreSessionConsumedInputsNoPhaseRollback(session_id, restore_first_turn_pending, pending_initial_prompt_present, pending_tool_result_message_count)
    \/ \E session_id \in SessionIdValues : \E arg_phase \in SessionFirstTurnPhaseValues : \E pending_initial_prompt_present \in BOOLEAN : \E pending_tool_result_message_count \in 0..2 : RecoverSessionFirstTurnPhase(session_id, arg_phase, pending_initial_prompt_present, pending_tool_result_message_count)
    \/ \E trimmed_text_byte_count \in 0..2 : \E idempotency_key_present \in BOOLEAN : \E existing_key_matches \in BOOLEAN : \E existing_key_conflicts \in BOOLEAN : \E active_turn_scoped \in BOOLEAN : ResolveSystemContextAppendEmpty(trimmed_text_byte_count, idempotency_key_present, existing_key_matches, existing_key_conflicts, active_turn_scoped)
    \/ \E trimmed_text_byte_count \in 0..2 : \E idempotency_key_present \in BOOLEAN : \E existing_key_matches \in BOOLEAN : \E existing_key_conflicts \in BOOLEAN : \E active_turn_scoped \in BOOLEAN : ResolveSystemContextAppendConflict(trimmed_text_byte_count, idempotency_key_present, existing_key_matches, existing_key_conflicts, active_turn_scoped)
    \/ \E trimmed_text_byte_count \in 0..2 : \E idempotency_key_present \in BOOLEAN : \E existing_key_matches \in BOOLEAN : \E existing_key_conflicts \in BOOLEAN : \E active_turn_scoped \in BOOLEAN : ResolveSystemContextAppendDuplicate(trimmed_text_byte_count, idempotency_key_present, existing_key_matches, existing_key_conflicts, active_turn_scoped)
    \/ \E trimmed_text_byte_count \in 0..2 : \E idempotency_key_present \in BOOLEAN : \E existing_key_matches \in BOOLEAN : \E existing_key_conflicts \in BOOLEAN : \E active_turn_scoped \in BOOLEAN : ResolveSystemContextAppendNew(trimmed_text_byte_count, idempotency_key_present, existing_key_matches, existing_key_conflicts, active_turn_scoped)
    \/ \E has_previous \in BOOLEAN : \E content_identical \in BOOLEAN : \E content_extends_previous \in BOOLEAN : \E appended_starts_with_separator \in BOOLEAN : \E incoming_is_runtime_context_append \in BOOLEAN : ResolveSystemContextPersistAppendAdmissionAdmit(has_previous, content_identical, content_extends_previous, appended_starts_with_separator, incoming_is_runtime_context_append)
    \/ \E has_previous \in BOOLEAN : \E content_identical \in BOOLEAN : \E content_extends_previous \in BOOLEAN : \E appended_starts_with_separator \in BOOLEAN : \E incoming_is_runtime_context_append \in BOOLEAN : ResolveSystemContextPersistAppendAdmissionReject(has_previous, content_identical, content_extends_previous, appended_starts_with_separator, incoming_is_runtime_context_append)
    \/ \E source_kind \in SystemContextSourceValues : ResolveSystemContextPendingApplyItemRuntimeSteer(source_kind)
    \/ \E source_kind \in SystemContextSourceValues : ResolveSystemContextPendingApplyItemNormal(source_kind)
    \/ \E source_kind \in SystemContextSourceValues : ResolveSystemContextSteerCleanupItemRuntimeSteer(source_kind)
    \/ \E source_kind \in SystemContextSourceValues : ResolveSystemContextSteerCleanupItemNormal(source_kind)
    \/ \E active_keys_have_known_pending_or_seen \in BOOLEAN : \E seen_keys_match_known_appends \in BOOLEAN : RestoreSystemContextSnapshot(active_keys_have_known_pending_or_seen, seen_keys_match_known_appends)
    \/ \E role \in RealtimeTranscriptRoleKindValues : \E response_discarded \in BOOLEAN : ResolveRealtimeItemObservedDiscardedAssistant(role, response_discarded)
    \/ \E role \in RealtimeTranscriptRoleKindValues : \E response_discarded \in BOOLEAN : ResolveRealtimeItemObservedPresent(role, response_discarded)
    \/ ResolveRealtimeItemSkipped
    \/ \E text_present \in BOOLEAN : \E segment_empty \in BOOLEAN : \E segment_matches \in BOOLEAN : ResolveRealtimeUserTranscriptFinalEmpty(text_present, segment_empty, segment_matches)
    \/ \E text_present \in BOOLEAN : \E segment_empty \in BOOLEAN : \E segment_matches \in BOOLEAN : ResolveRealtimeUserTranscriptFinalStore(text_present, segment_empty, segment_matches)
    \/ \E text_present \in BOOLEAN : \E segment_empty \in BOOLEAN : \E segment_matches \in BOOLEAN : ResolveRealtimeUserTranscriptFinalReplayOrConflict(text_present, segment_empty, segment_matches)
    \/ \E response_id_valid \in BOOLEAN : \E response_discarded \in BOOLEAN : \E delta_id_present \in BOOLEAN : \E delta_id_seen \in BOOLEAN : \E item_has_text \in BOOLEAN : \E current_lane \in RealtimeTranscriptLaneKindValues : \E requested_lane \in RealtimeTranscriptLaneKindValues : \E response_completed \in BOOLEAN : \E text_after_write_present \in BOOLEAN : ResolveRealtimeAssistantDeltaInvalidOrDuplicate(response_id_valid, response_discarded, delta_id_present, delta_id_seen, item_has_text, current_lane, requested_lane, response_completed, text_after_write_present)
    \/ \E response_id_valid \in BOOLEAN : \E response_discarded \in BOOLEAN : \E delta_id_present \in BOOLEAN : \E delta_id_seen \in BOOLEAN : \E item_has_text \in BOOLEAN : \E current_lane \in RealtimeTranscriptLaneKindValues : \E requested_lane \in RealtimeTranscriptLaneKindValues : \E response_completed \in BOOLEAN : \E text_after_write_present \in BOOLEAN : ResolveRealtimeAssistantDeltaDiscarded(response_id_valid, response_discarded, delta_id_present, delta_id_seen, item_has_text, current_lane, requested_lane, response_completed, text_after_write_present)
    \/ \E response_id_valid \in BOOLEAN : \E response_discarded \in BOOLEAN : \E delta_id_present \in BOOLEAN : \E delta_id_seen \in BOOLEAN : \E item_has_text \in BOOLEAN : \E current_lane \in RealtimeTranscriptLaneKindValues : \E requested_lane \in RealtimeTranscriptLaneKindValues : \E response_completed \in BOOLEAN : \E text_after_write_present \in BOOLEAN : ResolveRealtimeAssistantDeltaLaneConflict(response_id_valid, response_discarded, delta_id_present, delta_id_seen, item_has_text, current_lane, requested_lane, response_completed, text_after_write_present)
    \/ \E response_id_valid \in BOOLEAN : \E response_discarded \in BOOLEAN : \E delta_id_present \in BOOLEAN : \E delta_id_seen \in BOOLEAN : \E item_has_text \in BOOLEAN : \E current_lane \in RealtimeTranscriptLaneKindValues : \E requested_lane \in RealtimeTranscriptLaneKindValues : \E response_completed \in BOOLEAN : \E text_after_write_present \in BOOLEAN : ResolveRealtimeAssistantDeltaAccepted(response_id_valid, response_discarded, delta_id_present, delta_id_seen, item_has_text, current_lane, requested_lane, response_completed, text_after_write_present)
    \/ \E response_id_valid \in BOOLEAN : \E response_discarded \in BOOLEAN : \E item_materialized \in BOOLEAN : \E item_has_text \in BOOLEAN : \E current_lane \in RealtimeTranscriptLaneKindValues : \E requested_lane \in RealtimeTranscriptLaneKindValues : \E response_completed \in BOOLEAN : \E text_after_replace_present \in BOOLEAN : ResolveRealtimeAssistantReplacementInvalid(response_id_valid, response_discarded, item_materialized, item_has_text, current_lane, requested_lane, response_completed, text_after_replace_present)
    \/ \E response_id_valid \in BOOLEAN : \E response_discarded \in BOOLEAN : \E item_materialized \in BOOLEAN : \E item_has_text \in BOOLEAN : \E current_lane \in RealtimeTranscriptLaneKindValues : \E requested_lane \in RealtimeTranscriptLaneKindValues : \E response_completed \in BOOLEAN : \E text_after_replace_present \in BOOLEAN : ResolveRealtimeAssistantReplacementDiscarded(response_id_valid, response_discarded, item_materialized, item_has_text, current_lane, requested_lane, response_completed, text_after_replace_present)
    \/ \E response_id_valid \in BOOLEAN : \E response_discarded \in BOOLEAN : \E item_materialized \in BOOLEAN : \E item_has_text \in BOOLEAN : \E current_lane \in RealtimeTranscriptLaneKindValues : \E requested_lane \in RealtimeTranscriptLaneKindValues : \E response_completed \in BOOLEAN : \E text_after_replace_present \in BOOLEAN : ResolveRealtimeAssistantReplacementLocked(response_id_valid, response_discarded, item_materialized, item_has_text, current_lane, requested_lane, response_completed, text_after_replace_present)
    \/ \E response_id_valid \in BOOLEAN : \E response_discarded \in BOOLEAN : \E item_materialized \in BOOLEAN : \E item_has_text \in BOOLEAN : \E current_lane \in RealtimeTranscriptLaneKindValues : \E requested_lane \in RealtimeTranscriptLaneKindValues : \E response_completed \in BOOLEAN : \E text_after_replace_present \in BOOLEAN : ResolveRealtimeAssistantReplacementLaneConflict(response_id_valid, response_discarded, item_materialized, item_has_text, current_lane, requested_lane, response_completed, text_after_replace_present)
    \/ \E response_id_valid \in BOOLEAN : \E response_discarded \in BOOLEAN : \E item_materialized \in BOOLEAN : \E item_has_text \in BOOLEAN : \E current_lane \in RealtimeTranscriptLaneKindValues : \E requested_lane \in RealtimeTranscriptLaneKindValues : \E response_completed \in BOOLEAN : \E text_after_replace_present \in BOOLEAN : ResolveRealtimeAssistantReplacementAccepted(response_id_valid, response_discarded, item_materialized, item_has_text, current_lane, requested_lane, response_completed, text_after_replace_present)
    \/ \E response_id_valid \in BOOLEAN : \E response_discarded \in BOOLEAN : \E stop_reason \in RealtimeTranscriptStopReasonKindValues : ResolveRealtimeAssistantTurnCompletedInvalid(response_id_valid, response_discarded, stop_reason)
    \/ \E response_id_valid \in BOOLEAN : \E response_discarded \in BOOLEAN : \E stop_reason \in RealtimeTranscriptStopReasonKindValues : ResolveRealtimeAssistantTurnCompletedDiscard(response_id_valid, response_discarded, stop_reason)
    \/ \E response_id_valid \in BOOLEAN : \E response_discarded \in BOOLEAN : \E stop_reason \in RealtimeTranscriptStopReasonKindValues : ResolveRealtimeAssistantTurnCompletedToolUse(response_id_valid, response_discarded, stop_reason)
    \/ \E response_id_valid \in BOOLEAN : \E response_discarded \in BOOLEAN : \E stop_reason \in RealtimeTranscriptStopReasonKindValues : ResolveRealtimeAssistantTurnCompletedRecord(response_id_valid, response_discarded, stop_reason)
    \/ \E response_id_valid \in BOOLEAN : ResolveRealtimeAssistantTurnInterruptedInvalid(response_id_valid)
    \/ \E response_id_valid \in BOOLEAN : ResolveRealtimeAssistantTurnInterruptedValid(response_id_valid)
    \/ \E item_materialized \in BOOLEAN : \E predecessor_materialized \in BOOLEAN : \E item_skipped \in BOOLEAN : \E item_ready \in BOOLEAN : \E item_text_present \in BOOLEAN : \E role \in RealtimeTranscriptRoleKindValues : \E response_id_present \in BOOLEAN : \E completion_present \in BOOLEAN : \E completion_usage_consumed \in BOOLEAN : ResolveRealtimeMaterializeAlreadyDone(item_materialized, predecessor_materialized, item_skipped, item_ready, item_text_present, role, response_id_present, completion_present, completion_usage_consumed)
    \/ \E item_materialized \in BOOLEAN : \E predecessor_materialized \in BOOLEAN : \E item_skipped \in BOOLEAN : \E item_ready \in BOOLEAN : \E item_text_present \in BOOLEAN : \E role \in RealtimeTranscriptRoleKindValues : \E response_id_present \in BOOLEAN : \E completion_present \in BOOLEAN : \E completion_usage_consumed \in BOOLEAN : ResolveRealtimeMaterializeWaitForPredecessor(item_materialized, predecessor_materialized, item_skipped, item_ready, item_text_present, role, response_id_present, completion_present, completion_usage_consumed)
    \/ \E item_materialized \in BOOLEAN : \E predecessor_materialized \in BOOLEAN : \E item_skipped \in BOOLEAN : \E item_ready \in BOOLEAN : \E item_text_present \in BOOLEAN : \E role \in RealtimeTranscriptRoleKindValues : \E response_id_present \in BOOLEAN : \E completion_present \in BOOLEAN : \E completion_usage_consumed \in BOOLEAN : ResolveRealtimeMaterializeSkipped(item_materialized, predecessor_materialized, item_skipped, item_ready, item_text_present, role, response_id_present, completion_present, completion_usage_consumed)
    \/ \E item_materialized \in BOOLEAN : \E predecessor_materialized \in BOOLEAN : \E item_skipped \in BOOLEAN : \E item_ready \in BOOLEAN : \E item_text_present \in BOOLEAN : \E role \in RealtimeTranscriptRoleKindValues : \E response_id_present \in BOOLEAN : \E completion_present \in BOOLEAN : \E completion_usage_consumed \in BOOLEAN : ResolveRealtimeMaterializeWaitForReadyText(item_materialized, predecessor_materialized, item_skipped, item_ready, item_text_present, role, response_id_present, completion_present, completion_usage_consumed)
    \/ \E item_materialized \in BOOLEAN : \E predecessor_materialized \in BOOLEAN : \E item_skipped \in BOOLEAN : \E item_ready \in BOOLEAN : \E item_text_present \in BOOLEAN : \E role \in RealtimeTranscriptRoleKindValues : \E response_id_present \in BOOLEAN : \E completion_present \in BOOLEAN : \E completion_usage_consumed \in BOOLEAN : ResolveRealtimeMaterializeUser(item_materialized, predecessor_materialized, item_skipped, item_ready, item_text_present, role, response_id_present, completion_present, completion_usage_consumed)
    \/ \E item_materialized \in BOOLEAN : \E predecessor_materialized \in BOOLEAN : \E item_skipped \in BOOLEAN : \E item_ready \in BOOLEAN : \E item_text_present \in BOOLEAN : \E role \in RealtimeTranscriptRoleKindValues : \E response_id_present \in BOOLEAN : \E completion_present \in BOOLEAN : \E completion_usage_consumed \in BOOLEAN : ResolveRealtimeMaterializeAssistant(item_materialized, predecessor_materialized, item_skipped, item_ready, item_text_present, role, response_id_present, completion_present, completion_usage_consumed)
    \/ \E item_materialized \in BOOLEAN : \E predecessor_materialized \in BOOLEAN : \E item_skipped \in BOOLEAN : \E item_ready \in BOOLEAN : \E item_text_present \in BOOLEAN : \E role \in RealtimeTranscriptRoleKindValues : \E response_id_present \in BOOLEAN : \E completion_present \in BOOLEAN : \E completion_usage_consumed \in BOOLEAN : ResolveRealtimeMaterializeAssistantMissingCompletion(item_materialized, predecessor_materialized, item_skipped, item_ready, item_text_present, role, response_id_present, completion_present, completion_usage_consumed)
    \/ \E item_count \in 0..2 : \E first_seen_count \in 0..2 : \E first_seen_unique_count \in 0..2 : \E every_item_has_order_entry \in BOOLEAN : \E every_order_entry_has_item \in BOOLEAN : \E all_identity_fields_valid \in BOOLEAN : \E all_delta_ids_valid \in BOOLEAN : \E all_completion_response_ids_valid \in BOOLEAN : \E all_discarded_response_ids_valid \in BOOLEAN : \E all_materialized_items_were_ready_or_skipped \in BOOLEAN : \E all_assistant_items_have_response_unless_skipped \in BOOLEAN : \E all_ready_assistant_items_have_completion_or_are_skipped \in BOOLEAN : \E all_materialized_assistant_completions_consumed \in BOOLEAN : \E all_completed_assistant_text_items_are_ready_or_materialized_or_skipped \in BOOLEAN : \E all_discarded_assistant_items_are_skipped_or_materialized \in BOOLEAN : AuthorizeRestoreRealtimeTranscriptState(item_count, first_seen_count, first_seen_unique_count, every_item_has_order_entry, every_order_entry_has_item, all_identity_fields_valid, all_delta_ids_valid, all_completion_response_ids_valid, all_discarded_response_ids_valid, all_materialized_items_were_ready_or_skipped, all_assistant_items_have_response_unless_skipped, all_ready_assistant_items_have_completion_or_are_skipped, all_materialized_assistant_completions_consumed, all_completed_assistant_text_items_are_ready_or_materialized_or_skipped, all_discarded_assistant_items_are_skipped_or_materialized)
    \/ \E schema_version \in 0..2 : \E model_present \in BOOLEAN : AuthorizeSessionMetadataPersist(schema_version, model_present)
    \/ \E mob_tool_authority_context_present \in BOOLEAN : \E mob_tool_authority_context_generated \in BOOLEAN : AuthorizeSessionBuildStatePersist(mob_tool_authority_context_present, mob_tool_authority_context_generated)
    \/ RestoreSessionBuildState
    \/ \E source \in SessionSystemPromptSourceValues : \E prompt_present \in BOOLEAN : \E prompt_byte_count \in 0..2 : \E replacing_existing \in BOOLEAN : AuthorizeSystemPromptMutation(source, prompt_present, prompt_byte_count, replacing_existing)
    \/ \E session_tail \in ObservedSessionTailKindValues : \E staged_tool_result_count \in 0..2 : ResolvePendingContinuationWithBoundary(session_tail, staged_tool_result_count)
    \/ \E session_tail \in ObservedSessionTailKindValues : \E staged_tool_result_count \in 0..2 : ResolvePendingContinuationWithoutBoundary(session_tail, staged_tool_result_count)
    \/ \E provider_override_present \in BOOLEAN : \E model_override_present \in BOOLEAN : \E has_build_only_overrides \in BOOLEAN : \E first_turn_phase \in SessionFirstTurnPhaseValues : AuthorizeSessionResumeOverridesRejectProviderRequiresModel(provider_override_present, model_override_present, has_build_only_overrides, first_turn_phase)
    \/ \E provider_override_present \in BOOLEAN : \E model_override_present \in BOOLEAN : \E has_build_only_overrides \in BOOLEAN : \E first_turn_phase \in SessionFirstTurnPhaseValues : AuthorizeSessionResumeOverridesRejectBuildOnlyAfterFirstTurn(provider_override_present, model_override_present, has_build_only_overrides, first_turn_phase)
    \/ \E provider_override_present \in BOOLEAN : \E model_override_present \in BOOLEAN : \E has_build_only_overrides \in BOOLEAN : \E first_turn_phase \in SessionFirstTurnPhaseValues : AuthorizeSessionResumeOverridesAcceptRecomputeProvider(provider_override_present, model_override_present, has_build_only_overrides, first_turn_phase)
    \/ \E provider_override_present \in BOOLEAN : \E model_override_present \in BOOLEAN : \E has_build_only_overrides \in BOOLEAN : \E first_turn_phase \in SessionFirstTurnPhaseValues : AuthorizeSessionResumeOverridesAcceptUseOverride(provider_override_present, model_override_present, has_build_only_overrides, first_turn_phase)
    \/ \E provider_override_present \in BOOLEAN : \E model_override_present \in BOOLEAN : \E has_build_only_overrides \in BOOLEAN : \E first_turn_phase \in SessionFirstTurnPhaseValues : AuthorizeSessionResumeOverridesAcceptRetainStored(provider_override_present, model_override_present, has_build_only_overrides, first_turn_phase)
    \/ \E stored_transcript_diverged \in BOOLEAN : \E live_has_uncommitted_transcript \in BOOLEAN : \E runtime_system_context_diverged \in BOOLEAN : \E stored_is_archived \in BOOLEAN : ClassifyLiveSessionAuthorityLive(stored_transcript_diverged, live_has_uncommitted_transcript, runtime_system_context_diverged, stored_is_archived)
    \/ \E stored_transcript_diverged \in BOOLEAN : \E live_has_uncommitted_transcript \in BOOLEAN : \E runtime_system_context_diverged \in BOOLEAN : \E stored_is_archived \in BOOLEAN : ClassifyLiveSessionAuthorityDurableArchived(stored_transcript_diverged, live_has_uncommitted_transcript, runtime_system_context_diverged, stored_is_archived)
    \/ \E stored_transcript_diverged \in BOOLEAN : \E live_has_uncommitted_transcript \in BOOLEAN : \E runtime_system_context_diverged \in BOOLEAN : \E stored_is_archived \in BOOLEAN : ClassifyLiveSessionAuthorityDurableUncommitted(stored_transcript_diverged, live_has_uncommitted_transcript, runtime_system_context_diverged, stored_is_archived)
    \/ \E stored_transcript_diverged \in BOOLEAN : \E live_has_uncommitted_transcript \in BOOLEAN : \E runtime_system_context_diverged \in BOOLEAN : \E stored_is_archived \in BOOLEAN : ClassifyLiveSessionAuthorityDurableSystemContext(stored_transcript_diverged, live_has_uncommitted_transcript, runtime_system_context_diverged, stored_is_archived)
    \/ \E stored_transcript_diverged \in BOOLEAN : \E live_has_uncommitted_transcript \in BOOLEAN : \E runtime_system_context_diverged \in BOOLEAN : \E stored_is_archived \in BOOLEAN : ClassifyLiveSessionAuthorityDurableRevision(stored_transcript_diverged, live_has_uncommitted_transcript, runtime_system_context_diverged, stored_is_archived)
    \/ \E session_id \in SessionIdValues : \E has_metadata \in BOOLEAN : \E has_build_state \in BOOLEAN : RecoverSessionFromStoreAuthorized(session_id, has_metadata, has_build_state)
    \/ \E session_id \in SessionIdValues : \E has_metadata \in BOOLEAN : \E has_build_state \in BOOLEAN : RecoverSessionFromStoreUnrecoverable(session_id, has_metadata, has_build_state)
    \/ \E session_id \in SessionIdValues : \E result_count \in 0..2 : ApplyPendingToolResults(session_id, result_count)
    \/ \E session_id \in SessionIdValues : \E fork_or_rewrite_directive \in TranscriptEditKindValues : TranscriptEditFork(session_id, fork_or_rewrite_directive)
    \/ \E session_id \in SessionIdValues : \E fork_or_rewrite_directive \in TranscriptEditKindValues : TranscriptEditRewrite(session_id, fork_or_rewrite_directive)


CiStateConstraint == /\ model_step_count <= 6 /\ Cardinality(DOMAIN session_first_turn_phase) <= 1 /\ Cardinality(DOMAIN session_pending_initial_prompt_present) <= 1 /\ Cardinality(DOMAIN session_pending_tool_results_count) <= 1
DeepStateConstraint == /\ model_step_count <= 8 /\ Cardinality(DOMAIN session_first_turn_phase) <= 2 /\ Cardinality(DOMAIN session_pending_initial_prompt_present) <= 2 /\ Cardinality(DOMAIN session_pending_tool_results_count) <= 2

Spec == Init /\ [][Next]_vars


=============================================================================
