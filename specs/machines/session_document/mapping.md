# SessionDocumentMachine Mapping Note

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust machine catalog. Do not edit it by hand.

### Machine
- `SessionDocumentMachine`

### Code Anchors
- `session_document_authority`: `meerkat-core/src/generated/session_document.rs` — generated SessionDocumentMachine owner for MarkSessionInitialTurnPendingInactiveOrPending, MarkSessionInitialTurnPendingConsumed, StartSessionInitialTurnPending, StartSessionInitialTurnInactive, StartSessionInitialTurnConsumed, ResolveSessionFirstTurnOverridesAllowed, ResolveSessionFirstTurnOverridesDenied, StageSessionInitialPromptStore, StageSessionInitialPromptClear, StageSessionToolResults, ConsumeSessionDeferredInputsPending, ConsumeSessionDeferredInputsInactive, ConsumeSessionDeferredInputsConsumed, RestoreSessionConsumedInputs, RestoreSessionConsumedInputsNoPhaseRollback, RecoverSessionFirstTurnPhase, SessionFirstTurnPhaseResolved, SessionFirstTurnOverridesResolved, SessionInitialPromptStageResolved, SessionToolResultsStageResolved, SessionConsumedInputsRestoreResolved, and SessionFirstTurnPhaseRecovered

### Scenarios
- `session_first_turn_pending_consume` — MarkSessionInitialTurnPendingInactiveOrPending, MarkSessionInitialTurnPendingConsumed, StartSessionInitialTurnPending, StartSessionInitialTurnInactive, StartSessionInitialTurnConsumed, ConsumeSessionDeferredInputsPending, ConsumeSessionDeferredInputsInactive, and ConsumeSessionDeferredInputsConsumed own the per-session first-turn phase registry and emit SessionFirstTurnPhaseResolved without handwritten phase mutation
- `session_initial_inputs_stage` — StageSessionInitialPromptStore, StageSessionInitialPromptClear, StageSessionToolResults, ResolveSessionFirstTurnOverridesAllowed, and ResolveSessionFirstTurnOverridesDenied resolve initial-prompt and tool-results staging plus build-override legality from the machine-owned phase map under SessionInitialPromptStageResolved, SessionToolResultsStageResolved, and SessionFirstTurnOverridesResolved
- `session_first_turn_restore_recover` — RestoreSessionConsumedInputs, RestoreSessionConsumedInputsNoPhaseRollback, and RecoverSessionFirstTurnPhase rehydrate the per-session phase and presence/count registry from consumed-input rollback and durable snapshots under SessionConsumedInputsRestoreResolved and SessionFirstTurnPhaseRecovered

### Transitions
- `MarkSessionInitialTurnPendingInactiveOrPending`
  - anchors: `session_document_authority`
  - scenarios: `session_first_turn_pending_consume`
- `MarkSessionInitialTurnPendingConsumed`
  - anchors: `session_document_authority`
  - scenarios: `session_first_turn_pending_consume`
- `StartSessionInitialTurnPending`
  - anchors: `session_document_authority`
  - scenarios: `session_first_turn_pending_consume`
- `StartSessionInitialTurnInactive`
  - anchors: `session_document_authority`
  - scenarios: `session_first_turn_pending_consume`
- `StartSessionInitialTurnConsumed`
  - anchors: `session_document_authority`
  - scenarios: `session_first_turn_pending_consume`
- `ResolveSessionFirstTurnOverridesAllowed`
  - anchors: `session_document_authority`
  - scenarios: `session_initial_inputs_stage`
- `ResolveSessionFirstTurnOverridesDenied`
  - anchors: `session_document_authority`
  - scenarios: `session_initial_inputs_stage`
- `StageSessionInitialPromptStore`
  - anchors: `session_document_authority`
  - scenarios: `session_initial_inputs_stage`
- `StageSessionInitialPromptClear`
  - anchors: `session_document_authority`
  - scenarios: `session_initial_inputs_stage`
- `StageSessionToolResults`
  - anchors: `session_document_authority`
  - scenarios: `session_initial_inputs_stage`
- `ConsumeSessionDeferredInputsPending`
  - anchors: `session_document_authority`
  - scenarios: `session_first_turn_pending_consume`
- `ConsumeSessionDeferredInputsInactive`
  - anchors: `session_document_authority`
  - scenarios: `session_first_turn_pending_consume`
- `ConsumeSessionDeferredInputsConsumed`
  - anchors: `session_document_authority`
  - scenarios: `session_first_turn_pending_consume`
- `RestoreSessionConsumedInputs`
  - anchors: `session_document_authority`
  - scenarios: `session_first_turn_restore_recover`
- `RestoreSessionConsumedInputsNoPhaseRollback`
  - anchors: `session_document_authority`
  - scenarios: `session_first_turn_restore_recover`
- `RecoverSessionFirstTurnPhase`
  - anchors: `session_document_authority`
  - scenarios: `session_first_turn_restore_recover`

### Effects
- `SessionFirstTurnPhaseResolved`
  - anchors: `session_document_authority`
  - scenarios: `session_first_turn_pending_consume`, `session_initial_inputs_stage`, `session_first_turn_restore_recover`
- `SessionFirstTurnOverridesResolved`
  - anchors: `session_document_authority`
  - scenarios: `session_initial_inputs_stage`
- `SessionInitialPromptStageResolved`
  - anchors: `session_document_authority`
  - scenarios: `session_initial_inputs_stage`
- `SessionToolResultsStageResolved`
  - anchors: `session_document_authority`
  - scenarios: `session_initial_inputs_stage`
- `SessionConsumedInputsRestoreResolved`
  - anchors: `session_document_authority`
  - scenarios: `session_first_turn_restore_recover`
- `SessionFirstTurnPhaseRecovered`
  - anchors: `session_document_authority`
  - scenarios: `session_first_turn_restore_recover`

### Invariants
- `(none)`


<!-- GENERATED_COVERAGE_END -->
