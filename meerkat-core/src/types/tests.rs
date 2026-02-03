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
        structured_output: None,
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

#[test]
fn test_output_schema_new() {
    let schema = json!({
        "type": "object",
        "properties": {
            "name": {"type": "string"},
            "age": {"type": "integer"}
        },
        "required": ["name", "age"]
    });

    let output_schema = OutputSchema::new(schema.clone());

    assert_eq!(output_schema.schema, schema);
    assert!(output_schema.name.is_none());
    assert!(!output_schema.strict);
}

#[test]
fn test_output_schema_with_name() {
    let schema = json!({"type": "string"});
    let output_schema = OutputSchema::new(schema).with_name("my_output");

    assert_eq!(output_schema.name, Some("my_output".to_string()));
}

#[test]
fn test_output_schema_strict() {
    let schema = json!({"type": "string"});
    let output_schema = OutputSchema::new(schema).strict();

    assert!(output_schema.strict);
}

#[test]
fn test_output_schema_serde_roundtrip() {
    let schema = json!({
        "type": "object",
        "properties": {"field": {"type": "string"}}
    });
    let output_schema = OutputSchema::new(schema.clone())
        .with_name("test_schema")
        .strict();

    let json = serde_json::to_string(&output_schema).unwrap();
    let parsed: OutputSchema = serde_json::from_str(&json).unwrap();

    assert_eq!(parsed.schema, schema);
    assert_eq!(parsed.name, Some("test_schema".to_string()));
    assert!(parsed.strict);
}

#[test]
fn test_output_schema_serde_skip_none_name() {
    let schema = json!({"type": "string"});
    let output_schema = OutputSchema::new(schema);

    let json = serde_json::to_value(&output_schema).unwrap();

    // name should be skipped when None
    assert!(json.get("name").is_none() || json.get("name") == Some(&Value::Null));
}

#[test]
fn test_run_result_with_structured_output() {
    let result = RunResult {
        text: "Here's the structured data".to_string(),
        session_id: SessionId::new(),
        usage: Usage::default(),
        turns: 2,
        tool_calls: 0,
        structured_output: Some(json!({"name": "Alice", "age": 30})),
    };

    let json = serde_json::to_value(&result).unwrap();
    assert_eq!(json["structured_output"]["name"], "Alice");
    assert_eq!(json["structured_output"]["age"], 30);

    // Roundtrip
    let parsed: RunResult = serde_json::from_value(json).unwrap();
    assert!(parsed.structured_output.is_some());
    assert_eq!(parsed.structured_output.unwrap()["name"], "Alice");
}

#[test]
fn test_run_result_without_structured_output_skips_field() {
    let result = RunResult {
        text: "Regular response".to_string(),
        session_id: SessionId::new(),
        usage: Usage::default(),
        turns: 1,
        tool_calls: 0,
        structured_output: None,
    };

    let json = serde_json::to_value(&result).unwrap();

    // structured_output should be skipped when None
    assert!(
        json.get("structured_output").is_none()
            || json.get("structured_output") == Some(&Value::Null)
    );
}

// ===========================================================================
// New tests for ordered transcript types (spec section 3.1)
// ===========================================================================

mod ordered_transcript_types {
    use super::*;
    use serde_json::value::RawValue;

    // -----------------------------------------------------------------------
    // ProviderMeta serialization round-trip tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_provider_meta_anthropic_roundtrip() {
        let meta = ProviderMeta::Anthropic {
            signature: "opaque_sig_abc123".to_string(),
        };

        let json = serde_json::to_string(&meta).unwrap();
        let parsed: ProviderMeta = serde_json::from_str(&json).unwrap();

        assert_eq!(meta, parsed);
        // Verify tagged enum structure
        let value: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["provider"], "anthropic");
        assert_eq!(value["signature"], "opaque_sig_abc123");
    }

    #[test]
    fn test_provider_meta_gemini_roundtrip() {
        let meta = ProviderMeta::Gemini {
            thought_signature: "gemini_thought_xyz".to_string(),
        };

        let json = serde_json::to_string(&meta).unwrap();
        let parsed: ProviderMeta = serde_json::from_str(&json).unwrap();

        assert_eq!(meta, parsed);
        // Verify tagged enum structure with camelCase rename
        let value: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["provider"], "gemini");
        assert_eq!(value["thoughtSignature"], "gemini_thought_xyz");
    }

    #[test]
    fn test_provider_meta_openai_roundtrip() {
        let meta = ProviderMeta::OpenAi {
            id: "reasoning_item_123".to_string(),
            encrypted_content: Some("encrypted_tokens_abc".to_string()),
        };

        let json = serde_json::to_string(&meta).unwrap();
        let parsed: ProviderMeta = serde_json::from_str(&json).unwrap();

        assert_eq!(meta, parsed);
        // Verify tagged enum structure
        let value: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["provider"], "open_ai");
        assert_eq!(value["id"], "reasoning_item_123");
        assert_eq!(value["encrypted_content"], "encrypted_tokens_abc");
    }

    #[test]
    fn test_provider_meta_openai_without_encrypted_content() {
        let meta = ProviderMeta::OpenAi {
            id: "reasoning_item_456".to_string(),
            encrypted_content: None,
        };

        let json = serde_json::to_string(&meta).unwrap();
        let value: Value = serde_json::from_str(&json).unwrap();

        // encrypted_content should be skipped when None
        assert!(
            value.get("encrypted_content").is_none()
                || value.get("encrypted_content") == Some(&Value::Null)
        );

        // Roundtrip still works
        let parsed: ProviderMeta = serde_json::from_str(&json).unwrap();
        assert_eq!(meta, parsed);
    }

    // -----------------------------------------------------------------------
    // AssistantBlock serialization round-trip tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_assistant_block_text_roundtrip() {
        let block = AssistantBlock::Text {
            text: "Hello, world!".to_string(),
            meta: None,
        };

        let json = serde_json::to_string(&block).unwrap();
        let parsed: AssistantBlock = serde_json::from_str(&json).unwrap();

        assert_eq!(block, parsed);
        // Verify adjacently tagged enum structure
        let value: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["block_type"], "text");
        assert_eq!(value["data"]["text"], "Hello, world!");
    }

    #[test]
    fn test_assistant_block_text_with_gemini_meta() {
        let block = AssistantBlock::Text {
            text: "Response with signature".to_string(),
            meta: Some(ProviderMeta::Gemini {
                thought_signature: "trailing_sig".to_string(),
            }),
        };

        let json = serde_json::to_string(&block).unwrap();
        let parsed: AssistantBlock = serde_json::from_str(&json).unwrap();

        assert_eq!(block, parsed);
    }

    #[test]
    fn test_assistant_block_reasoning_roundtrip() {
        let block = AssistantBlock::Reasoning {
            text: "Let me think about this...".to_string(),
            meta: Some(ProviderMeta::Anthropic {
                signature: "thinking_sig_abc".to_string(),
            }),
        };

        let json = serde_json::to_string(&block).unwrap();
        let parsed: AssistantBlock = serde_json::from_str(&json).unwrap();

        assert_eq!(block, parsed);
        // Verify adjacently tagged enum structure
        let value: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["block_type"], "reasoning");
        assert_eq!(value["data"]["text"], "Let me think about this...");
        assert!(value["data"]["meta"].is_object());
    }

    #[test]
    fn test_assistant_block_reasoning_empty_text() {
        // OpenAI may have empty text with only encrypted_content
        let block = AssistantBlock::Reasoning {
            text: String::new(),
            meta: Some(ProviderMeta::OpenAi {
                id: "reasoning_123".to_string(),
                encrypted_content: Some("encrypted".to_string()),
            }),
        };

        let json = serde_json::to_string(&block).unwrap();
        let parsed: AssistantBlock = serde_json::from_str(&json).unwrap();

        assert_eq!(block, parsed);
    }

    #[test]
    fn test_assistant_block_tool_use_roundtrip() {
        let args = RawValue::from_string(r#"{"path":"/tmp/test.txt"}"#.to_string()).unwrap();
        let block = AssistantBlock::ToolUse {
            id: "tool_123".to_string(),
            name: "read_file".to_string(),
            args,
            meta: None,
        };

        let json = serde_json::to_string(&block).unwrap();
        let parsed: AssistantBlock = serde_json::from_str(&json).unwrap();

        // Verify adjacently tagged enum structure
        let value: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["block_type"], "tool_use");
        assert_eq!(value["data"]["id"], "tool_123");
        assert_eq!(value["data"]["name"], "read_file");

        // Check args round-trip (compare as JSON values)
        match parsed {
            AssistantBlock::ToolUse { id, name, args, .. } => {
                assert_eq!(id, "tool_123");
                assert_eq!(name, "read_file");
                let args_value: Value = serde_json::from_str(args.get()).unwrap();
                assert_eq!(args_value["path"], "/tmp/test.txt");
            }
            _ => panic!("Expected ToolUse block"),
        }
    }

    #[test]
    fn test_assistant_block_tool_use_with_gemini_meta() {
        let args = RawValue::from_string(r#"{"query":"test"}"#.to_string()).unwrap();
        let block = AssistantBlock::ToolUse {
            id: "tool_456".to_string(),
            name: "search".to_string(),
            args,
            meta: Some(ProviderMeta::Gemini {
                thought_signature: "first_parallel_call_sig".to_string(),
            }),
        };

        let json = serde_json::to_string(&block).unwrap();
        let parsed: AssistantBlock = serde_json::from_str(&json).unwrap();

        match parsed {
            AssistantBlock::ToolUse { meta, .. } => {
                assert!(meta.is_some());
                match meta.unwrap() {
                    ProviderMeta::Gemini { thought_signature } => {
                        assert_eq!(thought_signature, "first_parallel_call_sig");
                    }
                    _ => panic!("Expected Gemini meta"),
                }
            }
            _ => panic!("Expected ToolUse block"),
        }
    }

    // -----------------------------------------------------------------------
    // ToolCallView::parse_args tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_tool_call_view_parse_args() {
        #[derive(Debug, serde::Deserialize, PartialEq)]
        struct ReadFileArgs {
            path: String,
            #[serde(default)]
            encoding: Option<String>,
        }

        let args = RawValue::from_string(r#"{"path":"/tmp/test.txt","encoding":"utf-8"}"#.to_string()).unwrap();
        let view = ToolCallView {
            id: "tc_123",
            name: "read_file",
            args: &args,
        };

        let parsed: ReadFileArgs = view.parse_args().unwrap();
        assert_eq!(parsed.path, "/tmp/test.txt");
        assert_eq!(parsed.encoding, Some("utf-8".to_string()));
    }

    #[test]
    fn test_tool_call_view_parse_args_error() {
        #[derive(Debug, serde::Deserialize)]
        struct ExpectedArgs {
            #[allow(dead_code)]
            required_field: String,
        }

        let args = RawValue::from_string(r#"{"wrong_field":"value"}"#.to_string()).unwrap();
        let view = ToolCallView {
            id: "tc_456",
            name: "some_tool",
            args: &args,
        };

        let result: Result<ExpectedArgs, _> = view.parse_args();
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // AssistantMessage::tool_calls() iterator tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_assistant_message_tool_calls_iterator() {
        let args1 = RawValue::from_string(r#"{"path":"/a"}"#.to_string()).unwrap();
        let args2 = RawValue::from_string(r#"{"query":"test"}"#.to_string()).unwrap();

        let msg = BlockAssistantMessage {
            blocks: vec![
                AssistantBlock::Text {
                    text: "Let me help".to_string(),
                    meta: None,
                },
                AssistantBlock::ToolUse {
                    id: "tc_1".to_string(),
                    name: "read_file".to_string(),
                    args: args1,
                    meta: None,
                },
                AssistantBlock::Reasoning {
                    text: "thinking...".to_string(),
                    meta: None,
                },
                AssistantBlock::ToolUse {
                    id: "tc_2".to_string(),
                    name: "search".to_string(),
                    args: args2,
                    meta: None,
                },
            ],
            stop_reason: StopReason::ToolUse,
        };

        let tool_calls: Vec<_> = msg.tool_calls().collect();
        assert_eq!(tool_calls.len(), 2);
        assert_eq!(tool_calls[0].id, "tc_1");
        assert_eq!(tool_calls[0].name, "read_file");
        assert_eq!(tool_calls[1].id, "tc_2");
        assert_eq!(tool_calls[1].name, "search");
    }

    #[test]
    fn test_assistant_message_tool_calls_empty() {
        let msg = BlockAssistantMessage {
            blocks: vec![
                AssistantBlock::Text {
                    text: "No tools needed".to_string(),
                    meta: None,
                },
            ],
            stop_reason: StopReason::EndTurn,
        };

        let tool_calls: Vec<_> = msg.tool_calls().collect();
        assert!(tool_calls.is_empty());
    }

    #[test]
    fn test_assistant_message_has_tool_calls() {
        let args = RawValue::from_string(r#"{}"#.to_string()).unwrap();

        let msg_with_tools = BlockAssistantMessage {
            blocks: vec![AssistantBlock::ToolUse {
                id: "tc_1".to_string(),
                name: "test".to_string(),
                args,
                meta: None,
            }],
            stop_reason: StopReason::ToolUse,
        };
        assert!(msg_with_tools.has_tool_calls());

        let msg_without_tools = BlockAssistantMessage {
            blocks: vec![AssistantBlock::Text {
                text: "Hello".to_string(),
                meta: None,
            }],
            stop_reason: StopReason::EndTurn,
        };
        assert!(!msg_without_tools.has_tool_calls());
    }

    #[test]
    fn test_assistant_message_get_tool_use() {
        let args1 = RawValue::from_string(r#"{"a":1}"#.to_string()).unwrap();
        let args2 = RawValue::from_string(r#"{"b":2}"#.to_string()).unwrap();

        let msg = BlockAssistantMessage {
            blocks: vec![
                AssistantBlock::ToolUse {
                    id: "tc_first".to_string(),
                    name: "tool_a".to_string(),
                    args: args1,
                    meta: None,
                },
                AssistantBlock::ToolUse {
                    id: "tc_second".to_string(),
                    name: "tool_b".to_string(),
                    args: args2,
                    meta: None,
                },
            ],
            stop_reason: StopReason::ToolUse,
        };

        let found = msg.get_tool_use("tc_second");
        assert!(found.is_some());
        let view = found.unwrap();
        assert_eq!(view.name, "tool_b");

        let not_found = msg.get_tool_use("tc_nonexistent");
        assert!(not_found.is_none());
    }

    // -----------------------------------------------------------------------
    // AssistantMessage Display impl tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_assistant_message_display() {
        let args = RawValue::from_string(r#"{}"#.to_string()).unwrap();

        let msg = BlockAssistantMessage {
            blocks: vec![
                AssistantBlock::Text {
                    text: "First part. ".to_string(),
                    meta: None,
                },
                AssistantBlock::Reasoning {
                    text: "This should not appear".to_string(),
                    meta: None,
                },
                AssistantBlock::Text {
                    text: "Second part.".to_string(),
                    meta: None,
                },
                AssistantBlock::ToolUse {
                    id: "tc_1".to_string(),
                    name: "test".to_string(),
                    args,
                    meta: None,
                },
            ],
            stop_reason: StopReason::ToolUse,
        };

        let display = format!("{}", msg);
        assert_eq!(display, "First part. Second part.");
    }

    #[test]
    fn test_assistant_message_display_empty() {
        let msg = BlockAssistantMessage {
            blocks: vec![],
            stop_reason: StopReason::EndTurn,
        };

        let display = format!("{}", msg);
        assert_eq!(display, "");
    }

    // -----------------------------------------------------------------------
    // AssistantMessage text_blocks iterator tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_assistant_message_text_blocks() {
        let args = RawValue::from_string(r#"{}"#.to_string()).unwrap();

        let msg = BlockAssistantMessage {
            blocks: vec![
                AssistantBlock::Text {
                    text: "Hello".to_string(),
                    meta: None,
                },
                AssistantBlock::Reasoning {
                    text: "thinking".to_string(),
                    meta: None,
                },
                AssistantBlock::Text {
                    text: "World".to_string(),
                    meta: None,
                },
                AssistantBlock::ToolUse {
                    id: "tc_1".to_string(),
                    name: "test".to_string(),
                    args,
                    meta: None,
                },
            ],
            stop_reason: StopReason::EndTurn,
        };

        let text_blocks: Vec<_> = msg.text_blocks().collect();
        assert_eq!(text_blocks.len(), 2);
        assert_eq!(text_blocks[0], "Hello");
        assert_eq!(text_blocks[1], "World");
    }
}
