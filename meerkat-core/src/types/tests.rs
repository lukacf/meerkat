#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! RCT tests for core types
//!
//! These tests verify the serialization/deserialization contracts.

use super::*;
use serde_json::json;

fn schema_for<T: schemars::JsonSchema>() -> Value {
    let schema = schemars::schema_for!(T);
    let mut value = serde_json::to_value(&schema).unwrap_or(Value::Null);

    // Ensure object schemas always have `properties` and `required` keys.
    if let Value::Object(ref mut obj) = value {
        if obj.get("type").and_then(Value::as_str) == Some("object") {
            obj.entry("properties".to_string())
                .or_insert_with(|| Value::Object(serde_json::Map::new()));
            obj.entry("required".to_string())
                .or_insert_with(|| Value::Array(Vec::new()));
        }
    }

    value
}

#[test]
fn test_session_id_encoding() {
    // UUID v7 format test
    let id = SessionId::new();
    let json = serde_json::to_string(&id).unwrap();

    // Should be a valid UUID string in JSON
    assert!(json.starts_with('"'));
    assert!(json.ends_with('"'));

    // Should roundtrip
    let parsed: SessionId = serde_json::from_str(&json).unwrap();
    assert_eq!(id, parsed);

    // Parse from string should work
    let id_str = id.0.to_string();
    let parsed_from_str = SessionId::parse(&id_str).unwrap();
    assert_eq!(id, parsed_from_str);
}

#[test]
fn test_message_json_schema() {
    // System message
    let system = Message::System(SystemMessage {
        content: "You are a helpful assistant.".to_string(),
    });
    let json = serde_json::to_value(&system).unwrap();
    assert_eq!(json["role"], "system");
    assert_eq!(json["content"], "You are a helpful assistant.");

    // User message
    let user = Message::User(UserMessage {
        content: "Hello!".to_string(),
    });
    let json = serde_json::to_value(&user).unwrap();
    assert_eq!(json["role"], "user");
    assert_eq!(json["content"], "Hello!");

    // Assistant message
    let assistant = Message::Assistant(AssistantMessage {
        content: "Hi there!".to_string(),
        tool_calls: vec![],
        stop_reason: StopReason::EndTurn,
        usage: Usage::default(),
    });
    let json = serde_json::to_value(&assistant).unwrap();
    assert_eq!(json["role"], "assistant");
    assert_eq!(json["content"], "Hi there!");
    assert_eq!(json["stop_reason"], "end_turn");

    // Tool results
    let tool_results = Message::ToolResults {
        results: vec![ToolResult::new(
            "tool_123".to_string(),
            "Result content".to_string(),
            false,
        )],
    };
    let json = serde_json::to_value(&tool_results).unwrap();
    assert_eq!(json["role"], "tool_results");
    assert!(json["results"].is_array());
}

#[test]
fn test_tool_call_serialization() {
    let tool_call = ToolCall::new(
        "tc_abc123".to_string(),
        "read_file".to_string(),
        json!({"path": "/tmp/test.txt"}),
    );

    let json = serde_json::to_string(&tool_call).unwrap();
    let parsed: ToolCall = serde_json::from_str(&json).unwrap();

    assert_eq!(parsed.id, "tc_abc123");
    assert_eq!(parsed.name, "read_file");
    assert_eq!(parsed.args["path"], "/tmp/test.txt");
    assert!(parsed.thought_signature.is_none());
}

#[test]
fn test_tool_call_with_thought_signature() {
    let tool_call = ToolCall::with_thought_signature(
        "tc_gemini".to_string(),
        "search".to_string(),
        json!({"query": "test"}),
        "encrypted_thought_abc123".to_string(),
    );

    let json = serde_json::to_string(&tool_call).unwrap();
    let parsed: ToolCall = serde_json::from_str(&json).unwrap();

    assert_eq!(parsed.id, "tc_gemini");
    assert_eq!(
        parsed.thought_signature,
        Some("encrypted_thought_abc123".to_string())
    );
}

#[test]
fn test_tool_result_serialization() {
    let result = ToolResult::new(
        "tc_abc123".to_string(),
        "File contents here".to_string(),
        false,
    );

    let json = serde_json::to_string(&result).unwrap();
    let parsed: ToolResult = serde_json::from_str(&json).unwrap();

    assert_eq!(parsed.tool_use_id, "tc_abc123");
    assert_eq!(parsed.content, "File contents here");
    assert!(!parsed.is_error);
    assert!(parsed.thought_signature.is_none());

    // Error result
    let error_result = ToolResult::new(
        "tc_abc124".to_string(),
        "Permission denied".to_string(),
        true,
    );

    let json = serde_json::to_string(&error_result).unwrap();
    let parsed: ToolResult = serde_json::from_str(&json).unwrap();
    assert!(parsed.is_error);
}

#[test]
fn test_tool_result_with_thought_signature() {
    let result = ToolResult::with_thought_signature(
        "tc_gemini".to_string(),
        "Search results here".to_string(),
        false,
        "encrypted_thought_xyz".to_string(),
    );

    let json = serde_json::to_string(&result).unwrap();
    let parsed: ToolResult = serde_json::from_str(&json).unwrap();

    assert_eq!(
        parsed.thought_signature,
        Some("encrypted_thought_xyz".to_string())
    );
}

#[test]
fn test_tool_result_from_tool_call() {
    let tool_call = ToolCall::with_thought_signature(
        "tc_123".to_string(),
        "test_tool".to_string(),
        json!({}),
        "thought_sig".to_string(),
    );

    let result = ToolResult::from_tool_call(&tool_call, "output".to_string(), false);

    assert_eq!(result.tool_use_id, "tc_123");
    assert_eq!(result.content, "output");
    assert!(!result.is_error);
    assert_eq!(result.thought_signature, Some("thought_sig".to_string()));
}

#[test]
fn test_stop_reason_mapping() {
    // All variants should serialize to snake_case
    let reasons = vec![
        (StopReason::EndTurn, "end_turn"),
        (StopReason::ToolUse, "tool_use"),
        (StopReason::MaxTokens, "max_tokens"),
        (StopReason::StopSequence, "stop_sequence"),
        (StopReason::ContentFilter, "content_filter"),
        (StopReason::Cancelled, "cancelled"),
    ];

    for (reason, expected_str) in reasons {
        let json = serde_json::to_value(reason).unwrap();
        assert_eq!(json.as_str().unwrap(), expected_str);

        // Roundtrip
        let parsed: StopReason = serde_json::from_value(json).unwrap();
        assert_eq!(parsed, reason);
    }
}

#[test]
fn test_usage_accumulation() {
    let mut total = Usage::default();

    let turn1 = Usage {
        input_tokens: 100,
        output_tokens: 50,
        cache_creation_tokens: Some(10),
        cache_read_tokens: None,
    };

    let turn2 = Usage {
        input_tokens: 150,
        output_tokens: 75,
        cache_creation_tokens: None,
        cache_read_tokens: Some(10),
    };

    total.add(&turn1);
    assert_eq!(total.input_tokens, 100);
    assert_eq!(total.output_tokens, 50);
    assert_eq!(total.cache_creation_tokens, Some(10));
    assert_eq!(total.cache_read_tokens, None);

    total.add(&turn2);
    assert_eq!(total.input_tokens, 250);
    assert_eq!(total.output_tokens, 125);
    assert_eq!(total.cache_creation_tokens, Some(10));
    assert_eq!(total.cache_read_tokens, Some(10));
    assert_eq!(total.total_tokens(), 375);
}

#[test]
fn test_run_result_json_schema() {
    let result = RunResult {
        text: "Task completed".to_string(),
        session_id: SessionId::new(),
        usage: Usage {
            input_tokens: 1000,
            output_tokens: 500,
            cache_creation_tokens: None,
            cache_read_tokens: None,
        },
        turns: 3,
        tool_calls: 5,
    };

    let json = serde_json::to_value(&result).unwrap();

    assert_eq!(json["text"], "Task completed");
    assert!(json["session_id"].is_string());
    assert_eq!(json["usage"]["input_tokens"], 1000);
    assert_eq!(json["usage"]["output_tokens"], 500);
    assert_eq!(json["turns"], 3);
    assert_eq!(json["tool_calls"], 5);

    // Roundtrip
    let parsed: RunResult = serde_json::from_value(json).unwrap();
    assert_eq!(parsed.text, "Task completed");
    assert_eq!(parsed.turns, 3);
}

#[test]
fn test_tool_def_serialization() {
    #[derive(schemars::JsonSchema)]
    #[allow(dead_code)]
    struct TestToolDefInput {
        arg1: String,
    }

    let tool_def = ToolDef {
        name: "test_tool".to_string(),
        description: "A test tool".to_string(),
        input_schema: schema_for::<TestToolDefInput>(),
    };

    let json = serde_json::to_string(&tool_def).unwrap();
    let parsed: ToolDef = serde_json::from_str(&json).unwrap();

    assert_eq!(parsed.name, "test_tool");
    assert_eq!(parsed.description, "A test tool");
    assert_eq!(parsed.input_schema["type"], "object");
    assert_eq!(parsed.input_schema["required"], json!(["arg1"]));
}

#[test]
fn test_tool_def_empty_schema_serialization() {
    #[derive(schemars::JsonSchema)]
    struct EmptyObject {}

    let tool_def = ToolDef {
        name: "empty_tool".to_string(),
        description: "An empty tool".to_string(),
        input_schema: schema_for::<EmptyObject>(),
    };

    let json = serde_json::to_string(&tool_def).unwrap();
    let parsed: ToolDef = serde_json::from_str(&json).unwrap();

    assert_eq!(parsed.name, "empty_tool");
    assert_eq!(parsed.input_schema["type"], "object");
    assert_eq!(parsed.input_schema["required"], json!([]));
}

#[test]
fn test_artifact_ref_serialization() {
    let artifact = ArtifactRef {
        id: "artifact_123".to_string(),
        session_id: SessionId::new(),
        size_bytes: 1024,
        ttl_seconds: Some(3600),
        version: 1,
    };

    let json = serde_json::to_string(&artifact).unwrap();
    let parsed: ArtifactRef = serde_json::from_str(&json).unwrap();

    assert_eq!(parsed.id, "artifact_123");
    assert_eq!(parsed.size_bytes, 1024);
    assert_eq!(parsed.ttl_seconds, Some(3600));
    assert_eq!(parsed.version, 1);

    // Without TTL
    let permanent = ArtifactRef {
        id: "artifact_456".to_string(),
        session_id: SessionId::new(),
        size_bytes: 2048,
        ttl_seconds: None,
        version: 1,
    };

    let json = serde_json::to_value(&permanent).unwrap();
    assert!(!json.as_object().unwrap().contains_key("ttl_seconds"));
}

#[test]
fn test_session_checkpoint_empty() {
    // Empty session should serialize correctly
    let messages: Vec<Message> = vec![];
    let json = serde_json::to_string(&messages).unwrap();
    assert_eq!(json, "[]");

    let parsed: Vec<Message> = serde_json::from_str(&json).unwrap();
    assert!(parsed.is_empty());
}

#[test]
fn test_session_checkpoint_complex() {
    // Complex session with 50+ messages
    let mut messages = Vec::new();

    // Add system message
    messages.push(Message::System(SystemMessage {
        content: "You are a helpful coding assistant.".to_string(),
    }));

    // Add 25 user/assistant pairs with tool calls
    for i in 0..25 {
        messages.push(Message::User(UserMessage {
            content: format!("Request {}", i),
        }));

        if i % 3 == 0 {
            // With tool calls
            messages.push(Message::Assistant(AssistantMessage {
                content: format!("Let me help with request {}", i),
                tool_calls: vec![ToolCall::new(
                    format!("tc_{}", i),
                    "test_tool".to_string(),
                    json!({"index": i}),
                )],
                stop_reason: StopReason::ToolUse,
                usage: Usage {
                    input_tokens: 100 + i as u64,
                    output_tokens: 50 + i as u64,
                    cache_creation_tokens: None,
                    cache_read_tokens: None,
                },
            }));

            messages.push(Message::ToolResults {
                results: vec![ToolResult::new(
                    format!("tc_{}", i),
                    format!("Tool result for {}", i),
                    false,
                )],
            });

            messages.push(Message::Assistant(AssistantMessage {
                content: format!("Completed request {} with tool result", i),
                tool_calls: vec![],
                stop_reason: StopReason::EndTurn,
                usage: Usage {
                    input_tokens: 150 + i as u64,
                    output_tokens: 75 + i as u64,
                    cache_creation_tokens: None,
                    cache_read_tokens: None,
                },
            }));
        } else {
            // Without tool calls
            messages.push(Message::Assistant(AssistantMessage {
                content: format!("Response to request {}", i),
                tool_calls: vec![],
                stop_reason: StopReason::EndTurn,
                usage: Usage {
                    input_tokens: 100 + i as u64,
                    output_tokens: 50 + i as u64,
                    cache_creation_tokens: None,
                    cache_read_tokens: None,
                },
            }));
        }
    }

    assert!(messages.len() >= 50, "Should have at least 50 messages");

    // Serialize and deserialize
    let json = serde_json::to_string(&messages).unwrap();
    let parsed: Vec<Message> = serde_json::from_str(&json).unwrap();

    assert_eq!(messages.len(), parsed.len());

    // Verify first and last messages
    match &parsed[0] {
        Message::System(s) => assert!(s.content.contains("helpful")),
        _ => panic!("First message should be System"),
    }
}

#[test]
fn test_session_meta_timestamps() {
    use chrono::{DateTime, Utc};

    // Timestamps should be ISO8601 compatible
    let now = std::time::SystemTime::now();
    let datetime: DateTime<Utc> = now.into();
    let iso_string = datetime.to_rfc3339();

    // Should parse back
    let parsed = DateTime::parse_from_rfc3339(&iso_string).unwrap();
    assert_eq!(datetime.timestamp(), parsed.timestamp());
}
