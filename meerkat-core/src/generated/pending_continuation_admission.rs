// @generated — pending continuation admission authority for session turns

use std::fmt;

use crate::types::Message;

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
    #[default]
    RunPending,
    NoPendingBoundary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum PendingContinuationPublicTerminal {
    #[default]
    NoPendingBoundary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingContinuationResolution {
    pub disposition: PendingContinuationDisposition,
    pub public_terminal: Option<PendingContinuationPublicTerminal>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingContinuationAdmissionError {
    op: &'static str,
}

impl fmt::Display for PendingContinuationAdmissionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "generated pending-continuation authority rejected {}",
            self.op
        )
    }
}

impl std::error::Error for PendingContinuationAdmissionError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PendingContinuationAdmissionPhase {
    #[default]
    Ready,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PendingContinuationAdmissionMachineState {
    lifecycle_phase: PendingContinuationAdmissionPhase,
    last_public_terminal: Option<PendingContinuationPublicTerminal>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingContinuationAdmissionMachineAuthority {
    state: PendingContinuationAdmissionMachineState,
}

impl PendingContinuationAdmissionMachineAuthority {
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: PendingContinuationAdmissionMachineState {
                lifecycle_phase: PendingContinuationAdmissionPhase::Ready,
                last_public_terminal: None,
            },
        }
    }

    #[must_use]
    pub fn state(&self) -> &PendingContinuationAdmissionMachineState {
        &self.state
    }

    pub fn resolve_pending_continuation(
        &mut self,
        session_tail: ObservedSessionTailKind,
        staged_tool_result_count: u64,
    ) -> Result<PendingContinuationResolution, PendingContinuationAdmissionError> {
        if self.state.lifecycle_phase != PendingContinuationAdmissionPhase::Ready {
            return Err(PendingContinuationAdmissionError {
                op: "resolve_pending_continuation",
            });
        }
        if has_effective_pending_boundary(session_tail, staged_tool_result_count) {
            self.state.last_public_terminal = None;
            Ok(PendingContinuationResolution {
                disposition: PendingContinuationDisposition::RunPending,
                public_terminal: None,
            })
        } else {
            self.state.last_public_terminal =
                Some(PendingContinuationPublicTerminal::NoPendingBoundary);
            Ok(PendingContinuationResolution {
                disposition: PendingContinuationDisposition::NoPendingBoundary,
                public_terminal: Some(PendingContinuationPublicTerminal::NoPendingBoundary),
            })
        }
    }

    pub fn resolve_last_public_terminal(
        &self,
    ) -> Result<PendingContinuationPublicTerminal, PendingContinuationAdmissionError> {
        self.state
            .last_public_terminal
            .ok_or(PendingContinuationAdmissionError {
                op: "resolve_last_pending_continuation_public_terminal",
            })
    }
}

impl Default for PendingContinuationAdmissionMachineAuthority {
    fn default() -> Self {
        Self::new()
    }
}

#[must_use]
pub fn observe_session_tail(messages: &[Message]) -> ObservedSessionTailKind {
    match messages.last() {
        None => ObservedSessionTailKind::Empty,
        Some(Message::System(_)) => ObservedSessionTailKind::System,
        Some(Message::SystemNotice(_)) => ObservedSessionTailKind::SystemNotice,
        Some(Message::User(_)) => ObservedSessionTailKind::User,
        Some(Message::Assistant(_)) => ObservedSessionTailKind::Assistant,
        Some(Message::BlockAssistant(_)) => ObservedSessionTailKind::BlockAssistant,
        Some(Message::ToolResults { .. }) => ObservedSessionTailKind::ToolResults,
    }
}

pub fn resolve_pending_continuation(
    session_tail: ObservedSessionTailKind,
    staged_tool_result_count: u64,
) -> Result<PendingContinuationResolution, PendingContinuationAdmissionError> {
    PendingContinuationAdmissionMachineAuthority::new()
        .resolve_pending_continuation(session_tail, staged_tool_result_count)
}

fn tail_has_pending_boundary(session_tail: ObservedSessionTailKind) -> bool {
    matches!(
        session_tail,
        ObservedSessionTailKind::User | ObservedSessionTailKind::ToolResults
    )
}

fn has_effective_pending_boundary(
    session_tail: ObservedSessionTailKind,
    staged_tool_result_count: u64,
) -> bool {
    tail_has_pending_boundary(session_tail) || staged_tool_result_count > 0
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn user_and_tool_results_tails_admit_pending_continuation() {
        for tail in [
            ObservedSessionTailKind::User,
            ObservedSessionTailKind::ToolResults,
        ] {
            let resolution = resolve_pending_continuation(tail, 0)
                .expect("generated authority should resolve pending tail");
            assert_eq!(
                resolution.disposition,
                PendingContinuationDisposition::RunPending
            );
            assert_eq!(resolution.public_terminal, None);
        }
    }

    #[test]
    fn staged_tool_results_admit_pending_continuation() {
        let resolution = resolve_pending_continuation(ObservedSessionTailKind::Empty, 1)
            .expect("generated authority should resolve staged results");
        assert_eq!(
            resolution.disposition,
            PendingContinuationDisposition::RunPending
        );
        assert_eq!(resolution.public_terminal, None);
    }

    #[test]
    fn non_boundary_tail_emits_no_pending_terminal() {
        let resolution = resolve_pending_continuation(ObservedSessionTailKind::Assistant, 0)
            .expect("generated authority should resolve non-boundary tail");
        assert_eq!(
            resolution.disposition,
            PendingContinuationDisposition::NoPendingBoundary
        );
        assert_eq!(
            resolution.public_terminal,
            Some(PendingContinuationPublicTerminal::NoPendingBoundary)
        );
    }
}
