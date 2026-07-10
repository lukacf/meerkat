# SessionDocumentMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `1`
- Rust owner: `self` / `catalog::dsl::session_document`

## State
- Phase enum: `Ready`
- `session_first_turn_phase`: `Map<SessionId, SessionFirstTurnPhase>`
- `session_pending_initial_prompt_present`: `Map<SessionId, Bool>`
- `session_pending_tool_results_count`: `Map<SessionId, u64>`
- `session_lifecycle_terminal`: `Map<SessionId, SessionDocumentLifecycle>`

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
- `ResolveSystemContextPersistAppendAdmission`(has_previous: Bool, content_identical: Bool, content_extends_previous: Bool, appended_starts_with_separator: Bool, incoming_is_runtime_context_append: Bool)
- `ResolveRealtimeItemObserved`(role: RealtimeTranscriptRoleKind, response_discarded: Bool)
- `ResolveRealtimeItemSkipped`
- `ResolveRealtimeUserTranscriptFinal`(text_present: Bool, segment_empty: Bool, segment_matches: Bool)
- `ResolveRealtimeUserContentFinal`(content_present: Bool, segment_empty: Bool, segment_matches: Bool)
- `ResolveRealtimeUserContentIdentity`(identity_fields_valid: Bool, key_tombstoned: Bool, predecessor_materialized: Bool, existing_identity_present: Bool, existing_payload_matches: Bool, target_item_id_available: Bool, reducer_commit_proof_required: Bool, reducer_commit_proof_present: Bool)
- `ResolveRealtimeUserContentBlobStage`(pending_present: Bool, pending_matches_request: Bool)
- `ResolveRealtimeUserContentBlobRecovery`(pending_present: Bool, request_matches_pending: Bool, pending_blob_valid: Bool)
- `ResolveRealtimeUserContentBlobFinalize`(pending_present: Bool, pending_matches_committed: Bool)
- `ResolveRealtimeAssistantDelta`(response_id_valid: Bool, response_discarded: Bool, delta_id_present: Bool, delta_id_seen: Bool, item_has_text: Bool, current_lane: RealtimeTranscriptLaneKind, requested_lane: RealtimeTranscriptLaneKind, response_completed: Bool, text_after_write_present: Bool)
- `ResolveRealtimeAssistantTextReplacement`(response_id_valid: Bool, response_discarded: Bool, item_materialized: Bool, item_has_text: Bool, current_lane: RealtimeTranscriptLaneKind, requested_lane: RealtimeTranscriptLaneKind, response_completed: Bool, text_after_replace_present: Bool)
- `ResolveRealtimeAssistantTurnCompleted`(response_id_valid: Bool, response_discarded: Bool, stop_reason: RealtimeTranscriptStopReasonKind)
- `ResolveRealtimeAssistantTurnInterrupted`(response_id_valid: Bool)
- `ResolveRealtimeMaterializeCandidate`(item_materialized: Bool, predecessor_materialized: Bool, item_skipped: Bool, item_ready: Bool, item_text_present: Bool, role: RealtimeTranscriptRoleKind, response_id_present: Bool, completion_present: Bool, completion_usage_consumed: Bool)
- `RestoreRealtimeTranscriptState`(item_count: u64, first_seen_count: u64, first_seen_unique_count: u64, every_item_has_order_entry: Bool, every_order_entry_has_item: Bool, all_materialized_predecessor_references_exist: Bool, no_self_predecessor_references: Bool, causal_graph_acyclic: Bool, all_materialized_items_have_materialized_ancestry: Bool, all_identity_fields_valid: Bool, all_user_content_identity_keys_match: Bool, all_user_content_identity_fields_valid: Bool, all_user_content_identity_item_ids_unique: Bool, all_user_content_identities_reference_materialized_user_items: Bool, all_user_content_tombstones_valid: Bool, user_content_identities_and_tombstones_disjoint: Bool, pending_user_content_blob_fields_valid: Bool, pending_user_content_blob_uncommitted: Bool, all_delta_ids_valid: Bool, all_completion_response_ids_valid: Bool, all_discarded_response_ids_valid: Bool, all_materialized_items_were_ready_or_skipped: Bool, all_assistant_items_have_response_unless_skipped: Bool, all_ready_assistant_items_have_completion_or_are_skipped: Bool, all_materialized_assistant_completions_consumed: Bool, all_completed_assistant_text_items_are_ready_or_materialized_or_skipped: Bool, all_discarded_assistant_items_are_skipped_or_materialized: Bool)
- `AuthorizeSessionMetadataPersist`(schema_version: u64, model_present: Bool)
- `AuthorizeSessionBuildStatePersist`(mob_tool_authority_context_present: Bool, mob_tool_authority_context_generated: Bool)
- `RestoreSessionBuildState`
- `AuthorizeSystemPromptMutation`(source: SessionSystemPromptSource, prompt_present: Bool, prompt_byte_count: u64, replacing_existing: Bool)
- `ResolvePendingContinuation`(session_tail: ObservedSessionTailKind, staged_tool_result_count: u64)
- `AuthorizeSessionResumeOverrides`(provider_override_present: Bool, model_override_present: Bool, has_build_only_overrides: Bool, first_turn_phase: SessionFirstTurnPhase)
- `ClassifyLiveSessionAuthority`(stored_transcript_diverged: Bool, live_has_uncommitted_transcript: Bool, runtime_system_context_diverged: Bool, stored_is_archived: Bool)
- `RecoverSessionFromStore`(session_id: SessionId, has_metadata: Bool, has_build_state: Bool, runtime_projection_quarantined: Bool)
- `ResolveRuntimeProjectionRollback`(session_id: SessionId, row_continues_authority: Bool, row_is_runtime_checkpoint: Bool)
- `ResolveRuntimeSnapshotReadSource`(session_id: SessionId, store_head_extends_snapshot: Bool, store_head_is_runtime_checkpoint: Bool, session_is_live: Bool)
- `ApplyPendingToolResults`(session_id: SessionId, result_count: u64)
- `TranscriptEdit`(session_id: SessionId, fork_or_rewrite_directive: TranscriptEditKind)
- `RecoverSessionLifecycleTerminal`(session_id: SessionId, terminal: SessionDocumentLifecycle)
- `ArchiveSessionDocument`(session_id: SessionId, runtime_backed: Bool, durable_document_present: Bool, runtime_observation: SessionArchiveRuntimeObservation)

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
- `SystemContextPersistAppendAdmissionResolved`(admission: SystemContextPersistAppendAdmission)
- `RealtimeTranscriptEventResolved`(observe_item: Bool, observe_skipped: Bool, write_user_segment: Bool, append_assistant_segment: Bool, replace_assistant_segment: Bool, promote_lane: Bool, mark_item_ready: Bool, record_delta_id: Bool, remove_completion: Bool, record_completion: Bool, discard_response: Bool, discard_response_by_lane: Bool, mark_response_ready: Bool, materialize_ready_items: Bool)
- `RealtimeMaterializeCandidateResolved`(decision: RealtimeTranscriptMaterializeDecision, consume_usage: Bool)
- `RealtimeUserContentIdentityResolved`(disposition: RealtimeUserContentIdentityDisposition)
- `RealtimeUserContentBlobStageResolved`(disposition: RealtimeUserContentBlobStageDisposition)
- `RealtimeUserContentBlobRecoveryResolved`(disposition: RealtimeUserContentBlobRecoveryDisposition)
- `RealtimeUserContentBlobFinalizeResolved`(disposition: RealtimeUserContentBlobFinalizeDisposition)
- `RealtimeTranscriptSnapshotRestoreAuthorized`
- `SessionMetadataPersistAuthorized`
- `SessionBuildStatePersistAuthorized`
- `SessionBuildStateRestoreAuthorized`
- `SystemPromptMutationAuthorized`
- `PendingContinuationResolved`(disposition: PendingContinuationDisposition)
- `PendingContinuationPublicTerminalResolved`(terminal: PendingContinuationPublicTerminal)
- `SessionResumeOverridesAuthorized`(provider_selection: ResumeProviderSelection, self_hosted_selection: ResumeSelfHostedSelection, provider_overridden: Bool)
- `SessionResumeOverridesRejected`(reason: ResumeOverrideRejection)
- `LiveSessionAuthorityClassified`(authority: LiveSessionAuthorityKind, reason: LiveSessionAuthorityReason)
- `SessionStoreRecoverySourceResolved`(recoverable: Bool)
- `RuntimeProjectionRollbackResolved`(disposition: RuntimeProjectionRollbackDisposition)
- `RuntimeSnapshotReadSourceResolved`(read_from_store_head: Bool)
- `SessionToolResultsApplied`(session_id: SessionId, applied_count: u64)
- `TranscriptRewriteCommitted`(kind: TranscriptEditKind, success: Bool)
- `SessionLifecycleTerminalRecovered`
- `SessionArchiveResolved`(disposition: SessionArchiveDisposition, write_document: Bool, retire_runtime: Bool)

## Helpers
- `phase_allows_initial_turn_overrides`(phase: SessionFirstTurnPhase) -> `Bool`
- `should_store_initial_prompt`(phase: SessionFirstTurnPhase, prompt_has_content: Bool) -> `Bool`
- `append_is_empty`(trimmed_text_byte_count: u64) -> `Bool`
- `append_is_conflict`(idempotency_key_present: Bool, existing_key_conflicts: Bool) -> `Bool`
- `append_is_duplicate`(idempotency_key_present: Bool, existing_key_matches: Bool, existing_key_conflicts: Bool) -> `Bool`
- `append_is_new`(idempotency_key_present: Bool, existing_key_matches: Bool, existing_key_conflicts: Bool) -> `Bool`
- `persist_append_is_admissible`(has_previous: Bool, content_identical: Bool, content_extends_previous: Bool, appended_starts_with_separator: Bool, incoming_is_runtime_context_append: Bool) -> `Bool`
- `realtime_delta_is_duplicate`(delta_id_present: Bool, delta_id_seen: Bool) -> `Bool`
- `realtime_lane_accepts`(item_has_text: Bool, current_lane: RealtimeTranscriptLaneKind, requested_lane: RealtimeTranscriptLaneKind) -> `Bool`
- `realtime_should_mark_ready_after_write`(response_completed: Bool, text_after_write_present: Bool) -> `Bool`
- `realtime_stop_reason_discards`(stop_reason: RealtimeTranscriptStopReasonKind) -> `Bool`
- `realtime_stop_reason_removes_completion`(stop_reason: RealtimeTranscriptStopReasonKind) -> `Bool`
- `realtime_stop_reason_records_completion`(stop_reason: RealtimeTranscriptStopReasonKind) -> `Bool`
- `tail_has_pending_boundary`(session_tail: ObservedSessionTailKind) -> `Bool`
- `has_effective_pending_boundary`(session_tail: ObservedSessionTailKind, staged_tool_result_count: u64) -> `Bool`
- `resume_reject_provider_requires_model`(provider_override_present: Bool, model_override_present: Bool) -> `Bool`
- `resume_reject_build_only_after_first_turn`(has_build_only_overrides: Bool, first_turn_phase: SessionFirstTurnPhase) -> `Bool`
- `resume_overrides_admissible`(provider_override_present: Bool, model_override_present: Bool, has_build_only_overrides: Bool, first_turn_phase: SessionFirstTurnPhase) -> `Bool`
- `resume_provider_recompute_from_model`(model_override_present: Bool, provider_override_present: Bool) -> `Bool`
- `store_projection_can_recover_authority`(has_metadata: Bool, has_build_state: Bool, runtime_projection_quarantined: Bool) -> `Bool`
- `archive_should_retire_runtime`(runtime_backed: Bool, runtime_observation: SessionArchiveRuntimeObservation) -> `Bool`

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

### `ResolveSystemContextPersistAppendAdmissionAdmit`
- From: `Ready`
- On: `ResolveSystemContextPersistAppendAdmission`(has_previous, content_identical, content_extends_previous, appended_starts_with_separator, incoming_is_runtime_context_append)
- Guards:
  - ``
- Emits: `SystemContextPersistAppendAdmissionResolved`
- To: `Ready`

### `ResolveSystemContextPersistAppendAdmissionReject`
- From: `Ready`
- On: `ResolveSystemContextPersistAppendAdmission`(has_previous, content_identical, content_extends_previous, appended_starts_with_separator, incoming_is_runtime_context_append)
- Guards:
  - ``
- Emits: `SystemContextPersistAppendAdmissionResolved`
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

### `ResolveRealtimeUserContentIdentityInvalid`
- From: `Ready`
- On: `ResolveRealtimeUserContentIdentity`(identity_fields_valid, key_tombstoned, predecessor_materialized, existing_identity_present, existing_payload_matches, target_item_id_available, reducer_commit_proof_required, reducer_commit_proof_present)
- Guards:
  - ``
- Emits: `RealtimeUserContentIdentityResolved`
- To: `Ready`

### `ResolveRealtimeUserContentIdentityUnmaterializedPredecessor`
- From: `Ready`
- On: `ResolveRealtimeUserContentIdentity`(identity_fields_valid, key_tombstoned, predecessor_materialized, existing_identity_present, existing_payload_matches, target_item_id_available, reducer_commit_proof_required, reducer_commit_proof_present)
- Guards:
  - ``
- Emits: `RealtimeUserContentIdentityResolved`
- To: `Ready`

### `ResolveRealtimeUserContentIdentityConflict`
- From: `Ready`
- On: `ResolveRealtimeUserContentIdentity`(identity_fields_valid, key_tombstoned, predecessor_materialized, existing_identity_present, existing_payload_matches, target_item_id_available, reducer_commit_proof_required, reducer_commit_proof_present)
- Guards:
  - ``
- Emits: `RealtimeUserContentIdentityResolved`
- To: `Ready`

### `ResolveRealtimeUserContentIdentityReplay`
- From: `Ready`
- On: `ResolveRealtimeUserContentIdentity`(identity_fields_valid, key_tombstoned, predecessor_materialized, existing_identity_present, existing_payload_matches, target_item_id_available, reducer_commit_proof_required, reducer_commit_proof_present)
- Guards:
  - ``
- Emits: `RealtimeUserContentIdentityResolved`
- To: `Ready`

### `ResolveRealtimeUserContentIdentityCommitNew`
- From: `Ready`
- On: `ResolveRealtimeUserContentIdentity`(identity_fields_valid, key_tombstoned, predecessor_materialized, existing_identity_present, existing_payload_matches, target_item_id_available, reducer_commit_proof_required, reducer_commit_proof_present)
- Guards:
  - ``
- Emits: `RealtimeUserContentIdentityResolved`
- To: `Ready`

### `ResolveRealtimeUserContentBlobStageNew`
- From: `Ready`
- On: `ResolveRealtimeUserContentBlobStage`(pending_present, pending_matches_request)
- Guards:
  - ``
- Emits: `RealtimeUserContentBlobStageResolved`
- To: `Ready`

### `ResolveRealtimeUserContentBlobStageReuseExact`
- From: `Ready`
- On: `ResolveRealtimeUserContentBlobStage`(pending_present, pending_matches_request)
- Guards:
  - ``
- Emits: `RealtimeUserContentBlobStageResolved`
- To: `Ready`

### `ResolveRealtimeUserContentBlobStageRejectOccupied`
- From: `Ready`
- On: `ResolveRealtimeUserContentBlobStage`(pending_present, pending_matches_request)
- Guards:
  - ``
- Emits: `RealtimeUserContentBlobStageResolved`
- To: `Ready`

### `ResolveRealtimeUserContentBlobRecoveryNone`
- From: `Ready`
- On: `ResolveRealtimeUserContentBlobRecovery`(pending_present, request_matches_pending, pending_blob_valid)
- Guards:
  - ``
- Emits: `RealtimeUserContentBlobRecoveryResolved`
- To: `Ready`

### `ResolveRealtimeUserContentBlobRecoveryExact`
- From: `Ready`
- On: `ResolveRealtimeUserContentBlobRecovery`(pending_present, request_matches_pending, pending_blob_valid)
- Guards:
  - ``
- Emits: `RealtimeUserContentBlobRecoveryResolved`
- To: `Ready`

### `ResolveRealtimeUserContentBlobRecoveryCommitVerified`
- From: `Ready`
- On: `ResolveRealtimeUserContentBlobRecovery`(pending_present, request_matches_pending, pending_blob_valid)
- Guards:
  - ``
- Emits: `RealtimeUserContentBlobRecoveryResolved`
- To: `Ready`

### `ResolveRealtimeUserContentBlobRecoveryClearInvalid`
- From: `Ready`
- On: `ResolveRealtimeUserContentBlobRecovery`(pending_present, request_matches_pending, pending_blob_valid)
- Guards:
  - ``
- Emits: `RealtimeUserContentBlobRecoveryResolved`
- To: `Ready`

### `ResolveRealtimeUserContentBlobFinalizeNone`
- From: `Ready`
- On: `ResolveRealtimeUserContentBlobFinalize`(pending_present, pending_matches_committed)
- Guards:
  - ``
- Emits: `RealtimeUserContentBlobFinalizeResolved`
- To: `Ready`

### `ResolveRealtimeUserContentBlobFinalizeClearCommitted`
- From: `Ready`
- On: `ResolveRealtimeUserContentBlobFinalize`(pending_present, pending_matches_committed)
- Guards:
  - ``
- Emits: `RealtimeUserContentBlobFinalizeResolved`
- To: `Ready`

### `ResolveRealtimeUserContentBlobFinalizeRejectMismatch`
- From: `Ready`
- On: `ResolveRealtimeUserContentBlobFinalize`(pending_present, pending_matches_committed)
- Guards:
  - ``
- Emits: `RealtimeUserContentBlobFinalizeResolved`
- To: `Ready`

### `ResolveRealtimeUserContentFinalEmpty`
- From: `Ready`
- On: `ResolveRealtimeUserContentFinal`(content_present, segment_empty, segment_matches)
- Guards:
  - ``
- Emits: `RealtimeTranscriptEventResolved`
- To: `Ready`

### `ResolveRealtimeUserContentFinalStore`
- From: `Ready`
- On: `ResolveRealtimeUserContentFinal`(content_present, segment_empty, segment_matches)
- Guards:
  - ``
- Emits: `RealtimeTranscriptEventResolved`
- To: `Ready`

### `ResolveRealtimeUserContentFinalReplayOrConflict`
- From: `Ready`
- On: `ResolveRealtimeUserContentFinal`(content_present, segment_empty, segment_matches)
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
- On: `RestoreRealtimeTranscriptState`(item_count, first_seen_count, first_seen_unique_count, every_item_has_order_entry, every_order_entry_has_item, all_materialized_predecessor_references_exist, no_self_predecessor_references, causal_graph_acyclic, all_materialized_items_have_materialized_ancestry, all_identity_fields_valid, all_user_content_identity_keys_match, all_user_content_identity_fields_valid, all_user_content_identity_item_ids_unique, all_user_content_identities_reference_materialized_user_items, all_user_content_tombstones_valid, user_content_identities_and_tombstones_disjoint, pending_user_content_blob_fields_valid, pending_user_content_blob_uncommitted, all_delta_ids_valid, all_completion_response_ids_valid, all_discarded_response_ids_valid, all_materialized_items_were_ready_or_skipped, all_assistant_items_have_response_unless_skipped, all_ready_assistant_items_have_completion_or_are_skipped, all_materialized_assistant_completions_consumed, all_completed_assistant_text_items_are_ready_or_materialized_or_skipped, all_discarded_assistant_items_are_skipped_or_materialized)
- Guards:
  - ``
- Emits: `RealtimeTranscriptSnapshotRestoreAuthorized`
- To: `Ready`

### `AuthorizeSessionMetadataPersist`
- From: `Ready`
- On: `AuthorizeSessionMetadataPersist`(schema_version, model_present)
- Guards:
  - ``
- Emits: `SessionMetadataPersistAuthorized`
- To: `Ready`

### `AuthorizeSessionBuildStatePersist`
- From: `Ready`
- On: `AuthorizeSessionBuildStatePersist`(mob_tool_authority_context_present, mob_tool_authority_context_generated)
- Guards:
  - ``
- Emits: `SessionBuildStatePersistAuthorized`
- To: `Ready`

### `RestoreSessionBuildState`
- From: `Ready`
- On: `RestoreSessionBuildState`()
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

### `AuthorizeSessionResumeOverridesRejectProviderRequiresModel`
- From: `Ready`
- On: `AuthorizeSessionResumeOverrides`(provider_override_present, model_override_present, has_build_only_overrides, first_turn_phase)
- Guards:
  - ``
- Emits: `SessionResumeOverridesRejected`
- To: `Ready`

### `AuthorizeSessionResumeOverridesRejectBuildOnlyAfterFirstTurn`
- From: `Ready`
- On: `AuthorizeSessionResumeOverrides`(provider_override_present, model_override_present, has_build_only_overrides, first_turn_phase)
- Guards:
  - ``
- Emits: `SessionResumeOverridesRejected`
- To: `Ready`

### `AuthorizeSessionResumeOverridesAcceptRecomputeProvider`
- From: `Ready`
- On: `AuthorizeSessionResumeOverrides`(provider_override_present, model_override_present, has_build_only_overrides, first_turn_phase)
- Guards:
  - ``
- Emits: `SessionResumeOverridesAuthorized`
- To: `Ready`

### `AuthorizeSessionResumeOverridesAcceptUseOverride`
- From: `Ready`
- On: `AuthorizeSessionResumeOverrides`(provider_override_present, model_override_present, has_build_only_overrides, first_turn_phase)
- Guards:
  - ``
- Emits: `SessionResumeOverridesAuthorized`
- To: `Ready`

### `AuthorizeSessionResumeOverridesAcceptRetainStored`
- From: `Ready`
- On: `AuthorizeSessionResumeOverrides`(provider_override_present, model_override_present, has_build_only_overrides, first_turn_phase)
- Guards:
  - ``
- Emits: `SessionResumeOverridesAuthorized`
- To: `Ready`

### `ClassifyLiveSessionAuthorityLive`
- From: `Ready`
- On: `ClassifyLiveSessionAuthority`(stored_transcript_diverged, live_has_uncommitted_transcript, runtime_system_context_diverged, stored_is_archived)
- Guards:
  - ``
- Emits: `LiveSessionAuthorityClassified`
- To: `Ready`

### `ClassifyLiveSessionAuthorityDurableArchived`
- From: `Ready`
- On: `ClassifyLiveSessionAuthority`(stored_transcript_diverged, live_has_uncommitted_transcript, runtime_system_context_diverged, stored_is_archived)
- Guards:
  - ``
- Emits: `LiveSessionAuthorityClassified`
- To: `Ready`

### `ClassifyLiveSessionAuthorityDurableUncommitted`
- From: `Ready`
- On: `ClassifyLiveSessionAuthority`(stored_transcript_diverged, live_has_uncommitted_transcript, runtime_system_context_diverged, stored_is_archived)
- Guards:
  - ``
- Emits: `LiveSessionAuthorityClassified`
- To: `Ready`

### `ClassifyLiveSessionAuthorityDurableSystemContext`
- From: `Ready`
- On: `ClassifyLiveSessionAuthority`(stored_transcript_diverged, live_has_uncommitted_transcript, runtime_system_context_diverged, stored_is_archived)
- Guards:
  - ``
- Emits: `LiveSessionAuthorityClassified`
- To: `Ready`

### `ClassifyLiveSessionAuthorityDurableRevision`
- From: `Ready`
- On: `ClassifyLiveSessionAuthority`(stored_transcript_diverged, live_has_uncommitted_transcript, runtime_system_context_diverged, stored_is_archived)
- Guards:
  - ``
- Emits: `LiveSessionAuthorityClassified`
- To: `Ready`

### `RecoverSessionFromStoreAuthorized`
- From: `Ready`
- On: `RecoverSessionFromStore`(session_id, has_metadata, has_build_state, runtime_projection_quarantined)
- Guards:
  - ``
- Emits: `SessionStoreRecoverySourceResolved`
- To: `Ready`

### `RecoverSessionFromStoreUnrecoverable`
- From: `Ready`
- On: `RecoverSessionFromStore`(session_id, has_metadata, has_build_state, runtime_projection_quarantined)
- Guards:
  - ``
- Emits: `SessionStoreRecoverySourceResolved`
- To: `Ready`

### `ResolveRuntimeSnapshotReadSourceStoreHead`
- From: `Ready`
- On: `ResolveRuntimeSnapshotReadSource`(session_id, store_head_extends_snapshot, store_head_is_runtime_checkpoint, session_is_live)
- Guards:
  - ``
- Emits: `RuntimeSnapshotReadSourceResolved`
- To: `Ready`

### `ResolveRuntimeSnapshotReadSourceSnapshot`
- From: `Ready`
- On: `ResolveRuntimeSnapshotReadSource`(session_id, store_head_extends_snapshot, store_head_is_runtime_checkpoint, session_is_live)
- Guards:
  - ``
- Emits: `RuntimeSnapshotReadSourceResolved`
- To: `Ready`

### `ResolveRuntimeProjectionRollbackRebuild`
- From: `Ready`
- On: `ResolveRuntimeProjectionRollback`(session_id, row_continues_authority, row_is_runtime_checkpoint)
- Guards:
  - ``
- Emits: `RuntimeProjectionRollbackResolved`
- To: `Ready`

### `ResolveRuntimeProjectionRollbackReject`
- From: `Ready`
- On: `ResolveRuntimeProjectionRollback`(session_id, row_continues_authority, row_is_runtime_checkpoint)
- Guards:
  - ``
- Emits: `RuntimeProjectionRollbackResolved`
- To: `Ready`

### `ApplyPendingToolResults`
- From: `Ready`
- On: `ApplyPendingToolResults`(session_id, result_count)
- Emits: `SessionToolResultsApplied`
- To: `Ready`

### `TranscriptEditFork`
- From: `Ready`
- On: `TranscriptEdit`(session_id, fork_or_rewrite_directive)
- Guards:
  - ``
- Emits: `TranscriptRewriteCommitted`
- To: `Ready`

### `TranscriptEditRewrite`
- From: `Ready`
- On: `TranscriptEdit`(session_id, fork_or_rewrite_directive)
- Guards:
  - ``
- Emits: `TranscriptRewriteCommitted`
- To: `Ready`

### `RecoverSessionLifecycleTerminal`
- From: `Ready`
- On: `RecoverSessionLifecycleTerminal`(session_id, terminal)
- Guards:
  - ``
- Emits: `SessionLifecycleTerminalRecovered`
- To: `Ready`

### `ArchiveSessionDocumentActive`
- From: `Ready`
- On: `ArchiveSessionDocument`(session_id, runtime_backed, durable_document_present, runtime_observation)
- Guards:
  - ``
- Emits: `SessionArchiveResolved`
- To: `Ready`

### `ArchiveSessionDocumentAlreadyArchived`
- From: `Ready`
- On: `ArchiveSessionDocument`(session_id, runtime_backed, durable_document_present, runtime_observation)
- Guards:
  - ``
  - `runtime_quiescent`
- Emits: `SessionArchiveResolved`
- To: `Ready`

### `ArchiveSessionDocumentCompleteRetire`
- From: `Ready`
- On: `ArchiveSessionDocument`(session_id, runtime_backed, durable_document_present, runtime_observation)
- Guards:
  - ``
  - `runtime_residue`
- Emits: `SessionArchiveResolved`
- To: `Ready`

## Coverage
### Code Anchors
- `session_document_authority` (machine `SessionDocumentMachine`): `meerkat-core/src/generated/session_document.rs` — generated SessionDocumentMachine owner for MarkSessionInitialTurnPendingInactiveOrPending, MarkSessionInitialTurnPendingConsumed, StartSessionInitialTurnPending, StartSessionInitialTurnInactive, StartSessionInitialTurnConsumed, ResolveSessionFirstTurnOverridesAllowed, ResolveSessionFirstTurnOverridesDenied, StageSessionInitialPromptStore, StageSessionInitialPromptClear, StageSessionToolResults, ConsumeSessionDeferredInputsPending, ConsumeSessionDeferredInputsInactive, ConsumeSessionDeferredInputsConsumed, RestoreSessionConsumedInputs, RestoreSessionConsumedInputsNoPhaseRollback, RecoverSessionFirstTurnPhase, ResolveSystemContextAppendEmpty, ResolveSystemContextAppendConflict, ResolveSystemContextAppendDuplicate, ResolveSystemContextAppendNew, ResolveSystemContextPendingApplyItemRuntimeSteer, ResolveSystemContextPendingApplyItemNormal, ResolveSystemContextSteerCleanupItemRuntimeSteer, ResolveSystemContextSteerCleanupItemNormal, RestoreSystemContextSnapshot, ResolveRealtimeItemObservedDiscardedAssistant, ResolveRealtimeItemObservedPresent, ResolveRealtimeItemSkipped, ResolveRealtimeUserTranscriptFinalEmpty, ResolveRealtimeUserTranscriptFinalStore, ResolveRealtimeUserTranscriptFinalReplayOrConflict, ResolveRealtimeAssistantDeltaInvalidOrDuplicate, ResolveRealtimeAssistantDeltaDiscarded, ResolveRealtimeAssistantDeltaLaneConflict, ResolveRealtimeAssistantDeltaAccepted, ResolveRealtimeAssistantReplacementInvalid, ResolveRealtimeAssistantReplacementDiscarded, ResolveRealtimeAssistantReplacementLocked, ResolveRealtimeAssistantReplacementLaneConflict, ResolveRealtimeAssistantReplacementAccepted, ResolveRealtimeAssistantTurnCompletedInvalid, ResolveRealtimeAssistantTurnCompletedDiscard, ResolveRealtimeAssistantTurnCompletedToolUse, ResolveRealtimeAssistantTurnCompletedRecord, ResolveRealtimeAssistantTurnInterruptedInvalid, ResolveRealtimeAssistantTurnInterruptedValid, ResolveRealtimeMaterializeAlreadyDone, ResolveRealtimeMaterializeWaitForPredecessor, ResolveRealtimeMaterializeSkipped, ResolveRealtimeMaterializeWaitForReadyText, ResolveRealtimeMaterializeUser, ResolveRealtimeMaterializeAssistant, ResolveRealtimeMaterializeAssistantMissingCompletion, AuthorizeRestoreRealtimeTranscriptState, SessionFirstTurnPhaseResolved, SessionFirstTurnOverridesResolved, SessionInitialPromptStageResolved, SessionToolResultsStageResolved, SessionConsumedInputsRestoreResolved, SessionFirstTurnPhaseRecovered, SystemContextAppendResolved, SystemContextPendingApplyItemResolved, SystemContextSteerCleanupItemResolved, SystemContextSnapshotRestoreAuthorized, RealtimeTranscriptEventResolved, RealtimeMaterializeCandidateResolved, RealtimeTranscriptSnapshotRestoreAuthorized, AuthorizeSessionMetadataPersist, AuthorizeSessionBuildStatePersist, RestoreSessionBuildState, AuthorizeSystemPromptMutation, SessionMetadataPersistAuthorized, SessionBuildStatePersistAuthorized, SessionBuildStateRestoreAuthorized, and SystemPromptMutationAuthorized

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
