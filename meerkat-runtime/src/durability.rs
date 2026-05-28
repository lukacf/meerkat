//! §10 Durability diagnostics.
//!
//! Admission validity is emitted by the generated MeerkatMachine
//! `ResolveAdmissionValidation` transitions. This module is only the
//! compatibility/display mirror used to explain a generated durability
//! rejection.
//!
//! Diagnostic rules mirrored here:
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

/// Human-readable detail for a generated durability rejection.
pub(crate) fn durability_rejection_detail(input: &Input) -> Option<String> {
    durability_diagnostic_error(input).map(|error| error.to_string())
}

fn durability_diagnostic_error(input: &Input) -> Option<DurabilityError> {
    let durability = input.header().durability;

    // Check external ingress cannot submit Derived
    if durability == InputDurability::Derived {
        match &input.header().source {
            crate::input::InputOrigin::Operator
            | crate::input::InputOrigin::Peer { .. }
            | crate::input::InputOrigin::External { .. } => {
                return Some(DurabilityError::ExternalDerivedForbidden);
            }
            // System and Flow sources CAN submit Derived
            crate::input::InputOrigin::System | crate::input::InputOrigin::Flow { .. } => {}
        }
    }

    // Check Derived forbidden for specific input types
    if durability == InputDurability::Derived {
        match input {
            Input::Prompt(_) => {
                return Some(DurabilityError::DerivedForbidden {
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
                        return Some(DurabilityError::DerivedForbidden {
                            kind: format!("peer_{}", input.kind().as_str()),
                        });
                    }
                    // ResponseProgress CAN be Derived
                    Some(PeerConvention::ResponseProgress { .. }) | None => {}
                }
            }
            Input::FlowStep(_) => {
                return Some(DurabilityError::DerivedForbidden {
                    kind: "flow_step".into(),
                });
            }
            // External events, explicit continuations, and explicit operation
            // lifecycle inputs may be reconstructed or derived.
            Input::ExternalEvent(_) | Input::Continuation(_) | Input::Operation(_) => {}
        }
    }

    None
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::input::*;
    use chrono::Utc;
    use meerkat_core::lifecycle::InputId;
    use meerkat_core::types::HandlingMode;

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
            typed_turn_appends: Vec::new(),
            turn_metadata: None,
        });
        assert!(durability_rejection_detail(&input).is_some());
    }

    #[test]
    fn prompt_durable_accepted() {
        let input = Input::Prompt(PromptInput {
            header: make_header(InputDurability::Durable, InputOrigin::Operator),
            text: "hi".into(),
            blocks: None,
            typed_turn_appends: Vec::new(),
            turn_metadata: None,
        });
        assert!(durability_rejection_detail(&input).is_none());
    }

    #[test]
    fn prompt_ephemeral_accepted() {
        let input = Input::Prompt(PromptInput {
            header: make_header(InputDurability::Ephemeral, InputOrigin::Operator),
            text: "hi".into(),
            blocks: None,
            typed_turn_appends: Vec::new(),
            turn_metadata: None,
        });
        assert!(durability_rejection_detail(&input).is_none());
    }

    #[test]
    fn peer_message_derived_rejected() {
        let input = Input::Peer(PeerInput {
            header: make_header(InputDurability::Derived, InputOrigin::System),
            convention: Some(PeerConvention::Message),
            body: "hi".into(),
            payload: None,
            blocks: None,
            handling_mode: None,
        });
        assert!(durability_rejection_detail(&input).is_some());
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
            payload: Some(serde_json::json!({"subject": "x"})),
            blocks: None,
            handling_mode: None,
        });
        assert!(durability_rejection_detail(&input).is_some());
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
            payload: Some(serde_json::json!({"ok": true})),
            blocks: None,
            handling_mode: None,
        });
        assert!(durability_rejection_detail(&input).is_some());
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
            payload: Some(serde_json::json!({"progress": "working"})),
            blocks: None,
            handling_mode: None,
        });
        assert!(durability_rejection_detail(&input).is_none());
    }

    #[test]
    fn flow_step_derived_rejected() {
        let input = Input::FlowStep(FlowStepInput {
            header: make_header(InputDurability::Derived, InputOrigin::System),
            step_id: "s1".into(),
            instructions: "do it".into(),
            blocks: None,
            turn_metadata: None,
        });
        assert!(durability_rejection_detail(&input).is_some());
    }

    #[test]
    fn external_event_derived_from_system_accepted() {
        let input = Input::ExternalEvent(ExternalEventInput {
            header: make_header(InputDurability::Derived, InputOrigin::System),
            event_type: "test".into(),
            payload: serde_json::json!({}),
            blocks: None,
            handling_mode: HandlingMode::Queue,
            render_metadata: None,
        });
        assert!(durability_rejection_detail(&input).is_none());
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
            blocks: None,
            handling_mode: HandlingMode::Queue,
            render_metadata: None,
        });
        assert!(durability_rejection_detail(&input).is_some());
    }

    #[test]
    fn operator_derived_rejected() {
        let input = Input::Continuation(ContinuationInput {
            header: make_header(InputDurability::Derived, InputOrigin::Operator),
            reason: "test".into(),
            handling_mode: meerkat_core::types::HandlingMode::Steer,
            request_id: None,
            flow_tool_overlay: None,
            context_append: None,
            turn_append: None,
        });
        assert!(durability_rejection_detail(&input).is_some());
    }

    #[test]
    fn operation_derived_from_system_accepted() {
        let input = Input::Operation(OperationInput {
            header: make_header(InputDurability::Derived, InputOrigin::System),
            operation_id: meerkat_core::ops::OperationId::new(),
            event: meerkat_core::ops::OpEvent::Cancelled {
                id: meerkat_core::ops::OperationId::new(),
            },
        });
        assert!(durability_rejection_detail(&input).is_none());
    }
}
