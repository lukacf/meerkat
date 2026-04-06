//! Peer handling-mode validation — reject handling_mode on response conventions.
//!
//! Rules:
//! - handling_mode is FORBIDDEN for: PeerInput(ResponseProgress), PeerInput(ResponseTerminal)
//! - handling_mode is ALLOWED for: PeerInput(Message), PeerInput(Request), PeerInput(no convention)
//! - Non-peer inputs are always accepted (validation is a no-op)

use crate::input::{Input, PeerConvention};

/// Errors from peer handling-mode validation.
#[derive(Debug, Clone, thiserror::Error)]
#[non_exhaustive]
pub enum PeerHandlingModeError {
    /// handling_mode is not allowed on ResponseProgress peer inputs.
    #[error("handling_mode is forbidden on ResponseProgress peer inputs")]
    ForbiddenForResponseProgress,

    /// handling_mode is not allowed on ResponseTerminal peer inputs.
    #[error("handling_mode is forbidden on ResponseTerminal peer inputs")]
    ForbiddenForResponseTerminal,
}

/// Validate that a peer input does not carry handling_mode on response conventions.
pub fn validate_peer_handling_mode(input: &Input) -> Result<(), PeerHandlingModeError> {
    let Input::Peer(peer) = input else {
        return Ok(());
    };
    if peer.handling_mode.is_none() {
        return Ok(());
    }
    match &peer.convention {
        Some(PeerConvention::ResponseProgress { .. }) => {
            Err(PeerHandlingModeError::ForbiddenForResponseProgress)
        }
        Some(PeerConvention::ResponseTerminal { .. }) => {
            Err(PeerHandlingModeError::ForbiddenForResponseTerminal)
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
            header: make_header(),
            convention: Some(PeerConvention::ResponseProgress {
                request_id: "r".into(),
                phase: ResponseProgressPhase::InProgress,
            }),
            body: "working".into(),
            blocks: None,
            handling_mode: Some(HandlingMode::Queue),
        });
        let err = validate_peer_handling_mode(&input).unwrap_err();
        assert!(matches!(
            err,
            PeerHandlingModeError::ForbiddenForResponseProgress
        ));
    }

    #[test]
    fn response_terminal_with_handling_mode_rejected() {
        let input = Input::Peer(PeerInput {
            header: make_header(),
            convention: Some(PeerConvention::ResponseTerminal {
                request_id: "r".into(),
                status: ResponseTerminalStatus::Completed,
            }),
            body: "done".into(),
            blocks: None,
            handling_mode: Some(HandlingMode::Steer),
        });
        let err = validate_peer_handling_mode(&input).unwrap_err();
        assert!(matches!(
            err,
            PeerHandlingModeError::ForbiddenForResponseTerminal
        ));
    }

    #[test]
    fn message_with_handling_mode_accepted() {
        let input = Input::Peer(PeerInput {
            header: make_header(),
            convention: Some(PeerConvention::Message),
            body: "hi".into(),
            blocks: None,
            handling_mode: Some(HandlingMode::Queue),
        });
        assert!(validate_peer_handling_mode(&input).is_ok());
    }

    #[test]
    fn request_with_handling_mode_accepted() {
        let input = Input::Peer(PeerInput {
            header: make_header(),
            convention: Some(PeerConvention::Request {
                request_id: "r".into(),
                intent: "i".into(),
            }),
            body: "do it".into(),
            blocks: None,
            handling_mode: Some(HandlingMode::Steer),
        });
        assert!(validate_peer_handling_mode(&input).is_ok());
    }

    #[test]
    fn no_convention_with_handling_mode_accepted() {
        let input = Input::Peer(PeerInput {
            header: make_header(),
            convention: None,
            body: "hi".into(),
            blocks: None,
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
                header: make_header(),
                convention,
                body: "hi".into(),
                blocks: None,
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
            text: "hi".into(),
            blocks: None,
            turn_metadata: None,
        });
        assert!(validate_peer_handling_mode(&input).is_ok());
    }
}
