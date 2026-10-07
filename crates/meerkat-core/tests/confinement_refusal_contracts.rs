#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use meerkat_core::confinement::ConfinementRefusal;
use meerkat_core::error::{AgentError, ToolError};
use meerkat_core::ops::{
    ToolDispatchAdmissionSource, ToolDispatchSettlementFailure, ToolDispatchTerminalCause,
    ToolDispatchTerminalErrorKind,
};
use meerkat_core::{LiveBridgeEffectKind, LiveBridgeEffectOutcome};
use serde_json::json;

const CAUSES: [(ConfinementRefusal, &str); 5] = [
    (
        ConfinementRefusal::InvalidRequirement,
        "invalid_requirement",
    ),
    (ConfinementRefusal::InvalidLaunch, "invalid_launch"),
    (
        ConfinementRefusal::UnsupportedRequirement,
        "unsupported_requirement",
    ),
    (
        ConfinementRefusal::BackendUnavailable,
        "backend_unavailable",
    ),
    (ConfinementRefusal::PreparationFailed, "preparation_failed"),
];

#[test]
fn confinement_refusal_feedback_retains_all_exact_causes_and_settlement_companions() {
    let first = ToolDispatchSettlementFailure {
        admission_source: ToolDispatchAdmissionSource::ConfiguredGate,
        effect_kind: LiveBridgeEffectKind::ToolDispatch,
        physical_outcome: LiveBridgeEffectOutcome::Failed,
        failure_kind: ToolDispatchTerminalErrorKind::Unavailable,
    };
    let second = ToolDispatchSettlementFailure {
        admission_source: ToolDispatchAdmissionSource::ContextGate,
        failure_kind: ToolDispatchTerminalErrorKind::Other,
        ..first
    };
    for (refusal, wire_name) in CAUSES {
        assert_eq!(serde_json::to_value(refusal).unwrap(), json!(wire_name));
        assert_eq!(
            serde_json::from_value::<ConfinementRefusal>(json!(wire_name)).unwrap(),
            refusal,
        );
        let primary = ToolError::ConfinementRefused { refusal };
        for companions in [vec![], vec![first.clone(), second.clone()]] {
            let error = primary.clone().with_settlement_failures(companions.clone());
            assert_eq!(error.primary_error(), &primary);
            assert_eq!(error.error_code(), "confinement_refused");
            assert_eq!(error.structured_data(), Some(json!({"refusal": wire_name})));
            let mut expected = json!({
                "error": "confinement_refused",
                "message": primary.to_string(),
                "data": {"refusal": wire_name},
            });
            if !companions.is_empty() {
                expected["settlement_failures"] = json!(companions);
            }
            assert_eq!(error.to_error_payload(), expected);
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&error.to_transcript_content()).unwrap(),
                expected,
            );
            let AgentError::Tool { error: retained } = AgentError::tool(error.clone()) else {
                panic!("the existing tool carrier must retain the mechanical refusal");
            };
            assert_eq!(retained, error);
            let terminal = ToolDispatchTerminalCause::runtime_tool_error(&error);
            assert_eq!(
                terminal.kind(),
                ToolDispatchTerminalErrorKind::ConfinementRefused
            );
            assert_eq!(
                terminal,
                ToolDispatchTerminalCause::RuntimeToolError {
                    error: error.clone()
                }
            );
            assert_eq!(
                terminal.to_transcript_content(),
                error.to_transcript_content()
            );
            assert_eq!(
                error.settlement_failures().cloned().collect::<Vec<_>>(),
                companions
            );
            let (retained, failures) = error.into_primary_and_settlement_failures();
            assert_eq!(retained, primary);
            assert_eq!(failures, companions);
        }
    }
    assert_eq!(
        serde_json::to_value(ToolDispatchTerminalErrorKind::ConfinementRefused).unwrap(),
        json!("confinement_refused"),
    );
    assert_eq!(
        serde_json::from_value::<ToolDispatchTerminalErrorKind>(json!("confinement_refused"))
            .unwrap(),
        ToolDispatchTerminalErrorKind::ConfinementRefused,
    );
}

#[test]
fn confinement_refusal_wire_rejects_missing_or_invented_causes() {
    for invalid in [json!(null), json!("invented_cause"), json!({}), json!(3)] {
        assert!(serde_json::from_value::<ConfinementRefusal>(invalid).is_err());
    }
}

#[cfg(feature = "schema")]
#[test]
fn confinement_refusal_schema_preserves_the_five_exact_wire_causes() {
    let schema = serde_json::to_value(schemars::schema_for!(ConfinementRefusal)).unwrap();
    assert_eq!(schema["type"], "string");
    assert_eq!(
        schema["enum"],
        json!(CAUSES.map(|(_, wire_name)| wire_name))
    );
}

#[test]
fn confinement_terminal_class_appends_after_released_ordinals() {
    let released = [
        ToolDispatchTerminalErrorKind::NotFound,
        ToolDispatchTerminalErrorKind::Unavailable,
        ToolDispatchTerminalErrorKind::InvalidArguments,
        ToolDispatchTerminalErrorKind::ExecutionFailed,
        ToolDispatchTerminalErrorKind::Timeout,
        ToolDispatchTerminalErrorKind::AccessDenied,
        ToolDispatchTerminalErrorKind::AuthorizationRefused,
        ToolDispatchTerminalErrorKind::OperationObservationUnavailable,
        ToolDispatchTerminalErrorKind::OperationAuthorizationUnavailable,
        ToolDispatchTerminalErrorKind::PolicyDenied,
        ToolDispatchTerminalErrorKind::PolicyIndeterminate,
        ToolDispatchTerminalErrorKind::Other,
        ToolDispatchTerminalErrorKind::CallbackPending,
    ];
    for (ordinal, variant) in released.iter().enumerate() {
        assert_eq!(*variant as usize, ordinal);
    }
    assert_eq!(
        ToolDispatchTerminalErrorKind::ConfinementRefused as usize,
        released.len()
    );
    // Later additions append after every released ordinal.
    assert_eq!(
        ToolDispatchTerminalErrorKind::HookDenied as usize,
        released.len() + 1
    );
    assert_eq!(
        ToolDispatchTerminalErrorKind::OutcomeUncertain as usize,
        released.len() + 2
    );
}
