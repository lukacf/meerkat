# SessionDocumentMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `1`
- Rust owner: `self` / `catalog::dsl::session_document`

## State
- Phase enum: `Ready`
- `session_first_turn_phase`: `Map<SessionId, SessionFirstTurnPhase>`
- `session_pending_initial_prompt_present`: `Map<SessionId, Bool>`
- `session_pending_tool_results_count`: `Map<SessionId, u64>`

## Inputs
- `MarkSessionInitialTurnPending`(session_id: SessionId)
- `StartSessionInitialTurn`(session_id: SessionId)
- `StageSessionInitialPrompt`(session_id: SessionId, prompt_has_content: Bool)
- `StageSessionToolResults`(session_id: SessionId, result_count: u64)
- `ConsumeSessionDeferredInputs`(session_id: SessionId)
- `RestoreSessionConsumedInputs`(session_id: SessionId, restore_first_turn_pending: Bool, pending_initial_prompt_present: Bool, pending_tool_result_message_count: u64)
- `RecoverSessionFirstTurnPhase`(session_id: SessionId, phase: SessionFirstTurnPhase, pending_initial_prompt_present: Bool, pending_tool_result_message_count: u64)
- `ResolveSessionFirstTurnOverridesAllowed`(session_id: SessionId)
- `ResolveSystemContextAppend`(trimmed_text_byte_count: u64, idempotency_key_present: Bool, existing_key_matches: Bool, existing_key_conflicts: Bool, active_turn_scoped: Bool)
- `ResolveSystemContextPendingApplyItem`(source_kind: SystemContextSource)
- `ResolveSystemContextSteerCleanupItem`(source_kind: SystemContextSource)
- `RestoreSystemContextSnapshot`(active_keys_have_known_pending_or_seen: Bool, seen_keys_match_known_appends: Bool)
- `ResolveRealtimeItemObserved`(role: RealtimeTranscriptRoleKind, response_discarded: Bool)
- `ResolveRealtimeItemSkipped`
- `ResolveRealtimeUserTranscriptFinal`(text_present: Bool, segment_empty: Bool, segment_matches: Bool)
- `ResolveRealtimeAssistantDelta`(response_id_valid: Bool, response_discarded: Bool, delta_id_present: Bool, delta_id_seen: Bool, item_has_text: Bool, current_lane: RealtimeTranscriptLaneKind, requested_lane: RealtimeTranscriptLaneKind, response_completed: Bool, text_after_write_present: Bool)
- `ResolveRealtimeAssistantTextReplacement`(response_id_valid: Bool, response_discarded: Bool, item_materialized: Bool, item_has_text: Bool, current_lane: RealtimeTranscriptLaneKind, requested_lane: RealtimeTranscriptLaneKind, response_completed: Bool, text_after_replace_present: Bool)
- `ResolveRealtimeAssistantTurnCompleted`(response_id_valid: Bool, response_discarded: Bool, stop_reason: RealtimeTranscriptStopReasonKind)
- `ResolveRealtimeAssistantTurnInterrupted`(response_id_valid: Bool)
- `ResolveRealtimeMaterializeCandidate`(item_materialized: Bool, predecessor_materialized: Bool, item_skipped: Bool, item_ready: Bool, item_text_present: Bool, role: RealtimeTranscriptRoleKind, response_id_present: Bool, completion_present: Bool, completion_usage_consumed: Bool)
- `RestoreRealtimeTranscriptState`(item_count: u64, first_seen_count: u64, first_seen_unique_count: u64, every_item_has_order_entry: Bool, every_order_entry_has_item: Bool, all_identity_fields_valid: Bool, all_delta_ids_valid: Bool, all_completion_response_ids_valid: Bool, all_discarded_response_ids_valid: Bool, all_materialized_items_were_ready_or_skipped: Bool, all_assistant_items_have_response_unless_skipped: Bool, all_ready_assistant_items_have_completion_or_are_skipped: Bool, all_materialized_assistant_completions_consumed: Bool, all_completed_assistant_text_items_are_ready_or_materialized_or_skipped: Bool, all_discarded_assistant_items_are_skipped_or_materialized: Bool)
- `AuthorizeSessionMetadataPersist`(schema_version: u64, model_present: Bool, max_tokens: u64, structured_output_retries: u64, provider: SessionDurableProviderKind, self_hosted_server_present: Bool, provider_params_present: Bool, tooling_builtins: SessionToolCategoryOverrideKind, tooling_shell: SessionToolCategoryOverrideKind, tooling_comms: SessionToolCategoryOverrideKind, tooling_mob: SessionToolCategoryOverrideKind, tooling_memory: SessionToolCategoryOverrideKind, tooling_schedule: SessionToolCategoryOverrideKind, tooling_workgraph: SessionToolCategoryOverrideKind, tooling_image_generation: SessionToolCategoryOverrideKind, tooling_web_search: SessionToolCategoryOverrideKind, active_skill_count: u64, keep_alive: Bool, comms_name_present: Bool, peer_meta_present: Bool, realm_id_present: Bool, instance_id_present: Bool, backend_present: Bool, config_generation_present: Bool, auth_binding_present: Bool)
- `AuthorizeSessionBuildStatePersist`(system_prompt_present: Bool, output_schema_present: Bool, hook_entry_count: u64, disabled_hook_count: u64, budget_limits_present: Bool, recoverable_tool_count: u64, silent_comms_intent_count: u64, max_inline_peer_notifications_present: Bool, app_context_present: Bool, additional_instruction_count: u64, shell_env_count: u64, mob_tool_authority_context_present: Bool, mob_tool_authority_context_generated: Bool, call_timeout_override: SessionCallTimeoutOverrideKind)
- `RestoreSessionBuildState`(system_prompt_present: Bool, output_schema_present: Bool, hook_entry_count: u64, disabled_hook_count: u64, budget_limits_present: Bool, recoverable_tool_count: u64, silent_comms_intent_count: u64, max_inline_peer_notifications_present: Bool, app_context_present: Bool, additional_instruction_count: u64, shell_env_count: u64, mob_tool_authority_context_present: Bool, call_timeout_override: SessionCallTimeoutOverrideKind)
- `AuthorizeSystemPromptMutation`(source: SessionSystemPromptSource, prompt_present: Bool, prompt_byte_count: u64, replacing_existing: Bool)
- `ResolvePendingContinuation`(session_tail: ObservedSessionTailKind, staged_tool_result_count: u64)

## Signals

## Effects
- `SessionFirstTurnPhaseResolved`(phase: SessionFirstTurnPhase, was_pending: Bool)
- `SessionFirstTurnOverridesResolved`(allowed: Bool)
- `SessionInitialPromptStageResolved`(decision: SessionInitialPromptStageDecision)
- `SessionToolResultsStageResolved`(accepted_count: u64)
- `SessionConsumedInputsRestoreResolved`(restore_first_turn_pending: Bool, restore_initial_prompt: Bool, restore_tool_results: Bool)
- `SessionFirstTurnPhaseRecovered`
- `SystemContextAppendResolved`(decision: SystemContextAppendDecision, active_turn_scoped: Bool)
- `SystemContextPendingApplyItemResolved`(promote_to_applied: Bool, mark_seen_applied: Bool, remove_seen: Bool)
- `SystemContextSteerCleanupItemResolved`(discard: Bool)
- `SystemContextSnapshotRestoreAuthorized`
- `RealtimeTranscriptEventResolved`(observe_item: Bool, observe_skipped: Bool, write_user_segment: Bool, append_assistant_segment: Bool, replace_assistant_segment: Bool, promote_lane: Bool, mark_item_ready: Bool, record_delta_id: Bool, remove_completion: Bool, record_completion: Bool, discard_response: Bool, discard_response_by_lane: Bool, mark_response_ready: Bool, materialize_ready_items: Bool)
- `RealtimeMaterializeCandidateResolved`(decision: RealtimeTranscriptMaterializeDecision, consume_usage: Bool)
- `RealtimeTranscriptSnapshotRestoreAuthorized`
- `SessionMetadataPersistAuthorized`
- `SessionBuildStatePersistAuthorized`
- `SessionBuildStateRestoreAuthorized`
- `SystemPromptMutationAuthorized`
- `PendingContinuationResolved`(disposition: PendingContinuationDisposition)
- `PendingContinuationPublicTerminalResolved`(terminal: PendingContinuationPublicTerminal)

## Helpers
- `phase_allows_initial_turn_overrides`(phase: SessionFirstTurnPhase) -> `Bool`
- `should_store_initial_prompt`(phase: SessionFirstTurnPhase, prompt_has_content: Bool) -> `Bool`
- `append_is_empty`(trimmed_text_byte_count: u64) -> `Bool`
- `append_is_conflict`(idempotency_key_present: Bool, existing_key_conflicts: Bool) -> `Bool`
- `append_is_duplicate`(idempotency_key_present: Bool, existing_key_matches: Bool, existing_key_conflicts: Bool) -> `Bool`
- `append_is_new`(idempotency_key_present: Bool, existing_key_matches: Bool, existing_key_conflicts: Bool) -> `Bool`
- `realtime_delta_is_duplicate`(delta_id_present: Bool, delta_id_seen: Bool) -> `Bool`
- `realtime_lane_accepts`(item_has_text: Bool, current_lane: RealtimeTranscriptLaneKind, requested_lane: RealtimeTranscriptLaneKind) -> `Bool`
- `realtime_should_mark_ready_after_write`(response_completed: Bool, text_after_write_present: Bool) -> `Bool`
- `realtime_stop_reason_discards`(stop_reason: RealtimeTranscriptStopReasonKind) -> `Bool`
- `realtime_stop_reason_removes_completion`(stop_reason: RealtimeTranscriptStopReasonKind) -> `Bool`
- `realtime_stop_reason_records_completion`(stop_reason: RealtimeTranscriptStopReasonKind) -> `Bool`
- `tail_has_pending_boundary`(session_tail: ObservedSessionTailKind) -> `Bool`
- `has_effective_pending_boundary`(session_tail: ObservedSessionTailKind, staged_tool_result_count: u64) -> `Bool`

## Invariants

## Transitions
### `MarkSessionInitialTurnPendingInactiveOrPending`
- From: `Ready`
- On: `MarkSessionInitialTurnPending`(session_id)
- Guards:
  - ``
- Emits: `SessionFirstTurnPhaseResolved`
- To: `Ready`

### `MarkSessionInitialTurnPendingConsumed`
- From: `Ready`
- On: `MarkSessionInitialTurnPending`(session_id)
- Guards:
  - ``
- Emits: `SessionFirstTurnPhaseResolved`
- To: `Ready`

### `StartSessionInitialTurnPending`
- From: `Ready`
- On: `StartSessionInitialTurn`(session_id)
- Guards:
  - ``
- Emits: `SessionFirstTurnPhaseResolved`
- To: `Ready`

### `StartSessionInitialTurnInactive`
- From: `Ready`
- On: `StartSessionInitialTurn`(session_id)
- Guards:
  - ``
- Emits: `SessionFirstTurnPhaseResolved`
- To: `Ready`

### `StartSessionInitialTurnConsumed`
- From: `Ready`
- On: `StartSessionInitialTurn`(session_id)
- Guards:
  - ``
- Emits: `SessionFirstTurnPhaseResolved`
- To: `Ready`

### `ResolveSessionFirstTurnOverridesAllowed`
- From: `Ready`
- On: `ResolveSessionFirstTurnOverridesAllowed`(session_id)
- Guards:
  - ``
- Emits: `SessionFirstTurnOverridesResolved`
- To: `Ready`

### `ResolveSessionFirstTurnOverridesDenied`
- From: `Ready`
- On: `ResolveSessionFirstTurnOverridesAllowed`(session_id)
- Guards:
  - ``
- Emits: `SessionFirstTurnOverridesResolved`
- To: `Ready`

### `StageSessionInitialPromptStore`
- From: `Ready`
- On: `StageSessionInitialPrompt`(session_id, prompt_has_content)
- Guards:
  - ``
- Emits: `SessionInitialPromptStageResolved`
- To: `Ready`

### `StageSessionInitialPromptClear`
- From: `Ready`
- On: `StageSessionInitialPrompt`(session_id, prompt_has_content)
- Guards:
  - ``
- Emits: `SessionInitialPromptStageResolved`
- To: `Ready`

### `StageSessionToolResults`
- From: `Ready`
- On: `StageSessionToolResults`(session_id, result_count)
- Guards:
  - ``
- Emits: `SessionToolResultsStageResolved`
- To: `Ready`

### `ConsumeSessionDeferredInputsPending`
- From: `Ready`
- On: `ConsumeSessionDeferredInputs`(session_id)
- Guards:
  - ``
- Emits: `SessionFirstTurnPhaseResolved`
- To: `Ready`

### `ConsumeSessionDeferredInputsInactive`
- From: `Ready`
- On: `ConsumeSessionDeferredInputs`(session_id)
- Guards:
  - ``
- Emits: `SessionFirstTurnPhaseResolved`
- To: `Ready`

### `ConsumeSessionDeferredInputsConsumed`
- From: `Ready`
- On: `ConsumeSessionDeferredInputs`(session_id)
- Guards:
  - ``
- Emits: `SessionFirstTurnPhaseResolved`
- To: `Ready`

### `RestoreSessionConsumedInputs`
- From: `Ready`
- On: `RestoreSessionConsumedInputs`(session_id, restore_first_turn_pending, pending_initial_prompt_present, pending_tool_result_message_count)
- Guards:
  - ``
- Emits: `SessionConsumedInputsRestoreResolved`
- To: `Ready`

### `RestoreSessionConsumedInputsNoPhaseRollback`
- From: `Ready`
- On: `RestoreSessionConsumedInputs`(session_id, restore_first_turn_pending, pending_initial_prompt_present, pending_tool_result_message_count)
- Guards:
  - ``
- Emits: `SessionConsumedInputsRestoreResolved`
- To: `Ready`

### `RecoverSessionFirstTurnPhase`
- From: `Ready`
- On: `RecoverSessionFirstTurnPhase`(session_id, phase, pending_initial_prompt_present, pending_tool_result_message_count)
- Guards:
  - ``
- Emits: `SessionFirstTurnPhaseRecovered`
- To: `Ready`

### `ResolveSystemContextAppendEmpty`
- From: `Ready`
- On: `ResolveSystemContextAppend`(trimmed_text_byte_count, idempotency_key_present, existing_key_matches, existing_key_conflicts, active_turn_scoped)
- Guards:
  - ``
- Emits: `SystemContextAppendResolved`
- To: `Ready`

### `ResolveSystemContextAppendConflict`
- From: `Ready`
- On: `ResolveSystemContextAppend`(trimmed_text_byte_count, idempotency_key_present, existing_key_matches, existing_key_conflicts, active_turn_scoped)
- Guards:
  - ``
- Emits: `SystemContextAppendResolved`
- To: `Ready`

### `ResolveSystemContextAppendDuplicate`
- From: `Ready`
- On: `ResolveSystemContextAppend`(trimmed_text_byte_count, idempotency_key_present, existing_key_matches, existing_key_conflicts, active_turn_scoped)
- Guards:
  - ``
- Emits: `SystemContextAppendResolved`
- To: `Ready`

### `ResolveSystemContextAppendNew`
- From: `Ready`
- On: `ResolveSystemContextAppend`(trimmed_text_byte_count, idempotency_key_present, existing_key_matches, existing_key_conflicts, active_turn_scoped)
- Guards:
  - ``
- Emits: `SystemContextAppendResolved`
- To: `Ready`

### `ResolveSystemContextPendingApplyItemRuntimeSteer`
- From: `Ready`
- On: `ResolveSystemContextPendingApplyItem`(source_kind)
- Guards:
  - ``
- Emits: `SystemContextPendingApplyItemResolved`
- To: `Ready`

### `ResolveSystemContextPendingApplyItemNormal`
- From: `Ready`
- On: `ResolveSystemContextPendingApplyItem`(source_kind)
- Guards:
  - ``
- Emits: `SystemContextPendingApplyItemResolved`
- To: `Ready`

### `ResolveSystemContextSteerCleanupItemRuntimeSteer`
- From: `Ready`
- On: `ResolveSystemContextSteerCleanupItem`(source_kind)
- Guards:
  - ``
- Emits: `SystemContextSteerCleanupItemResolved`
- To: `Ready`

### `ResolveSystemContextSteerCleanupItemNormal`
- From: `Ready`
- On: `ResolveSystemContextSteerCleanupItem`(source_kind)
- Guards:
  - ``
- Emits: `SystemContextSteerCleanupItemResolved`
- To: `Ready`

### `RestoreSystemContextSnapshot`
- From: `Ready`
- On: `RestoreSystemContextSnapshot`(active_keys_have_known_pending_or_seen, seen_keys_match_known_appends)
- Guards:
  - ``
- Emits: `SystemContextSnapshotRestoreAuthorized`
- To: `Ready`

### `ResolveRealtimeItemObservedDiscardedAssistant`
- From: `Ready`
- On: `ResolveRealtimeItemObserved`(role, response_discarded)
- Guards:
  - ``
- Emits: `RealtimeTranscriptEventResolved`
- To: `Ready`

### `ResolveRealtimeItemObservedPresent`
- From: `Ready`
- On: `ResolveRealtimeItemObserved`(role, response_discarded)
- Guards:
  - ``
- Emits: `RealtimeTranscriptEventResolved`
- To: `Ready`

### `ResolveRealtimeItemSkipped`
- From: `Ready`
- On: `ResolveRealtimeItemSkipped`()
- Emits: `RealtimeTranscriptEventResolved`
- To: `Ready`

### `ResolveRealtimeUserTranscriptFinalEmpty`
- From: `Ready`
- On: `ResolveRealtimeUserTranscriptFinal`(text_present, segment_empty, segment_matches)
- Guards:
  - ``
- Emits: `RealtimeTranscriptEventResolved`
- To: `Ready`

### `ResolveRealtimeUserTranscriptFinalStore`
- From: `Ready`
- On: `ResolveRealtimeUserTranscriptFinal`(text_present, segment_empty, segment_matches)
- Guards:
  - ``
- Emits: `RealtimeTranscriptEventResolved`
- To: `Ready`

### `ResolveRealtimeUserTranscriptFinalReplayOrConflict`
- From: `Ready`
- On: `ResolveRealtimeUserTranscriptFinal`(text_present, segment_empty, segment_matches)
- Guards:
  - ``
- Emits: `RealtimeTranscriptEventResolved`
- To: `Ready`

### `ResolveRealtimeAssistantDeltaInvalidOrDuplicate`
- From: `Ready`
- On: `ResolveRealtimeAssistantDelta`(response_id_valid, response_discarded, delta_id_present, delta_id_seen, item_has_text, current_lane, requested_lane, response_completed, text_after_write_present)
- Guards:
  - ``
- Emits: `RealtimeTranscriptEventResolved`
- To: `Ready`

### `ResolveRealtimeAssistantDeltaDiscarded`
- From: `Ready`
- On: `ResolveRealtimeAssistantDelta`(response_id_valid, response_discarded, delta_id_present, delta_id_seen, item_has_text, current_lane, requested_lane, response_completed, text_after_write_present)
- Guards:
  - ``
- Emits: `RealtimeTranscriptEventResolved`
- To: `Ready`

### `ResolveRealtimeAssistantDeltaLaneConflict`
- From: `Ready`
- On: `ResolveRealtimeAssistantDelta`(response_id_valid, response_discarded, delta_id_present, delta_id_seen, item_has_text, current_lane, requested_lane, response_completed, text_after_write_present)
- Guards:
  - ``
- Emits: `RealtimeTranscriptEventResolved`
- To: `Ready`

### `ResolveRealtimeAssistantDeltaAccepted`
- From: `Ready`
- On: `ResolveRealtimeAssistantDelta`(response_id_valid, response_discarded, delta_id_present, delta_id_seen, item_has_text, current_lane, requested_lane, response_completed, text_after_write_present)
- Guards:
  - ``
- Emits: `RealtimeTranscriptEventResolved`
- To: `Ready`

### `ResolveRealtimeAssistantReplacementInvalid`
- From: `Ready`
- On: `ResolveRealtimeAssistantTextReplacement`(response_id_valid, response_discarded, item_materialized, item_has_text, current_lane, requested_lane, response_completed, text_after_replace_present)
- Guards:
  - ``
- Emits: `RealtimeTranscriptEventResolved`
- To: `Ready`

### `ResolveRealtimeAssistantReplacementDiscarded`
- From: `Ready`
- On: `ResolveRealtimeAssistantTextReplacement`(response_id_valid, response_discarded, item_materialized, item_has_text, current_lane, requested_lane, response_completed, text_after_replace_present)
- Guards:
  - ``
- Emits: `RealtimeTranscriptEventResolved`
- To: `Ready`

### `ResolveRealtimeAssistantReplacementLocked`
- From: `Ready`
- On: `ResolveRealtimeAssistantTextReplacement`(response_id_valid, response_discarded, item_materialized, item_has_text, current_lane, requested_lane, response_completed, text_after_replace_present)
- Guards:
  - ``
- Emits: `RealtimeTranscriptEventResolved`
- To: `Ready`

### `ResolveRealtimeAssistantReplacementLaneConflict`
- From: `Ready`
- On: `ResolveRealtimeAssistantTextReplacement`(response_id_valid, response_discarded, item_materialized, item_has_text, current_lane, requested_lane, response_completed, text_after_replace_present)
- Guards:
  - ``
- Emits: `RealtimeTranscriptEventResolved`
- To: `Ready`

### `ResolveRealtimeAssistantReplacementAccepted`
- From: `Ready`
- On: `ResolveRealtimeAssistantTextReplacement`(response_id_valid, response_discarded, item_materialized, item_has_text, current_lane, requested_lane, response_completed, text_after_replace_present)
- Guards:
  - ``
- Emits: `RealtimeTranscriptEventResolved`
- To: `Ready`

### `ResolveRealtimeAssistantTurnCompletedInvalid`
- From: `Ready`
- On: `ResolveRealtimeAssistantTurnCompleted`(response_id_valid, response_discarded, stop_reason)
- Guards:
  - ``
- Emits: `RealtimeTranscriptEventResolved`
- To: `Ready`

### `ResolveRealtimeAssistantTurnCompletedDiscard`
- From: `Ready`
- On: `ResolveRealtimeAssistantTurnCompleted`(response_id_valid, response_discarded, stop_reason)
- Guards:
  - ``
- Emits: `RealtimeTranscriptEventResolved`
- To: `Ready`

### `ResolveRealtimeAssistantTurnCompletedToolUse`
- From: `Ready`
- On: `ResolveRealtimeAssistantTurnCompleted`(response_id_valid, response_discarded, stop_reason)
- Guards:
  - ``
- Emits: `RealtimeTranscriptEventResolved`
- To: `Ready`

### `ResolveRealtimeAssistantTurnCompletedRecord`
- From: `Ready`
- On: `ResolveRealtimeAssistantTurnCompleted`(response_id_valid, response_discarded, stop_reason)
- Guards:
  - ``
- Emits: `RealtimeTranscriptEventResolved`
- To: `Ready`

### `ResolveRealtimeAssistantTurnInterruptedInvalid`
- From: `Ready`
- On: `ResolveRealtimeAssistantTurnInterrupted`(response_id_valid)
- Guards:
  - ``
- Emits: `RealtimeTranscriptEventResolved`
- To: `Ready`

### `ResolveRealtimeAssistantTurnInterruptedValid`
- From: `Ready`
- On: `ResolveRealtimeAssistantTurnInterrupted`(response_id_valid)
- Guards:
  - ``
- Emits: `RealtimeTranscriptEventResolved`
- To: `Ready`

### `ResolveRealtimeMaterializeAlreadyDone`
- From: `Ready`
- On: `ResolveRealtimeMaterializeCandidate`(item_materialized, predecessor_materialized, item_skipped, item_ready, item_text_present, role, response_id_present, completion_present, completion_usage_consumed)
- Guards:
  - ``
- Emits: `RealtimeMaterializeCandidateResolved`
- To: `Ready`

### `ResolveRealtimeMaterializeWaitForPredecessor`
- From: `Ready`
- On: `ResolveRealtimeMaterializeCandidate`(item_materialized, predecessor_materialized, item_skipped, item_ready, item_text_present, role, response_id_present, completion_present, completion_usage_consumed)
- Guards:
  - ``
- Emits: `RealtimeMaterializeCandidateResolved`
- To: `Ready`

### `ResolveRealtimeMaterializeSkipped`
- From: `Ready`
- On: `ResolveRealtimeMaterializeCandidate`(item_materialized, predecessor_materialized, item_skipped, item_ready, item_text_present, role, response_id_present, completion_present, completion_usage_consumed)
- Guards:
  - ``
- Emits: `RealtimeMaterializeCandidateResolved`
- To: `Ready`

### `ResolveRealtimeMaterializeWaitForReadyText`
- From: `Ready`
- On: `ResolveRealtimeMaterializeCandidate`(item_materialized, predecessor_materialized, item_skipped, item_ready, item_text_present, role, response_id_present, completion_present, completion_usage_consumed)
- Guards:
  - ``
- Emits: `RealtimeMaterializeCandidateResolved`
- To: `Ready`

### `ResolveRealtimeMaterializeUser`
- From: `Ready`
- On: `ResolveRealtimeMaterializeCandidate`(item_materialized, predecessor_materialized, item_skipped, item_ready, item_text_present, role, response_id_present, completion_present, completion_usage_consumed)
- Guards:
  - ``
- Emits: `RealtimeMaterializeCandidateResolved`
- To: `Ready`

### `ResolveRealtimeMaterializeAssistant`
- From: `Ready`
- On: `ResolveRealtimeMaterializeCandidate`(item_materialized, predecessor_materialized, item_skipped, item_ready, item_text_present, role, response_id_present, completion_present, completion_usage_consumed)
- Guards:
  - ``
- Emits: `RealtimeMaterializeCandidateResolved`
- To: `Ready`

### `ResolveRealtimeMaterializeAssistantMissingCompletion`
- From: `Ready`
- On: `ResolveRealtimeMaterializeCandidate`(item_materialized, predecessor_materialized, item_skipped, item_ready, item_text_present, role, response_id_present, completion_present, completion_usage_consumed)
- Guards:
  - ``
- Emits: `RealtimeMaterializeCandidateResolved`
- To: `Ready`

### `AuthorizeRestoreRealtimeTranscriptState`
- From: `Ready`
- On: `RestoreRealtimeTranscriptState`(item_count, first_seen_count, first_seen_unique_count, every_item_has_order_entry, every_order_entry_has_item, all_identity_fields_valid, all_delta_ids_valid, all_completion_response_ids_valid, all_discarded_response_ids_valid, all_materialized_items_were_ready_or_skipped, all_assistant_items_have_response_unless_skipped, all_ready_assistant_items_have_completion_or_are_skipped, all_materialized_assistant_completions_consumed, all_completed_assistant_text_items_are_ready_or_materialized_or_skipped, all_discarded_assistant_items_are_skipped_or_materialized)
- Guards:
  - ``
- Emits: `RealtimeTranscriptSnapshotRestoreAuthorized`
- To: `Ready`

### `AuthorizeSessionMetadataPersist`
- From: `Ready`
- On: `AuthorizeSessionMetadataPersist`(schema_version, model_present, max_tokens, structured_output_retries, provider, self_hosted_server_present, provider_params_present, tooling_builtins, tooling_shell, tooling_comms, tooling_mob, tooling_memory, tooling_schedule, tooling_workgraph, tooling_image_generation, tooling_web_search, active_skill_count, keep_alive, comms_name_present, peer_meta_present, realm_id_present, instance_id_present, backend_present, config_generation_present, auth_binding_present)
- Guards:
  - ``
- Emits: `SessionMetadataPersistAuthorized`
- To: `Ready`

### `AuthorizeSessionBuildStatePersist`
- From: `Ready`
- On: `AuthorizeSessionBuildStatePersist`(system_prompt_present, output_schema_present, hook_entry_count, disabled_hook_count, budget_limits_present, recoverable_tool_count, silent_comms_intent_count, max_inline_peer_notifications_present, app_context_present, additional_instruction_count, shell_env_count, mob_tool_authority_context_present, mob_tool_authority_context_generated, call_timeout_override)
- Guards:
  - ``
- Emits: `SessionBuildStatePersistAuthorized`
- To: `Ready`

### `RestoreSessionBuildState`
- From: `Ready`
- On: `RestoreSessionBuildState`(system_prompt_present, output_schema_present, hook_entry_count, disabled_hook_count, budget_limits_present, recoverable_tool_count, silent_comms_intent_count, max_inline_peer_notifications_present, app_context_present, additional_instruction_count, shell_env_count, mob_tool_authority_context_present, call_timeout_override)
- Emits: `SessionBuildStateRestoreAuthorized`
- To: `Ready`

### `AuthorizeSystemPromptMutation`
- From: `Ready`
- On: `AuthorizeSystemPromptMutation`(source, prompt_present, prompt_byte_count, replacing_existing)
- Guards:
  - ``
- Emits: `SystemPromptMutationAuthorized`
- To: `Ready`

### `ResolvePendingContinuationWithBoundary`
- From: `Ready`
- On: `ResolvePendingContinuation`(session_tail, staged_tool_result_count)
- Guards:
  - ``
- Emits: `PendingContinuationResolved`
- To: `Ready`

### `ResolvePendingContinuationWithoutBoundary`
- From: `Ready`
- On: `ResolvePendingContinuation`(session_tail, staged_tool_result_count)
- Guards:
  - ``
- Emits: `PendingContinuationResolved`, `PendingContinuationPublicTerminalResolved`
- To: `Ready`

## Coverage
### Code Anchors
- `meerkat-core/src/generated/session_document.rs` — generated SessionDocumentMachine owner for MarkSessionInitialTurnPendingInactiveOrPending, MarkSessionInitialTurnPendingConsumed, StartSessionInitialTurnPending, StartSessionInitialTurnInactive, StartSessionInitialTurnConsumed, ResolveSessionFirstTurnOverridesAllowed, ResolveSessionFirstTurnOverridesDenied, StageSessionInitialPromptStore, StageSessionInitialPromptClear, StageSessionToolResults, ConsumeSessionDeferredInputsPending, ConsumeSessionDeferredInputsInactive, ConsumeSessionDeferredInputsConsumed, RestoreSessionConsumedInputs, RestoreSessionConsumedInputsNoPhaseRollback, RecoverSessionFirstTurnPhase, ResolveSystemContextAppendEmpty, ResolveSystemContextAppendConflict, ResolveSystemContextAppendDuplicate, ResolveSystemContextAppendNew, ResolveSystemContextPendingApplyItemRuntimeSteer, ResolveSystemContextPendingApplyItemNormal, ResolveSystemContextSteerCleanupItemRuntimeSteer, ResolveSystemContextSteerCleanupItemNormal, RestoreSystemContextSnapshot, ResolveRealtimeItemObservedDiscardedAssistant, ResolveRealtimeItemObservedPresent, ResolveRealtimeItemSkipped, ResolveRealtimeUserTranscriptFinalEmpty, ResolveRealtimeUserTranscriptFinalStore, ResolveRealtimeUserTranscriptFinalReplayOrConflict, ResolveRealtimeAssistantDeltaInvalidOrDuplicate, ResolveRealtimeAssistantDeltaDiscarded, ResolveRealtimeAssistantDeltaLaneConflict, ResolveRealtimeAssistantDeltaAccepted, ResolveRealtimeAssistantReplacementInvalid, ResolveRealtimeAssistantReplacementDiscarded, ResolveRealtimeAssistantReplacementLocked, ResolveRealtimeAssistantReplacementLaneConflict, ResolveRealtimeAssistantReplacementAccepted, ResolveRealtimeAssistantTurnCompletedInvalid, ResolveRealtimeAssistantTurnCompletedDiscard, ResolveRealtimeAssistantTurnCompletedToolUse, ResolveRealtimeAssistantTurnCompletedRecord, ResolveRealtimeAssistantTurnInterruptedInvalid, ResolveRealtimeAssistantTurnInterruptedValid, ResolveRealtimeMaterializeAlreadyDone, ResolveRealtimeMaterializeWaitForPredecessor, ResolveRealtimeMaterializeSkipped, ResolveRealtimeMaterializeWaitForReadyText, ResolveRealtimeMaterializeUser, ResolveRealtimeMaterializeAssistant, ResolveRealtimeMaterializeAssistantMissingCompletion, AuthorizeRestoreRealtimeTranscriptState, SessionFirstTurnPhaseResolved, SessionFirstTurnOverridesResolved, SessionInitialPromptStageResolved, SessionToolResultsStageResolved, SessionConsumedInputsRestoreResolved, SessionFirstTurnPhaseRecovered, SystemContextAppendResolved, SystemContextPendingApplyItemResolved, SystemContextSteerCleanupItemResolved, SystemContextSnapshotRestoreAuthorized, RealtimeTranscriptEventResolved, RealtimeMaterializeCandidateResolved, RealtimeTranscriptSnapshotRestoreAuthorized, AuthorizeSessionMetadataPersist, AuthorizeSessionBuildStatePersist, RestoreSessionBuildState, AuthorizeSystemPromptMutation, SessionMetadataPersistAuthorized, SessionBuildStatePersistAuthorized, SessionBuildStateRestoreAuthorized, and SystemPromptMutationAuthorized

### Scenarios
- `session_first_turn_pending_consume` — MarkSessionInitialTurnPendingInactiveOrPending, MarkSessionInitialTurnPendingConsumed, StartSessionInitialTurnPending, StartSessionInitialTurnInactive, StartSessionInitialTurnConsumed, ConsumeSessionDeferredInputsPending, ConsumeSessionDeferredInputsInactive, and ConsumeSessionDeferredInputsConsumed own the per-session first-turn phase registry and emit SessionFirstTurnPhaseResolved without handwritten phase mutation
- `session_initial_inputs_stage` — StageSessionInitialPromptStore, StageSessionInitialPromptClear, StageSessionToolResults, ResolveSessionFirstTurnOverridesAllowed, and ResolveSessionFirstTurnOverridesDenied resolve initial-prompt and tool-results staging plus build-override legality from the machine-owned phase map under SessionInitialPromptStageResolved, SessionToolResultsStageResolved, and SessionFirstTurnOverridesResolved
- `session_first_turn_restore_recover` — RestoreSessionConsumedInputs, RestoreSessionConsumedInputsNoPhaseRollback, and RecoverSessionFirstTurnPhase rehydrate the per-session phase and presence/count registry from consumed-input rollback and durable snapshots under SessionConsumedInputsRestoreResolved and SessionFirstTurnPhaseRecovered
- `session_system_context_append_resolve` — ResolveSystemContextAppendEmpty, ResolveSystemContextAppendConflict, ResolveSystemContextAppendDuplicate, and ResolveSystemContextAppendNew decide the runtime system-context append disposition from typed key-present/matches/conflicts observations under SystemContextAppendResolved without the shell deciding
- `session_system_context_apply_discard` — ResolveSystemContextPendingApplyItemRuntimeSteer, ResolveSystemContextPendingApplyItemNormal, ResolveSystemContextSteerCleanupItemRuntimeSteer, and ResolveSystemContextSteerCleanupItemNormal decide per-append apply/discard from the typed SystemContextSource marker (not a runtime:steer string prefix) under SystemContextPendingApplyItemResolved and SystemContextSteerCleanupItemResolved
- `session_system_context_snapshot_restore` — RestoreSystemContextSnapshot authorizes a durable system-context snapshot only when active keys have known pending-or-seen entries and seen keys match known appends under SystemContextSnapshotRestoreAuthorized
- `session_realtime_transcript_event_resolve` — ResolveRealtimeItemObservedDiscardedAssistant, ResolveRealtimeItemObservedPresent, ResolveRealtimeItemSkipped, ResolveRealtimeUserTranscriptFinalEmpty, ResolveRealtimeUserTranscriptFinalStore, ResolveRealtimeUserTranscriptFinalReplayOrConflict, ResolveRealtimeAssistantDeltaInvalidOrDuplicate, ResolveRealtimeAssistantDeltaDiscarded, ResolveRealtimeAssistantDeltaLaneConflict, ResolveRealtimeAssistantDeltaAccepted, ResolveRealtimeAssistantReplacementInvalid, ResolveRealtimeAssistantReplacementDiscarded, ResolveRealtimeAssistantReplacementLocked, ResolveRealtimeAssistantReplacementLaneConflict, ResolveRealtimeAssistantReplacementAccepted, ResolveRealtimeAssistantTurnCompletedInvalid, ResolveRealtimeAssistantTurnCompletedDiscard, ResolveRealtimeAssistantTurnCompletedToolUse, ResolveRealtimeAssistantTurnInterruptedInvalid, and ResolveRealtimeAssistantTurnInterruptedValid resolve the realtime-transcript action vector from typed raw observations (set membership, segment concat emptiness, lane, completion) under RealtimeTranscriptEventResolved without the shell deciding; the shell mirrors the emitted action vector onto its bulky SessionRealtimeTranscriptState
- `session_realtime_transcript_materialize_and_restore` — ResolveRealtimeAssistantTurnCompletedRecord, ResolveRealtimeMaterializeAlreadyDone, ResolveRealtimeMaterializeWaitForPredecessor, ResolveRealtimeMaterializeSkipped, ResolveRealtimeMaterializeWaitForReadyText, ResolveRealtimeMaterializeUser, ResolveRealtimeMaterializeAssistant, ResolveRealtimeMaterializeAssistantMissingCompletion, and AuthorizeRestoreRealtimeTranscriptState resolve the per-item materialize verdict and durable snapshot-restore legality under RealtimeMaterializeCandidateResolved and RealtimeTranscriptSnapshotRestoreAuthorized; the shell performs only the topological ordering and message assembly
- `session_durable_config_authorize_restore` — AuthorizeSessionMetadataPersist, AuthorizeSessionBuildStatePersist, RestoreSessionBuildState, and AuthorizeSystemPromptMutation decide durable-config persist/restore/system-prompt admission from typed presence/count/kind observations the meerkat-core shell extracts from SessionMetadata and SessionBuildState under SessionMetadataPersistAuthorized, SessionBuildStatePersistAuthorized, SessionBuildStateRestoreAuthorized, and SystemPromptMutationAuthorized; a rejected request matches no transition and surfaces as Err, and the shell mirrors the verdict and passes the original typed value through unchanged
