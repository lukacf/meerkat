#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Regression tests that pin wire type field names and shapes.
//!
//! Pattern: construct -> serde_json::to_value() -> assert field presence by string key
//! -> deserialize back -> assert roundtrip.
//!
//! These catch silent renames and accidental field removals.

use meerkat_contracts::{
    ContractVersion, CoreCreateParams, ErrorCode, KNOWN_AGENT_EVENT_TYPES, WireError, WireEvent,
    WireRunResult, WireSessionHistory, WireSessionInfo, WireSessionMessage, WireSessionSummary,
    WireUsage,
};
use meerkat_core::{
    AgentEvent, BudgetType, HookPatch, HookPoint, HookReasonCode, RunResult, SessionId, StopReason,
    ToolConfigChangeOperation, ToolConfigChangedPayload, Usage,
};

// ---------------------------------------------------------------------------
// 1. WireRunResult required fields
// ---------------------------------------------------------------------------

#[test]
fn wire_run_result_required_fields() {
    let wire = WireRunResult {
        session_id: SessionId::new(),
        session_ref: None,
        text: "hello".to_string(),
        turns: 3,
        tool_calls: 2,
        usage: WireUsage::default(),
        structured_output: None,
        schema_warnings: None,
        skill_diagnostics: None,
    };
    let value = serde_json::to_value(&wire).unwrap();

    assert!(value.get("session_id").is_some(), "missing session_id");
    assert!(value.get("text").is_some(), "missing text");
    assert!(value.get("turns").is_some(), "missing turns");
    assert!(value.get("tool_calls").is_some(), "missing tool_calls");
    assert!(value.get("usage").is_some(), "missing usage");
}

// ---------------------------------------------------------------------------
// 2. WireRunResult optional fields omitted when None
// ---------------------------------------------------------------------------

#[test]
fn wire_run_result_optional_omitted() {
    let wire = WireRunResult {
        session_id: SessionId::new(),
        session_ref: None,
        text: "ok".to_string(),
        turns: 1,
        tool_calls: 0,
        usage: WireUsage::default(),
        structured_output: None,
        schema_warnings: None,
        skill_diagnostics: None,
    };
    let value = serde_json::to_value(&wire).unwrap();

    assert!(
        value.get("structured_output").is_none(),
        "structured_output should be absent when None"
    );
    assert!(
        value.get("schema_warnings").is_none(),
        "schema_warnings should be absent when None"
    );
    assert!(
        value.get("skill_diagnostics").is_none(),
        "skill_diagnostics should be absent when None"
    );
}

// ---------------------------------------------------------------------------
// 3. WireRunResult roundtrip
// ---------------------------------------------------------------------------

#[test]
fn wire_run_result_roundtrip() {
    let wire = WireRunResult {
        session_id: SessionId::new(),
        session_ref: Some("ref-1".to_string()),
        text: "result text".to_string(),
        turns: 5,
        tool_calls: 3,
        usage: WireUsage {
            input_tokens: 100,
            output_tokens: 50,
            total_tokens: 150,
            cache_creation_tokens: Some(10),
            cache_read_tokens: Some(20),
        },
        structured_output: Some(serde_json::json!({"key": "value"})),
        schema_warnings: None,
        skill_diagnostics: None,
    };

    let json = serde_json::to_value(&wire).unwrap();
    let roundtrip: WireRunResult = serde_json::from_value(json).unwrap();

    assert_eq!(wire.session_id, roundtrip.session_id);
    assert_eq!(wire.session_ref, roundtrip.session_ref);
    assert_eq!(wire.text, roundtrip.text);
    assert_eq!(wire.turns, roundtrip.turns);
    assert_eq!(wire.tool_calls, roundtrip.tool_calls);
    assert_eq!(wire.usage.input_tokens, roundtrip.usage.input_tokens);
    assert_eq!(wire.usage.output_tokens, roundtrip.usage.output_tokens);
    assert_eq!(wire.usage.total_tokens, roundtrip.usage.total_tokens);
    assert_eq!(
        wire.usage.cache_creation_tokens,
        roundtrip.usage.cache_creation_tokens
    );
    assert_eq!(
        wire.usage.cache_read_tokens,
        roundtrip.usage.cache_read_tokens
    );
    assert_eq!(wire.structured_output, roundtrip.structured_output);
}

// ---------------------------------------------------------------------------
// 4. WireSessionInfo required fields
// ---------------------------------------------------------------------------

#[test]
fn wire_session_info_required_fields() {
    let wire = WireSessionInfo {
        session_id: SessionId::new(),
        session_ref: None,
        created_at: 1000,
        updated_at: 2000,
        message_count: 5,
        is_active: true,
        last_assistant_text: None,
        labels: Default::default(),
    };
    let value = serde_json::to_value(&wire).unwrap();

    assert!(value.get("session_id").is_some(), "missing session_id");
    assert!(value.get("created_at").is_some(), "missing created_at");
    assert!(value.get("updated_at").is_some(), "missing updated_at");
    assert!(
        value.get("message_count").is_some(),
        "missing message_count"
    );
    assert!(value.get("is_active").is_some(), "missing is_active");
}

// ---------------------------------------------------------------------------
// 5. WireSessionSummary required fields
// ---------------------------------------------------------------------------

#[test]
fn wire_session_summary_required_fields() {
    let wire = WireSessionSummary {
        session_id: SessionId::new(),
        session_ref: None,
        created_at: 1000,
        updated_at: 2000,
        message_count: 10,
        total_tokens: 500,
        is_active: true,
        labels: Default::default(),
    };
    let value = serde_json::to_value(&wire).unwrap();

    assert!(value.get("session_id").is_some(), "missing session_id");
    assert!(value.get("created_at").is_some(), "missing created_at");
    assert!(value.get("updated_at").is_some(), "missing updated_at");
    assert!(
        value.get("message_count").is_some(),
        "missing message_count"
    );
    assert!(value.get("total_tokens").is_some(), "missing total_tokens");
    assert!(value.get("is_active").is_some(), "missing is_active");
}

// ---------------------------------------------------------------------------
// 6. WireSessionHistory required fields
// ---------------------------------------------------------------------------

#[test]
fn wire_session_history_required_fields() {
    let wire = WireSessionHistory {
        session_id: SessionId::new(),
        session_ref: Some("session-ref".to_string()),
        message_count: 3,
        offset: 1,
        limit: Some(2),
        has_more: false,
        messages: vec![
            WireSessionMessage::System {
                content: "system".to_string(),
            },
            WireSessionMessage::User {
                content: meerkat_contracts::WireContentInput::Text("user".to_string()),
            },
        ],
    };
    let value = serde_json::to_value(&wire).unwrap();

    assert!(value.get("session_id").is_some(), "missing session_id");
    assert!(
        value.get("message_count").is_some(),
        "missing message_count"
    );
    assert!(value.get("offset").is_some(), "missing offset");
    assert!(value.get("has_more").is_some(), "missing has_more");
    assert!(value.get("messages").is_some(), "missing messages");
}

// ---------------------------------------------------------------------------
// 7. WireSessionHistory roundtrip
// ---------------------------------------------------------------------------

#[test]
fn wire_session_history_roundtrip() {
    let wire = WireSessionHistory {
        session_id: SessionId::new(),
        session_ref: Some("history-ref".to_string()),
        message_count: 4,
        offset: 2,
        limit: Some(2),
        has_more: true,
        messages: vec![
            WireSessionMessage::BlockAssistant {
                blocks: vec![],
                stop_reason: meerkat_contracts::WireStopReason::EndTurn,
            },
            WireSessionMessage::ToolResults {
                results: vec![meerkat_contracts::WireToolResult {
                    tool_use_id: "tool-1".to_string(),
                    content: meerkat_contracts::WireToolResultContent::Text("ok".to_string()),
                    is_error: false,
                }],
            },
        ],
    };

    let json = serde_json::to_value(&wire).unwrap();
    let roundtrip: WireSessionHistory = serde_json::from_value(json).unwrap();

    assert_eq!(wire.session_id, roundtrip.session_id);
    assert_eq!(wire.session_ref, roundtrip.session_ref);
    assert_eq!(wire.message_count, roundtrip.message_count);
    assert_eq!(wire.offset, roundtrip.offset);
    assert_eq!(wire.limit, roundtrip.limit);
    assert_eq!(wire.has_more, roundtrip.has_more);
    assert_eq!(wire.messages, roundtrip.messages);
}

// ---------------------------------------------------------------------------
// 8. WireEvent envelope shape
// ---------------------------------------------------------------------------

#[test]
fn wire_event_envelope_shape() {
    let wire = WireEvent {
        session_id: SessionId::new(),
        sequence: 42,
        event: AgentEvent::TurnStarted { turn_number: 1 },
        contract_version: ContractVersion::CURRENT,
    };
    let value = serde_json::to_value(&wire).unwrap();

    assert!(value.get("session_id").is_some(), "missing session_id");
    assert!(value.get("sequence").is_some(), "missing sequence");
    assert!(value.get("event").is_some(), "missing event");
    assert!(
        value.get("contract_version").is_some(),
        "missing contract_version"
    );
}

// ---------------------------------------------------------------------------
// 9. WireUsage fields
// ---------------------------------------------------------------------------

#[test]
fn wire_usage_fields() {
    let wire = WireUsage {
        input_tokens: 100,
        output_tokens: 50,
        total_tokens: 150,
        cache_creation_tokens: None,
        cache_read_tokens: None,
    };
    let value = serde_json::to_value(&wire).unwrap();

    assert!(value.get("input_tokens").is_some(), "missing input_tokens");
    assert!(
        value.get("output_tokens").is_some(),
        "missing output_tokens"
    );
    assert!(value.get("total_tokens").is_some(), "missing total_tokens");
}

// ---------------------------------------------------------------------------
// 10. WireError shape
// ---------------------------------------------------------------------------

#[test]
fn wire_error_shape() {
    let wire = WireError::new(ErrorCode::SessionNotFound, "not found");
    let value = serde_json::to_value(&wire).unwrap();

    assert!(value.get("code").is_some(), "missing code");
    assert!(value.get("category").is_some(), "missing category");
    assert!(value.get("message").is_some(), "missing message");
}

// ---------------------------------------------------------------------------
// 9. AgentEvent all variants roundtrip
// ---------------------------------------------------------------------------

#[test]
fn agent_event_all_variants_roundtrip() {
    let session_id = SessionId::new();

    // Variants that can be constructed without chrono or uuid (direct construction).
    let direct_variants: Vec<AgentEvent> = vec![
        AgentEvent::RunStarted {
            session_id: session_id.clone(),
            prompt: "hello".to_string(),
        },
        AgentEvent::RunCompleted {
            session_id: session_id.clone(),
            result: "done".to_string(),
            usage: Usage {
                input_tokens: 10,
                output_tokens: 5,
                cache_creation_tokens: None,
                cache_read_tokens: None,
            },
        },
        AgentEvent::RunFailed {
            session_id,
            error: "boom".to_string(),
        },
        AgentEvent::HookStarted {
            hook_id: "h1".to_string(),
            point: HookPoint::PreLlmRequest,
        },
        AgentEvent::HookCompleted {
            hook_id: "h1".to_string(),
            point: HookPoint::PostLlmResponse,
            duration_ms: 42,
        },
        AgentEvent::HookFailed {
            hook_id: "h1".to_string(),
            point: HookPoint::RunStarted,
            error: "hook error".to_string(),
        },
        AgentEvent::HookDenied {
            hook_id: "h1".to_string(),
            point: HookPoint::PreToolExecution,
            reason_code: HookReasonCode::PolicyViolation,
            message: "denied".to_string(),
            payload: None,
        },
        AgentEvent::HookRewriteApplied {
            hook_id: "h1".to_string(),
            point: HookPoint::PostLlmResponse,
            patch: HookPatch::AssistantText {
                text: "rewritten".to_string(),
            },
        },
        AgentEvent::TurnStarted { turn_number: 1 },
        AgentEvent::ReasoningDelta {
            delta: "thinking...".to_string(),
        },
        AgentEvent::ReasoningComplete {
            content: "I think therefore I am".to_string(),
        },
        AgentEvent::TextDelta {
            delta: "chunk".to_string(),
        },
        AgentEvent::TextComplete {
            content: "full text".to_string(),
        },
        AgentEvent::ToolCallRequested {
            id: "tc1".to_string(),
            name: "read_file".to_string(),
            args: serde_json::json!({"path": "/tmp"}),
        },
        AgentEvent::ToolResultReceived {
            id: "tc1".to_string(),
            name: "read_file".to_string(),
            is_error: false,
        },
        AgentEvent::TurnCompleted {
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
        },
        AgentEvent::ToolExecutionStarted {
            id: "tc2".to_string(),
            name: "shell".to_string(),
        },
        AgentEvent::ToolExecutionCompleted {
            id: "tc2".to_string(),
            name: "shell".to_string(),
            result: "ok".to_string(),
            is_error: false,
            duration_ms: 100,
            has_images: false,
        },
        AgentEvent::ToolExecutionTimedOut {
            id: "tc3".to_string(),
            name: "slow_tool".to_string(),
            timeout_ms: 5000,
        },
        AgentEvent::CompactionStarted {
            input_tokens: 120_000,
            estimated_history_tokens: 150_000,
            message_count: 42,
        },
        AgentEvent::CompactionCompleted {
            summary_tokens: 2048,
            messages_before: 42,
            messages_after: 8,
        },
        AgentEvent::CompactionFailed {
            error: "LLM failed".to_string(),
        },
        AgentEvent::BudgetWarning {
            budget_type: BudgetType::Tokens,
            used: 8000,
            limit: 10000,
            percent: 0.8,
        },
        AgentEvent::Retrying {
            attempt: 1,
            max_attempts: 3,
            error: "rate limited".to_string(),
            delay_ms: 1000,
        },
        AgentEvent::SkillsResolved {
            skills: vec![meerkat_core::skills::SkillId("test/skill".to_string())],
            injection_bytes: 256,
        },
        AgentEvent::SkillResolutionFailed {
            reference: "bad/ref".to_string(),
            error: "not found".to_string(),
        },
        // InteractionComplete and InteractionFailed are constructed via JSON
        // below to avoid a direct uuid crate dependency.
        AgentEvent::StreamTruncated {
            reason: "channel full".to_string(),
        },
        AgentEvent::ToolConfigChanged {
            payload: ToolConfigChangedPayload {
                operation: ToolConfigChangeOperation::Add,
                target: "filesystem".to_string(),
                status: "applied".to_string(),
                persisted: true,
                applied_at_turn: Some(5),
            },
        },
    ];

    for event in &direct_variants {
        let json = serde_json::to_value(event).unwrap();

        // Every variant must have a "type" discriminator tag
        assert!(
            json.get("type").is_some(),
            "missing type tag on event: {event:?}"
        );

        // Roundtrip
        let roundtrip: AgentEvent = serde_json::from_value(json.clone()).unwrap();
        let json2 = serde_json::to_value(&roundtrip).unwrap();
        assert_eq!(json, json2, "roundtrip mismatch for event: {event:?}");
    }

    // Variants constructed via JSON to avoid direct chrono/uuid crate dependencies.
    let json_constructed_variants: Vec<serde_json::Value> = vec![
        // HookPatchPublished requires chrono::DateTime<Utc>
        serde_json::json!({
            "type": "hook_patch_published",
            "hook_id": "h2",
            "point": "post_llm_response",
            "envelope": {
                "revision": 1,
                "hook_id": "h2",
                "point": "post_llm_response",
                "patch": {
                    "patch_type": "assistant_text",
                    "text": "patched"
                },
                "published_at": "2025-01-01T00:00:00Z"
            }
        }),
        // InteractionComplete requires uuid::Uuid for InteractionId
        serde_json::json!({
            "type": "interaction_complete",
            "interaction_id": "550e8400-e29b-41d4-a716-446655440000",
            "result": "response"
        }),
        // InteractionFailed requires uuid::Uuid for InteractionId
        serde_json::json!({
            "type": "interaction_failed",
            "interaction_id": "550e8400-e29b-41d4-a716-446655440001",
            "error": "timeout"
        }),
    ];

    for json_val in &json_constructed_variants {
        let event: AgentEvent = serde_json::from_value(json_val.clone()).unwrap();
        let re_serialized = serde_json::to_value(&event).unwrap();

        assert!(
            re_serialized.get("type").is_some(),
            "missing type tag on JSON-constructed event"
        );

        // Roundtrip the re-serialized form
        let roundtrip: AgentEvent = serde_json::from_value(re_serialized.clone()).unwrap();
        let json2 = serde_json::to_value(&roundtrip).unwrap();
        assert_eq!(
            re_serialized,
            json2,
            "roundtrip mismatch for JSON-constructed event: {}",
            json_val.get("type").unwrap()
        );
    }

    // All 31 AgentEvent variants are covered: 28 direct + 3 from JSON.
    // If a new variant is added and not covered here, the exhaustive
    // agent_event_type() match in meerkat-core will fail to compile,
    // prompting addition here too.
}

// ---------------------------------------------------------------------------
// 10. RunResult -> WireRunResult conversion
// ---------------------------------------------------------------------------

#[test]
fn documented_event_catalog_covers_core_agent_event_discriminators() {
    let events = vec![
        AgentEvent::RunStarted {
            session_id: SessionId::new(),
            prompt: "hello".to_string(),
        },
        AgentEvent::RunCompleted {
            session_id: SessionId::new(),
            result: "done".to_string(),
            usage: Usage::default(),
        },
        AgentEvent::RunFailed {
            session_id: SessionId::new(),
            error: "nope".to_string(),
        },
        AgentEvent::HookStarted {
            hook_id: "hook-1".to_string(),
            point: HookPoint::RunStarted,
        },
        AgentEvent::HookCompleted {
            hook_id: "hook-1".to_string(),
            point: HookPoint::RunStarted,
            duration_ms: 1,
        },
        AgentEvent::HookFailed {
            hook_id: "hook-1".to_string(),
            point: HookPoint::RunStarted,
            error: "boom".to_string(),
        },
        AgentEvent::HookDenied {
            hook_id: "hook-1".to_string(),
            point: HookPoint::RunStarted,
            reason_code: HookReasonCode::RuntimeError,
            message: "denied".to_string(),
            payload: None,
        },
        AgentEvent::HookRewriteApplied {
            hook_id: "hook-1".to_string(),
            point: HookPoint::RunStarted,
            patch: HookPatch::AssistantText {
                text: "patched".to_string(),
            },
        },
        AgentEvent::HookPatchPublished {
            hook_id: "hook-1".to_string(),
            point: HookPoint::RunStarted,
            envelope: serde_json::from_value(serde_json::json!({
                "revision": 1,
                "hook_id": "hook-1",
                "point": "run_started",
                "patch": {
                    "patch_type": "assistant_text",
                    "text": "patched"
                },
                "published_at": "2026-03-24T00:00:00Z"
            }))
            .unwrap(),
        },
        AgentEvent::TurnStarted { turn_number: 1 },
        AgentEvent::ReasoningDelta {
            delta: "think".to_string(),
        },
        AgentEvent::ReasoningComplete {
            content: "done".to_string(),
        },
        AgentEvent::TextDelta {
            delta: "chunk".to_string(),
        },
        AgentEvent::TextComplete {
            content: "done".to_string(),
        },
        AgentEvent::ToolCallRequested {
            id: "tool-1".to_string(),
            name: "search".to_string(),
            args: serde_json::json!({}),
        },
        AgentEvent::ToolResultReceived {
            id: "tool-1".to_string(),
            name: "search".to_string(),
            is_error: false,
        },
        AgentEvent::TurnCompleted {
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
        },
        AgentEvent::ToolExecutionStarted {
            id: "tool-1".to_string(),
            name: "search".to_string(),
        },
        AgentEvent::ToolExecutionCompleted {
            id: "tool-1".to_string(),
            name: "search".to_string(),
            result: "ok".to_string(),
            is_error: false,
            duration_ms: 1,
            has_images: false,
        },
        AgentEvent::ToolExecutionTimedOut {
            id: "tool-1".to_string(),
            name: "search".to_string(),
            timeout_ms: 1000,
        },
        AgentEvent::CompactionStarted {
            input_tokens: 1,
            estimated_history_tokens: 2,
            message_count: 3,
        },
        AgentEvent::CompactionCompleted {
            summary_tokens: 1,
            messages_before: 3,
            messages_after: 1,
        },
        AgentEvent::CompactionFailed {
            error: "failed".to_string(),
        },
        AgentEvent::BudgetWarning {
            budget_type: BudgetType::Time,
            used: 1,
            limit: 2,
            percent: 50.0,
        },
        AgentEvent::Retrying {
            attempt: 1,
            max_attempts: 2,
            error: "retry".to_string(),
            delay_ms: 100,
        },
        AgentEvent::SkillsResolved {
            skills: vec![],
            injection_bytes: 0,
        },
        AgentEvent::SkillResolutionFailed {
            reference: "skill".to_string(),
            error: "missing".to_string(),
        },
        AgentEvent::InteractionComplete {
            interaction_id: serde_json::from_value(serde_json::json!(
                "550e8400-e29b-41d4-a716-446655440000"
            ))
            .unwrap(),
            result: "ok".to_string(),
        },
        AgentEvent::InteractionFailed {
            interaction_id: serde_json::from_value(serde_json::json!(
                "550e8400-e29b-41d4-a716-446655440001"
            ))
            .unwrap(),
            error: "failed".to_string(),
        },
        AgentEvent::StreamTruncated {
            reason: "lag".to_string(),
        },
        AgentEvent::ToolConfigChanged {
            payload: ToolConfigChangedPayload {
                operation: ToolConfigChangeOperation::Reload,
                target: "external".to_string(),
                status: "applied".to_string(),
                persisted: true,
                applied_at_turn: Some(1),
            },
        },
    ];

    for event in events {
        let kind = meerkat_core::agent_event_type(&event);
        assert!(
            KNOWN_AGENT_EVENT_TYPES.contains(&kind),
            "documented event catalog missing {kind}"
        );
    }
}

#[test]
fn wire_run_result_from_run_result_conversion() {
    let session_id = SessionId::new();
    let run = RunResult {
        text: "result".to_string(),
        session_id: session_id.clone(),
        usage: Usage {
            input_tokens: 200,
            output_tokens: 100,
            cache_creation_tokens: Some(50),
            cache_read_tokens: Some(30),
        },
        turns: 4,
        tool_calls: 7,
        structured_output: Some(serde_json::json!({"answer": 42})),
        schema_warnings: None,
        skill_diagnostics: None,
    };

    let wire: WireRunResult = run.into();

    assert_eq!(wire.session_id, session_id);
    assert_eq!(wire.text, "result");
    assert_eq!(wire.turns, 4);
    assert_eq!(wire.tool_calls, 7);
    assert_eq!(wire.usage.input_tokens, 200);
    assert_eq!(wire.usage.output_tokens, 100);
    assert_eq!(wire.usage.total_tokens, 300); // 200 + 100
    assert_eq!(wire.usage.cache_creation_tokens, Some(50));
    assert_eq!(wire.usage.cache_read_tokens, Some(30));
    assert_eq!(
        wire.structured_output,
        Some(serde_json::json!({"answer": 42}))
    );
    // session_ref is always None from From<RunResult>
    assert!(wire.session_ref.is_none());
}

// ---------------------------------------------------------------------------
// 11. CoreCreateParams minimal deserialize
// ---------------------------------------------------------------------------

#[test]
fn core_create_params_minimal_deserialize() {
    let json = r#"{"prompt":"hi"}"#;
    let params: CoreCreateParams = serde_json::from_str(json).unwrap();

    assert_eq!(params.prompt, "hi");
    assert!(params.model.is_none());
    assert!(params.provider.is_none());
    assert!(params.max_tokens.is_none());
    assert!(params.system_prompt.is_none());
    assert!(params.labels.is_none());
    assert!(params.additional_instructions.is_none());
    assert!(params.app_context.is_none());
    assert!(params.shell_env.is_none());
}

// ---------------------------------------------------------------------------
// 12. Usage -> WireUsage conversion
// ---------------------------------------------------------------------------

#[test]
fn wire_usage_from_usage_conversion() {
    let usage = Usage {
        input_tokens: 500,
        output_tokens: 250,
        cache_creation_tokens: Some(100),
        cache_read_tokens: Some(75),
    };

    let wire: WireUsage = usage.into();

    assert_eq!(wire.input_tokens, 500);
    assert_eq!(wire.output_tokens, 250);
    assert_eq!(wire.total_tokens, 750); // 500 + 250
    assert_eq!(wire.cache_creation_tokens, Some(100));
    assert_eq!(wire.cache_read_tokens, Some(75));
}
