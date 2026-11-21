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

/// An instance's known cause can narrow the historical reason-level class.
/// Unknown causes and absolute eligibility times do not invent a new class.
#[test]
fn refusal_diagnostics_preserve_projection_and_narrow_retryability() {
    assert_eq!(
        crate::REASONING_CAUSE_NEW_SIGN_IN_REQUIRED,
        "new_sign_in_required"
    );
    assert_eq!(
        crate::REASONING_CAUSE_TOKEN_EXCHANGE_UNAVAILABLE,
        "token_exchange_unavailable"
    );
    for reason in REASONS {
        for cause in [
            None,
            Some(crate::REASONING_CAUSE_TOKEN_EXCHANGE_UNAVAILABLE),
            Some(crate::REASONING_CAUSE_NEW_SIGN_IN_REQUIRED),
            Some("future_unavailability_cause"),
            Some(""),
        ] {
            for not_before in [None, Some("2020-01-02T03:04:05Z")] {
                let mut document = serde_json::to_value(refusal_with_reason(reason))
                    .expect("serialize the baseline refusal");
                if let Some(cause) = cause {
                    document["cause"] = json!(cause);
                }
                if let Some(not_before) = not_before {
                    document["not_before"] = json!(not_before);
                }
                let refusal = refusal_of(&document);
                let data = SessionError::Agent(refused(&refusal))
                    .structured_data()
                    .expect("a refused level has structured data");
                let expected_retryable = reason
                    == ReasoningEffortRefusalReason::SupportedLevelsUnavailable
                    && cause != Some(crate::REASONING_CAUSE_NEW_SIGN_IN_REQUIRED);
                assert_eq!(
                    data,
                    json!({
                        "kind": SessionError::REASONING_EFFORT_REFUSED_KIND,
                        "retryable": expected_retryable,
                        "reasoning_effort_refusal": document,
                    }),
                    "{reason:?}, cause={cause:?}, not_before={not_before:?}"
                );
                assert_eq!(
                    SessionError::reasoning_effort_refusal_from_data(&data),
                    Some(refusal)
                );
                assert_eq!(
                    reason.is_transient(),
                    reason == ReasoningEffortRefusalReason::SupportedLevelsUnavailable,
                    "the closed reason's historical class remains unchanged"
                );
            }
        }
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
            refusal: Box::new(refusal.clone()),
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
            refusal: Box::new(refusal.clone()),
        })
    );
    assert_eq!(
        CoreApplyFailureCause::from_agent_error(&refused(&refusal)).reason(),
        Some(&AgentErrorReason::ReasoningEffortRefused {
            refusal: Box::new(refusal),
        })
    );
}

/// The shared SDK expectations are checked against the typed owner: the
/// typed cases against actual producer serialization, the compatibility
/// cases and the times against the typed reader. The JSON is expected data,
/// not a recorded execution log.
#[test]
fn refusal_diagnostics_match_the_shared_sdk_cases() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("diagnostics_cases.json")).expect("shared cases");
    assert_eq!(
        fixture["known_causes"]["new_sign_in_required"],
        crate::REASONING_CAUSE_NEW_SIGN_IN_REQUIRED
    );
    assert_eq!(
        fixture["known_causes"]["token_exchange_unavailable"],
        crate::REASONING_CAUSE_TOKEN_EXCHANGE_UNAVAILABLE
    );
    for case in fixture["typed_cases"].as_array().expect("typed cases") {
        let refusal = refusal_of(&case["data"]["reasoning_effort_refusal"]);
        let data = SessionError::Agent(refused(&refusal))
            .structured_data()
            .expect("typed refusal data");
        assert_eq!(data, case["data"], "{}", case["name"]);
        let event = crate::AgentEvent::RunFailed {
            session_id: crate::SessionId::new(),
            identity: Default::default(),
            error_report: AgentErrorReport::from_agent_error(&refused(&refusal)),
            terminal_cause_kind: None,
        };
        let wire = serde_json::to_value(event).expect("serialize actual RunFailed");
        assert_eq!(wire["type"], "run_failed");
        assert_eq!(wire["error_report"]["class"], "config");
        assert_eq!(
            wire["error_report"]["reason"], case["event_reason"],
            "{}",
            case["name"]
        );
        assert_eq!(
            serde_json::Value::Bool(refusal.is_retryable()),
            case["transient"],
            "{}",
            case["name"]
        );
    }
    // The compatibility cases are documents no typed producer writes (an
    // outer mark that differs, a null member). The typed reader takes each,
    // and its own class is the one the SDKs are held to.
    for case in fixture["compatibility_cases"]
        .as_array()
        .expect("compatibility cases")
    {
        let refusal = SessionError::reasoning_effort_refusal_from_data(&case["data"])
            .unwrap_or_else(|| panic!("the typed reader takes {}", case["name"]));
        assert_eq!(
            serde_json::Value::Bool(refusal.is_retryable()),
            case["transient"],
            "{}",
            case["name"]
        );
    }
    // One rule for `not_before` in every reader: the listed timestamps are
    // taken, and a refusal that carries any other string is not read.
    let document = &fixture["typed_cases"][0]["data"];
    for (member, taken) in [("readable", true), ("unreadable", false)] {
        let times = fixture["times"][member].as_array().expect("times");
        assert!(!times.is_empty(), "{member}");
        for time in times {
            let mut data = document.clone();
            data["reasoning_effort_refusal"]["not_before"] = time.clone();
            assert_eq!(
                SessionError::reasoning_effort_refusal_from_data(&data).is_some(),
                taken,
                "{member}: {time}"
            );
        }
    }
}
