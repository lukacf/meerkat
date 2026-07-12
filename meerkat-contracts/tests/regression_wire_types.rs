#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Regression tests that pin wire type field names and shapes.
//!
//! Pattern: construct -> serde_json::to_value() -> assert field presence by string key
//! -> deserialize back -> assert roundtrip.
//!
//! These catch silent renames and accidental field removals.

#[cfg(feature = "schema")]
use meerkat_contracts::emit::emit_all_schemas;
use meerkat_contracts::{
    BridgeInterruptPayload, ContractVersion, CoreCreateParams, ErrorCode, KNOWN_AGENT_EVENT_TYPES,
    RealtimeImageChunk, RealtimeInputChunk, WireError, WireEvent, WireRunResult,
    WireSessionHistory, WireSessionInfo, WireSessionMessage, WireSessionSummary, WireUsage,
};
use meerkat_core::event::BackgroundJobTerminalStatus;
use meerkat_core::{
    AgentErrorClass, AgentEvent, AssistantImageEvent, AssistantImageId, BlobId, BlobRef,
    BudgetType, ContentBlock, ContentInput, HookFailureReason, HookId, HookPoint, HookReasonCode,
    InteractionFailureReason, MediaType, Message, ProviderImageMetadata, RevisedPromptDisposition,
    RunResult, SessionId, SkillResolutionFailureReason, StopReason, ToolCallArguments,
    ToolConfigChangeOperation, ToolConfigChangeStatus, ToolConfigChangedPayload,
    TranscriptRevisionBody, TranscriptRewriteCommit, TranscriptRewriteReason,
    TranscriptRewriteRecord, TranscriptRewriteSelection, Usage, UserMessage,
    transcript_messages_digest,
};

fn tool_args(value: serde_json::Value) -> ToolCallArguments {
    ToolCallArguments::from_value(value).expect("test tool args must be an object")
}

fn rewrite_record_fixture() -> TranscriptRewriteRecord {
    let parent_messages = vec![Message::User(UserMessage::with_blocks(vec![
        ContentBlock::Text {
            text: "before rewrite".to_string(),
        },
    ]))];
    let revision_messages = vec![Message::User(UserMessage::with_blocks(vec![
        ContentBlock::Text {
            text: "after rewrite".to_string(),
        },
    ]))];
    let parent_revision = transcript_messages_digest(&parent_messages).expect("parent digest");
    let revision = transcript_messages_digest(&revision_messages).expect("revision digest");
    TranscriptRewriteRecord::new(
        TranscriptRewriteCommit {
            parent_revision: parent_revision.clone(),
            revision: revision.clone(),
            selection: TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
            original_span_digest: transcript_messages_digest(&parent_messages)
                .expect("original digest"),
            replacement_digest: transcript_messages_digest(&revision_messages)
                .expect("replacement digest"),
            messages_before: 1,
            messages_after: 1,
            reason: TranscriptRewriteReason::new("compaction"),
            actor: Some("test".to_string()),
            committed_at: meerkat_core::time_compat::SystemTime::now(),
        },
        TranscriptRevisionBody {
            revision: parent_revision,
            parent_revision: None,
            messages: parent_messages,
            created_at: meerkat_core::time_compat::SystemTime::now(),
        },
        TranscriptRevisionBody {
            revision,
            parent_revision: None,
            messages: revision_messages,
            created_at: meerkat_core::time_compat::SystemTime::now(),
        },
    )
    .expect("valid rewrite record")
}

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
        terminal_cause_kind: None,
        structured_output: None,
        extraction_error: None,
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
        terminal_cause_kind: None,
        structured_output: None,
        extraction_error: None,
        schema_warnings: None,
        skill_diagnostics: None,
    };
    let value = serde_json::to_value(&wire).unwrap();

    assert!(
        value.get("terminal_cause_kind").is_none(),
        "terminal_cause_kind should be absent when None"
    );
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
        terminal_cause_kind: None,
        structured_output: Some(serde_json::json!({"key": "value"})),
        extraction_error: None,
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
        model: "claude-sonnet-4-5".to_string(),
        provider: "anthropic".to_string(),
        last_assistant_text: None,
        resolved_capabilities: None,
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
    assert!(value.get("model").is_some(), "missing model");
    assert!(value.get("provider").is_some(), "missing provider");
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
                created_at: "2026-04-27T00:00:00Z".to_string(),
            },
            WireSessionMessage::User {
                content: meerkat_contracts::WireContentInput::Text("user".to_string()),
                transcript_role: meerkat_core::types::TranscriptUserRole::Conversational,
                interaction_id: None,
                run_id: None,
                created_at: "2026-04-27T00:00:01Z".to_string(),
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
                interaction_id: None,
                run_id: None,
                created_at: "2026-04-27T00:00:02Z".to_string(),
            },
            WireSessionMessage::ToolResults {
                results: vec![meerkat_contracts::WireToolResult {
                    tool_use_id: "tool-1".to_string(),
                    content: meerkat_contracts::WireToolResultContent::Text("ok".to_string()),
                    is_error: false,
                }],
                created_at: "2026-04-27T00:00:03Z".to_string(),
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
    assert_eq!(
        serde_json::to_value(&wire.messages).unwrap(),
        serde_json::to_value(&roundtrip.messages).unwrap()
    );
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
    let failed_skill_key = meerkat_core::skills::SkillKey::builtin(
        meerkat_core::skills::SkillName::parse("bad-skill").expect("valid name"),
    );

    // Variants that can be constructed without chrono or uuid (direct construction).
    let direct_variants: Vec<AgentEvent> = vec![
        AgentEvent::RunStarted {
            session_id: session_id.clone(),
            input: meerkat_core::types::RunInput::Content {
                content: ContentInput::Text("hello".to_string()),
            },
        },
        AgentEvent::RunCompleted {
            session_id: session_id.clone(),
            result: "done".to_string(),
            structured_output: Some(serde_json::json!({"ok": true})),
            extraction_required: false,
            usage: Usage {
                input_tokens: 10,
                output_tokens: 5,
                cache_creation_tokens: None,
                cache_read_tokens: None,
            },
            terminal_cause_kind: None,
        },
        AgentEvent::RunFailed {
            session_id,
            error_report: meerkat_core::event::AgentErrorReport {
                class: AgentErrorClass::Internal,
                reason: None,
                message: "boom".to_string(),
            },
            terminal_cause_kind: None,
        },
        AgentEvent::HookStarted {
            hook_id: HookId::new("h1"),
            point: HookPoint::PreLlmRequest,
        },
        AgentEvent::HookCompleted {
            hook_id: HookId::new("h1"),
            point: HookPoint::PostLlmResponse,
            duration_ms: 42,
        },
        AgentEvent::HookFailed {
            hook_id: HookId::new("h1"),
            point: HookPoint::RunStarted,
            reason: HookFailureReason::execution_failed("hook error"),
        },
        AgentEvent::HookDenied {
            hook_id: HookId::new("h1"),
            point: HookPoint::PreToolExecution,
            reason_code: HookReasonCode::PolicyViolation,
            message: "denied".to_string(),
            payload: None,
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
            args: tool_args(serde_json::json!({"path": "/tmp"})),
        },
        AgentEvent::ToolResultReceived {
            id: "tc1".to_string(),
            name: "read_file".to_string(),
            content: ContentBlock::text_vec("ok".to_string()),
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
            content: ContentBlock::text_vec("ok".to_string()),
            is_error: false,
            duration_ms: 100,
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
            reason: meerkat_core::CompactionFailureReason::LlmFailed {
                error_class: meerkat_core::AgentErrorClass::Llm,
                message: "LLM failed".to_string(),
            },
        },
        AgentEvent::BudgetWarning {
            budget_type: BudgetType::Tokens,
            used: 8000,
            limit: 10000,
            percent: 0.8,
        },
        AgentEvent::Retrying {
            retry: meerkat_core::LlmRetrySchedule {
                failure: meerkat_core::LlmRetryFailure {
                    provider: "anthropic".to_string(),
                    kind: meerkat_core::LlmRetryFailureKind::RateLimited,
                    retry_after_ms: Some(1000),
                    duration_ms: None,
                    message: "rate limited".to_string(),
                },
                plan: meerkat_core::LlmRetryPlan {
                    attempt: 1,
                    max_retries: 3,
                    computed_delay_ms: 1000,
                    selected_delay_ms: 1000,
                    retry_after_hint_ms: Some(1000),
                    rate_limit_floor_applied: false,
                    budget_capped: false,
                },
            },
        },
        AgentEvent::SkillsResolved {
            skills: vec![meerkat_core::skills::SkillKey::builtin(
                meerkat_core::skills::SkillName::parse("test-skill").expect("valid name"),
            )],
            injection_bytes: 256,
        },
        AgentEvent::SkillResolutionFailed {
            skill_key: Some(failed_skill_key.clone()),
            reason: SkillResolutionFailureReason::NotFound {
                key: failed_skill_key,
            },
        },
        // InteractionComplete and InteractionFailed are constructed via JSON
        // below to avoid a direct uuid crate dependency.
        AgentEvent::StreamTruncated {
            reason: meerkat_core::event::StreamTruncationReason::ChannelFull,
        },
        AgentEvent::ToolConfigChanged {
            payload: ToolConfigChangedPayload::new(
                ToolConfigChangeOperation::Add,
                "filesystem",
                ToolConfigChangeStatus::external_tool_delta(
                    meerkat_core::ExternalToolDeltaPhase::Applied,
                    None,
                ),
                true,
            )
            .with_applied_at_turn(Some(5)),
        },
        AgentEvent::background_job_completed(
            "j_123",
            "sleep 2",
            BackgroundJobTerminalStatus::Completed,
            "exit_code: 0",
        ),
        AgentEvent::TranscriptRewriteCommitted {
            session_id: SessionId::new(),
            record: rewrite_record_fixture(),
        },
    ];

    for event in &direct_variants {
        let json = serde_json::to_value(event).unwrap();

        // Every variant must have a "type" discriminator tag
        assert!(
            json.get("type").is_some(),
            "missing type tag on event: {event:?}"
        );
        if json["type"] == "skill_resolution_failed" {
            assert!(json.get("skill_key").is_some(), "missing typed skill_key");
            assert!(json.get("reason").is_some(), "missing typed failure reason");
            assert_eq!(json["reason"]["reason_type"], "not_found");
            assert!(
                json.get("reference").is_none(),
                "legacy display mirror `reference` must not be serialized"
            );
            assert!(
                json.get("error").is_none(),
                "legacy display mirror `error` must not be serialized"
            );
        }

        // Roundtrip
        let roundtrip: AgentEvent = serde_json::from_value(json.clone()).unwrap();
        let json2 = serde_json::to_value(&roundtrip).unwrap();
        assert_eq!(json, json2, "roundtrip mismatch for event: {event:?}");
    }

    // Variants constructed via JSON to avoid direct chrono/uuid crate dependencies.
    let json_constructed_variants: Vec<serde_json::Value> = vec![
        // InteractionComplete requires uuid::Uuid for InteractionId
        serde_json::json!({
            "type": "interaction_complete",
            "interaction_id": "550e8400-e29b-41d4-a716-446655440000",
            "result": "response"
        }),
        // InteractionFailed requires uuid::Uuid for InteractionId and the
        // typed `reason` cause (the `error` string is the fused display mirror).
        serde_json::json!({
            "type": "interaction_failed",
            "interaction_id": "550e8400-e29b-41d4-a716-446655440001",
            "reason": { "kind": "abandoned", "detail": "timeout" },
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

    // AgentEvent variants are covered: direct variants plus JSON-only UUID cases.
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
            input: meerkat_core::types::RunInput::Content {
                content: ContentInput::Text("hello".to_string()),
            },
        },
        AgentEvent::RunCompleted {
            session_id: SessionId::new(),
            result: "done".to_string(),
            structured_output: None,
            extraction_required: false,
            usage: Usage::default(),
            terminal_cause_kind: None,
        },
        AgentEvent::RunFailed {
            session_id: SessionId::new(),
            error_report: meerkat_core::event::AgentErrorReport {
                class: AgentErrorClass::Internal,
                reason: None,
                message: "nope".to_string(),
            },
            terminal_cause_kind: None,
        },
        AgentEvent::HookStarted {
            hook_id: HookId::new("hook-1"),
            point: HookPoint::RunStarted,
        },
        AgentEvent::HookCompleted {
            hook_id: HookId::new("hook-1"),
            point: HookPoint::RunStarted,
            duration_ms: 1,
        },
        AgentEvent::HookFailed {
            hook_id: HookId::new("hook-1"),
            point: HookPoint::RunStarted,
            reason: HookFailureReason::execution_failed("boom"),
        },
        AgentEvent::HookDenied {
            hook_id: HookId::new("hook-1"),
            point: HookPoint::RunStarted,
            reason_code: HookReasonCode::RuntimeError,
            message: "denied".to_string(),
            payload: None,
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
            args: tool_args(serde_json::json!({})),
        },
        AgentEvent::ToolResultReceived {
            id: "tool-1".to_string(),
            name: "search".to_string(),
            content: ContentBlock::text_vec("ok".to_string()),
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
            content: ContentBlock::text_vec("ok".to_string()),
            is_error: false,
            duration_ms: 1,
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
            reason: meerkat_core::CompactionFailureReason::LlmFailed {
                error_class: meerkat_core::AgentErrorClass::Llm,
                message: "failed".to_string(),
            },
        },
        AgentEvent::BudgetWarning {
            budget_type: BudgetType::Time,
            used: 1,
            limit: 2,
            percent: 50.0,
        },
        AgentEvent::Retrying {
            retry: meerkat_core::LlmRetrySchedule {
                failure: meerkat_core::LlmRetryFailure {
                    provider: "openai".to_string(),
                    kind: meerkat_core::LlmRetryFailureKind::RetryableProviderError,
                    retry_after_ms: None,
                    duration_ms: None,
                    message: "retry".to_string(),
                },
                plan: meerkat_core::LlmRetryPlan {
                    attempt: 1,
                    max_retries: 2,
                    computed_delay_ms: 100,
                    selected_delay_ms: 100,
                    retry_after_hint_ms: None,
                    rate_limit_floor_applied: false,
                    budget_capped: false,
                },
            },
        },
        AgentEvent::SkillsResolved {
            skills: vec![],
            injection_bytes: 0,
        },
        AgentEvent::SkillResolutionFailed {
            skill_key: None,
            reason: SkillResolutionFailureReason::Unknown {
                message: "missing".to_string(),
            },
        },
        AgentEvent::InteractionComplete {
            interaction_id: serde_json::from_value(serde_json::json!(
                "550e8400-e29b-41d4-a716-446655440000"
            ))
            .unwrap(),
            result: "ok".to_string(),
            structured_output: Some(serde_json::json!({"answer": 42})),
        },
        AgentEvent::InteractionFailed {
            interaction_id: serde_json::from_value(serde_json::json!(
                "550e8400-e29b-41d4-a716-446655440001"
            ))
            .unwrap(),
            reason: InteractionFailureReason::abandoned("failed"),
        },
        AgentEvent::StreamTruncated {
            reason: meerkat_core::event::StreamTruncationReason::StreamLagged { dropped: 3 },
        },
        AgentEvent::ToolConfigChanged {
            payload: ToolConfigChangedPayload::new(
                ToolConfigChangeOperation::Reload,
                "external",
                ToolConfigChangeStatus::external_tool_delta(
                    meerkat_core::ExternalToolDeltaPhase::Applied,
                    None,
                ),
                true,
            )
            .with_applied_at_turn(Some(1)),
        },
        AgentEvent::AssistantImageAppended {
            image: AssistantImageEvent {
                image_id: AssistantImageId::new(uuid::Uuid::new_v4()),
                blob_ref: BlobRef {
                    blob_id: BlobId::new("image-1"),
                    media_type: "image/png".to_string(),
                },
                media_type: MediaType::new("image/png"),
                width: 1024,
                height: 1024,
                revised_prompt: RevisedPromptDisposition::NotRequested,
                meta: ProviderImageMetadata::NotEmitted,
            },
        },
        AgentEvent::background_job_completed(
            "j_123",
            "sleep 2",
            BackgroundJobTerminalStatus::Completed,
            "exit_code: 0",
        ),
        AgentEvent::TranscriptRewriteCommitted {
            session_id: SessionId::new(),
            record: rewrite_record_fixture(),
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

#[cfg(feature = "schema")]
#[test]
fn emitted_rpc_catalog_type_names_resolve_to_schema_artifacts() {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "meerkat-contracts-schema-test-{}-{unique}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create schema tempdir");
    emit_all_schemas(&dir).expect("emit schemas");

    // Union of every emitted flat type-map artifact. SDK codegen and JSON-RPC
    // clients resolve a catalog `params_type` / `result_type` against this whole
    // set, not just params.json + wire-types.json. A catalog type name with no
    // emitted schema anywhere is Generated-Artifact Theater (Dogma Rule 9): the
    // SDK cannot generate it and the named contract does not exist.
    let mut defined: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for file in [
        "params.json",
        "wire-types.json",
        "runtime-host.json",
        "models.json",
        "capabilities.json",
        "errors.json",
        "events.json",
    ] {
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join(file)).expect(file)).expect(file);
        if let Some(object) = value.as_object() {
            for (name, schema) in object {
                if schema.is_object() {
                    defined.insert(name.clone());
                }
            }
        }
    }

    let rpc_methods: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("rpc-methods.json")).expect("rpc methods"))
            .expect("parse rpc methods");
    let methods = rpc_methods["methods"].as_array().expect("methods array");

    // Methods whose params embed deep `meerkat-core` domain graphs that do not
    // yet derive `JsonSchema` (`ContentInput`, `ToolDef`, `PeerMeta`,
    // `BudgetLimits`, hook/skill overlays). The request body is an open object
    // in generated SDKs until these are promoted to contracts wire types. This
    // allowlist must only SHRINK — adding a new entry is a regression and must
    // be justified.
    let unschematized_params: std::collections::BTreeSet<&str> =
        ["CreateSessionParams", "StartTurnParams"]
            .into_iter()
            .collect();

    let resolves = |type_name: &str| -> bool {
        type_name
            .split('|')
            .map(str::trim)
            .all(|part| part == "Value" || defined.contains(part))
    };

    let mut missing: Vec<String> = Vec::new();
    let mut checked = 0usize;
    for method in methods {
        let name = method["name"].as_str().expect("method name");
        checked += 1;
        if let Some(params_type) = method.get("params_type").and_then(|value| value.as_str())
            && !unschematized_params.contains(params_type)
            && !resolves(params_type)
        {
            missing.push(format!("{name}: params_type `{params_type}`"));
        }
        if let Some(result_type) = method.get("result_type").and_then(|value| value.as_str())
            && !resolves(result_type)
        {
            missing.push(format!("{name}: result_type `{result_type}`"));
        }
    }

    assert!(
        checked >= 100,
        "expected to check the full RPC catalog, only saw {checked} methods"
    );
    assert!(
        missing.is_empty(),
        "rpc_catalog names types with no emitted schema artifact \
         (Generated-Artifact Theater — define the type in meerkat-contracts so \
         emit-schemas lands it, or fix the catalog name):\n  {}",
        missing.join("\n  ")
    );
    std::fs::remove_dir_all(&dir).expect("remove schema tempdir");
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
        terminal_cause_kind: None,
        structured_output: Some(serde_json::json!({"answer": 42})),
        extraction_error: None,
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
    assert!(params.system_prompt.is_inherit());
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

// ---------------------------------------------------------------------------
// 13. RealtimeInputChunk audio variant pins kind tag (live-adapter shared shape)
// ---------------------------------------------------------------------------

#[test]
fn realtime_input_chunk_audio_variant_kind_tag() {
    let input_chunk = RealtimeInputChunk::AudioChunk(meerkat_contracts::RealtimeAudioChunk {
        mime_type: "audio/pcm".to_string(),
        sample_rate_hz: 24_000,
        channels: 1,
        data: "AQID".to_string(),
    });
    let input_value = serde_json::to_value(&input_chunk).unwrap();
    assert_eq!(
        input_value.get("kind").and_then(|v| v.as_str()),
        Some("audio_chunk")
    );
}

#[test]
fn realtime_image_chunk_is_a_named_public_contract() {
    let input_chunk = RealtimeInputChunk::ImageChunk(RealtimeImageChunk {
        idempotency_key: "image-request-1".to_string(),
        mime_type: "image/png".to_string(),
        data: "iVBORw0KGgo=".to_string(),
    });
    let input_value = serde_json::to_value(&input_chunk).unwrap();
    assert_eq!(
        input_value.get("kind").and_then(|value| value.as_str()),
        Some("image_chunk")
    );
    assert_eq!(input_value["mime_type"], "image/png");
}

#[test]
fn bridge_interrupt_payload_is_exported_from_the_crate_root() {
    fn assert_root_export<T>() {}
    assert_root_export::<BridgeInterruptPayload>();
}
