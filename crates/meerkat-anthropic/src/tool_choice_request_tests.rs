//! Typed tool choice lowered to Anthropic's `tool_choice`. `Auto` keeps
//! today's bytes. A forced choice is refused locally under explicit thinking
//! and on models proven to reject it (Claude Opus 5.5: live 400 "not
//! supported for this model"; Claude Sonnet 5.5: documented 400); elsewhere
//! it is sent, and the provider's own rejection maps to the same typed error.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use meerkat_core::lifecycle::run_primitive::{AnthropicThinkingConfig, ProviderTag};
use meerkat_core::{Message, ToolChoice, ToolDef, UserMessage};
use meerkat_llm_core::{LlmError, LlmRequest, ToolChoiceRefusal};
use serde_json::{Value, json};

use crate::AnthropicClient;

/// Cataloged, thinking off unless configured: forced choices are accepted.
const FORCEABLE: &str = "claude-opus-4-8";

fn tool(name: &str) -> Arc<ToolDef> {
    Arc::new(ToolDef {
        name: name.into(),
        description: format!("{name} tool"),
        input_schema: json!({"type": "object", "properties": {}}),
        provenance: None,
    })
}

fn request(model: &str, choice: ToolChoice) -> LlmRequest {
    LlmRequest::new(model, vec![Message::User(UserMessage::text("hi"))])
        .with_tools(vec![tool("lookup"), tool("deny_probe")])
        .with_tool_choice(choice)
}

fn body(request: &LlmRequest) -> Result<Value, LlmError> {
    AnthropicClient::new("test-key".to_string())
        .unwrap()
        .build_request_body(request)
}

fn refusal(result: Result<Value, LlmError>) -> ToolChoiceRefusal {
    match result {
        Err(LlmError::ToolChoiceUnsupported { reason, .. }) => reason,
        other => panic!("expected a typed tool-choice refusal, got {other:?}"),
    }
}

fn forced() -> [ToolChoice; 2] {
    [
        ToolChoice::Required,
        ToolChoice::Tool {
            name: "deny_probe".into(),
        },
    ]
}

#[test]
fn auto_sends_no_tool_choice() {
    for model in [FORCEABLE, "claude-opus-5-5"] {
        let body = body(&request(model, ToolChoice::Auto)).unwrap();
        assert!(body.get("tool_choice").is_none(), "{model}: {body}");
    }
}

#[test]
fn every_choice_lowers_to_its_native_value_on_a_forceable_model() {
    for (choice, expected) in [
        (ToolChoice::Required, json!({"type": "any"})),
        (ToolChoice::None, json!({"type": "none"})),
        (
            ToolChoice::Tool {
                name: "deny_probe".into(),
            },
            json!({"type": "tool", "name": "deny_probe"}),
        ),
    ] {
        let body = body(&request(FORCEABLE, choice.clone())).unwrap();
        assert_eq!(body["tool_choice"], expected, "{choice:?}");
    }
}

#[test]
fn forced_choice_is_refused_only_on_models_proven_to_reject_it() {
    for model in ["claude-opus-5-5", "claude-sonnet-5-5"] {
        for choice in forced() {
            assert_eq!(
                refusal(body(&request(model, choice.clone()))),
                ToolChoiceRefusal::ModelDoesNotSupportForcedToolChoice,
                "{model} {choice:?}"
            );
        }
        // Forbidding tool calls stays available.
        let none = body(&request(model, ToolChoice::None)).unwrap();
        assert_eq!(none["tool_choice"], json!({"type": "none"}), "{model}");
    }
    // Live-accepted (claude-sonnet-5, claude-haiku-4-5-20251001) and unproven
    // cataloged models send the forced choice.
    for model in [
        "claude-sonnet-5",
        "claude-haiku-4-5-20251001",
        "claude-opus-5",
        "claude-fable-5-1",
    ] {
        let body = body(&request(model, ToolChoice::Required)).unwrap();
        assert_eq!(body["tool_choice"], json!({"type": "any"}), "{model}");
    }
}

#[test]
fn catalog_facts_match_the_live_probes() {
    use meerkat_core::Provider;
    let forced = |model: &str| {
        meerkat_models::capabilities_for(Provider::Anthropic, model)
            .map(|caps| caps.supports_forced_tool_choice)
    };
    assert_eq!(forced("claude-opus-5-5"), Some(false));
    // Documented: forced tool use is not supported on Sonnet 5.5.
    assert_eq!(forced("claude-sonnet-5-5"), Some(false));
    assert_eq!(forced("claude-haiku-4-5-20251001"), Some(true));
    // claude-sonnet-5 is now a catalog row; the live probe accepted forced
    // choices, and its model documentation agrees.
    assert_eq!(forced("claude-sonnet-5"), Some(true));
}

/// Anthropic's own 400 for an unsupported forced choice, on a request that
/// forced one, is the typed non-retryable refusal; other 400s are not.
#[test]
fn provider_rejection_of_a_forced_choice_maps_to_the_typed_refusal() {
    let rejection = r#"{"type":"error","error":{"type":"invalid_request_error","message":"tool_choice: type \"tool\" and \"any\" are not supported for this model."}}"#;
    let forced_request = request("claude-future-9", ToolChoice::Required);
    match crate::request_support::provider_forced_tool_choice_rejection(
        &forced_request,
        400,
        rejection,
    ) {
        Some(LlmError::ToolChoiceUnsupported { reason, choice, .. }) => {
            assert_eq!(
                reason,
                ToolChoiceRefusal::ModelDoesNotSupportForcedToolChoice
            );
            assert_eq!(choice, ToolChoice::Required);
        }
        other => panic!("expected the typed refusal, got {other:?}"),
    }
    let auto = request("claude-future-9", ToolChoice::Auto);
    assert!(
        crate::request_support::provider_forced_tool_choice_rejection(&auto, 400, rejection)
            .is_none(),
        "only a forced request maps"
    );
    let other = r#"{"type":"error","error":{"type":"invalid_request_error","message":"messages: at least one message is required"}}"#;
    assert!(
        crate::request_support::provider_forced_tool_choice_rejection(&forced_request, 400, other)
            .is_none()
    );
    assert!(
        crate::request_support::provider_forced_tool_choice_rejection(
            &forced_request,
            500,
            rejection
        )
        .is_none()
    );
}

/// The real stream path: a 400 from the provider on a forced request
/// surfaces as the typed refusal, not a generic invalid request.
#[tokio::test]
async fn stream_maps_the_provider_400_for_a_forced_choice() {
    use axum::{Router, http::StatusCode, response::IntoResponse, routing::post};
    use futures::StreamExt;
    use meerkat_llm_core::LlmClient;
    async fn reject() -> impl IntoResponse {
        (
            StatusCode::BAD_REQUEST,
            [("content-type", "application/json")],
            r#"{"type":"error","error":{"type":"invalid_request_error","message":"tool_choice: type \"tool\" and \"any\" are not supported for this model."}}"#,
        )
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, Router::new().route("/v1/messages", post(reject)))
            .await
            .unwrap();
    });
    let client = AnthropicClient::new("test-key".to_string())
        .unwrap()
        .with_base_url(format!("http://{addr}"));
    let request = request(
        "claude-future-9",
        ToolChoice::Tool {
            name: "deny_probe".into(),
        },
    );
    let mut stream = client.stream(&request);
    let mut typed = None;
    while let Some(event) = stream.next().await {
        match event {
            Err(error) => {
                typed = Some(error);
                break;
            }
            Ok(meerkat_llm_core::LlmEvent::Done {
                outcome: meerkat_llm_core::LlmDoneOutcome::Error { error },
            }) => {
                typed = Some(error);
                break;
            }
            Ok(_) => {}
        }
    }
    server.abort();
    match typed {
        Some(LlmError::ToolChoiceUnsupported { reason, .. }) => {
            assert_eq!(
                reason,
                ToolChoiceRefusal::ModelDoesNotSupportForcedToolChoice
            );
        }
        other => panic!("expected the typed refusal from the stream, got {other:?}"),
    }
}

#[test]
fn forced_choice_is_refused_under_explicit_thinking_never_switched_off() {
    for choice in forced() {
        let mut thinking = request(FORCEABLE, choice.clone());
        thinking = thinking.with_anthropic_tag_merge(|tag| {
            tag.thinking = Some(AnthropicThinkingConfig::Adaptive);
        });
        assert_eq!(
            refusal(body(&thinking)),
            ToolChoiceRefusal::ForcedToolWithThinking,
            "{choice:?}"
        );
        // Sonnet 4.5 accepts a thinking budget (Opus 4.8 refuses it first).
        let mut budget = request("claude-sonnet-4-5", choice);
        budget.provider_params = Some(ProviderTag::Anthropic(
            meerkat_core::lifecycle::run_primitive::AnthropicProviderTag {
                thinking_budget_tokens: Some(2048),
                ..Default::default()
            },
        ));
        assert_eq!(
            refusal(body(&budget)),
            ToolChoiceRefusal::ForcedToolWithThinking
        );
    }
}

#[test]
fn uncatalogued_models_pass_the_forced_choice_through() {
    let body = body(&request("claude-future-9", ToolChoice::Required)).unwrap();
    assert_eq!(body["tool_choice"], json!({"type": "any"}));
}

#[test]
fn unoffered_tool_and_forcing_without_tools_are_refused_locally() {
    assert_eq!(
        refusal(body(&request(
            FORCEABLE,
            ToolChoice::Tool {
                name: "missing".into()
            }
        ))),
        ToolChoiceRefusal::ToolNotOffered
    );
    let mut toolless = request(FORCEABLE, ToolChoice::Required);
    toolless.tools.clear();
    assert_eq!(refusal(body(&toolless)), ToolChoiceRefusal::NoToolsOffered);
}
