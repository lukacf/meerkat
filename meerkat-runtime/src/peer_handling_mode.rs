//! Peer handling-mode validation — reject handling_mode on response progress conventions.
//!
//! Rules:
//! - handling_mode is FORBIDDEN for: PeerInput(ResponseProgress)
//! - handling_mode is ALLOWED for: PeerInput(Message), PeerInput(Request), PeerInput(ResponseTerminal), PeerInput(no convention)
//! - Steer handling_mode is FORBIDDEN on peer inputs carrying injected
//!   context: steer realization stages live system-context appends only, so
//!   the injected-context transcript appends would be silently dropped
//! - Non-peer inputs are always accepted (validation is a no-op)

use crate::input::{Input, PeerConvention};
use meerkat_core::types::HandlingMode;

/// Errors from peer handling-mode validation.
#[derive(Debug, Clone, thiserror::Error)]
#[non_exhaustive]
pub enum PeerHandlingModeError {
    /// handling_mode is not allowed on ResponseProgress peer inputs.
    #[error("handling_mode is forbidden on ResponseProgress peer inputs")]
    ForbiddenForResponseProgress,
    /// Steer deliveries realize as live system-context appends, which carry
    /// no transcript boundary for injected context to precede. Fail closed
    /// rather than silently dropping host-attached context.
    #[error("steer handling_mode cannot carry injected context on peer inputs")]
    SteerCannotCarryInjectedContext,
}

/// Validate that a peer input does not carry handling_mode on response progress.
pub fn validate_peer_handling_mode(input: &Input) -> Result<(), PeerHandlingModeError> {
    let Input::Peer(peer) = input else {
        return Ok(());
    };
    if peer.handling_mode == Some(HandlingMode::Steer) && !peer.injected_context.is_empty() {
        return Err(PeerHandlingModeError::SteerCannotCarryInjectedContext);
    }
    if peer.handling_mode.is_none() {
        return Ok(());
    }
    match &peer.convention {
        Some(PeerConvention::ResponseProgress { .. }) => {
            Err(PeerHandlingModeError::ForbiddenForResponseProgress)
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::input::*;
    use chrono::Utc;
    use meerkat_core::lifecycle::InputId;
    use meerkat_core::types::HandlingMode;

    fn make_header() -> InputHeader {
        InputHeader {
            id: InputId::new(),
            timestamp: Utc::now(),
            source: InputOrigin::Peer {
                peer_id: "peer-1".into(),
                display_identity: None,
                runtime_id: None,
            },
            durability: InputDurability::Durable,
            visibility: InputVisibility::default(),
            idempotency_key: None,
            supersession_key: None,
            correlation_id: None,
        }
    }

    #[test]
    fn response_progress_with_handling_mode_rejected() {
        let input = Input::Peer(PeerInput {
            objective_id: None,
            injected_context: Vec::new(),
            sender_taint: None,
            header: make_header(),
            convention: Some(PeerConvention::ResponseProgress {
                request_id: "r".into(),
                phase: ResponseProgressPhase::InProgress,
            }),
            content: "working".into(),
            payload: Some(serde_json::json!({"progress": "working"})),
            handling_mode: Some(HandlingMode::Queue),
        });
        let err = validate_peer_handling_mode(&input).unwrap_err();
        assert!(matches!(
            err,
            PeerHandlingModeError::ForbiddenForResponseProgress
        ));
    }

    #[test]
    fn response_terminal_with_handling_mode_accepted() {
        let input = Input::Peer(PeerInput {
            objective_id: None,
            injected_context: Vec::new(),
            sender_taint: None,
            header: make_header(),
            convention: Some(PeerConvention::ResponseTerminal {
                request_id: "r".into(),
                status: ResponseTerminalStatus::Completed,
            }),
            content: "done".into(),
            payload: Some(serde_json::json!({"ok": true})),
            handling_mode: Some(HandlingMode::Steer),
        });
        assert!(validate_peer_handling_mode(&input).is_ok());
    }

    #[test]
    fn response_terminal_with_queue_handling_mode_accepted() {
        let input = Input::Peer(PeerInput {
            objective_id: None,
            injected_context: Vec::new(),
            sender_taint: None,
            header: make_header(),
            convention: Some(PeerConvention::ResponseTerminal {
                request_id: "r".into(),
                status: ResponseTerminalStatus::Completed,
            }),
            content: "done".into(),
            payload: Some(serde_json::json!({"ok": true})),
            handling_mode: Some(HandlingMode::Queue),
        });
        assert!(validate_peer_handling_mode(&input).is_ok());
    }

    #[test]
    fn message_with_handling_mode_accepted() {
        let input = Input::Peer(PeerInput {
            objective_id: None,
            injected_context: Vec::new(),
            sender_taint: None,
            header: make_header(),
            convention: Some(PeerConvention::Message),
            content: "hi".into(),
            payload: None,
            handling_mode: Some(HandlingMode::Queue),
        });
        assert!(validate_peer_handling_mode(&input).is_ok());
    }

    #[test]
    fn request_with_handling_mode_accepted() {
        let input = Input::Peer(PeerInput {
            objective_id: None,
            injected_context: Vec::new(),
            sender_taint: None,
            header: make_header(),
            convention: Some(PeerConvention::Request {
                request_id: "r".into(),
                intent: "i".into(),
            }),
            content: "do it".into(),
            payload: Some(serde_json::json!({"subject": "x"})),
            handling_mode: Some(HandlingMode::Steer),
        });
        assert!(validate_peer_handling_mode(&input).is_ok());
    }

    #[test]
    fn no_convention_with_handling_mode_accepted() {
        let input = Input::Peer(PeerInput {
            objective_id: None,
            injected_context: Vec::new(),
            sender_taint: None,
            header: make_header(),
            convention: None,
            content: "hi".into(),
            payload: None,
            handling_mode: Some(HandlingMode::Queue),
        });
        assert!(validate_peer_handling_mode(&input).is_ok());
    }

    /// Steer realization stages live system-context appends only — injected
    /// context would silently vanish. The accept boundary fails closed.
    #[test]
    fn steer_with_injected_context_rejected() {
        let input = Input::Peer(PeerInput {
            objective_id: None,
            injected_context: vec![meerkat_core::types::ContentInput::Text(
                "ambient".to_string(),
            )],
            sender_taint: None,
            header: make_header(),
            convention: Some(PeerConvention::Message),
            content: "urgent".into(),
            payload: None,
            handling_mode: Some(HandlingMode::Steer),
        });
        let err = validate_peer_handling_mode(&input).unwrap_err();
        assert!(matches!(
            err,
            PeerHandlingModeError::SteerCannotCarryInjectedContext
        ));
    }

    /// Queue-mode deliveries stage at a run boundary, which carries the
    /// transcript appends — injected context is deliverable there.
    #[test]
    fn queue_with_injected_context_accepted() {
        let input = Input::Peer(PeerInput {
            objective_id: None,
            injected_context: vec![meerkat_core::types::ContentInput::Text(
                "ambient".to_string(),
            )],
            sender_taint: None,
            header: make_header(),
            convention: Some(PeerConvention::Message),
            content: "work".into(),
            payload: None,
            handling_mode: Some(HandlingMode::Queue),
        });
        assert!(validate_peer_handling_mode(&input).is_ok());
    }

    #[test]
    fn peer_without_handling_mode_always_accepted() {
        for convention in [
            Some(PeerConvention::Message),
            Some(PeerConvention::Request {
                request_id: "r".into(),
                intent: "i".into(),
            }),
            Some(PeerConvention::ResponseProgress {
                request_id: "r".into(),
                phase: ResponseProgressPhase::InProgress,
            }),
            Some(PeerConvention::ResponseTerminal {
                request_id: "r".into(),
                status: ResponseTerminalStatus::Completed,
            }),
            None,
        ] {
            let input = Input::Peer(PeerInput {
                objective_id: None,
                injected_context: Vec::new(),
                sender_taint: None,
                header: make_header(),
                convention,
                content: "hi".into(),
                payload: None,
                handling_mode: None,
            });
            assert!(
                validate_peer_handling_mode(&input).is_ok(),
                "should accept peer without handling_mode"
            );
        }
    }

    #[test]
    fn non_peer_input_always_accepted() {
        let input = Input::Prompt(PromptInput {
            injected_context: Vec::new(),
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Operator,
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            content: "hi".into(),
            typed_turn_appends: Vec::new(),
            turn_metadata: None,
        });
        assert!(validate_peer_handling_mode(&input).is_ok());
    }
}
