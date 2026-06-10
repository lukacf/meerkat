//! Pending-continuation admission, owned by the canonical
//! [`session_document::SessionDocumentMachine`].
//!
//! The pending-boundary disposition (`RunPending` / `NoPendingBoundary`) is a
//! session-document-tail-derived SEMANTIC fact. It is decided by a
//! SessionDocumentMachine transition (`ResolvePendingContinuation`), not here:
//! this module only carries the PURE mechanical [`observe_session_tail`]
//! encoder (the `messages.last()` → [`ObservedSessionTailKind`] map) and a thin
//! driver that runs the machine op and MIRRORS the emitted disposition. No
//! decision lives in this shell.

use crate::session_document;
use crate::types::Message;

// Re-export the canonical pending-continuation vocabulary (owned by the
// SessionDocumentMachine) under this module so consumers have one ergonomic
// path for the encoder, the typed tail/disposition, and the driver.
pub use session_document::SessionDocumentError as PendingContinuationAdmissionError;
pub use session_document::{
    ObservedSessionTailKind, PendingContinuationDisposition, PendingContinuationPublicTerminal,
};

/// Mirror of the [`session_document::SessionDocumentMachine`]
/// pending-continuation decision for a single resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingContinuationResolution {
    pub disposition: PendingContinuationDisposition,
    pub public_terminal: Option<PendingContinuationPublicTerminal>,
}

/// Pure mechanical encoder: classify the last message of a transcript into the
/// typed [`ObservedSessionTailKind`]. This is a structural encoding only — it
/// makes no admission decision. The machine decides the disposition from this
/// observation.
#[must_use]
pub fn observe_session_tail(messages: &[Message]) -> ObservedSessionTailKind {
    match messages.last() {
        None => ObservedSessionTailKind::Empty,
        Some(Message::System(_)) => ObservedSessionTailKind::System,
        Some(Message::SystemNotice(_)) => ObservedSessionTailKind::SystemNotice,
        Some(Message::User(_)) => ObservedSessionTailKind::User,
        Some(Message::BlockAssistant(_)) => ObservedSessionTailKind::BlockAssistant,
        Some(Message::ToolResults { .. }) => ObservedSessionTailKind::ToolResults,
    }
}

/// Drive the canonical SessionDocumentMachine `ResolvePendingContinuation`
/// transition and MIRROR the emitted disposition (and public terminal witness).
/// The pending-continuation region of SessionDocumentMachine is stateless
/// (self-loop in `Ready`), so a fresh authority per resolution is canonical.
pub fn resolve_pending_continuation(
    session_tail: ObservedSessionTailKind,
    staged_tool_result_count: u64,
) -> Result<PendingContinuationResolution, PendingContinuationAdmissionError> {
    let mut authority = session_document::SessionDocumentMachineAuthority::new();
    let effects = authority.resolve_pending_continuation(session_tail, staged_tool_result_count)?;
    let mut disposition = None;
    let mut public_terminal = None;
    for effect in effects {
        match effect {
            session_document::SessionDocumentEffect::PendingContinuationResolved {
                disposition: value,
            } => {
                disposition = Some(value);
            }
            session_document::SessionDocumentEffect::PendingContinuationPublicTerminalResolved {
                terminal,
            } => {
                public_terminal = Some(terminal);
            }
            _ => {}
        }
    }
    let Some(disposition) = disposition else {
        // Both pending-continuation transitions always emit
        // `PendingContinuationResolved`, so this is unreachable; surface it as a
        // typed authority error rather than panicking if the contract ever drifts.
        return Err(PendingContinuationAdmissionError::new(
            "pending_continuation_resolution_effect",
        ));
    };
    Ok(PendingContinuationResolution {
        disposition,
        public_terminal,
    })
}
