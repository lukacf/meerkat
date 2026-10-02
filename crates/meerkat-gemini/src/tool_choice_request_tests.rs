//! Typed tool choice lowered to Gemini's `toolConfig.functionCallingConfig`.
//! `Auto` keeps today's bytes; the config merges with the server-side tool
//! flag rather than replacing it.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use meerkat_core::lifecycle::run_primitive::OpaqueProviderBody;
use meerkat_core::{Message, ToolChoice, ToolDef, UserMessage};
use meerkat_llm_core::{LlmError, LlmRequest, ToolChoiceRefusal};
use serde_json::{Value, json};

use crate::GeminiClient;

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
        "gemini-3.1-pro-preview",
        vec![Message::User(UserMessage::text("hi".to_string()))],
    )
    .with_tools(vec![tool("lookup"), tool("deny_probe")])
    .with_tool_choice(choice)
}

fn body(request: &LlmRequest) -> Result<Value, LlmError> {
    GeminiClient::new("test-key".to_string()).build_request_body(request)
}

#[test]
fn auto_sends_no_tool_config() {
    let body = body(&request(ToolChoice::Auto)).unwrap();
    assert!(body.get("toolConfig").is_none(), "{body}");
}

#[test]
fn every_choice_lowers_to_function_calling_config() {
    for (choice, expected) in [
        (ToolChoice::Required, json!({"mode": "ANY"})),
        (ToolChoice::None, json!({"mode": "NONE"})),
        (
            ToolChoice::Tool {
                name: "deny_probe".into(),
            },
            json!({"mode": "ANY", "allowedFunctionNames": ["deny_probe"]}),
        ),
    ] {
        let body = body(&request(choice.clone())).unwrap();
        assert_eq!(
            body["toolConfig"],
            json!({"functionCallingConfig": expected}),
            "{choice:?}"
        );
    }
}

#[test]
fn function_calling_config_merges_with_the_server_side_tool_flag() {
    let request = request(ToolChoice::Tool {
        name: "lookup".into(),
    })
    .with_gemini_tag_merge(|tag| {
        tag.google_search = Some(OpaqueProviderBody::from_value(&json!({})));
    });
    let body = body(&request).unwrap();
    assert_eq!(body["toolConfig"]["includeServerSideToolInvocations"], true);
    assert_eq!(
        body["toolConfig"]["functionCallingConfig"],
        json!({"mode": "ANY", "allowedFunctionNames": ["lookup"]})
    );
}

#[test]
fn unoffered_tool_and_forcing_without_tools_are_refused_locally() {
    let reason = |result: Result<Value, LlmError>| match result {
        Err(LlmError::ToolChoiceUnsupported { reason, .. }) => reason,
        other => panic!("expected a typed tool-choice refusal, got {other:?}"),
    };
    assert_eq!(
        reason(body(&request(ToolChoice::Tool {
            name: "missing".into()
        }))),
        ToolChoiceRefusal::ToolNotOffered
    );
    let mut toolless = request(ToolChoice::Required);
    toolless.tools.clear();
    assert_eq!(reason(body(&toolless)), ToolChoiceRefusal::NoToolsOffered);
}
