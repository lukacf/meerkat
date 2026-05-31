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

## Signals

## Effects
- `SessionFirstTurnPhaseResolved`(phase: SessionFirstTurnPhase, was_pending: Bool)
- `SessionFirstTurnOverridesResolved`(allowed: Bool)
- `SessionInitialPromptStageResolved`(decision: SessionInitialPromptStageDecision)
- `SessionToolResultsStageResolved`(accepted_count: u64)
- `SessionConsumedInputsRestoreResolved`(restore_first_turn_pending: Bool, restore_initial_prompt: Bool, restore_tool_results: Bool)
- `SessionFirstTurnPhaseRecovered`

## Helpers
- `phase_allows_initial_turn_overrides`(phase: SessionFirstTurnPhase) -> `Bool`
- `should_store_initial_prompt`(phase: SessionFirstTurnPhase, prompt_has_content: Bool) -> `Bool`

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

## Coverage
### Code Anchors
- `meerkat-core/src/generated/session_document.rs` — generated SessionDocumentMachine owner for MarkSessionInitialTurnPendingInactiveOrPending, MarkSessionInitialTurnPendingConsumed, StartSessionInitialTurnPending, StartSessionInitialTurnInactive, StartSessionInitialTurnConsumed, ResolveSessionFirstTurnOverridesAllowed, ResolveSessionFirstTurnOverridesDenied, StageSessionInitialPromptStore, StageSessionInitialPromptClear, StageSessionToolResults, ConsumeSessionDeferredInputsPending, ConsumeSessionDeferredInputsInactive, ConsumeSessionDeferredInputsConsumed, RestoreSessionConsumedInputs, RestoreSessionConsumedInputsNoPhaseRollback, RecoverSessionFirstTurnPhase, SessionFirstTurnPhaseResolved, SessionFirstTurnOverridesResolved, SessionInitialPromptStageResolved, SessionToolResultsStageResolved, SessionConsumedInputsRestoreResolved, and SessionFirstTurnPhaseRecovered

### Scenarios
- `session_first_turn_pending_consume` — MarkSessionInitialTurnPendingInactiveOrPending, MarkSessionInitialTurnPendingConsumed, StartSessionInitialTurnPending, StartSessionInitialTurnInactive, StartSessionInitialTurnConsumed, ConsumeSessionDeferredInputsPending, ConsumeSessionDeferredInputsInactive, and ConsumeSessionDeferredInputsConsumed own the per-session first-turn phase registry and emit SessionFirstTurnPhaseResolved without handwritten phase mutation
- `session_initial_inputs_stage` — StageSessionInitialPromptStore, StageSessionInitialPromptClear, StageSessionToolResults, ResolveSessionFirstTurnOverridesAllowed, and ResolveSessionFirstTurnOverridesDenied resolve initial-prompt and tool-results staging plus build-override legality from the machine-owned phase map under SessionInitialPromptStageResolved, SessionToolResultsStageResolved, and SessionFirstTurnOverridesResolved
- `session_first_turn_restore_recover` — RestoreSessionConsumedInputs, RestoreSessionConsumedInputsNoPhaseRollback, and RecoverSessionFirstTurnPhase rehydrate the per-session phase and presence/count registry from consumed-input rollback and durable snapshots under SessionConsumedInputsRestoreResolved and SessionFirstTurnPhaseRecovered
