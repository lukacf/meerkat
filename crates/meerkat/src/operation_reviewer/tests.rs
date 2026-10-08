use super::*;
use meerkat_core::Provider;
use meerkat_core::approval::review::MAX_REVIEW_CONTEXT_BYTES;

fn identity(model: &str) -> meerkat_core::SessionLlmIdentity {
    meerkat_core::SessionLlmIdentity {
        model: model.to_owned(),
        provider: Provider::OpenAI,
        self_hosted_server_id: None,
        provider_params: None,
        auth_binding: None,
    }
}

#[test]
fn model_config_requires_an_explicit_route_and_nonzero_output_budget() {
    for model in ["", " ", "\n\t"] {
        assert!(matches!(
            ModelReviewerConfig::new(identity(model), 4096),
            Err(ModelReviewerConfigError::EmptyModel)
        ));
    }
    assert!(matches!(
        ModelReviewerConfig::new(identity("review-model"), 0),
        Err(ModelReviewerConfigError::EmptyOutputBudget)
    ));
    let config =
        ModelReviewerConfig::new(identity("review-model"), 4096).expect("explicit host route");
    assert_eq!(config.model(), "review-model");
    assert_eq!(config.identity().provider, Provider::OpenAI);
    assert_eq!(config.max_output_tokens(), 4096);

    let mut unsupported = identity("review-model");
    unsupported.provider_params = Some(ProviderParamsOverride::default());
    assert!(matches!(
        ModelReviewerConfig::new(unsupported, 4096),
        Err(ModelReviewerConfigError::UnsupportedProviderParameters)
    ));
}

#[test]
fn context_material_is_bounded_without_truncation_and_debug_redacted() {
    assert!(ReviewContextMaterial::from_text(" ".to_owned()).is_err());
    assert!(ReviewContextMaterial::from_text("x".repeat(MAX_REVIEW_CONTEXT_BYTES + 1)).is_err());
    let exact = "x".repeat(MAX_REVIEW_CONTEXT_BYTES);
    let material = ReviewContextMaterial::from_text(exact.clone()).expect("exact byte bound");
    assert_eq!(material.as_str(), exact);
    let material = ReviewContextMaterial::from_text("private original content".to_owned()).unwrap();
    assert!(!format!("{material:?}").contains("private original content"));
}

#[test]
fn bounded_response_accepts_only_the_three_exact_verdicts() {
    for (text, expected) in [
        (r#"{"verdict":"allow"}"#, ReviewVerdict::Allow),
        (r#"{"verdict":"deny"}"#, ReviewVerdict::Deny),
        (r#"{"verdict":"escalate"}"#, ReviewVerdict::Escalate),
    ] {
        let mut response = ReviewResponse::default();
        for part in text.as_bytes().chunks(3) {
            response
                .push_text(std::str::from_utf8(part).unwrap())
                .unwrap();
        }
        assert_eq!(response.finish(StopReason::EndTurn), Ok(expected));
    }
}

#[test]
fn malformed_ambiguous_or_extra_output_never_becomes_allow() {
    for text in [
        "",
        "allow",
        r#"{"verdict":"ALLOW"}"#,
        r#"{"verdict":"approve"}"#,
        r#"{"verdict":true}"#,
        r#"{"verdict":null}"#,
        r#"{"verdict":"allow","verdict":"deny"}"#,
        r#"{"verdict":"deny","verdict":"allow"}"#,
        r#"{"verdict":"allow","rationale":"private"}"#,
        r#"{"verdict":"allow","actor":"admin"}"#,
        r#"[{"verdict":"allow"}]"#,
        r#"{"verdict":"allow"} {"verdict":"allow"}"#,
        "```json\n{\"verdict\":\"allow\"}\n```",
    ] {
        let mut response = ReviewResponse::default();
        response.push_text(text).unwrap();
        assert_eq!(
            response.finish(StopReason::EndTurn),
            Err(ReviewResponseError::InvalidVerdict)
        );
    }
}

#[test]
fn syntactically_complete_allow_does_not_hide_truncation_or_cancellation() {
    for reason in [
        StopReason::MaxTokens,
        StopReason::ToolUse,
        StopReason::StopSequence,
        StopReason::ContentFilter,
        StopReason::Cancelled,
    ] {
        let mut response = ReviewResponse::default();
        response.push_text(r#"{"verdict":"allow"}"#).unwrap();
        assert_eq!(
            response.finish(reason),
            Err(ReviewResponseError::Incomplete)
        );
    }
}

#[test]
fn response_byte_limit_is_sticky_and_debug_does_not_expose_output() {
    let mut response = ReviewResponse::default();
    response
        .push_text(&" ".repeat(MAX_REVIEW_RESPONSE_BYTES))
        .unwrap();
    assert_eq!(response.push_text("x"), Err(ReviewResponseError::TooLarge));
    assert_eq!(
        response.push_text(r#"{"verdict":"allow"}"#),
        Err(ReviewResponseError::TooLarge)
    );
    assert_eq!(
        response.finish(StopReason::EndTurn),
        Err(ReviewResponseError::TooLarge)
    );

    let mut response = ReviewResponse::default();
    response.push_text("private reviewer output").unwrap();
    assert!(!format!("{response:?}").contains("private reviewer output"));
}

#[test]
fn unexpected_effect_output_cannot_be_ignored_before_a_valid_verdict() {
    let mut response = ReviewResponse::default();
    response.reject_non_verdict_output();
    assert_eq!(
        response.push_text(r#"{"verdict":"allow"}"#),
        Err(ReviewResponseError::UnexpectedOutput)
    );
    assert_eq!(
        response.finish(StopReason::EndTurn),
        Err(ReviewResponseError::UnexpectedOutput)
    );
}

fn text_event(text: &str) -> LlmEvent {
    LlmEvent::TextDelta {
        delta: text.to_owned(),
        meta: None,
    }
}

fn end_turn() -> LlmEvent {
    LlmEvent::Done {
        outcome: LlmDoneOutcome::Success {
            stop_reason: StopReason::EndTurn,
        },
    }
}

async fn parse_events(events: Vec<LlmEvent>) -> Result<ReviewVerdict, ReviewResponseError> {
    collect_review_response(Box::pin(futures::stream::iter(events.into_iter().map(Ok)))).await
}

#[tokio::test]
async fn canonical_final_output_replaces_provisional_text_and_discards_reasoning() {
    assert_eq!(
        parse_events(vec![
            LlmEvent::ReasoningDelta {
                delta: "private reasoning".repeat(1024)
            },
            text_event(r#"{"verdict":"deny"}"#),
            LlmEvent::AssistantOutput {
                blocks: vec![
                    meerkat_core::AssistantBlock::Reasoning {
                        text: "private final reasoning".to_owned(),
                        meta: None,
                    },
                    meerkat_core::AssistantBlock::Text {
                        text: r#"{"verdict":"allow"}"#.to_owned(),
                        meta: None,
                    },
                ]
            },
            end_turn(),
        ])
        .await,
        Ok(ReviewVerdict::Allow)
    );
}

#[tokio::test]
async fn valid_text_requires_terminal_success_and_does_not_hide_later_failures() {
    assert_eq!(
        parse_events(vec![text_event(r#"{"verdict":"allow"}"#)]).await,
        Err(ReviewResponseError::Incomplete)
    );
    assert_eq!(
        parse_events(vec![
            text_event(r#"{"verdict":"allow"}"#),
            end_turn(),
            LlmEvent::OperationObservationFailed {
                operation_id: meerkat_core::OperationId::new(),
                phase: meerkat_core::authorization::OperationObservationPhase::Outcome,
            },
        ])
        .await,
        Err(ReviewResponseError::ObservationUnavailable)
    );
    assert_eq!(
        parse_events(vec![
            text_event(r#"{"verdict":"allow"}"#),
            LlmEvent::Done {
                outcome: LlmDoneOutcome::Error {
                    error: meerkat_llm_core::LlmError::InvalidRequest {
                        message: "private error".to_owned()
                    },
                }
            },
        ])
        .await,
        Err(ReviewResponseError::ModelUnavailable)
    );
}

#[tokio::test]
async fn all_model_observation_failure_carriers_remain_infrastructure_errors() {
    let error = meerkat_llm_core::LlmError::OperationObservationUnavailable;
    for events in [
        vec![Err(error.clone())],
        vec![Ok(LlmEvent::Done {
            outcome: LlmDoneOutcome::Error { error },
        })],
        vec![
            Ok(text_event(r#"{"verdict":"allow"}"#)),
            Ok(end_turn()),
            Ok(LlmEvent::OperationObservationFailed {
                operation_id: meerkat_core::OperationId::new(),
                phase: meerkat_core::authorization::OperationObservationPhase::Outcome,
            }),
        ],
    ] {
        let result = collect_review_response(Box::pin(futures::stream::iter(events)))
            .await
            .map_err(ReviewerFailure::from);
        assert_eq!(
            result,
            Err(ReviewerFailure::ObservationUnavailable(
                meerkat_core::authorization::OperationObservationError,
            ))
        );
    }
}

#[tokio::test]
async fn ordinary_model_and_invalid_output_failures_remain_review_unavailable() {
    let error = meerkat_llm_core::LlmError::InvalidRequest {
        message: "private provider error".into(),
    };
    for events in [
        vec![Err(error.clone())],
        vec![Ok(LlmEvent::Done {
            outcome: LlmDoneOutcome::Error { error },
        })],
        vec![Ok(text_event("not a verdict")), Ok(end_turn())],
    ] {
        let result = collect_review_response(Box::pin(futures::stream::iter(events)))
            .await
            .map_err(ReviewerFailure::from);
        assert_eq!(result, Err(ReviewerFailure::Unavailable));
    }
}

#[tokio::test]
async fn terminal_text_cannot_erase_an_unexpected_tool_output() {
    assert_eq!(
        parse_events(vec![
            LlmEvent::ToolCallDelta {
                id: "unexpected-call".to_owned(),
                name: Some("write".to_owned()),
                args_delta: "{".to_owned(),
            },
            LlmEvent::AssistantOutput {
                blocks: vec![meerkat_core::AssistantBlock::Text {
                    text: r#"{"verdict":"allow"}"#.to_owned(),
                    meta: None,
                }]
            },
            end_turn(),
        ])
        .await,
        Err(ReviewResponseError::UnexpectedOutput)
    );
}

#[tokio::test]
async fn semantic_content_after_done_cannot_replace_a_completed_verdict() {
    for late in [
        text_event(r#"{"verdict":"allow"}"#),
        LlmEvent::AssistantOutput {
            blocks: vec![meerkat_core::AssistantBlock::Text {
                text: r#"{"verdict":"allow"}"#.to_owned(),
                meta: None,
            }],
        },
    ] {
        assert_eq!(
            parse_events(vec![text_event(r#"{"verdict":"deny"}"#), end_turn(), late,]).await,
            Err(ReviewResponseError::UnexpectedOutput)
        );
    }
}

#[cfg(all(feature = "openai", not(target_arch = "wasm32")))]
mod governed;
