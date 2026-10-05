//! Typed tool choice lowered to the Responses and Chat Completions bodies.
//! `Auto` keeps today's bytes; every other choice reaches the native field,
//! and a choice the request or backend cannot honour is a typed refusal.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use meerkat_core::{Message, ToolChoice, ToolDef, UserMessage};
use meerkat_llm_core::{LlmError, LlmRequest, ToolChoiceRefusal};
use serde_json::{Value, json};

use crate::OpenAiClient;
use crate::client_compatible::{OpenAiCompatibleClient, OpenAiCompatibleMode};

fn tool(name: &str) -> Arc<ToolDef> {
    Arc::new(ToolDef {
        name: name.into(),
        description: format!("{name} tool"),
        input_schema: json!({"type": "object", "properties": {}}),
        provenance: None,
    })
}

fn request(choice: ToolChoice) -> LlmRequest {
    LlmRequest::new(
        "gpt-5.2",
        vec![Message::User(UserMessage::text("hi".to_string()))],
    )
    .with_tools(vec![tool("lookup"), tool("deny_probe")])
    .with_tool_choice(choice)
}

fn responses(request: &LlmRequest) -> Result<Value, LlmError> {
    OpenAiClient::new("test-key".to_string()).build_request_body(request)
}

fn chat(request: &LlmRequest) -> Result<Value, LlmError> {
    OpenAiCompatibleClient::new(
        OpenAiCompatibleMode::ChatCompletions,
        "qwen3".to_string(),
        "http://localhost:8000/v1".to_string(),
        None,
        true,
        false,
        false,
    )
    .build_chat_completions_body(request)
}

fn refusal(result: Result<Value, LlmError>) -> ToolChoiceRefusal {
    match result {
        Err(LlmError::ToolChoiceUnsupported { reason, .. }) => reason,
        other => panic!("expected a typed tool-choice refusal, got {other:?}"),
    }
}

#[test]
fn auto_keeps_the_responses_and_chat_bodies_byte_identical() {
    let mut legacy = request(ToolChoice::Auto);
    // A request decoded from today's serialized shape has no tool_choice key.
    let encoded = serde_json::to_value(&legacy).unwrap();
    assert!(encoded.get("tool_choice").is_none(), "{encoded}");
    legacy = serde_json::from_value(encoded).unwrap();
    assert_eq!(legacy.tool_choice, ToolChoice::Auto);

    let body = responses(&legacy).unwrap();
    assert!(body.get("tool_choice").is_none(), "{body}");
    let body = chat(&legacy).unwrap();
    assert!(body.get("tool_choice").is_none(), "{body}");
}

#[test]
fn responses_lowers_every_choice_to_its_native_value() {
    for (choice, expected) in [
        (ToolChoice::Required, json!("required")),
        (ToolChoice::None, json!("none")),
        (
            ToolChoice::Tool {
                name: "deny_probe".into(),
            },
            json!({"type": "function", "name": "deny_probe"}),
        ),
    ] {
        let body = responses(&request(choice.clone())).unwrap();
        assert_eq!(body["tool_choice"], expected, "{choice:?}");
        assert_eq!(
            body["tools"].as_array().unwrap().len(),
            2,
            "tools unchanged"
        );
    }
}

#[test]
fn chat_completions_lowers_every_choice_to_its_native_value() {
    for (choice, expected) in [
        (ToolChoice::Required, json!("required")),
        (ToolChoice::None, json!("none")),
        (
            ToolChoice::Tool {
                name: "deny_probe".into(),
            },
            json!({"type": "function", "function": {"name": "deny_probe"}}),
        ),
    ] {
        let body = chat(&request(choice.clone())).unwrap();
        assert_eq!(body["tool_choice"], expected, "{choice:?}");
    }
}

#[test]
fn unoffered_tool_and_forcing_without_tools_are_refused_locally() {
    let unoffered = request(ToolChoice::Tool {
        name: "not_offered".into(),
    });
    assert_eq!(
        refusal(responses(&unoffered)),
        ToolChoiceRefusal::ToolNotOffered
    );
    assert_eq!(refusal(chat(&unoffered)), ToolChoiceRefusal::ToolNotOffered);
    let mut toolless = request(ToolChoice::Required);
    toolless.tools.clear();
    assert_eq!(
        refusal(responses(&toolless)),
        ToolChoiceRefusal::NoToolsOffered
    );
    assert_eq!(refusal(chat(&toolless)), ToolChoiceRefusal::NoToolsOffered);
    // `None` needs no tools: it only forbids calling them.
    let mut none = request(ToolChoice::None);
    none.tools.clear();
    assert_eq!(responses(&none).unwrap()["tool_choice"], "none");
}

#[test]
fn chatgpt_backend_keeps_its_fixed_auto_and_refuses_other_choices() {
    let client = OpenAiClient::new("test-key".to_string()).with_chatgpt_backend_wire();
    let body = client
        .build_request_body(&request(ToolChoice::Auto))
        .unwrap();
    assert_eq!(
        body["tool_choice"], "auto",
        "the backend's fixed value is unchanged"
    );
    for choice in [
        ToolChoice::Required,
        ToolChoice::None,
        ToolChoice::Tool {
            name: "lookup".into(),
        },
    ] {
        assert_eq!(
            refusal(client.build_request_body(&request(choice))),
            ToolChoiceRefusal::BackendFixesToolChoice
        );
    }
}

#[test]
fn refusal_projects_a_non_retryable_typed_failure() {
    let error = responses(&request(ToolChoice::Tool {
        name: "not_offered".into(),
    }))
    .unwrap_err();
    assert!(!error.is_retryable());
    let text = format!("{:?}", error.failure_reason());
    assert!(
        text.contains(meerkat_llm_core::TOOL_CHOICE_UNSUPPORTED_DETAILS_CLASS),
        "{text}"
    );
    assert!(text.contains("tool_not_offered"), "{text}");
}
