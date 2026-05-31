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

## Helpers
- `phase_allows_initial_turn_overrides`(phase: SessionFirstTurnPhase) -> `Bool`
- `should_store_initial_prompt`(phase: SessionFirstTurnPhase, prompt_has_content: Bool) -> `Bool`
- `append_is_empty`(trimmed_text_byte_count: u64) -> `Bool`
- `append_is_conflict`(idempotency_key_present: Bool, existing_key_conflicts: Bool) -> `Bool`
- `append_is_duplicate`(idempotency_key_present: Bool, existing_key_matches: Bool, existing_key_conflicts: Bool) -> `Bool`
- `append_is_new`(idempotency_key_present: Bool, existing_key_matches: Bool, existing_key_conflicts: Bool) -> `Bool`

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

## Coverage
### Code Anchors
- `meerkat-core/src/generated/session_document.rs` — generated SessionDocumentMachine owner for MarkSessionInitialTurnPendingInactiveOrPending, MarkSessionInitialTurnPendingConsumed, StartSessionInitialTurnPending, StartSessionInitialTurnInactive, StartSessionInitialTurnConsumed, ResolveSessionFirstTurnOverridesAllowed, ResolveSessionFirstTurnOverridesDenied, StageSessionInitialPromptStore, StageSessionInitialPromptClear, StageSessionToolResults, ConsumeSessionDeferredInputsPending, ConsumeSessionDeferredInputsInactive, ConsumeSessionDeferredInputsConsumed, RestoreSessionConsumedInputs, RestoreSessionConsumedInputsNoPhaseRollback, RecoverSessionFirstTurnPhase, ResolveSystemContextAppendEmpty, ResolveSystemContextAppendConflict, ResolveSystemContextAppendDuplicate, ResolveSystemContextAppendNew, ResolveSystemContextPendingApplyItemRuntimeSteer, ResolveSystemContextPendingApplyItemNormal, ResolveSystemContextSteerCleanupItemRuntimeSteer, ResolveSystemContextSteerCleanupItemNormal, RestoreSystemContextSnapshot, SessionFirstTurnPhaseResolved, SessionFirstTurnOverridesResolved, SessionInitialPromptStageResolved, SessionToolResultsStageResolved, SessionConsumedInputsRestoreResolved, SessionFirstTurnPhaseRecovered, SystemContextAppendResolved, SystemContextPendingApplyItemResolved, SystemContextSteerCleanupItemResolved, and SystemContextSnapshotRestoreAuthorized

### Scenarios
- `session_first_turn_pending_consume` — MarkSessionInitialTurnPendingInactiveOrPending, MarkSessionInitialTurnPendingConsumed, StartSessionInitialTurnPending, StartSessionInitialTurnInactive, StartSessionInitialTurnConsumed, ConsumeSessionDeferredInputsPending, ConsumeSessionDeferredInputsInactive, and ConsumeSessionDeferredInputsConsumed own the per-session first-turn phase registry and emit SessionFirstTurnPhaseResolved without handwritten phase mutation
- `session_initial_inputs_stage` — StageSessionInitialPromptStore, StageSessionInitialPromptClear, StageSessionToolResults, ResolveSessionFirstTurnOverridesAllowed, and ResolveSessionFirstTurnOverridesDenied resolve initial-prompt and tool-results staging plus build-override legality from the machine-owned phase map under SessionInitialPromptStageResolved, SessionToolResultsStageResolved, and SessionFirstTurnOverridesResolved
- `session_first_turn_restore_recover` — RestoreSessionConsumedInputs, RestoreSessionConsumedInputsNoPhaseRollback, and RecoverSessionFirstTurnPhase rehydrate the per-session phase and presence/count registry from consumed-input rollback and durable snapshots under SessionConsumedInputsRestoreResolved and SessionFirstTurnPhaseRecovered
- `session_system_context_append_resolve` — ResolveSystemContextAppendEmpty, ResolveSystemContextAppendConflict, ResolveSystemContextAppendDuplicate, and ResolveSystemContextAppendNew decide the runtime system-context append disposition from typed key-present/matches/conflicts observations under SystemContextAppendResolved without the shell deciding
- `session_system_context_apply_discard` — ResolveSystemContextPendingApplyItemRuntimeSteer, ResolveSystemContextPendingApplyItemNormal, ResolveSystemContextSteerCleanupItemRuntimeSteer, and ResolveSystemContextSteerCleanupItemNormal decide per-append apply/discard from the typed SystemContextSource marker (not a runtime:steer string prefix) under SystemContextPendingApplyItemResolved and SystemContextSteerCleanupItemResolved
- `session_system_context_snapshot_restore` — RestoreSystemContextSnapshot authorizes a durable system-context snapshot only when active keys have known pending-or-seen entries and seen keys match known appends under SystemContextSnapshotRestoreAuthorized
