//! Typed tool choice lowered to Anthropic's `tool_choice`. `Auto` keeps
//! today's bytes. A forced choice is refused where Anthropic would answer
//! 400: under explicit thinking, and on cataloged models whose thinking
//! cannot be disabled (Opus 5.5 documents the 400 for `any`/`tool`).
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
fn forced_choice_is_refused_on_models_whose_thinking_cannot_be_disabled() {
    for model in [
        "claude-opus-5-5",
        "claude-opus-5",
        "claude-fable-5",
        "claude-fable-5-1",
        "claude-sonnet-5-5",
    ] {
        for choice in forced() {
            assert_eq!(
                refusal(body(&request(model, choice.clone()))),
                ToolChoiceRefusal::ForcedToolWithThinking,
                "{model} {choice:?}"
            );
        }
        // Forbidding tool calls stays available everywhere.
        let none = body(&request(model, ToolChoice::None)).unwrap();
        assert_eq!(none["tool_choice"], json!({"type": "none"}), "{model}");
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
