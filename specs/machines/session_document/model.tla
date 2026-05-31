---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for SessionDocumentMachine.

CONSTANTS BooleanValues, NatValues, ObservedSessionTailKindValues, PendingContinuationDispositionValues, PendingContinuationPublicTerminalValues, RealtimeTranscriptLaneKindValues, RealtimeTranscriptMaterializeDecisionValues, RealtimeTranscriptRoleKindValues, RealtimeTranscriptStopReasonKindValues, ResumeOverrideRejectionValues, ResumeProviderSelectionValues, ResumeSelfHostedSelectionValues, SessionCallTimeoutOverrideKindValues, SessionDurableProviderKindValues, SessionFirstTurnPhaseValues, SessionIdValues, SessionInitialPromptStageDecisionValues, SessionSystemPromptSourceValues, SessionToolCategoryOverrideKindValues, SystemContextAppendDecisionValues, SystemContextSourceValues

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

resume_provider_recompute_from_model(model_override_present, provider_override_present) == (model_override_present /\ (provider_override_present = FALSE))
resume_reject_clear_and_set_auth_binding(clear_auth_binding, auth_binding_override_present) == (clear_auth_binding /\ auth_binding_override_present)
resume_reject_clear_and_set_provider_params(clear_provider_params, provider_params_override_present) == (clear_provider_params /\ provider_params_override_present)
resume_reject_provider_requires_model(provider_override_present, model_override_present) == (provider_override_present /\ (model_override_present = FALSE))
tail_has_pending_boundary(session_tail) == ((session_tail = "User") \/ (session_tail = "ToolResults"))
realtime_stop_reason_records_completion(stop_reason) == (stop_reason = "Other")
realtime_stop_reason_removes_completion(stop_reason) == (stop_reason = "ToolUse")
realtime_stop_reason_discards(stop_reason) == (stop_reason = "Cancelled")
realtime_should_mark_ready_after_write(response_completed, text_after_write_present) == (response_completed /\ text_after_write_present)
realtime_lane_accepts(item_has_text, current_lane, requested_lane) == ((current_lane = requested_lane) \/ (item_has_text = FALSE))
realtime_delta_is_duplicate(delta_id_present, delta_id_seen) == (delta_id_present /\ delta_id_seen)
append_is_new(idempotency_key_present, existing_key_matches, existing_key_conflicts) == ((idempotency_key_present = FALSE) \/ ((existing_key_matches = FALSE) /\ (existing_key_conflicts = FALSE)))
append_is_duplicate(idempotency_key_present, existing_key_matches, existing_key_conflicts) == (idempotency_key_present /\ existing_key_matches /\ (existing_key_conflicts = FALSE))
append_is_conflict(idempotency_key_present, existing_key_conflicts) == (idempotency_key_present /\ existing_key_conflicts)
append_is_empty(trimmed_text_byte_count) == (trimmed_text_byte_count = 0)
should_store_initial_prompt(arg_phase, prompt_has_content) == ((arg_phase = "Pending") /\ prompt_has_content)
phase_allows_initial_turn_overrides(arg_phase) == (arg_phase = "Pending")
resume_reject_build_only_after_first_turn(has_build_only_overrides, first_turn_phase) == (has_build_only_overrides /\ (phase_allows_initial_turn_overrides(first_turn_phase) = FALSE))
has_effective_pending_boundary(session_tail, staged_tool_result_count) == (tail_has_pending_boundary(session_tail) \/ (staged_tool_result_count > 0))
resume_overrides_admissible(provider_override_present, model_override_present, clear_provider_params, provider_params_override_present, clear_auth_binding, auth_binding_override_present, has_build_only_overrides, first_turn_phase) == ((resume_reject_provider_requires_model(provider_override_present, model_override_present) = FALSE) /\ (resume_reject_clear_and_set_provider_params(clear_provider_params, provider_params_override_present) = FALSE) /\ (resume_reject_clear_and_set_auth_binding(clear_auth_binding, auth_binding_override_present) = FALSE) /\ (resume_reject_build_only_after_first_turn(has_build_only_overrides, first_turn_phase) = FALSE))

Init ==
    /\ phase = "Ready"
    /\ model_step_count = 0
    /\ session_first_turn_phase = [x \in {} |-> None]
    /\ session_pending_initial_prompt_present = [x \in {} |-> None]
    /\ session_pending_tool_results_count = [x \in {} |-> None]

MarkSessionInitialTurnPendingInactiveOrPending(session_id) ==
    /\ phase = "Ready"
    /\ (((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Inactive") \/ ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Pending"))
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
    /\ (((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Inactive") \/ ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Pending") \/ ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Consumed"))
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
    /\ ((arg_phase = "Inactive") \/ (arg_phase = "Pending") \/ (arg_phase = "Consumed"))
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
    /\ ((role # "Assistant") \/ (response_discarded = FALSE))
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
    /\ ((response_id_valid = FALSE) \/ realtime_delta_is_duplicate(delta_id_present, delta_id_seen))
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
    /\ (response_id_valid /\ (response_discarded \/ realtime_stop_reason_discards(stop_reason)))
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
    /\ ((item_materialized = FALSE) /\ predecessor_materialized /\ (item_skipped = FALSE) /\ ((item_ready = FALSE) \/ (item_text_present = FALSE)))
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
    /\ ((item_materialized = FALSE) /\ predecessor_materialized /\ (item_skipped = FALSE) /\ item_ready /\ item_text_present /\ (role = "Assistant") /\ ((response_id_present = FALSE) \/ (completion_present = FALSE)))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


AuthorizeRestoreRealtimeTranscriptState(item_count, first_seen_count, first_seen_unique_count, every_item_has_order_entry, every_order_entry_has_item, all_identity_fields_valid, all_delta_ids_valid, all_completion_response_ids_valid, all_discarded_response_ids_valid, all_materialized_items_were_ready_or_skipped, all_assistant_items_have_response_unless_skipped, all_ready_assistant_items_have_completion_or_are_skipped, all_materialized_assistant_completions_consumed, all_completed_assistant_text_items_are_ready_or_materialized_or_skipped, all_discarded_assistant_items_are_skipped_or_materialized) ==
    /\ phase = "Ready"
    /\ ((item_count = first_seen_count) /\ (first_seen_count = first_seen_unique_count) /\ every_item_has_order_entry /\ every_order_entry_has_item /\ all_identity_fields_valid /\ all_delta_ids_valid /\ all_completion_response_ids_valid /\ all_discarded_response_ids_valid /\ all_materialized_items_were_ready_or_skipped /\ all_assistant_items_have_response_unless_skipped /\ all_ready_assistant_items_have_completion_or_are_skipped /\ all_materialized_assistant_completions_consumed /\ all_completed_assistant_text_items_are_ready_or_materialized_or_skipped /\ all_discarded_assistant_items_are_skipped_or_materialized)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


AuthorizeSessionMetadataPersist(schema_version, model_present, max_tokens, structured_output_retries, provider, self_hosted_server_present, provider_params_present, tooling_builtins, tooling_shell, tooling_comms, tooling_mob, tooling_memory, tooling_schedule, tooling_workgraph, tooling_image_generation, tooling_web_search, active_skill_count, keep_alive, comms_name_present, peer_meta_present, realm_id_present, instance_id_present, backend_present, config_generation_present, auth_binding_present) ==
    /\ phase = "Ready"
    /\ ((schema_version > 0) /\ (model_present = TRUE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


AuthorizeSessionBuildStatePersist(system_prompt_present, output_schema_present, hook_entry_count, disabled_hook_count, budget_limits_present, recoverable_tool_count, silent_comms_intent_count, max_inline_peer_notifications_present, app_context_present, additional_instruction_count, shell_env_count, mob_tool_authority_context_present, mob_tool_authority_context_generated, call_timeout_override) ==
    /\ phase = "Ready"
    /\ ((mob_tool_authority_context_present = FALSE) \/ (mob_tool_authority_context_generated = TRUE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


RestoreSessionBuildState(system_prompt_present, output_schema_present, hook_entry_count, disabled_hook_count, budget_limits_present, recoverable_tool_count, silent_comms_intent_count, max_inline_peer_notifications_present, app_context_present, additional_instruction_count, shell_env_count, mob_tool_authority_context_present, call_timeout_override) ==
    /\ phase = "Ready"
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


AuthorizeSystemPromptMutation(source, prompt_present, prompt_byte_count, replacing_existing) ==
    /\ phase = "Ready"
    /\ ((prompt_present = TRUE) \/ (prompt_byte_count = 0))
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


AuthorizeSessionResumeOverridesRejectProviderRequiresModel(provider_override_present, model_override_present, clear_provider_params, provider_params_override_present, clear_auth_binding, auth_binding_override_present, has_build_only_overrides, first_turn_phase) ==
    /\ phase = "Ready"
    /\ resume_reject_provider_requires_model(provider_override_present, model_override_present)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


AuthorizeSessionResumeOverridesRejectClearAndSetProviderParams(provider_override_present, model_override_present, clear_provider_params, provider_params_override_present, clear_auth_binding, auth_binding_override_present, has_build_only_overrides, first_turn_phase) ==
    /\ phase = "Ready"
    /\ ((resume_reject_provider_requires_model(provider_override_present, model_override_present) = FALSE) /\ resume_reject_clear_and_set_provider_params(clear_provider_params, provider_params_override_present))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


AuthorizeSessionResumeOverridesRejectClearAndSetAuthBinding(provider_override_present, model_override_present, clear_provider_params, provider_params_override_present, clear_auth_binding, auth_binding_override_present, has_build_only_overrides, first_turn_phase) ==
    /\ phase = "Ready"
    /\ ((resume_reject_provider_requires_model(provider_override_present, model_override_present) = FALSE) /\ (resume_reject_clear_and_set_provider_params(clear_provider_params, provider_params_override_present) = FALSE) /\ resume_reject_clear_and_set_auth_binding(clear_auth_binding, auth_binding_override_present))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


AuthorizeSessionResumeOverridesRejectBuildOnlyAfterFirstTurn(provider_override_present, model_override_present, clear_provider_params, provider_params_override_present, clear_auth_binding, auth_binding_override_present, has_build_only_overrides, first_turn_phase) ==
    /\ phase = "Ready"
    /\ ((resume_reject_provider_requires_model(provider_override_present, model_override_present) = FALSE) /\ (resume_reject_clear_and_set_provider_params(clear_provider_params, provider_params_override_present) = FALSE) /\ (resume_reject_clear_and_set_auth_binding(clear_auth_binding, auth_binding_override_present) = FALSE) /\ resume_reject_build_only_after_first_turn(has_build_only_overrides, first_turn_phase))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


AuthorizeSessionResumeOverridesAcceptRecomputeProvider(provider_override_present, model_override_present, clear_provider_params, provider_params_override_present, clear_auth_binding, auth_binding_override_present, has_build_only_overrides, first_turn_phase) ==
    /\ phase = "Ready"
    /\ (resume_overrides_admissible(provider_override_present, model_override_present, clear_provider_params, provider_params_override_present, clear_auth_binding, auth_binding_override_present, has_build_only_overrides, first_turn_phase) /\ resume_provider_recompute_from_model(model_override_present, provider_override_present))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


AuthorizeSessionResumeOverridesAcceptUseOverride(provider_override_present, model_override_present, clear_provider_params, provider_params_override_present, clear_auth_binding, auth_binding_override_present, has_build_only_overrides, first_turn_phase) ==
    /\ phase = "Ready"
    /\ (resume_overrides_admissible(provider_override_present, model_override_present, clear_provider_params, provider_params_override_present, clear_auth_binding, auth_binding_override_present, has_build_only_overrides, first_turn_phase) /\ (resume_provider_recompute_from_model(model_override_present, provider_override_present) = FALSE) /\ provider_override_present)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


AuthorizeSessionResumeOverridesAcceptRetainStored(provider_override_present, model_override_present, clear_provider_params, provider_params_override_present, clear_auth_binding, auth_binding_override_present, has_build_only_overrides, first_turn_phase) ==
    /\ phase = "Ready"
    /\ (resume_overrides_admissible(provider_override_present, model_override_present, clear_provider_params, provider_params_override_present, clear_auth_binding, auth_binding_override_present, has_build_only_overrides, first_turn_phase) /\ (resume_provider_recompute_from_model(model_override_present, provider_override_present) = FALSE) /\ (provider_override_present = FALSE))
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
    \/ \E schema_version \in 0..2 : \E model_present \in BOOLEAN : \E max_tokens \in 0..2 : \E structured_output_retries \in 0..2 : \E provider \in SessionDurableProviderKindValues : \E self_hosted_server_present \in BOOLEAN : \E provider_params_present \in BOOLEAN : \E tooling_builtins \in SessionToolCategoryOverrideKindValues : \E tooling_shell \in SessionToolCategoryOverrideKindValues : \E tooling_comms \in SessionToolCategoryOverrideKindValues : \E tooling_mob \in SessionToolCategoryOverrideKindValues : \E tooling_memory \in SessionToolCategoryOverrideKindValues : \E tooling_schedule \in SessionToolCategoryOverrideKindValues : \E tooling_workgraph \in SessionToolCategoryOverrideKindValues : \E tooling_image_generation \in SessionToolCategoryOverrideKindValues : \E tooling_web_search \in SessionToolCategoryOverrideKindValues : \E active_skill_count \in 0..2 : \E keep_alive \in BOOLEAN : \E comms_name_present \in BOOLEAN : \E peer_meta_present \in BOOLEAN : \E realm_id_present \in BOOLEAN : \E instance_id_present \in BOOLEAN : \E backend_present \in BOOLEAN : \E config_generation_present \in BOOLEAN : \E auth_binding_present \in BOOLEAN : AuthorizeSessionMetadataPersist(schema_version, model_present, max_tokens, structured_output_retries, provider, self_hosted_server_present, provider_params_present, tooling_builtins, tooling_shell, tooling_comms, tooling_mob, tooling_memory, tooling_schedule, tooling_workgraph, tooling_image_generation, tooling_web_search, active_skill_count, keep_alive, comms_name_present, peer_meta_present, realm_id_present, instance_id_present, backend_present, config_generation_present, auth_binding_present)
    \/ \E system_prompt_present \in BOOLEAN : \E output_schema_present \in BOOLEAN : \E hook_entry_count \in 0..2 : \E disabled_hook_count \in 0..2 : \E budget_limits_present \in BOOLEAN : \E recoverable_tool_count \in 0..2 : \E silent_comms_intent_count \in 0..2 : \E max_inline_peer_notifications_present \in BOOLEAN : \E app_context_present \in BOOLEAN : \E additional_instruction_count \in 0..2 : \E shell_env_count \in 0..2 : \E mob_tool_authority_context_present \in BOOLEAN : \E mob_tool_authority_context_generated \in BOOLEAN : \E call_timeout_override \in SessionCallTimeoutOverrideKindValues : AuthorizeSessionBuildStatePersist(system_prompt_present, output_schema_present, hook_entry_count, disabled_hook_count, budget_limits_present, recoverable_tool_count, silent_comms_intent_count, max_inline_peer_notifications_present, app_context_present, additional_instruction_count, shell_env_count, mob_tool_authority_context_present, mob_tool_authority_context_generated, call_timeout_override)
    \/ \E system_prompt_present \in BOOLEAN : \E output_schema_present \in BOOLEAN : \E hook_entry_count \in 0..2 : \E disabled_hook_count \in 0..2 : \E budget_limits_present \in BOOLEAN : \E recoverable_tool_count \in 0..2 : \E silent_comms_intent_count \in 0..2 : \E max_inline_peer_notifications_present \in BOOLEAN : \E app_context_present \in BOOLEAN : \E additional_instruction_count \in 0..2 : \E shell_env_count \in 0..2 : \E mob_tool_authority_context_present \in BOOLEAN : \E call_timeout_override \in SessionCallTimeoutOverrideKindValues : RestoreSessionBuildState(system_prompt_present, output_schema_present, hook_entry_count, disabled_hook_count, budget_limits_present, recoverable_tool_count, silent_comms_intent_count, max_inline_peer_notifications_present, app_context_present, additional_instruction_count, shell_env_count, mob_tool_authority_context_present, call_timeout_override)
    \/ \E source \in SessionSystemPromptSourceValues : \E prompt_present \in BOOLEAN : \E prompt_byte_count \in 0..2 : \E replacing_existing \in BOOLEAN : AuthorizeSystemPromptMutation(source, prompt_present, prompt_byte_count, replacing_existing)
    \/ \E session_tail \in ObservedSessionTailKindValues : \E staged_tool_result_count \in 0..2 : ResolvePendingContinuationWithBoundary(session_tail, staged_tool_result_count)
    \/ \E session_tail \in ObservedSessionTailKindValues : \E staged_tool_result_count \in 0..2 : ResolvePendingContinuationWithoutBoundary(session_tail, staged_tool_result_count)
    \/ \E provider_override_present \in BOOLEAN : \E model_override_present \in BOOLEAN : \E clear_provider_params \in BOOLEAN : \E provider_params_override_present \in BOOLEAN : \E clear_auth_binding \in BOOLEAN : \E auth_binding_override_present \in BOOLEAN : \E has_build_only_overrides \in BOOLEAN : \E first_turn_phase \in SessionFirstTurnPhaseValues : AuthorizeSessionResumeOverridesRejectProviderRequiresModel(provider_override_present, model_override_present, clear_provider_params, provider_params_override_present, clear_auth_binding, auth_binding_override_present, has_build_only_overrides, first_turn_phase)
    \/ \E provider_override_present \in BOOLEAN : \E model_override_present \in BOOLEAN : \E clear_provider_params \in BOOLEAN : \E provider_params_override_present \in BOOLEAN : \E clear_auth_binding \in BOOLEAN : \E auth_binding_override_present \in BOOLEAN : \E has_build_only_overrides \in BOOLEAN : \E first_turn_phase \in SessionFirstTurnPhaseValues : AuthorizeSessionResumeOverridesRejectClearAndSetProviderParams(provider_override_present, model_override_present, clear_provider_params, provider_params_override_present, clear_auth_binding, auth_binding_override_present, has_build_only_overrides, first_turn_phase)
    \/ \E provider_override_present \in BOOLEAN : \E model_override_present \in BOOLEAN : \E clear_provider_params \in BOOLEAN : \E provider_params_override_present \in BOOLEAN : \E clear_auth_binding \in BOOLEAN : \E auth_binding_override_present \in BOOLEAN : \E has_build_only_overrides \in BOOLEAN : \E first_turn_phase \in SessionFirstTurnPhaseValues : AuthorizeSessionResumeOverridesRejectClearAndSetAuthBinding(provider_override_present, model_override_present, clear_provider_params, provider_params_override_present, clear_auth_binding, auth_binding_override_present, has_build_only_overrides, first_turn_phase)
    \/ \E provider_override_present \in BOOLEAN : \E model_override_present \in BOOLEAN : \E clear_provider_params \in BOOLEAN : \E provider_params_override_present \in BOOLEAN : \E clear_auth_binding \in BOOLEAN : \E auth_binding_override_present \in BOOLEAN : \E has_build_only_overrides \in BOOLEAN : \E first_turn_phase \in SessionFirstTurnPhaseValues : AuthorizeSessionResumeOverridesRejectBuildOnlyAfterFirstTurn(provider_override_present, model_override_present, clear_provider_params, provider_params_override_present, clear_auth_binding, auth_binding_override_present, has_build_only_overrides, first_turn_phase)
    \/ \E provider_override_present \in BOOLEAN : \E model_override_present \in BOOLEAN : \E clear_provider_params \in BOOLEAN : \E provider_params_override_present \in BOOLEAN : \E clear_auth_binding \in BOOLEAN : \E auth_binding_override_present \in BOOLEAN : \E has_build_only_overrides \in BOOLEAN : \E first_turn_phase \in SessionFirstTurnPhaseValues : AuthorizeSessionResumeOverridesAcceptRecomputeProvider(provider_override_present, model_override_present, clear_provider_params, provider_params_override_present, clear_auth_binding, auth_binding_override_present, has_build_only_overrides, first_turn_phase)
    \/ \E provider_override_present \in BOOLEAN : \E model_override_present \in BOOLEAN : \E clear_provider_params \in BOOLEAN : \E provider_params_override_present \in BOOLEAN : \E clear_auth_binding \in BOOLEAN : \E auth_binding_override_present \in BOOLEAN : \E has_build_only_overrides \in BOOLEAN : \E first_turn_phase \in SessionFirstTurnPhaseValues : AuthorizeSessionResumeOverridesAcceptUseOverride(provider_override_present, model_override_present, clear_provider_params, provider_params_override_present, clear_auth_binding, auth_binding_override_present, has_build_only_overrides, first_turn_phase)
    \/ \E provider_override_present \in BOOLEAN : \E model_override_present \in BOOLEAN : \E clear_provider_params \in BOOLEAN : \E provider_params_override_present \in BOOLEAN : \E clear_auth_binding \in BOOLEAN : \E auth_binding_override_present \in BOOLEAN : \E has_build_only_overrides \in BOOLEAN : \E first_turn_phase \in SessionFirstTurnPhaseValues : AuthorizeSessionResumeOverridesAcceptRetainStored(provider_override_present, model_override_present, clear_provider_params, provider_params_override_present, clear_auth_binding, auth_binding_override_present, has_build_only_overrides, first_turn_phase)


CiStateConstraint == /\ model_step_count <= 6 /\ Cardinality(DOMAIN session_first_turn_phase) <= 1 /\ Cardinality(DOMAIN session_pending_initial_prompt_present) <= 1 /\ Cardinality(DOMAIN session_pending_tool_results_count) <= 1
DeepStateConstraint == /\ model_step_count <= 8 /\ Cardinality(DOMAIN session_first_turn_phase) <= 2 /\ Cardinality(DOMAIN session_pending_initial_prompt_present) <= 2 /\ Cardinality(DOMAIN session_pending_tool_results_count) <= 2

Spec == Init /\ [][Next]_vars


=============================================================================
