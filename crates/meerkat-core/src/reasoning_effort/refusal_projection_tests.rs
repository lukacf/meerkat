//! A refused `reasoning_effort` level on Core's error path: the typed
//! agent error, its report and run-failure reason, its class and turn
//! failure source, the session error's structured data and its reader, and
//! the executor boundary. Synthetic model ids only: core holds no provider
//! model names.

use serde_json::json;

use crate::error::AgentError;
use crate::event::{AgentErrorClass, AgentErrorReason, AgentErrorReport};
use crate::lifecycle::core_executor::{
    CoreApplyFailureCause, CoreApplyFailureCauseKind, CoreExecutorError,
};
use crate::service::SessionError;
use crate::turn_execution_authority::TurnFailureSourceKind;
use crate::{ReasoningEffortRefusal, ReasoningEffortRefusalReason};

/// The eight closed reasons, in their documented order.
const REASONS: [ReasoningEffortRefusalReason; 8] = [
    ReasoningEffortRefusalReason::UnsupportedLevel,
    ReasoningEffortRefusalReason::SupportedLevelsUnknown,
    ReasoningEffortRefusalReason::SupportedLevelsUnavailable,
    ReasoningEffortRefusalReason::ConflictingProviderEffort,
    ReasoningEffortRefusalReason::ConflictingThinkingBudget,
    ReasoningEffortRefusalReason::ConflictingThinkingType,
    ReasoningEffortRefusalReason::ConflictingReasoningBody,
    ReasoningEffortRefusalReason::ConflictingReasoningMode,
];

/// The documented first example: a catalogued level the model does not
/// accept on its direct route.
fn unsupported_level_document() -> serde_json::Value {
    json!({
        "requested": "max", "provider": "anthropic", "model": "test-anthropic-default",
        "route": {"backend": "anthropic_api", "api": "messages"},
        "reason": "unsupported_level", "supported": ["low", "medium", "high"],
        "facts": "catalog",
    })
}

/// A refusal that names no route: the client reported none.
fn unrouted_document() -> serde_json::Value {
    json!({
        "requested": "high", "provider": "other", "model": "test-custom-model",
        "route": null,
        "reason": "supported_levels_unknown", "supported": null, "facts": null,
    })
}

fn refusal_of(document: &serde_json::Value) -> ReasoningEffortRefusal {
    serde_json::from_value(document.clone()).expect("a refusal document reads")
}

fn refusal_with_reason(reason: ReasoningEffortRefusalReason) -> ReasoningEffortRefusal {
    ReasoningEffortRefusal {
        reason,
        ..refusal_of(&unsupported_level_document())
    }
}

fn refused(refusal: &ReasoningEffortRefusal) -> AgentError {
    AgentError::ReasoningEffortRefused(Box::new(refusal.clone()))
}

/// The transient class has one typed owner: true for the unavailable
/// account list only. `supported_levels_unknown` holds until a
/// configuration changes, like the other six.
#[test]
fn only_the_unavailable_reason_is_transient() {
    for reason in REASONS {
        assert_eq!(
            reason.is_transient(),
            reason == ReasoningEffortRefusalReason::SupportedLevelsUnavailable,
            "{reason:?}"
        );
    }
}

/// The error data carries the transient class beside the refusal, written
/// from the typed classifier: `true` for the unavailable reason, `false`
/// for the other seven.
#[test]
fn the_error_data_marks_only_the_unavailable_reason_retryable() {
    for reason in REASONS {
        let refusal = refusal_with_reason(reason);
        let data = SessionError::Agent(refused(&refusal))
            .structured_data()
            .expect("a refused level has structured data");
        assert_eq!(
            data["retryable"],
            json!(reason == ReasoningEffortRefusalReason::SupportedLevelsUnavailable),
            "{reason:?}"
        );
    }
}

/// The agent error's text is the refusal's own text, with no prefix.
#[test]
fn the_agent_error_message_is_the_refusals_text() {
    let refusal = refusal_of(&unsupported_level_document());
    assert_eq!(refused(&refusal).to_string(), refusal.to_string());
    assert_eq!(
        refused(&refusal).to_string(),
        "reasoning effort 'max' is not supported by anthropic model 'test-anthropic-default' \
         on anthropic_api/messages; supported: low, medium, high"
    );
}

/// The refusal is the request's own configuration: class `config`, turn
/// failure source `ConfigError`, never retried, never a fallback trigger.
/// That holds for every reason, the transient one included: Meerkat does
/// not retry it, the caller may.
#[test]
fn the_refusal_is_config_class_and_never_a_fallback_trigger() {
    for reason in REASONS {
        let error = refused(&refusal_with_reason(reason));
        assert_eq!(AgentErrorClass::from(&error), AgentErrorClass::Config);
        assert_eq!(
            TurnFailureSourceKind::from_agent_error(&error).expect("a turn failure source"),
            TurnFailureSourceKind::ConfigError
        );
        assert!(!error.is_recoverable(), "{reason:?} is never retried");
        assert!(!error.is_graceful(), "{reason:?} is a failure");
        assert!(
            crate::model_fallback::model_fallback_trigger(&error).is_none(),
            "{reason:?} never triggers a model fallback"
        );
    }
}

/// A run refused for its level fails with the typed reason: the agent
/// error's report carries it, and its wire form is the refusal under the
/// `reasoning_effort_refused` tag, which reads back to the same reason. A
/// refusal that names no route keeps `"route": null` both ways.
#[test]
fn a_refused_level_is_a_typed_run_failure_reason_on_the_wire() {
    for document in [unsupported_level_document(), unrouted_document()] {
        let refusal = refusal_of(&document);
        let error = refused(&refusal);
        let reason = AgentErrorReason::ReasoningEffortRefused {
            refusal: refusal.clone(),
        };
        let report = AgentErrorReport::from_agent_error(&error);
        assert_eq!(report.class, AgentErrorClass::Config);
        assert_eq!(report.reason, Some(reason.clone()));
        assert_eq!(report.message, refusal.to_string());

        let wire = serde_json::to_value(&reason).expect("serialize");
        assert_eq!(
            wire,
            json!({"reason_type": "reasoning_effort_refused", "refusal": document})
        );
        let decoded: AgentErrorReason = serde_json::from_value(wire).expect("decode");
        assert_eq!(decoded, reason);
    }
}

/// The session error's structured data is the documented object, and the
/// reader gives the same typed refusal back; a refusal that names no route
/// round-trips with `"route": null`. Data of another kind, or a refusal
/// this binary cannot read, fails closed.
#[test]
fn a_refused_level_reads_back_off_its_structured_data() {
    for document in [unsupported_level_document(), unrouted_document()] {
        let refusal = refusal_of(&document);
        let error = SessionError::Agent(refused(&refusal));
        assert_eq!(error.code(), "AGENT_ERROR");
        let data = error.structured_data().expect("structured data");
        assert_eq!(
            data,
            json!({
                "kind": "reasoning_effort_refused",
                "retryable": false,
                "reasoning_effort_refusal": document,
            })
        );
        assert_eq!(
            SessionError::reasoning_effort_refusal_from_data(&data),
            Some(refusal)
        );
    }

    // The reader reads the refusal and ignores `retryable`.
    let without_the_advisory_key = json!({
        "kind": SessionError::REASONING_EFFORT_REFUSED_KIND,
        "reasoning_effort_refusal": unsupported_level_document(),
    });
    assert_eq!(
        SessionError::reasoning_effort_refusal_from_data(&without_the_advisory_key),
        Some(refusal_of(&unsupported_level_document()))
    );

    for other in [
        json!({}),
        json!({
            "kind": SessionError::TOOL_GRANT_GATE_REFUSED_KIND,
            "reasoning_effort_refusal": unsupported_level_document(),
        }),
        json!({"kind": SessionError::REASONING_EFFORT_REFUSED_KIND}),
        json!({
            "kind": SessionError::REASONING_EFFORT_REFUSED_KIND,
            "reasoning_effort_refusal": {"reason": "a_later_reason"},
        }),
    ] {
        assert_eq!(
            SessionError::reasoning_effort_refusal_from_data(&other),
            None,
            "{other}"
        );
    }
    // The grant-gate reader does not read this kind either.
    let data = SessionError::Agent(refused(&refusal_of(&unsupported_level_document())))
        .structured_data()
        .expect("structured data");
    assert_eq!(SessionError::tool_grant_gate_refusal_from_data(&data), None);
}

/// A level refused inside a session's build or turn keeps its type at the
/// executor boundary, as the apply failure's reason the failed turn's
/// error carries.
#[test]
fn a_refused_level_keeps_its_typed_reason_at_the_executor_boundary() {
    let refusal = refusal_of(&unsupported_level_document());
    let err =
        CoreExecutorError::apply_failed_from_session_error(SessionError::Agent(refused(&refusal)));
    let cause = err.apply_failure_cause();
    assert_eq!(cause.kind, CoreApplyFailureCauseKind::RuntimeTurn);
    assert_eq!(cause.message(), refusal.to_string());
    assert_eq!(
        cause.reason(),
        Some(&AgentErrorReason::ReasoningEffortRefused {
            refusal: refusal.clone(),
        })
    );
    assert_eq!(
        CoreApplyFailureCause::from_agent_error(&refused(&refusal)).reason(),
        Some(&AgentErrorReason::ReasoningEffortRefused { refusal })
    );
}
