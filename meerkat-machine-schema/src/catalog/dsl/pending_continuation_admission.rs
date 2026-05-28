use meerkat_machine_dsl::machine;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum ObservedSessionTailKind {
    #[default]
    Empty,
    System,
    SystemNotice,
    User,
    Assistant,
    BlockAssistant,
    ToolResults,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum PendingContinuationDisposition {
    RunPending,
    #[default]
    NoPendingBoundary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum PendingContinuationPublicTerminal {
    #[default]
    NoPendingBoundary,
}

machine! {
    machine PendingContinuationAdmissionMachine {
        version: 1,
        rust: "self" / "catalog::dsl::pending_continuation_admission",

        state {
            lifecycle_phase: PendingContinuationAdmissionPhase,
            last_public_terminal: Option<Enum<PendingContinuationPublicTerminal>>,
        }

        init(Ready) {
            last_public_terminal = None,
        }

        terminal []

        phase PendingContinuationAdmissionPhase {
            Ready,
        }

        input PendingContinuationAdmissionInput {
            ResolvePendingContinuation {
                session_tail: Enum<ObservedSessionTailKind>,
                staged_tool_result_count: u64,
            },
            ResolveLastPendingContinuationPublicTerminal,
        }

        effect PendingContinuationAdmissionEffect {
            PendingContinuationResolved { disposition: Enum<PendingContinuationDisposition> },
            PendingContinuationPublicTerminalResolved { terminal: Enum<PendingContinuationPublicTerminal> },
        }

        helper tail_has_pending_boundary(session_tail: Enum<ObservedSessionTailKind>) -> bool {
            session_tail == ObservedSessionTailKind::User || session_tail == ObservedSessionTailKind::ToolResults
        }

        helper has_effective_pending_boundary(session_tail: Enum<ObservedSessionTailKind>, staged_tool_result_count: u64) -> bool {
            tail_has_pending_boundary(session_tail) || staged_tool_result_count > 0
        }

        disposition PendingContinuationResolved => local,
        disposition PendingContinuationPublicTerminalResolved => local,

        transition ResolveWithBoundary {
            on input ResolvePendingContinuation {
                session_tail,
                staged_tool_result_count
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && has_effective_pending_boundary(session_tail, staged_tool_result_count)
            }
            update {
                self.last_public_terminal = None;
            }
            to Ready
            emit PendingContinuationResolved { disposition: PendingContinuationDisposition::RunPending }
        }

        transition ResolveWithoutBoundary {
            on input ResolvePendingContinuation {
                session_tail,
                staged_tool_result_count
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && has_effective_pending_boundary(session_tail, staged_tool_result_count) == false
            }
            update {
                self.last_public_terminal = Some(PendingContinuationPublicTerminal::NoPendingBoundary);
            }
            to Ready
            emit PendingContinuationResolved { disposition: PendingContinuationDisposition::NoPendingBoundary }
            emit PendingContinuationPublicTerminalResolved { terminal: PendingContinuationPublicTerminal::NoPendingBoundary }
        }

        transition ResolveLastNoPendingTerminal {
            on input ResolveLastPendingContinuationPublicTerminal
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.last_public_terminal == Some(PendingContinuationPublicTerminal::NoPendingBoundary)
            }
            update {}
            to Ready
            emit PendingContinuationPublicTerminalResolved { terminal: PendingContinuationPublicTerminal::NoPendingBoundary }
        }
    }
}
