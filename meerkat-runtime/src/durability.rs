//! §10 Durability validation — enforce durability rules on inputs.
//!
//! Rules:
//! - Derived is FORBIDDEN for: PromptInput, PeerInput(Message/Request/ResponseTerminal), FlowStepInput
//! - External ingress cannot submit Derived durability

use crate::input::{Input, InputDurability, PeerConvention};

/// Errors from durability validation.
#[derive(Debug, Clone, thiserror::Error)]
#[non_exhaustive]
pub enum DurabilityError {
    /// Derived durability is not allowed for this input type.
    #[error("Derived durability forbidden for {kind}")]
    DerivedForbidden { kind: String },

    /// External source cannot submit derived inputs.
    #[error("External ingress cannot submit derived inputs")]
    ExternalDerivedForbidden,
}

/// Validate the durability of an input.
pub fn validate_durability(input: &Input) -> Result<(), DurabilityError> {
    let durability = input.header().durability;

    // Check external ingress cannot submit Derived
    if durability == InputDurability::Derived {
        match &input.header().source {
            crate::input::InputOrigin::Operator
            | crate::input::InputOrigin::Peer { .. }
            | crate::input::InputOrigin::External { .. } => {
                return Err(DurabilityError::ExternalDerivedForbidden);
            }
            // System and Flow sources CAN submit Derived
            crate::input::InputOrigin::System | crate::input::InputOrigin::Flow { .. } => {}
        }
    }

    // Check Derived forbidden for specific input types
    if durability == InputDurability::Derived {
        match input {
            Input::Prompt(_) => {
                return Err(DurabilityError::DerivedForbidden {
                    kind: "prompt".into(),
                });
            }
            Input::Peer(p) => {
                match &p.convention {
                    Some(
                        PeerConvention::Message
                        | PeerConvention::Request { .. }
                        | PeerConvention::ResponseTerminal { .. },
                    ) => {
                        return Err(DurabilityError::DerivedForbidden {
                            kind: format!("peer_{}", input.kind_id().0),
                        });
                    }
                    // ResponseProgress CAN be Derived
                    Some(PeerConvention::ResponseProgress { .. }) | None => {}
                }
            }
            Input::FlowStep(_) => {
                return Err(DurabilityError::DerivedForbidden {
                    kind: "flow_step".into(),
                });
            }
            // ExternalEvent, SystemGenerated, Projected CAN be Derived
            Input::ExternalEvent(_) | Input::SystemGenerated(_) | Input::Projected(_) => {}
        }
    }

    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::input::*;
    use chrono::Utc;
    use meerkat_core::lifecycle::InputId;

    fn make_header(durability: InputDurability, source: InputOrigin) -> InputHeader {
        InputHeader {
            id: InputId::new(),
            timestamp: Utc::now(),
            source,
            durability,
            visibility: InputVisibility::default(),
            idempotency_key: None,
            supersession_key: None,
            correlation_id: None,
        }
    }

    #[test]
    fn prompt_derived_rejected() {
        let input = Input::Prompt(PromptInput {
            header: make_header(InputDurability::Derived, InputOrigin::System),
            text: "hi".into(),
            blocks: None,
            turn_metadata: None,
        });
        assert!(validate_durability(&input).is_err());
    }

    #[test]
    fn prompt_durable_accepted() {
        let input = Input::Prompt(PromptInput {
            header: make_header(InputDurability::Durable, InputOrigin::Operator),
            text: "hi".into(),
            blocks: None,
            turn_metadata: None,
        });
        assert!(validate_durability(&input).is_ok());
    }

    #[test]
    fn prompt_ephemeral_accepted() {
        let input = Input::Prompt(PromptInput {
            header: make_header(InputDurability::Ephemeral, InputOrigin::Operator),
            text: "hi".into(),
            blocks: None,
            turn_metadata: None,
        });
        assert!(validate_durability(&input).is_ok());
    }

    #[test]
    fn peer_message_derived_rejected() {
        let input = Input::Peer(PeerInput {
            header: make_header(InputDurability::Derived, InputOrigin::System),
            convention: Some(PeerConvention::Message),
            body: "hi".into(),
            blocks: None,
        });
        assert!(validate_durability(&input).is_err());
    }

    #[test]
    fn peer_request_derived_rejected() {
        let input = Input::Peer(PeerInput {
            header: make_header(InputDurability::Derived, InputOrigin::System),
            convention: Some(PeerConvention::Request {
                request_id: "r".into(),
                intent: "i".into(),
            }),
            body: "hi".into(),
            blocks: None,
        });
        assert!(validate_durability(&input).is_err());
    }

    #[test]
    fn peer_response_terminal_derived_rejected() {
        let input = Input::Peer(PeerInput {
            header: make_header(InputDurability::Derived, InputOrigin::System),
            convention: Some(PeerConvention::ResponseTerminal {
                request_id: "r".into(),
                status: ResponseTerminalStatus::Completed,
            }),
            body: "done".into(),
            blocks: None,
        });
        assert!(validate_durability(&input).is_err());
    }

    #[test]
    fn peer_response_progress_derived_accepted() {
        let input = Input::Peer(PeerInput {
            header: make_header(InputDurability::Derived, InputOrigin::System),
            convention: Some(PeerConvention::ResponseProgress {
                request_id: "r".into(),
                phase: ResponseProgressPhase::InProgress,
            }),
            body: "working".into(),
            blocks: None,
        });
        assert!(validate_durability(&input).is_ok());
    }

    #[test]
    fn flow_step_derived_rejected() {
        let input = Input::FlowStep(FlowStepInput {
            header: make_header(InputDurability::Derived, InputOrigin::System),
            step_id: "s1".into(),
            instructions: "do it".into(),
            turn_metadata: None,
        });
        assert!(validate_durability(&input).is_err());
    }

    #[test]
    fn external_event_derived_from_system_accepted() {
        let input = Input::ExternalEvent(ExternalEventInput {
            header: make_header(InputDurability::Derived, InputOrigin::System),
            event_type: "test".into(),
            payload: serde_json::json!({}),
        });
        assert!(validate_durability(&input).is_ok());
    }

    #[test]
    fn external_ingress_derived_rejected() {
        let input = Input::ExternalEvent(ExternalEventInput {
            header: make_header(
                InputDurability::Derived,
                InputOrigin::External {
                    source_name: "webhook".into(),
                },
            ),
            event_type: "test".into(),
            payload: serde_json::json!({}),
        });
        assert!(validate_durability(&input).is_err());
    }

    #[test]
    fn operator_derived_rejected() {
        let input = Input::SystemGenerated(SystemGeneratedInput {
            header: make_header(InputDurability::Derived, InputOrigin::Operator),
            generator: "test".into(),
            content: "content".into(),
        });
        assert!(validate_durability(&input).is_err());
    }

    #[test]
    fn projected_derived_from_system_accepted() {
        let input = Input::Projected(ProjectedInput {
            header: make_header(InputDurability::Derived, InputOrigin::System),
            rule_id: "rule-1".into(),
            source_event_id: "evt-1".into(),
            content: "projected".into(),
        });
        assert!(validate_durability(&input).is_ok());
    }
}
