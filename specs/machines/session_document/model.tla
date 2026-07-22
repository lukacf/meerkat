---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for SessionDocumentMachine.

CONSTANTS BooleanValues, LegacyCheckpointMigrationDispositionValues, LegacyCheckpointTranscriptRelationValues, LiveSessionAuthorityKindValues, LiveSessionAuthorityReasonValues, NatValues, ObservedSessionTailKindValues, PendingContinuationDispositionValues, PendingContinuationPublicTerminalValues, RealtimeTranscriptLaneKindValues, RealtimeTranscriptMaterializeDecisionValues, RealtimeTranscriptRoleKindValues, RealtimeTranscriptStopReasonKindValues, RealtimeUserContentBlobFinalizeDispositionValues, RealtimeUserContentBlobRecoveryDispositionValues, RealtimeUserContentBlobStageDispositionValues, RealtimeUserContentIdentityDispositionValues, ResumeOverrideRejectionValues, ResumeProviderSelectionValues, ResumeSelfHostedSelectionValues, RuntimeCheckpointProjectionDispositionValues, RuntimeProjectionRollbackDispositionValues, SessionArchiveDispositionValues, SessionArchiveRuntimeObservationValues, SessionDocumentLifecycleValues, SessionFirstTurnPhaseValues, SessionIdValues, SessionInitialPromptStageDecisionValues, SessionSystemPromptSourceValues, SystemContextAppendDecisionValues, SystemContextPersistAppendAdmissionValues, SystemContextSourceValues, TranscriptEditKindValues

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

MapSessionIdBoolValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in SessionIdValues, v \in BOOLEAN }
MapSessionIdSessionDocumentLifecycleValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in SessionIdValues, v \in SessionDocumentLifecycleValues }
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

VARIABLES phase, model_step_count, session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal

vars == << phase, model_step_count, session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>

archive_should_retire_runtime(runtime_backed, runtime_observation) == (runtime_backed /\ (runtime_observation = "RetirementRequired"))
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
    /\ session_lifecycle_terminal = [x \in {} |-> None]

MarkSessionInitialTurnPendingInactiveOrPending(session_id) ==
    /\ phase = "Ready"
    /\ (IF ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Inactive") THEN TRUE ELSE ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Pending"))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_first_turn_phase' = MapSet(session_first_turn_phase, session_id, "Pending")
    /\ UNCHANGED << session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


MarkSessionInitialTurnPendingConsumed(session_id) ==
    /\ phase = "Ready"
    /\ ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Consumed")
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


StartSessionInitialTurnPending(session_id) ==
    /\ phase = "Ready"
    /\ ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Pending")
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_first_turn_phase' = MapSet(session_first_turn_phase, session_id, "Consumed")
    /\ UNCHANGED << session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


StartSessionInitialTurnInactive(session_id) ==
    /\ phase = "Ready"
    /\ ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Inactive")
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


StartSessionInitialTurnConsumed(session_id) ==
    /\ phase = "Ready"
    /\ ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Consumed")
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveSessionFirstTurnOverridesAllowed(session_id) ==
    /\ phase = "Ready"
    /\ phase_allows_initial_turn_overrides((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveSessionFirstTurnOverridesDenied(session_id) ==
    /\ phase = "Ready"
    /\ (phase_allows_initial_turn_overrides((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None)) = FALSE)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


StageSessionInitialPromptStore(session_id, prompt_has_content) ==
    /\ phase = "Ready"
    /\ should_store_initial_prompt((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None), prompt_has_content)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_pending_initial_prompt_present' = MapSet(session_pending_initial_prompt_present, session_id, TRUE)
    /\ UNCHANGED << session_first_turn_phase, session_pending_tool_results_count, session_lifecycle_terminal >>


StageSessionInitialPromptClear(session_id, prompt_has_content) ==
    /\ phase = "Ready"
    /\ (should_store_initial_prompt((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None), prompt_has_content) = FALSE)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_pending_initial_prompt_present' = MapSet(session_pending_initial_prompt_present, session_id, FALSE)
    /\ UNCHANGED << session_first_turn_phase, session_pending_tool_results_count, session_lifecycle_terminal >>


StageSessionToolResults(session_id, result_count) ==
    /\ phase = "Ready"
    /\ (IF ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Inactive") THEN TRUE ELSE (IF ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Pending") THEN TRUE ELSE ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Consumed")))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_pending_tool_results_count' = MapSet(session_pending_tool_results_count, session_id, result_count)
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_lifecycle_terminal >>


ConsumeSessionDeferredInputsPending(session_id) ==
    /\ phase = "Ready"
    /\ ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Pending")
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_first_turn_phase' = MapSet(session_first_turn_phase, session_id, "Consumed")
    /\ session_pending_initial_prompt_present' = MapSet(session_pending_initial_prompt_present, session_id, FALSE)
    /\ session_pending_tool_results_count' = MapSet(session_pending_tool_results_count, session_id, 0)
    /\ UNCHANGED << session_lifecycle_terminal >>


ConsumeSessionDeferredInputsInactive(session_id) ==
    /\ phase = "Ready"
    /\ ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Inactive")
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_pending_initial_prompt_present' = MapSet(session_pending_initial_prompt_present, session_id, FALSE)
    /\ session_pending_tool_results_count' = MapSet(session_pending_tool_results_count, session_id, 0)
    /\ UNCHANGED << session_first_turn_phase, session_lifecycle_terminal >>


ConsumeSessionDeferredInputsConsumed(session_id) ==
    /\ phase = "Ready"
    /\ ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Consumed")
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_pending_initial_prompt_present' = MapSet(session_pending_initial_prompt_present, session_id, FALSE)
    /\ session_pending_tool_results_count' = MapSet(session_pending_tool_results_count, session_id, 0)
    /\ UNCHANGED << session_first_turn_phase, session_lifecycle_terminal >>


RestoreSessionConsumedInputs(session_id, restore_first_turn_pending, pending_initial_prompt_present, pending_tool_result_message_count) ==
    /\ phase = "Ready"
    /\ restore_first_turn_pending
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_first_turn_phase' = MapSet(session_first_turn_phase, session_id, "Pending")
    /\ session_pending_initial_prompt_present' = MapSet(session_pending_initial_prompt_present, session_id, pending_initial_prompt_present)
    /\ session_pending_tool_results_count' = MapSet(session_pending_tool_results_count, session_id, pending_tool_result_message_count)
    /\ UNCHANGED << session_lifecycle_terminal >>


RestoreSessionConsumedInputsNoPhaseRollback(session_id, restore_first_turn_pending, pending_initial_prompt_present, pending_tool_result_message_count) ==
    /\ phase = "Ready"
    /\ (restore_first_turn_pending = FALSE)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_pending_initial_prompt_present' = MapSet(session_pending_initial_prompt_present, session_id, pending_initial_prompt_present)
    /\ session_pending_tool_results_count' = MapSet(session_pending_tool_results_count, session_id, pending_tool_result_message_count)
    /\ UNCHANGED << session_first_turn_phase, session_lifecycle_terminal >>


RecoverSessionFirstTurnPhase(session_id, arg_phase, pending_initial_prompt_present, pending_tool_result_message_count) ==
    /\ phase = "Ready"
    /\ (IF (arg_phase = "Inactive") THEN TRUE ELSE (IF (arg_phase = "Pending") THEN TRUE ELSE (arg_phase = "Consumed")))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_first_turn_phase' = MapSet(session_first_turn_phase, session_id, arg_phase)
    /\ session_pending_initial_prompt_present' = MapSet(session_pending_initial_prompt_present, session_id, pending_initial_prompt_present)
    /\ session_pending_tool_results_count' = MapSet(session_pending_tool_results_count, session_id, pending_tool_result_message_count)
    /\ UNCHANGED << session_lifecycle_terminal >>


ResolveSystemContextAppendEmpty(trimmed_text_byte_count, idempotency_key_present, existing_key_matches, existing_key_conflicts, active_turn_scoped) ==
    /\ phase = "Ready"
    /\ append_is_empty(trimmed_text_byte_count)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveSystemContextAppendConflict(trimmed_text_byte_count, idempotency_key_present, existing_key_matches, existing_key_conflicts, active_turn_scoped) ==
    /\ phase = "Ready"
    /\ ((append_is_empty(trimmed_text_byte_count) = FALSE) /\ append_is_conflict(idempotency_key_present, existing_key_conflicts))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveSystemContextAppendDuplicate(trimmed_text_byte_count, idempotency_key_present, existing_key_matches, existing_key_conflicts, active_turn_scoped) ==
    /\ phase = "Ready"
    /\ ((append_is_empty(trimmed_text_byte_count) = FALSE) /\ append_is_duplicate(idempotency_key_present, existing_key_matches, existing_key_conflicts))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveSystemContextAppendNew(trimmed_text_byte_count, idempotency_key_present, existing_key_matches, existing_key_conflicts, active_turn_scoped) ==
    /\ phase = "Ready"
    /\ ((append_is_empty(trimmed_text_byte_count) = FALSE) /\ append_is_new(idempotency_key_present, existing_key_matches, existing_key_conflicts))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveSystemContextPersistAppendAdmissionAdmit(has_previous, content_identical, content_extends_previous, appended_starts_with_separator, incoming_is_runtime_context_append) ==
    /\ phase = "Ready"
    /\ persist_append_is_admissible(has_previous, content_identical, content_extends_previous, appended_starts_with_separator, incoming_is_runtime_context_append)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveSystemContextPersistAppendAdmissionReject(has_previous, content_identical, content_extends_previous, appended_starts_with_separator, incoming_is_runtime_context_append) ==
    /\ phase = "Ready"
    /\ (persist_append_is_admissible(has_previous, content_identical, content_extends_previous, appended_starts_with_separator, incoming_is_runtime_context_append) = FALSE)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveSystemContextPendingApplyItemRuntimeSteer(source_kind) ==
    /\ phase = "Ready"
    /\ (source_kind = "RuntimeSteer")
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveSystemContextPendingApplyItemNormal(source_kind) ==
    /\ phase = "Ready"
    /\ (source_kind = "Normal")
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveSystemContextSteerCleanupItemRuntimeSteer(source_kind) ==
    /\ phase = "Ready"
    /\ (source_kind = "RuntimeSteer")
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveSystemContextSteerCleanupItemNormal(source_kind) ==
    /\ phase = "Ready"
    /\ (source_kind = "Normal")
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


RestoreSystemContextSnapshot(active_turn_membership_is_consistent, seen_keys_match_known_appends) ==
    /\ phase = "Ready"
    /\ (active_turn_membership_is_consistent /\ seen_keys_match_known_appends)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveRealtimeItemObservedDiscardedAssistant(role, response_discarded) ==
    /\ phase = "Ready"
    /\ ((role = "Assistant") /\ response_discarded)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveRealtimeItemObservedPresent(role, response_discarded) ==
    /\ phase = "Ready"
    /\ (IF (role # "Assistant") THEN TRUE ELSE (response_discarded = FALSE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveRealtimeItemSkipped ==
    /\ phase = "Ready"
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveRealtimeUserTranscriptFinalEmpty(text_present, segment_empty, segment_matches) ==
    /\ phase = "Ready"
    /\ (text_present = FALSE)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveRealtimeUserTranscriptFinalStore(text_present, segment_empty, segment_matches) ==
    /\ phase = "Ready"
    /\ (text_present /\ segment_empty)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveRealtimeUserTranscriptFinalReplayOrConflict(text_present, segment_empty, segment_matches) ==
    /\ phase = "Ready"
    /\ (text_present /\ (segment_empty = FALSE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveRealtimeUserContentIdentityInvalid(identity_fields_valid, key_tombstoned, predecessor_materialized, existing_identity_present, existing_payload_matches, target_item_id_available, reducer_commit_proof_required, reducer_commit_proof_present) ==
    /\ phase = "Ready"
    /\ (IF (identity_fields_valid = FALSE) THEN TRUE ELSE ((key_tombstoned = FALSE) /\ predecessor_materialized /\ (existing_identity_present = FALSE) /\ target_item_id_available /\ reducer_commit_proof_required /\ (reducer_commit_proof_present = FALSE)))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveRealtimeUserContentIdentityUnmaterializedPredecessor(identity_fields_valid, key_tombstoned, predecessor_materialized, existing_identity_present, existing_payload_matches, target_item_id_available, reducer_commit_proof_required, reducer_commit_proof_present) ==
    /\ phase = "Ready"
    /\ (identity_fields_valid /\ (key_tombstoned = FALSE) /\ (predecessor_materialized = FALSE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveRealtimeUserContentIdentityConflict(identity_fields_valid, key_tombstoned, predecessor_materialized, existing_identity_present, existing_payload_matches, target_item_id_available, reducer_commit_proof_required, reducer_commit_proof_present) ==
    /\ phase = "Ready"
    /\ (identity_fields_valid /\ (IF key_tombstoned THEN TRUE ELSE (predecessor_materialized /\ (IF (existing_identity_present /\ (existing_payload_matches = FALSE)) THEN TRUE ELSE ((existing_identity_present = FALSE) /\ (target_item_id_available = FALSE))))))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveRealtimeUserContentIdentityReplay(identity_fields_valid, key_tombstoned, predecessor_materialized, existing_identity_present, existing_payload_matches, target_item_id_available, reducer_commit_proof_required, reducer_commit_proof_present) ==
    /\ phase = "Ready"
    /\ (identity_fields_valid /\ (key_tombstoned = FALSE) /\ predecessor_materialized /\ existing_identity_present /\ existing_payload_matches)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveRealtimeUserContentIdentityCommitNew(identity_fields_valid, key_tombstoned, predecessor_materialized, existing_identity_present, existing_payload_matches, target_item_id_available, reducer_commit_proof_required, reducer_commit_proof_present) ==
    /\ phase = "Ready"
    /\ (identity_fields_valid /\ (key_tombstoned = FALSE) /\ predecessor_materialized /\ (existing_identity_present = FALSE) /\ target_item_id_available /\ (IF (reducer_commit_proof_required = FALSE) THEN TRUE ELSE reducer_commit_proof_present))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveRealtimeUserContentBlobStageNew(pending_present, pending_matches_request) ==
    /\ phase = "Ready"
    /\ (pending_present = FALSE)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveRealtimeUserContentBlobStageReuseExact(pending_present, pending_matches_request) ==
    /\ phase = "Ready"
    /\ (pending_present /\ pending_matches_request)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveRealtimeUserContentBlobStageRejectOccupied(pending_present, pending_matches_request) ==
    /\ phase = "Ready"
    /\ (pending_present /\ (pending_matches_request = FALSE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveRealtimeUserContentBlobRecoveryNone(pending_present, request_matches_pending, pending_blob_valid) ==
    /\ phase = "Ready"
    /\ (pending_present = FALSE)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveRealtimeUserContentBlobRecoveryExact(pending_present, request_matches_pending, pending_blob_valid) ==
    /\ phase = "Ready"
    /\ (pending_present /\ request_matches_pending)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveRealtimeUserContentBlobRecoveryCommitVerified(pending_present, request_matches_pending, pending_blob_valid) ==
    /\ phase = "Ready"
    /\ (pending_present /\ (request_matches_pending = FALSE) /\ pending_blob_valid)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveRealtimeUserContentBlobRecoveryClearInvalid(pending_present, request_matches_pending, pending_blob_valid) ==
    /\ phase = "Ready"
    /\ (pending_present /\ (request_matches_pending = FALSE) /\ (pending_blob_valid = FALSE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveRealtimeUserContentBlobFinalizeNone(pending_present, pending_matches_committed) ==
    /\ phase = "Ready"
    /\ (pending_present = FALSE)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveRealtimeUserContentBlobFinalizeClearCommitted(pending_present, pending_matches_committed) ==
    /\ phase = "Ready"
    /\ (pending_present /\ pending_matches_committed)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveRealtimeUserContentBlobFinalizeRejectMismatch(pending_present, pending_matches_committed) ==
    /\ phase = "Ready"
    /\ (pending_present /\ (pending_matches_committed = FALSE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveRealtimeUserContentFinalEmpty(content_present, segment_empty, segment_matches) ==
    /\ phase = "Ready"
    /\ (content_present = FALSE)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveRealtimeUserContentFinalStore(content_present, segment_empty, segment_matches) ==
    /\ phase = "Ready"
    /\ (content_present /\ segment_empty)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveRealtimeUserContentFinalReplayOrConflict(content_present, segment_empty, segment_matches) ==
    /\ phase = "Ready"
    /\ (content_present /\ (segment_empty = FALSE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveRealtimeAssistantDeltaInvalidOrDuplicate(response_id_valid, response_discarded, delta_id_present, delta_id_seen, item_has_text, current_lane, requested_lane, response_completed, text_after_write_present) ==
    /\ phase = "Ready"
    /\ (IF (response_id_valid = FALSE) THEN TRUE ELSE realtime_delta_is_duplicate(delta_id_present, delta_id_seen))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveRealtimeAssistantDeltaDiscarded(response_id_valid, response_discarded, delta_id_present, delta_id_seen, item_has_text, current_lane, requested_lane, response_completed, text_after_write_present) ==
    /\ phase = "Ready"
    /\ (response_id_valid /\ response_discarded)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveRealtimeAssistantDeltaLaneConflict(response_id_valid, response_discarded, delta_id_present, delta_id_seen, item_has_text, current_lane, requested_lane, response_completed, text_after_write_present) ==
    /\ phase = "Ready"
    /\ (response_id_valid /\ (response_discarded = FALSE) /\ (realtime_delta_is_duplicate(delta_id_present, delta_id_seen) = FALSE) /\ (realtime_lane_accepts(item_has_text, current_lane, requested_lane) = FALSE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveRealtimeAssistantDeltaAccepted(response_id_valid, response_discarded, delta_id_present, delta_id_seen, item_has_text, current_lane, requested_lane, response_completed, text_after_write_present) ==
    /\ phase = "Ready"
    /\ (response_id_valid /\ (response_discarded = FALSE) /\ (realtime_delta_is_duplicate(delta_id_present, delta_id_seen) = FALSE) /\ realtime_lane_accepts(item_has_text, current_lane, requested_lane))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveRealtimeAssistantReplacementInvalid(response_id_valid, response_discarded, item_materialized, item_has_text, current_lane, requested_lane, response_completed, text_after_replace_present) ==
    /\ phase = "Ready"
    /\ (response_id_valid = FALSE)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveRealtimeAssistantReplacementDiscarded(response_id_valid, response_discarded, item_materialized, item_has_text, current_lane, requested_lane, response_completed, text_after_replace_present) ==
    /\ phase = "Ready"
    /\ (response_id_valid /\ response_discarded)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveRealtimeAssistantReplacementLocked(response_id_valid, response_discarded, item_materialized, item_has_text, current_lane, requested_lane, response_completed, text_after_replace_present) ==
    /\ phase = "Ready"
    /\ (response_id_valid /\ (response_discarded = FALSE) /\ item_materialized)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveRealtimeAssistantReplacementLaneConflict(response_id_valid, response_discarded, item_materialized, item_has_text, current_lane, requested_lane, response_completed, text_after_replace_present) ==
    /\ phase = "Ready"
    /\ (response_id_valid /\ (response_discarded = FALSE) /\ (item_materialized = FALSE) /\ (realtime_lane_accepts(item_has_text, current_lane, requested_lane) = FALSE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveRealtimeAssistantReplacementAccepted(response_id_valid, response_discarded, item_materialized, item_has_text, current_lane, requested_lane, response_completed, text_after_replace_present) ==
    /\ phase = "Ready"
    /\ (response_id_valid /\ (response_discarded = FALSE) /\ (item_materialized = FALSE) /\ realtime_lane_accepts(item_has_text, current_lane, requested_lane))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveRealtimeAssistantTurnCompletedInvalid(response_id_valid, response_discarded, stop_reason) ==
    /\ phase = "Ready"
    /\ (response_id_valid = FALSE)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveRealtimeAssistantTurnCompletedDiscard(response_id_valid, response_discarded, stop_reason) ==
    /\ phase = "Ready"
    /\ (response_id_valid /\ (IF response_discarded THEN TRUE ELSE realtime_stop_reason_discards(stop_reason)))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveRealtimeAssistantTurnCompletedToolUse(response_id_valid, response_discarded, stop_reason) ==
    /\ phase = "Ready"
    /\ (response_id_valid /\ (response_discarded = FALSE) /\ realtime_stop_reason_removes_completion(stop_reason))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveRealtimeAssistantTurnCompletedRecord(response_id_valid, response_discarded, stop_reason) ==
    /\ phase = "Ready"
    /\ (response_id_valid /\ (response_discarded = FALSE) /\ realtime_stop_reason_records_completion(stop_reason))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveRealtimeAssistantTurnInterruptedInvalid(response_id_valid) ==
    /\ phase = "Ready"
    /\ (response_id_valid = FALSE)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveRealtimeAssistantTurnInterruptedValid(response_id_valid) ==
    /\ phase = "Ready"
    /\ response_id_valid
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveRealtimeMaterializeAlreadyDone(item_materialized, predecessor_materialized, item_skipped, item_ready, item_text_present, role, response_id_present, completion_present, completion_usage_consumed) ==
    /\ phase = "Ready"
    /\ item_materialized
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveRealtimeMaterializeWaitForPredecessor(item_materialized, predecessor_materialized, item_skipped, item_ready, item_text_present, role, response_id_present, completion_present, completion_usage_consumed) ==
    /\ phase = "Ready"
    /\ ((item_materialized = FALSE) /\ (predecessor_materialized = FALSE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveRealtimeMaterializeSkipped(item_materialized, predecessor_materialized, item_skipped, item_ready, item_text_present, role, response_id_present, completion_present, completion_usage_consumed) ==
    /\ phase = "Ready"
    /\ ((item_materialized = FALSE) /\ predecessor_materialized /\ item_skipped)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveRealtimeMaterializeWaitForReadyText(item_materialized, predecessor_materialized, item_skipped, item_ready, item_text_present, role, response_id_present, completion_present, completion_usage_consumed) ==
    /\ phase = "Ready"
    /\ ((item_materialized = FALSE) /\ predecessor_materialized /\ (item_skipped = FALSE) /\ (IF (item_ready = FALSE) THEN TRUE ELSE (item_text_present = FALSE)))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveRealtimeMaterializeUser(item_materialized, predecessor_materialized, item_skipped, item_ready, item_text_present, role, response_id_present, completion_present, completion_usage_consumed) ==
    /\ phase = "Ready"
    /\ ((item_materialized = FALSE) /\ predecessor_materialized /\ (item_skipped = FALSE) /\ item_ready /\ item_text_present /\ (role = "User"))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveRealtimeMaterializeAssistant(item_materialized, predecessor_materialized, item_skipped, item_ready, item_text_present, role, response_id_present, completion_present, completion_usage_consumed) ==
    /\ phase = "Ready"
    /\ ((item_materialized = FALSE) /\ predecessor_materialized /\ (item_skipped = FALSE) /\ item_ready /\ item_text_present /\ (role = "Assistant") /\ response_id_present /\ completion_present)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveRealtimeMaterializeAssistantMissingCompletion(item_materialized, predecessor_materialized, item_skipped, item_ready, item_text_present, role, response_id_present, completion_present, completion_usage_consumed) ==
    /\ phase = "Ready"
    /\ ((item_materialized = FALSE) /\ predecessor_materialized /\ (item_skipped = FALSE) /\ item_ready /\ item_text_present /\ (role = "Assistant") /\ (IF (response_id_present = FALSE) THEN TRUE ELSE (completion_present = FALSE)))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


AuthorizeRestoreRealtimeTranscriptState(item_count, first_seen_count, first_seen_unique_count, every_item_has_order_entry, every_order_entry_has_item, all_materialized_predecessor_references_exist, no_self_predecessor_references, causal_graph_acyclic, all_materialized_items_have_materialized_ancestry, all_identity_fields_valid, all_user_content_identity_keys_match, all_user_content_identity_fields_valid, all_user_content_identity_item_ids_unique, all_user_content_identities_reference_materialized_user_items, all_user_content_tombstones_valid, user_content_identities_and_tombstones_disjoint, pending_user_content_blob_fields_valid, pending_user_content_blob_uncommitted, all_delta_ids_valid, all_completion_response_ids_valid, all_discarded_response_ids_valid, all_materialized_items_were_ready_or_skipped, all_assistant_items_have_response_unless_skipped, all_ready_assistant_items_have_completion_or_are_skipped, all_materialized_assistant_completions_consumed, all_completed_assistant_text_items_are_ready_or_materialized_or_skipped, all_discarded_assistant_items_are_skipped_or_materialized) ==
    /\ phase = "Ready"
    /\ ((item_count = first_seen_count) /\ (first_seen_count = first_seen_unique_count) /\ every_item_has_order_entry /\ every_order_entry_has_item /\ all_materialized_predecessor_references_exist /\ no_self_predecessor_references /\ causal_graph_acyclic /\ all_materialized_items_have_materialized_ancestry /\ all_identity_fields_valid /\ all_user_content_identity_keys_match /\ all_user_content_identity_fields_valid /\ all_user_content_identity_item_ids_unique /\ all_user_content_identities_reference_materialized_user_items /\ all_user_content_tombstones_valid /\ user_content_identities_and_tombstones_disjoint /\ pending_user_content_blob_fields_valid /\ pending_user_content_blob_uncommitted /\ all_delta_ids_valid /\ all_completion_response_ids_valid /\ all_discarded_response_ids_valid /\ all_materialized_items_were_ready_or_skipped /\ all_assistant_items_have_response_unless_skipped /\ all_ready_assistant_items_have_completion_or_are_skipped /\ all_materialized_assistant_completions_consumed /\ all_completed_assistant_text_items_are_ready_or_materialized_or_skipped /\ all_discarded_assistant_items_are_skipped_or_materialized)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


AuthorizeSessionMetadataPersist(schema_version, model_present) ==
    /\ phase = "Ready"
    /\ ((schema_version > 0) /\ (model_present = TRUE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


AuthorizeSessionBuildStatePersist(mob_tool_authority_context_present, mob_tool_authority_context_generated) ==
    /\ phase = "Ready"
    /\ (IF (mob_tool_authority_context_present = FALSE) THEN TRUE ELSE (mob_tool_authority_context_generated = TRUE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


RestoreSessionBuildState ==
    /\ phase = "Ready"
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


AuthorizeSystemPromptMutation(source, prompt_present, prompt_byte_count, replacing_existing) ==
    /\ phase = "Ready"
    /\ (IF (prompt_present = TRUE) THEN TRUE ELSE (prompt_byte_count = 0))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolvePendingContinuationWithBoundary(session_tail, staged_tool_result_count) ==
    /\ phase = "Ready"
    /\ has_effective_pending_boundary(session_tail, staged_tool_result_count)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolvePendingContinuationWithoutBoundary(session_tail, staged_tool_result_count) ==
    /\ phase = "Ready"
    /\ (has_effective_pending_boundary(session_tail, staged_tool_result_count) = FALSE)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


AuthorizeSessionResumeOverridesRejectProviderRequiresModel(provider_override_present, model_override_present, self_hosted_server_override_present, has_build_only_overrides, first_turn_phase) ==
    /\ phase = "Ready"
    /\ resume_reject_provider_requires_model(provider_override_present, model_override_present)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


AuthorizeSessionResumeOverridesRejectBuildOnlyAfterFirstTurn(provider_override_present, model_override_present, self_hosted_server_override_present, has_build_only_overrides, first_turn_phase) ==
    /\ phase = "Ready"
    /\ ((resume_reject_provider_requires_model(provider_override_present, model_override_present) = FALSE) /\ resume_reject_build_only_after_first_turn(has_build_only_overrides, first_turn_phase))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


AuthorizeSessionResumeOverridesAcceptRecomputeProvider(provider_override_present, model_override_present, self_hosted_server_override_present, has_build_only_overrides, first_turn_phase) ==
    /\ phase = "Ready"
    /\ (resume_overrides_admissible(provider_override_present, model_override_present, has_build_only_overrides, first_turn_phase) /\ resume_provider_recompute_from_model(model_override_present, provider_override_present) /\ (self_hosted_server_override_present = FALSE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


AuthorizeSessionResumeOverridesAcceptRecomputeProviderWithSelfHostedOverride(provider_override_present, model_override_present, self_hosted_server_override_present, has_build_only_overrides, first_turn_phase) ==
    /\ phase = "Ready"
    /\ (resume_overrides_admissible(provider_override_present, model_override_present, has_build_only_overrides, first_turn_phase) /\ resume_provider_recompute_from_model(model_override_present, provider_override_present) /\ self_hosted_server_override_present)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


AuthorizeSessionResumeOverridesAcceptUseOverride(provider_override_present, model_override_present, self_hosted_server_override_present, has_build_only_overrides, first_turn_phase) ==
    /\ phase = "Ready"
    /\ (resume_overrides_admissible(provider_override_present, model_override_present, has_build_only_overrides, first_turn_phase) /\ (resume_provider_recompute_from_model(model_override_present, provider_override_present) = FALSE) /\ provider_override_present /\ (self_hosted_server_override_present = FALSE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


AuthorizeSessionResumeOverridesAcceptUseOverrideWithSelfHostedOverride(provider_override_present, model_override_present, self_hosted_server_override_present, has_build_only_overrides, first_turn_phase) ==
    /\ phase = "Ready"
    /\ (resume_overrides_admissible(provider_override_present, model_override_present, has_build_only_overrides, first_turn_phase) /\ (resume_provider_recompute_from_model(model_override_present, provider_override_present) = FALSE) /\ provider_override_present /\ self_hosted_server_override_present)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


AuthorizeSessionResumeOverridesAcceptRetainStored(provider_override_present, model_override_present, self_hosted_server_override_present, has_build_only_overrides, first_turn_phase) ==
    /\ phase = "Ready"
    /\ (resume_overrides_admissible(provider_override_present, model_override_present, has_build_only_overrides, first_turn_phase) /\ (resume_provider_recompute_from_model(model_override_present, provider_override_present) = FALSE) /\ (provider_override_present = FALSE) /\ (self_hosted_server_override_present = FALSE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


AuthorizeSessionResumeOverridesAcceptRetainStoredWithSelfHostedOverride(provider_override_present, model_override_present, self_hosted_server_override_present, has_build_only_overrides, first_turn_phase) ==
    /\ phase = "Ready"
    /\ (resume_overrides_admissible(provider_override_present, model_override_present, has_build_only_overrides, first_turn_phase) /\ (resume_provider_recompute_from_model(model_override_present, provider_override_present) = FALSE) /\ (provider_override_present = FALSE) /\ self_hosted_server_override_present)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ClassifyLiveSessionAuthorityLive(stored_transcript_diverged, live_has_uncommitted_transcript, runtime_system_context_diverged, stored_is_archived) ==
    /\ phase = "Ready"
    /\ ((stored_transcript_diverged = FALSE) /\ (live_has_uncommitted_transcript = FALSE) /\ (runtime_system_context_diverged = FALSE) /\ (stored_is_archived = FALSE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ClassifyLiveSessionAuthorityDurableArchived(stored_transcript_diverged, live_has_uncommitted_transcript, runtime_system_context_diverged, stored_is_archived) ==
    /\ phase = "Ready"
    /\ (stored_is_archived = TRUE)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ClassifyLiveSessionAuthorityDurableUncommitted(stored_transcript_diverged, live_has_uncommitted_transcript, runtime_system_context_diverged, stored_is_archived) ==
    /\ phase = "Ready"
    /\ ((stored_is_archived = FALSE) /\ (live_has_uncommitted_transcript = TRUE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ClassifyLiveSessionAuthorityDurableSystemContext(stored_transcript_diverged, live_has_uncommitted_transcript, runtime_system_context_diverged, stored_is_archived) ==
    /\ phase = "Ready"
    /\ ((stored_is_archived = FALSE) /\ (live_has_uncommitted_transcript = FALSE) /\ (runtime_system_context_diverged = TRUE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ClassifyLiveSessionAuthorityDurableRevision(stored_transcript_diverged, live_has_uncommitted_transcript, runtime_system_context_diverged, stored_is_archived) ==
    /\ phase = "Ready"
    /\ ((stored_is_archived = FALSE) /\ (live_has_uncommitted_transcript = FALSE) /\ (runtime_system_context_diverged = FALSE) /\ (stored_transcript_diverged = TRUE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


RecoverSessionFromStoreAuthorized(session_id, has_metadata, has_build_state, runtime_projection_quarantined) ==
    /\ phase = "Ready"
    /\ store_projection_can_recover_authority(has_metadata, has_build_state, runtime_projection_quarantined)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


RecoverSessionFromStoreUnrecoverable(session_id, has_metadata, has_build_state, runtime_projection_quarantined) ==
    /\ phase = "Ready"
    /\ (store_projection_can_recover_authority(has_metadata, has_build_state, runtime_projection_quarantined) = FALSE)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveRuntimeSnapshotReadSourceStoreHead(session_id, store_head_extends_snapshot, store_head_is_runtime_checkpoint, session_is_live) ==
    /\ phase = "Ready"
    /\ ((store_head_extends_snapshot = TRUE) /\ (store_head_is_runtime_checkpoint = FALSE) /\ (session_is_live = FALSE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveRuntimeSnapshotReadSourceSnapshot(session_id, store_head_extends_snapshot, store_head_is_runtime_checkpoint, session_is_live) ==
    /\ phase = "Ready"
    /\ (IF (store_head_extends_snapshot = FALSE) THEN TRUE ELSE (IF (store_head_is_runtime_checkpoint = TRUE) THEN TRUE ELSE (session_is_live = TRUE)))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveRuntimeProjectionRollbackRebuild(session_id, row_continues_authority, row_is_runtime_checkpoint) ==
    /\ phase = "Ready"
    /\ ((row_continues_authority = TRUE) /\ (row_is_runtime_checkpoint = TRUE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveRuntimeProjectionRollbackReject(session_id, row_continues_authority, row_is_runtime_checkpoint) ==
    /\ phase = "Ready"
    /\ (IF (row_continues_authority = FALSE) THEN TRUE ELSE (row_is_runtime_checkpoint = FALSE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveRuntimeCheckpointProjectionActive(session_id) ==
    /\ phase = "Ready"
    /\ ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_lifecycle_terminal) THEN Some((IF session_id \in DOMAIN session_lifecycle_terminal THEN session_lifecycle_terminal[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_lifecycle_terminal) THEN Some((IF session_id \in DOMAIN session_lifecycle_terminal THEN session_lifecycle_terminal[session_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Active")
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveRuntimeCheckpointProjectionArchived(session_id) ==
    /\ phase = "Ready"
    /\ ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_lifecycle_terminal) THEN Some((IF session_id \in DOMAIN session_lifecycle_terminal THEN session_lifecycle_terminal[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_lifecycle_terminal) THEN Some((IF session_id \in DOMAIN session_lifecycle_terminal THEN session_lifecycle_terminal[session_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Archived")
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveLegacyCheckpointMigrationSnapshotIdenticalProjection(session_id, runtime_snapshot_present, runtime_snapshot_legacy, store_row_present, store_row_legacy, transcript_relation) ==
    /\ phase = "Ready"
    /\ ((runtime_snapshot_present = TRUE) /\ (runtime_snapshot_legacy = TRUE) /\ (store_row_present = TRUE) /\ (store_row_legacy = TRUE) /\ (transcript_relation = "Identical"))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveLegacyCheckpointMigrationSnapshotAheadOfProjection(session_id, runtime_snapshot_present, runtime_snapshot_legacy, store_row_present, store_row_legacy, transcript_relation) ==
    /\ phase = "Ready"
    /\ ((runtime_snapshot_present = TRUE) /\ (runtime_snapshot_legacy = TRUE) /\ (store_row_present = TRUE) /\ (store_row_legacy = TRUE) /\ (transcript_relation = "SnapshotExtendsProjection"))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveLegacyCheckpointMigrationProjectionExtension(session_id, runtime_snapshot_present, runtime_snapshot_legacy, store_row_present, store_row_legacy, transcript_relation) ==
    /\ phase = "Ready"
    /\ ((runtime_snapshot_present = TRUE) /\ (runtime_snapshot_legacy = TRUE) /\ (store_row_present = TRUE) /\ (store_row_legacy = TRUE) /\ (transcript_relation = "ProjectionExtendsSnapshot"))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveLegacyCheckpointMigrationDivergentCopies(session_id, runtime_snapshot_present, runtime_snapshot_legacy, store_row_present, store_row_legacy, transcript_relation) ==
    /\ phase = "Ready"
    /\ ((runtime_snapshot_present = TRUE) /\ (runtime_snapshot_legacy = TRUE) /\ (store_row_present = TRUE) /\ (store_row_legacy = TRUE) /\ (transcript_relation = "Divergent"))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveLegacyCheckpointMigrationSnapshotOnly(session_id, runtime_snapshot_present, runtime_snapshot_legacy, store_row_present, store_row_legacy, transcript_relation) ==
    /\ phase = "Ready"
    /\ ((runtime_snapshot_present = TRUE) /\ (runtime_snapshot_legacy = TRUE) /\ (store_row_present = FALSE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveLegacyCheckpointMigrationStoreRowOnly(session_id, runtime_snapshot_present, runtime_snapshot_legacy, store_row_present, store_row_legacy, transcript_relation) ==
    /\ phase = "Ready"
    /\ ((runtime_snapshot_present = FALSE) /\ (store_row_present = TRUE) /\ (store_row_legacy = TRUE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ResolveLegacyCheckpointMigrationSnapshotLegacyProjectionTyped(session_id, runtime_snapshot_present, runtime_snapshot_legacy, store_row_present, store_row_legacy, transcript_relation) ==
    /\ phase = "Ready"
    /\ ((runtime_snapshot_present = TRUE) /\ (runtime_snapshot_legacy = TRUE) /\ (store_row_present = TRUE) /\ (store_row_legacy = FALSE))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ApplyPendingToolResults(session_id, result_count) ==
    /\ phase = "Ready"
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


TranscriptEditFork(session_id, fork_or_rewrite_directive) ==
    /\ phase = "Ready"
    /\ (fork_or_rewrite_directive = "Fork")
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


TranscriptEditRewrite(session_id, fork_or_rewrite_directive) ==
    /\ phase = "Ready"
    /\ (fork_or_rewrite_directive = "Rewrite")
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


RecoverSessionLifecycleTerminal(session_id, terminal) ==
    /\ phase = "Ready"
    /\ (IF (terminal = "Active") THEN TRUE ELSE (terminal = "Archived"))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_lifecycle_terminal' = MapSet(session_lifecycle_terminal, session_id, terminal)
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


ReviveArchivedSessionDocument(session_id) ==
    /\ phase = "Ready"
    /\ ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_lifecycle_terminal) THEN Some((IF session_id \in DOMAIN session_lifecycle_terminal THEN session_lifecycle_terminal[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_lifecycle_terminal) THEN Some((IF session_id \in DOMAIN session_lifecycle_terminal THEN session_lifecycle_terminal[session_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Archived")
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_lifecycle_terminal' = MapSet(session_lifecycle_terminal, session_id, "Active")
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


ArchiveSessionDocumentActive(session_id, runtime_backed, durable_document_present, runtime_observation) ==
    /\ phase = "Ready"
    /\ ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_lifecycle_terminal) THEN Some((IF session_id \in DOMAIN session_lifecycle_terminal THEN session_lifecycle_terminal[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_lifecycle_terminal) THEN Some((IF session_id \in DOMAIN session_lifecycle_terminal THEN session_lifecycle_terminal[session_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Active")
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_lifecycle_terminal' = MapSet(session_lifecycle_terminal, session_id, "Archived")
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


ArchiveSessionDocumentAlreadyArchived(session_id, runtime_backed, durable_document_present, runtime_observation) ==
    /\ phase = "Ready"
    /\ ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_lifecycle_terminal) THEN Some((IF session_id \in DOMAIN session_lifecycle_terminal THEN session_lifecycle_terminal[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_lifecycle_terminal) THEN Some((IF session_id \in DOMAIN session_lifecycle_terminal THEN session_lifecycle_terminal[session_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Archived")
    /\ (runtime_observation # "RetirementRequired")
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


ArchiveSessionDocumentCompleteRetire(session_id, runtime_backed, durable_document_present, runtime_observation) ==
    /\ phase = "Ready"
    /\ ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_lifecycle_terminal) THEN Some((IF session_id \in DOMAIN session_lifecycle_terminal THEN session_lifecycle_terminal[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_lifecycle_terminal) THEN Some((IF session_id \in DOMAIN session_lifecycle_terminal THEN session_lifecycle_terminal[session_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Archived")
    /\ (runtime_observation = "RetirementRequired")
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count, session_lifecycle_terminal >>


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
    \/ RestoreSystemContextSnapshot(TRUE, TRUE)
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
    \/ \E schema_version \in 0..2 : AuthorizeSessionMetadataPersist(schema_version, TRUE)
    \/ \E mob_tool_authority_context_present \in BOOLEAN : \E mob_tool_authority_context_generated \in BOOLEAN : AuthorizeSessionBuildStatePersist(mob_tool_authority_context_present, mob_tool_authority_context_generated)
    \/ RestoreSessionBuildState
    \/ \E source \in SessionSystemPromptSourceValues : \E prompt_present \in BOOLEAN : \E prompt_byte_count \in 0..2 : \E replacing_existing \in BOOLEAN : AuthorizeSystemPromptMutation(source, prompt_present, prompt_byte_count, replacing_existing)
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
    \/ ClassifyLiveSessionAuthorityLive(FALSE, FALSE, FALSE, FALSE)
    \/ \E stored_transcript_diverged \in BOOLEAN : \E live_has_uncommitted_transcript \in BOOLEAN : \E runtime_system_context_diverged \in BOOLEAN : ClassifyLiveSessionAuthorityDurableArchived(stored_transcript_diverged, live_has_uncommitted_transcript, runtime_system_context_diverged, TRUE)
    \/ \E stored_transcript_diverged \in BOOLEAN : \E runtime_system_context_diverged \in BOOLEAN : ClassifyLiveSessionAuthorityDurableUncommitted(stored_transcript_diverged, TRUE, runtime_system_context_diverged, FALSE)
    \/ \E stored_transcript_diverged \in BOOLEAN : ClassifyLiveSessionAuthorityDurableSystemContext(stored_transcript_diverged, FALSE, TRUE, FALSE)
    \/ ClassifyLiveSessionAuthorityDurableRevision(TRUE, FALSE, FALSE, FALSE)
    \/ \E session_id \in SessionIdValues : \E has_metadata \in BOOLEAN : \E has_build_state \in BOOLEAN : \E runtime_projection_quarantined \in BOOLEAN : RecoverSessionFromStoreAuthorized(session_id, has_metadata, has_build_state, runtime_projection_quarantined)
    \/ \E session_id \in SessionIdValues : \E has_metadata \in BOOLEAN : \E has_build_state \in BOOLEAN : \E runtime_projection_quarantined \in BOOLEAN : RecoverSessionFromStoreUnrecoverable(session_id, has_metadata, has_build_state, runtime_projection_quarantined)
    \/ \E session_id \in SessionIdValues : ResolveRuntimeSnapshotReadSourceStoreHead(session_id, TRUE, FALSE, FALSE)
    \/ \E session_id \in SessionIdValues : \E store_head_extends_snapshot \in BOOLEAN : \E store_head_is_runtime_checkpoint \in BOOLEAN : \E session_is_live \in BOOLEAN : ResolveRuntimeSnapshotReadSourceSnapshot(session_id, store_head_extends_snapshot, store_head_is_runtime_checkpoint, session_is_live)
    \/ \E session_id \in SessionIdValues : ResolveRuntimeProjectionRollbackRebuild(session_id, TRUE, TRUE)
    \/ \E session_id \in SessionIdValues : \E row_continues_authority \in BOOLEAN : \E row_is_runtime_checkpoint \in BOOLEAN : ResolveRuntimeProjectionRollbackReject(session_id, row_continues_authority, row_is_runtime_checkpoint)
    \/ \E session_id \in SessionIdValues : ResolveRuntimeCheckpointProjectionActive(session_id)
    \/ \E session_id \in SessionIdValues : ResolveRuntimeCheckpointProjectionArchived(session_id)
    \/ \E session_id \in SessionIdValues : \E transcript_relation \in LegacyCheckpointTranscriptRelationValues : ResolveLegacyCheckpointMigrationSnapshotIdenticalProjection(session_id, TRUE, TRUE, TRUE, TRUE, transcript_relation)
    \/ \E session_id \in SessionIdValues : \E transcript_relation \in LegacyCheckpointTranscriptRelationValues : ResolveLegacyCheckpointMigrationSnapshotAheadOfProjection(session_id, TRUE, TRUE, TRUE, TRUE, transcript_relation)
    \/ \E session_id \in SessionIdValues : \E transcript_relation \in LegacyCheckpointTranscriptRelationValues : ResolveLegacyCheckpointMigrationProjectionExtension(session_id, TRUE, TRUE, TRUE, TRUE, transcript_relation)
    \/ \E session_id \in SessionIdValues : \E transcript_relation \in LegacyCheckpointTranscriptRelationValues : ResolveLegacyCheckpointMigrationDivergentCopies(session_id, TRUE, TRUE, TRUE, TRUE, transcript_relation)
    \/ \E session_id \in SessionIdValues : \E store_row_legacy \in BOOLEAN : \E transcript_relation \in LegacyCheckpointTranscriptRelationValues : ResolveLegacyCheckpointMigrationSnapshotOnly(session_id, TRUE, TRUE, FALSE, store_row_legacy, transcript_relation)
    \/ \E session_id \in SessionIdValues : \E runtime_snapshot_legacy \in BOOLEAN : \E transcript_relation \in LegacyCheckpointTranscriptRelationValues : ResolveLegacyCheckpointMigrationStoreRowOnly(session_id, FALSE, runtime_snapshot_legacy, TRUE, TRUE, transcript_relation)
    \/ \E session_id \in SessionIdValues : \E transcript_relation \in LegacyCheckpointTranscriptRelationValues : ResolveLegacyCheckpointMigrationSnapshotLegacyProjectionTyped(session_id, TRUE, TRUE, TRUE, FALSE, transcript_relation)
    \/ \E session_id \in SessionIdValues : \E result_count \in 0..2 : ApplyPendingToolResults(session_id, result_count)
    \/ \E session_id \in SessionIdValues : \E fork_or_rewrite_directive \in TranscriptEditKindValues : TranscriptEditFork(session_id, fork_or_rewrite_directive)
    \/ \E session_id \in SessionIdValues : \E fork_or_rewrite_directive \in TranscriptEditKindValues : TranscriptEditRewrite(session_id, fork_or_rewrite_directive)
    \/ \E session_id \in SessionIdValues : \E terminal \in SessionDocumentLifecycleValues : RecoverSessionLifecycleTerminal(session_id, terminal)
    \/ \E session_id \in SessionIdValues : ReviveArchivedSessionDocument(session_id)
    \/ \E session_id \in SessionIdValues : \E runtime_backed \in BOOLEAN : \E durable_document_present \in BOOLEAN : \E runtime_observation \in SessionArchiveRuntimeObservationValues : ArchiveSessionDocumentActive(session_id, runtime_backed, durable_document_present, runtime_observation)
    \/ \E session_id \in SessionIdValues : \E runtime_backed \in BOOLEAN : \E durable_document_present \in BOOLEAN : \E runtime_observation \in SessionArchiveRuntimeObservationValues : ArchiveSessionDocumentAlreadyArchived(session_id, runtime_backed, durable_document_present, runtime_observation)
    \/ \E session_id \in SessionIdValues : \E runtime_backed \in BOOLEAN : \E durable_document_present \in BOOLEAN : \E runtime_observation \in SessionArchiveRuntimeObservationValues : ArchiveSessionDocumentCompleteRetire(session_id, runtime_backed, durable_document_present, runtime_observation)


CiStateConstraint == /\ model_step_count <= 6 /\ Cardinality(DOMAIN session_first_turn_phase) <= 1 /\ Cardinality(DOMAIN session_pending_initial_prompt_present) <= 1 /\ Cardinality(DOMAIN session_pending_tool_results_count) <= 1 /\ Cardinality(DOMAIN session_lifecycle_terminal) <= 1
DeepStateConstraint == /\ model_step_count <= 8 /\ Cardinality(DOMAIN session_first_turn_phase) <= 2 /\ Cardinality(DOMAIN session_pending_initial_prompt_present) <= 2 /\ Cardinality(DOMAIN session_pending_tool_results_count) <= 2 /\ Cardinality(DOMAIN session_lifecycle_terminal) <= 2

Spec == Init /\ [][Next]_vars


=============================================================================
