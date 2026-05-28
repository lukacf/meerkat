use meerkat_machine_dsl::machine;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum DeferredTurnFirstTurnPhase {
    #[default]
    Inactive,
    Pending,
    Consumed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum InitialPromptStageDecision {
    #[default]
    Clear,
    Store,
}

machine! {
    machine SessionDeferredTurnAuthorityMachine {
        version: 1,
        rust: "self" / "catalog::dsl::session_deferred_turn_authority",

        state {
            lifecycle_phase: SessionDeferredTurnAuthorityPhase,
        }

        init(Ready) {}

        terminal []

        phase SessionDeferredTurnAuthorityPhase {
            Ready,
        }

        input SessionDeferredTurnAuthorityInput {
            MarkInitialTurnPending { current_phase: Enum<DeferredTurnFirstTurnPhase> },
            StartInitialTurn { current_phase: Enum<DeferredTurnFirstTurnPhase> },
            RestoreInitialTurnPending { restore_requested: bool },
            AllowsInitialTurnOverrides { current_phase: Enum<DeferredTurnFirstTurnPhase> },
            ResolveInitialPromptStage {
                current_phase: Enum<DeferredTurnFirstTurnPhase>,
                prompt_has_content: bool,
            },
            ResolveToolResultsStage {
                current_phase: Enum<DeferredTurnFirstTurnPhase>,
                result_count: u64,
            },
            ResolveConsumedInputsRestore {
                restore_first_turn_pending: bool,
                pending_initial_prompt_present: bool,
                pending_tool_result_message_count: u64,
            },
            RestoreDeferredTurnState {
                current_phase: Enum<DeferredTurnFirstTurnPhase>,
                pending_initial_prompt_present: bool,
                pending_tool_result_message_count: u64,
            },
        }

        effect SessionDeferredTurnAuthorityEffect {
            FirstTurnPhaseResolved {
                phase: Enum<DeferredTurnFirstTurnPhase>,
                was_pending: bool,
            },
            InitialTurnOverridesResolved { allowed: bool },
            InitialPromptStageResolved { decision: Enum<InitialPromptStageDecision> },
            ToolResultsStageResolved { accepted_count: u64 },
            ConsumedInputsRestoreResolved {
                restore_first_turn_pending: bool,
                restore_initial_prompt: bool,
                restore_tool_results: bool,
            },
            SnapshotRestoreAuthorized,
        }

        helper phase_allows_initial_turn_overrides(current_phase: Enum<DeferredTurnFirstTurnPhase>) -> bool {
            current_phase == DeferredTurnFirstTurnPhase::Pending
        }

        helper should_store_initial_prompt(
            current_phase: Enum<DeferredTurnFirstTurnPhase>,
            prompt_has_content: bool
        ) -> bool {
            current_phase == DeferredTurnFirstTurnPhase::Pending && prompt_has_content
        }

        disposition FirstTurnPhaseResolved => local,
        disposition InitialTurnOverridesResolved => local,
        disposition InitialPromptStageResolved => local,
        disposition ToolResultsStageResolved => local,
        disposition ConsumedInputsRestoreResolved => local,
        disposition SnapshotRestoreAuthorized => local,

        transition MarkInitialTurnPendingInactiveOrPending {
            on input MarkInitialTurnPending { current_phase }
            guard {
                self.lifecycle_phase == Phase::Ready
                && (current_phase == DeferredTurnFirstTurnPhase::Inactive
                    || current_phase == DeferredTurnFirstTurnPhase::Pending)
            }
            update {}
            to Ready
            emit FirstTurnPhaseResolved {
                phase: DeferredTurnFirstTurnPhase::Pending,
                was_pending: false
            }
        }

        transition MarkInitialTurnPendingConsumed {
            on input MarkInitialTurnPending { current_phase }
            guard {
                self.lifecycle_phase == Phase::Ready
                && current_phase == DeferredTurnFirstTurnPhase::Consumed
            }
            update {}
            to Ready
            emit FirstTurnPhaseResolved {
                phase: DeferredTurnFirstTurnPhase::Consumed,
                was_pending: false
            }
        }

        transition StartInitialTurnPending {
            on input StartInitialTurn { current_phase }
            guard {
                self.lifecycle_phase == Phase::Ready
                && current_phase == DeferredTurnFirstTurnPhase::Pending
            }
            update {}
            to Ready
            emit FirstTurnPhaseResolved {
                phase: DeferredTurnFirstTurnPhase::Consumed,
                was_pending: true
            }
        }

        transition StartInitialTurnInactive {
            on input StartInitialTurn { current_phase }
            guard {
                self.lifecycle_phase == Phase::Ready
                && current_phase == DeferredTurnFirstTurnPhase::Inactive
            }
            update {}
            to Ready
            emit FirstTurnPhaseResolved {
                phase: DeferredTurnFirstTurnPhase::Inactive,
                was_pending: false
            }
        }

        transition StartInitialTurnConsumed {
            on input StartInitialTurn { current_phase }
            guard {
                self.lifecycle_phase == Phase::Ready
                && current_phase == DeferredTurnFirstTurnPhase::Consumed
            }
            update {}
            to Ready
            emit FirstTurnPhaseResolved {
                phase: DeferredTurnFirstTurnPhase::Consumed,
                was_pending: false
            }
        }

        transition RestoreInitialTurnPending {
            on input RestoreInitialTurnPending { restore_requested }
            guard { self.lifecycle_phase == Phase::Ready && restore_requested }
            update {}
            to Ready
            emit FirstTurnPhaseResolved {
                phase: DeferredTurnFirstTurnPhase::Pending,
                was_pending: false
            }
        }

        transition ResolveInitialTurnOverridesAllowed {
            on input AllowsInitialTurnOverrides { current_phase }
            guard {
                self.lifecycle_phase == Phase::Ready
                && phase_allows_initial_turn_overrides(current_phase)
            }
            update {}
            to Ready
            emit InitialTurnOverridesResolved { allowed: true }
        }

        transition ResolveInitialTurnOverridesDenied {
            on input AllowsInitialTurnOverrides { current_phase }
            guard {
                self.lifecycle_phase == Phase::Ready
                && phase_allows_initial_turn_overrides(current_phase) == false
            }
            update {}
            to Ready
            emit InitialTurnOverridesResolved { allowed: false }
        }

        transition ResolveInitialPromptStore {
            on input ResolveInitialPromptStage { current_phase, prompt_has_content }
            guard {
                self.lifecycle_phase == Phase::Ready
                && should_store_initial_prompt(current_phase, prompt_has_content)
            }
            update {}
            to Ready
            emit InitialPromptStageResolved { decision: InitialPromptStageDecision::Store }
        }

        transition ResolveInitialPromptClear {
            on input ResolveInitialPromptStage { current_phase, prompt_has_content }
            guard {
                self.lifecycle_phase == Phase::Ready
                && should_store_initial_prompt(current_phase, prompt_has_content) == false
            }
            update {}
            to Ready
            emit InitialPromptStageResolved { decision: InitialPromptStageDecision::Clear }
        }

        transition ResolveToolResultsStage {
            on input ResolveToolResultsStage { current_phase, result_count }
            guard {
                self.lifecycle_phase == Phase::Ready
                && (current_phase == DeferredTurnFirstTurnPhase::Inactive
                    || current_phase == DeferredTurnFirstTurnPhase::Pending
                    || current_phase == DeferredTurnFirstTurnPhase::Consumed)
            }
            update {}
            to Ready
            emit ToolResultsStageResolved { accepted_count: result_count }
        }

        transition ResolveConsumedInputsRestore {
            on input ResolveConsumedInputsRestore {
                restore_first_turn_pending,
                pending_initial_prompt_present,
                pending_tool_result_message_count
            }
            guard { self.lifecycle_phase == Phase::Ready }
            update {}
            to Ready
            emit ConsumedInputsRestoreResolved {
                restore_first_turn_pending: restore_first_turn_pending,
                restore_initial_prompt: pending_initial_prompt_present,
                restore_tool_results: pending_tool_result_message_count > 0
            }
        }

        transition AuthorizeRestoreDeferredTurnState {
            on input RestoreDeferredTurnState {
                current_phase,
                pending_initial_prompt_present,
                pending_tool_result_message_count
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && (current_phase == DeferredTurnFirstTurnPhase::Inactive
                    || current_phase == DeferredTurnFirstTurnPhase::Pending
                    || current_phase == DeferredTurnFirstTurnPhase::Consumed)
            }
            update {}
            to Ready
            emit SnapshotRestoreAuthorized
        }
    }
}
