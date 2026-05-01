//! Peer handling-mode validation — reject handling_mode on response progress conventions.
//!
//! Rules:
//! - handling_mode is FORBIDDEN for: PeerInput(ResponseProgress)
//! - handling_mode is ALLOWED for: PeerInput(Message), PeerInput(Request), PeerInput(ResponseTerminal), PeerInput(no convention)
//! - Non-peer inputs are always accepted (validation is a no-op)

use crate::input::{Input, PeerConvention};

/// Errors from peer handling-mode validation.
#[derive(Debug, Clone, thiserror::Error)]
#[non_exhaustive]
pub enum PeerHandlingModeError {
    /// handling_mode is not allowed on ResponseProgress peer inputs.
    #[error("handling_mode is forbidden on ResponseProgress peer inputs")]
    ForbiddenForResponseProgress,
    /// Legacy handling_mode and canonical turn_metadata.handling_mode disagree.
    #[error("peer handling_mode diverges from turn_metadata.handling_mode")]
    DivergentHandlingMode,
}

/// Validate that a peer input does not carry handling_mode on response progress.
pub fn validate_peer_handling_mode(input: &Input) -> Result<(), PeerHandlingModeError> {
    let Input::Peer(peer) = input else {
        return Ok(());
    };
    let metadata_handling_mode = peer
        .turn_metadata
        .as_ref()
        .and_then(|metadata| metadata.handling_mode);
    if let (Some(legacy), Some(canonical)) = (peer.handling_mode, metadata_handling_mode)
        && legacy != canonical
    {
        return Err(PeerHandlingModeError::DivergentHandlingMode);
    }
    let handling_mode = metadata_handling_mode.or(peer.handling_mode);
    if handling_mode.is_none() {
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
    use meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata;
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
            header: make_header(),
            convention: Some(PeerConvention::ResponseProgress {
                request_id: "r".into(),
                phase: ResponseProgressPhase::InProgress,
            }),
            body: "working".into(),
            payload: Some(serde_json::json!({"progress": "working"})),
            blocks: None,
            turn_metadata: None,
            handling_mode: Some(HandlingMode::Queue),
        });
        let err = validate_peer_handling_mode(&input).unwrap_err();
        assert!(matches!(
            err,
            PeerHandlingModeError::ForbiddenForResponseProgress
        ));
    }

    #[test]
    fn response_progress_with_turn_metadata_handling_mode_rejected() {
        let input = Input::Peer(PeerInput {
            header: make_header(),
            convention: Some(PeerConvention::ResponseProgress {
                request_id: "r".into(),
                phase: ResponseProgressPhase::InProgress,
            }),
            body: "working".into(),
            payload: Some(serde_json::json!({"progress": "working"})),
            blocks: None,
            turn_metadata: Some(RuntimeTurnMetadata {
                handling_mode: Some(HandlingMode::Queue),
                ..Default::default()
            }),
            handling_mode: None,
        });
        let err = validate_peer_handling_mode(&input).unwrap_err();
        assert!(matches!(
            err,
            PeerHandlingModeError::ForbiddenForResponseProgress
        ));
    }

    #[test]
    fn divergent_legacy_and_turn_metadata_handling_modes_rejected() {
        let input = Input::Peer(PeerInput {
            header: make_header(),
            convention: Some(PeerConvention::Message),
            body: "hi".into(),
            payload: None,
            blocks: None,
            turn_metadata: Some(RuntimeTurnMetadata {
                handling_mode: Some(HandlingMode::Steer),
                ..Default::default()
            }),
            handling_mode: Some(HandlingMode::Queue),
        });
        let err = validate_peer_handling_mode(&input).unwrap_err();
        assert!(matches!(err, PeerHandlingModeError::DivergentHandlingMode));
    }

    #[test]
    fn matching_legacy_and_turn_metadata_handling_modes_accepted() {
        let input = Input::Peer(PeerInput {
            header: make_header(),
            convention: Some(PeerConvention::Message),
            body: "hi".into(),
            payload: None,
            blocks: None,
            turn_metadata: Some(RuntimeTurnMetadata {
                handling_mode: Some(HandlingMode::Queue),
                ..Default::default()
            }),
            handling_mode: Some(HandlingMode::Queue),
        });
        assert!(validate_peer_handling_mode(&input).is_ok());
    }

    #[test]
    fn response_terminal_with_handling_mode_accepted() {
        let input = Input::Peer(PeerInput {
            header: make_header(),
            convention: Some(PeerConvention::ResponseTerminal {
                request_id: "r".into(),
                status: ResponseTerminalStatus::Completed,
            }),
            body: "done".into(),
            payload: Some(serde_json::json!({"ok": true})),
            blocks: None,
            turn_metadata: None,
            handling_mode: Some(HandlingMode::Steer),
        });
        assert!(validate_peer_handling_mode(&input).is_ok());
    }

    #[test]
    fn response_terminal_with_queue_handling_mode_accepted() {
        let input = Input::Peer(PeerInput {
            header: make_header(),
            convention: Some(PeerConvention::ResponseTerminal {
                request_id: "r".into(),
                status: ResponseTerminalStatus::Completed,
            }),
            body: "done".into(),
            payload: Some(serde_json::json!({"ok": true})),
            blocks: None,
            turn_metadata: None,
            handling_mode: Some(HandlingMode::Queue),
        });
        assert!(validate_peer_handling_mode(&input).is_ok());
    }

    #[test]
    fn message_with_handling_mode_accepted() {
        let input = Input::Peer(PeerInput {
            header: make_header(),
            convention: Some(PeerConvention::Message),
            body: "hi".into(),
            payload: None,
            blocks: None,
            turn_metadata: None,
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
            payload: Some(serde_json::json!({"subject": "x"})),
            blocks: None,
            turn_metadata: None,
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
            payload: None,
            blocks: None,
            turn_metadata: None,
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
                payload: None,
                blocks: None,
                turn_metadata: None,
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
            build_only_overrides: None,
        });
        assert!(validate_peer_handling_mode(&input).is_ok());
    }
}
