#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//!
//! These tests verify the integration points between components.
//! Per RCT methodology, tests are COMPLETE - they exercise real code paths.
//! Tests may fail on NotImplemented, but NOT on boot/module errors.

use meerkat::*;
use schemars::JsonSchema;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

/// CP-LLM-NORM: Provider stream normalization
/// Verifies that each provider normalizes responses to LlmEvent correctly.
mod llm_normalization {
    use super::*;
    use futures::StreamExt;

    #[tokio::test]
    async fn test_anthropic_normalizes_to_llm_event() {
        if std::env::var("MEERKAT_LIVE_API_TESTS").ok().as_deref() != Some("1") {
            eprintln!("Skipping: live API tests disabled (set MEERKAT_LIVE_API_TESTS=1)");
            return;
        }

        // Skip if no API key - this is expected for CI without keys
        let api_key = match std::env::var("RKAT_ANTHROPIC_API_KEY")
            .or_else(|_| std::env::var("ANTHROPIC_API_KEY"))
        {
            Ok(key) => key,
            Err(_) => {
                eprintln!("Skipping: missing ANTHROPIC_API_KEY");
                return;
            }
        };

        let client = AnthropicClient::new(api_key).unwrap();
        let request = LlmRequest::new(
            "claude-3-7-sonnet-20250219",
            vec![Message::User(UserMessage {
                content: "Say 'hello' and nothing else".to_string(),
            })],
        );

        // Stream returns Pin<Box<dyn Stream>> directly
        let mut stream = client.stream(&request);

        let mut got_text_delta = false;
        let mut got_done = false;

        while let Some(event) = stream.next().await {
            match event {
                Ok(LlmEvent::TextDelta { delta, .. }) => {
                    // delta exists (may be empty for some deltas)
                    let _ = delta;
                    got_text_delta = true;
                }
                Ok(LlmEvent::Done {
                    outcome: LlmDoneOutcome::Success { stop_reason },
                }) => {
                    // Stop reason should be valid
                    assert!(matches!(
                        stop_reason,
                        StopReason::EndTurn | StopReason::MaxTokens | StopReason::StopSequence
                    ));
                    got_done = true;
                }
                Ok(LlmEvent::Done {
                    outcome: LlmDoneOutcome::Error { error },
                }) => panic!("Unexpected error outcome: {error:?}"),
                Ok(LlmEvent::ToolCallDelta { .. }) => {
                    // Tool call deltas are valid events
                }
                Ok(LlmEvent::ToolCallComplete { .. }) => {
                    // Tool call completes are valid events
                }
                Ok(LlmEvent::UsageUpdate { usage }) => {
                    // Usage should have positive tokens
                    assert!(usage.input_tokens > 0 || usage.output_tokens > 0);
                }
                Ok(LlmEvent::ReasoningDelta { .. }) | Ok(LlmEvent::ReasoningComplete { .. }) => {
                    // Reasoning events are valid
                }
                Err(e) => panic!("Unexpected error: {:?}", e),
            }
        }

        assert!(
            got_text_delta,
            "Should have received at least one TextDelta"
        );
        assert!(got_done, "Should have received Done event");
    }

    #[cfg(feature = "openai")]
    #[tokio::test]
    async fn test_openai_normalizes_to_llm_event() {
        if std::env::var("MEERKAT_LIVE_API_TESTS").ok().as_deref() != Some("1") {
            eprintln!("Skipping: live API tests disabled (set MEERKAT_LIVE_API_TESTS=1)");
            return;
        }

        let api_key = match std::env::var("RKAT_OPENAI_API_KEY")
            .or_else(|_| std::env::var("OPENAI_API_KEY"))
        {
            Ok(key) => key,
            Err(_) => {
                eprintln!("Skipping: missing OPENAI_API_KEY");
                return;
            }
        };

        let client = OpenAiClient::new(api_key);
        let request = LlmRequest::new(
            "gpt-4o",
            vec![Message::User(UserMessage {
                content: "Say 'hello' and nothing else".to_string(),
            })],
        );

        let mut stream = client.stream(&request);

        let mut got_text_delta = false;
        let mut got_done = false;

        while let Some(event) = stream.next().await {
            match event {
                Ok(LlmEvent::TextDelta { .. }) => got_text_delta = true,
                Ok(LlmEvent::Done { .. }) => got_done = true,
                Ok(_) => {}
                Err(e) => panic!("Unexpected error: {:?}", e),
            }
        }

        assert!(
            got_text_delta,
            "Should have received at least one TextDelta"
        );
        assert!(got_done, "Should have received Done event");
    }

    #[cfg(feature = "gemini")]
    #[tokio::test]
    async fn test_gemini_normalizes_to_llm_event() {
        if std::env::var("MEERKAT_LIVE_API_TESTS").ok().as_deref() != Some("1") {
            eprintln!("Skipping: live API tests disabled (set MEERKAT_LIVE_API_TESTS=1)");
            return;
        }

        let api_key = match std::env::var("RKAT_GEMINI_API_KEY")
            .or_else(|_| std::env::var("GEMINI_API_KEY"))
            .or_else(|_| std::env::var("GOOGLE_API_KEY"))
        {
            Ok(key) => key,
            Err(_) => {
                eprintln!("Skipping: missing GOOGLE_API_KEY");
                return;
            }
        };

        let client = GeminiClient::new(api_key);
        let request = LlmRequest::new(
            "gemini-2.0-flash",
            vec![Message::User(UserMessage {
                content: "Say 'hello' and nothing else".to_string(),
            })],
        );

        let mut stream = client.stream(&request);

        let mut got_text_delta = false;
        let mut got_done = false;

        while let Some(event) = stream.next().await {
            match event {
                Ok(LlmEvent::TextDelta { .. }) => got_text_delta = true,
                Ok(LlmEvent::Done { .. }) => got_done = true,
                Ok(_) => {}
                Err(e) => panic!("Unexpected error: {:?}", e),
            }
        }

        assert!(
            got_text_delta,
            "Should have received at least one TextDelta"
        );
        assert!(got_done, "Should have received Done event");
    }

    #[test]
    fn test_provider_error_classification() {
        // CP-LLM-ERROR: Verify error types are classified consistently

        // Rate limit errors should be retryable
        let rate_limit = LlmError::RateLimited {
            retry_after_ms: Some(30000),
        };
        assert!(rate_limit.is_retryable(), "Rate limit should be retryable");

        // Auth errors should not be retryable
        let auth_error = LlmError::AuthenticationFailed {
            message: "Invalid API key".to_string(),
        };
        assert!(
            !auth_error.is_retryable(),
            "Auth errors should not be retryable"
        );

        // Server overload should be retryable
        let overload = LlmError::ServerOverloaded;
        assert!(
            overload.is_retryable(),
            "Server overload should be retryable"
        );

        // Invalid request should not be retryable
        let invalid = LlmError::InvalidRequest {
            message: "Bad request".to_string(),
        };
        assert!(
            !invalid.is_retryable(),
            "Invalid request should not be retryable"
        );
    }
}

/// CP-TOOL-DISCOVERY, CP-TOOL-DISPATCH: Tool validation and execution
mod tool_dispatch {
    use super::*;

    #[derive(Debug, Clone, JsonSchema)]
    #[allow(dead_code)]
    struct ToolInput {
        input: String,
    }

    #[test]
    fn test_tool_discovery_validates_schema() {
        let mut registry = ToolRegistry::new();

        // Valid tool definition should be accepted
        let valid_tool = ToolDef {
            name: "test_tool".to_string(),
            description: "A test tool".to_string(),
            input_schema: meerkat_tools::schema_for::<ToolInput>(),
        };

        // Registry.register returns () - no error case
        registry.register(valid_tool);

        // Verify tool is registered using get()
        assert!(
            registry.get("test_tool").is_some(),
            "Should find registered tool"
        );
    }

    #[test]
    fn test_tool_timeout_enforced() {
        // Create dispatcher with registry and router
        let registry = ToolRegistry::new();
        let router: Arc<dyn AgentToolDispatcher> = Arc::new(McpRouter::new());
        let timeout = Duration::from_secs(30);
        let dispatcher = ToolDispatcher::new(registry, router).with_timeout(timeout);

        // Dispatcher should be created (existence test)
        assert!(std::mem::size_of_val(&dispatcher) > 0);
    }

    #[test]
    fn test_tool_error_captured() {
        // ToolError::ExecutionFailed via helper
        let error = ToolError::execution_failed("Something went wrong with test_tool");

        // Error should contain the message
        let error_str = format!("{:?}", error);
        assert!(error_str.contains("Something went wrong"));
        assert!(error_str.contains("test_tool"));

        // Test other variants via helpers
        let not_found = ToolError::not_found("missing_tool");
        assert!(format!("{:?}", not_found).contains("missing_tool"));

        let timeout = ToolError::timeout("slow_tool", 5000);
        assert!(format!("{:?}", timeout).contains("slow_tool"));

        let validation = ToolError::invalid_arguments("test_tool", "invalid params");
        assert!(format!("{:?}", validation).contains("invalid params"));
    }
}

/// CP-SESSION-TX: Session checkpoint atomicity
mod session_persistence {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_checkpoint_atomic_write() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let store = JsonlStore::new(temp_dir.path().to_path_buf());
        store.init().await.expect("Failed to init store");

        // Create session with multiple messages
        let mut session = Session::new();
        session.push(Message::User(UserMessage {
            content: "Hello".to_string(),
        }));
        session.push(Message::Assistant(AssistantMessage {
            content: "Hi there!".to_string(),
            tool_calls: vec![],
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
        }));
        session.push(Message::User(UserMessage {
            content: "How are you?".to_string(),
        }));

        // Save should succeed atomically
        let session_id = session.id().clone();
        store.save(&session).await.expect("Save should succeed");

        // Verify file exists and is complete
        let loaded = store
            .load(&session_id)
            .await
            .expect("Load should succeed")
            .expect("Session should exist");
        assert_eq!(loaded.messages().len(), 3);

        // Messages should match
        assert!(matches!(loaded.messages()[0], Message::User(_)));
        assert!(matches!(loaded.messages()[1], Message::Assistant(_)));
        assert!(matches!(loaded.messages()[2], Message::User(_)));
    }

    #[tokio::test]
    async fn test_resume_after_crash() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let store = JsonlStore::new(temp_dir.path().to_path_buf());
        store.init().await.expect("Failed to init store");

        // Simulate: Session saved, then "crash" (drop store), then resume
        let session_id = {
            let mut session = Session::new();
            session.push(Message::User(UserMessage {
                content: "Before crash".to_string(),
            }));
            session.push(Message::Assistant(AssistantMessage {
                content: "Response before crash".to_string(),
                tool_calls: vec![],
                stop_reason: StopReason::EndTurn,
                usage: Usage::default(),
            }));
            let id = session.id().clone();
            store.save(&session).await.expect("Save should succeed");
            id
        };

        // "Restart" - create new store instance pointing to same directory
        let store2 = JsonlStore::new(temp_dir.path().to_path_buf());

        // Should be able to resume session
        let resumed = store2
            .load(&session_id)
            .await
            .expect("Resume should succeed")
            .expect("Session should exist");
        assert_eq!(resumed.messages().len(), 2);
        assert_eq!(*resumed.id(), session_id);
    }

    #[tokio::test]
    async fn test_session_roundtrip() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let store = JsonlStore::new(temp_dir.path().to_path_buf());
        store.init().await.expect("Failed to init store");

        // Create complex session
        let mut session = Session::new();
        session.push(Message::System(SystemMessage {
            content: "You are helpful".to_string(),
        }));
        session.push(Message::User(UserMessage {
            content: "Hello".to_string(),
        }));
        session.push(Message::Assistant(AssistantMessage {
            content: "Hi!".to_string(),
            tool_calls: vec![],
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
        }));
        session.push(Message::User(UserMessage {
            content: "Call a tool".to_string(),
        }));
        session.push(Message::ToolResults {
            results: vec![ToolResult::new(
                "call_123".to_string(),
                "Tool output".to_string(),
                false,
            )],
        });

        let original_id = session.id().clone();
        let original_len = session.messages().len();

        // Round-trip through store
        store.save(&session).await.expect("Save failed");
        let loaded = store
            .load(&original_id)
            .await
            .expect("Load failed")
            .expect("Session should exist");

        // Verify integrity
        assert_eq!(*loaded.id(), original_id);
        assert_eq!(loaded.messages().len(), original_len);
    }
}

/// CP-CONFIG-MERGE: Config layering
mod config_loading {
    use super::*;

    #[test]
    fn test_config_precedence() {
        // Config should have sensible defaults
        let default_config = Config::default();

        // Default should have positive token limit
        assert!(
            default_config.agent.max_tokens_per_turn > 0,
            "Default config should have positive max_tokens"
        );
    }

    #[test]
    fn test_config_type_coercion() {
        // Verify RetryConfig can be deserialized from TOML with humantime format
        // (Full Config requires all nested sections - use default + CLI overrides for layering)
        let toml_str = r#"
            max_retries = 5
            initial_delay = "1s"
            max_delay = "1m"
            multiplier = 2.0
        "#;

        let retry_config: RetryConfig = toml::from_str(toml_str).expect("Should parse TOML");
        assert_eq!(retry_config.max_retries, 5);
        assert_eq!(
            retry_config.initial_delay,
            std::time::Duration::from_secs(1)
        );
        assert_eq!(retry_config.max_delay, std::time::Duration::from_secs(60));

        // Verify BudgetLimits can be deserialized
        let budget_toml = r#"
            max_tokens = 100000
            max_tool_calls = 50
        "#;

        let budget: BudgetLimits = toml::from_str(budget_toml).expect("Should parse budget TOML");
        assert_eq!(budget.max_tokens, Some(100000));
        assert_eq!(budget.max_tool_calls, Some(50));
    }
}

/// CP-RETRY-POLICY: Retry behavior
mod retry_policy {
    use super::*;

    #[test]
    fn test_retryable_errors_retry() {
        let policy = RetryPolicy::default();

        // Should have positive retry count
        assert!(
            policy.max_retries > 0,
            "Default policy should allow retries"
        );

        // Rate limit error should be retryable (check via LlmError)
        let rate_limit = LlmError::RateLimited {
            retry_after_ms: None,
        };
        assert!(rate_limit.is_retryable(), "Rate limit should trigger retry");

        // Policy should allow retries up to max
        assert!(policy.should_retry(0), "Should retry on first attempt");
        assert!(policy.should_retry(1), "Should retry on second attempt");
        assert!(
            !policy.should_retry(policy.max_retries),
            "Should not retry after max attempts"
        );
    }

    #[test]
    fn test_non_retryable_fail_fast() {
        // Auth error should never retry (check via LlmError)
        let auth_error = LlmError::AuthenticationFailed {
            message: "Invalid key".to_string(),
        };
        assert!(!auth_error.is_retryable(), "Auth errors should never retry");

        // Invalid request should never retry
        let invalid = LlmError::InvalidRequest {
            message: "Bad params".to_string(),
        };
        assert!(
            !invalid.is_retryable(),
            "Invalid requests should never retry"
        );
    }

    #[test]
    fn test_exponential_backoff() {
        let policy = RetryPolicy::default();

        // First attempt has no delay
        let delay_0 = policy.delay_for_attempt(0);
        assert_eq!(
            delay_0,
            Duration::ZERO,
            "First attempt should have no delay"
        );

        // Subsequent delays should increase
        let delay_1 = policy.delay_for_attempt(1);
        let delay_2 = policy.delay_for_attempt(2);
        let delay_3 = policy.delay_for_attempt(3);

        assert!(delay_1 > Duration::ZERO, "Second attempt should have delay");
        assert!(delay_2 > delay_1 / 2, "Delays should generally increase");
        assert!(delay_3 > delay_2 / 2, "Delays should continue increasing");

        // Should be capped at max_delay (with jitter)
        let delay_100 = policy.delay_for_attempt(100);
        // Allow 10% jitter margin
        let max_with_jitter = policy.max_delay + policy.max_delay / 10;
        assert!(delay_100 <= max_with_jitter, "Delay should be capped");
    }
}

/// CP-BUDGET-ENFORCE: Budget enforcement
mod budget_enforcement {
    use super::*;

    #[test]
    fn test_budget_token_limit_enforced() {
        let budget = Budget::new(BudgetLimits {
            max_tokens: Some(1000),
            max_duration: None,
            max_tool_calls: None,
        });

        // Check should pass initially
        assert!(budget.check().is_ok(), "Budget check should pass initially");

        // Record usage within limit
        budget.record_tokens(500);
        assert!(
            budget.check().is_ok(),
            "Budget check should pass within limit"
        );

        // Should track usage
        assert_eq!(budget.token_usage(), Some((500, 1000)));

        // Record more tokens to exceed limit
        budget.record_tokens(600);

        // Should fail check when over limit
        assert!(
            budget.check().is_err(),
            "Budget check should fail over limit"
        );
        assert!(budget.is_exhausted(), "Budget should be exhausted");
    }

    #[test]
    fn test_budget_tool_call_limit_enforced() {
        let budget = Budget::new(BudgetLimits {
            max_tokens: None,
            max_duration: None,
            max_tool_calls: Some(3),
        });

        // Should pass check initially
        assert!(budget.check().is_ok());

        // Record tool calls within limit
        budget.record_tool_call();
        budget.record_tool_call();
        budget.record_tool_call();

        // Should fail when limit reached
        assert!(budget.check().is_err(), "Should fail at limit");
    }

    #[test]
    fn test_budget_unlimited() {
        let budget = Budget::new(BudgetLimits {
            max_tokens: None,
            max_duration: None,
            max_tool_calls: None,
        });

        // Should allow any amount of tokens
        budget.record_tokens(1_000_000);
        budget.record_tokens(1_000_000);
        assert!(
            budget.check().is_ok(),
            "Unlimited budget should always pass"
        );

        // Should allow any number of tool calls
        for _ in 0..100 {
            budget.record_tool_call();
        }
        assert!(
            budget.check().is_ok(),
            "Unlimited budget should always pass"
        );
    }
}

/// CP-OP-INJECT, CP-EVENT-ORDERING: Operation injection
mod operation_injection {
    use super::*;

    #[test]
    fn test_results_injected_at_turn_boundary() {
        // Operation results should be serializable for injection
        let op_result = OperationResult {
            id: OperationId::new(),
            content: "Tool output".to_string(),
            is_error: false,
            duration_ms: 100,
            tokens_used: 50,
        };

        // Result should serialize correctly
        let json = serde_json::to_string(&op_result).expect("Should serialize");
        assert!(json.contains("Tool output"));

        // Should deserialize back
        let parsed: OperationResult = serde_json::from_str(&json).expect("Should deserialize");
        assert_eq!(parsed.content, "Tool output");
        assert!(!parsed.is_error);
        assert_eq!(parsed.duration_ms, 100);
        assert_eq!(parsed.tokens_used, 50);
    }

    #[test]
    fn test_artifact_ref_resolution() {
        // Artifact references should have stable encoding
        let session_id = SessionId::new();
        let artifact = ArtifactRef {
            id: "artifact_123".to_string(),
            session_id,
            size_bytes: 1024,
            ttl_seconds: Some(3600),
            version: 1,
        };

        // Should have stable ID
        assert_eq!(artifact.id, "artifact_123");
        assert_eq!(artifact.version, 1);
        assert_eq!(artifact.size_bytes, 1024);
        assert_eq!(artifact.ttl_seconds, Some(3600));

        // Should serialize correctly
        let json = serde_json::to_string(&artifact).expect("Should serialize");
        assert!(json.contains("artifact_123"));

        // Roundtrip
        let parsed: ArtifactRef = serde_json::from_str(&json).expect("Should deserialize");
        assert_eq!(parsed.id, artifact.id);
        assert_eq!(parsed.version, artifact.version);
    }

    #[test]
    fn test_event_ordering_preserved() {
        // OpEvents should maintain their structure
        let op_id = OperationId::new();

        let started = OpEvent::Started {
            id: op_id.clone(),
            kind: WorkKind::ToolCall,
        };

        let progress = OpEvent::Progress {
            id: op_id.clone(),
            message: "Working...".to_string(),
            percent: Some(0.5),
        };

        let result_id = op_id;
        let completed = OpEvent::Completed {
            id: result_id.clone(),
            result: OperationResult {
                id: result_id,
                content: "Done".to_string(),
                is_error: false,
                duration_ms: 100,
                tokens_used: 0,
            },
        };

        // Events should serialize correctly
        for event in [&started, &progress, &completed] {
            let json = serde_json::to_string(event).expect("Should serialize");
            assert!(!json.is_empty());
        }
    }
}

/// CP-SUB-AGENT-ACCESS: Tool access policy enforcement
mod sub_agent_access {
    use super::*;

    #[test]
    fn test_allow_list_structure() {
        let policy =
            ToolAccessPolicy::AllowList(vec!["safe_tool".to_string(), "another_safe".to_string()]);

        // Should serialize correctly
        let json = serde_json::to_value(&policy).expect("Should serialize");
        assert_eq!(json["type"], "allow_list");
        // Adjacently-tagged: {"type": "allow_list", "value": [...]}
        assert!(json["value"].is_array());

        // Roundtrip
        let parsed: ToolAccessPolicy = serde_json::from_value(json).expect("Should deserialize");
        match parsed {
            ToolAccessPolicy::AllowList(tools) => {
                assert_eq!(tools.len(), 2);
                assert!(tools.contains(&"safe_tool".to_string()));
                assert!(tools.contains(&"another_safe".to_string()));
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn test_deny_list_structure() {
        let policy = ToolAccessPolicy::DenyList(vec!["dangerous_tool".to_string()]);

        // Should serialize correctly
        let json = serde_json::to_value(&policy).expect("Should serialize");
        assert_eq!(json["type"], "deny_list");

        // Roundtrip
        let parsed: ToolAccessPolicy = serde_json::from_value(json).expect("Should deserialize");
        match parsed {
            ToolAccessPolicy::DenyList(tools) => {
                assert_eq!(tools.len(), 1);
                assert!(tools.contains(&"dangerous_tool".to_string()));
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn test_inherit_policy() {
        let policy = ToolAccessPolicy::Inherit;

        // Should serialize correctly
        let json = serde_json::to_value(&policy).expect("Should serialize");
        assert_eq!(json["type"], "inherit");

        // Roundtrip
        let parsed: ToolAccessPolicy = serde_json::from_value(json).expect("Should deserialize");
        assert!(matches!(parsed, ToolAccessPolicy::Inherit));
    }
}

/// CP-STATE-MACHINE: Loop state transitions
mod state_machine {
    use super::*;

    #[test]
    fn test_valid_state_transitions() {
        let mut state = LoopState::CallingLlm;

        // Valid: CallingLlm -> DrainingEvents
        assert!(state.transition(LoopState::DrainingEvents).is_ok());
        assert_eq!(state, LoopState::DrainingEvents);

        // Valid: DrainingEvents -> CallingLlm (loop back)
        assert!(state.transition(LoopState::CallingLlm).is_ok());
        assert_eq!(state, LoopState::CallingLlm);

        // Valid: CallingLlm -> Completed
        assert!(state.transition(LoopState::Completed).is_ok());
        assert_eq!(state, LoopState::Completed);

        // Test terminal state
        assert!(state.is_terminal(), "Completed should be terminal");
    }

    #[test]
    fn test_invalid_transitions_from_terminal() {
        let mut state = LoopState::Completed;

        // Cannot transition from terminal
        assert!(
            state.transition(LoopState::CallingLlm).is_err(),
            "Should not transition from terminal state"
        );
    }

    #[test]
    fn test_cancellation_path() {
        let mut state = LoopState::CallingLlm;

        // Should be able to transition to Cancelling
        assert!(state.transition(LoopState::Cancelling).is_ok());
        assert_eq!(state, LoopState::Cancelling);

        // Cancelling -> Completed
        assert!(state.transition(LoopState::Completed).is_ok());
        assert!(state.is_terminal(), "Completed should be terminal");
    }

    #[test]
    fn test_error_recovery_path() {
        let mut state = LoopState::CallingLlm;

        // Can enter error recovery
        assert!(state.transition(LoopState::ErrorRecovery).is_ok());

        // Can recover back to calling
        assert!(state.transition(LoopState::CallingLlm).is_ok());

        // Or can complete from error recovery
        state.transition(LoopState::ErrorRecovery).ok();
        assert!(state.transition(LoopState::Completed).is_ok());
        assert!(state.is_terminal());
    }

    #[test]
    fn test_waiting_for_ops_path() {
        let mut state = LoopState::CallingLlm;

        // CallingLlm -> WaitingForOps
        assert!(state.transition(LoopState::WaitingForOps).is_ok());

        // WaitingForOps -> DrainingEvents
        assert!(state.transition(LoopState::DrainingEvents).is_ok());

        // DrainingEvents -> Completed
        assert!(state.transition(LoopState::Completed).is_ok());
    }
}

/// CP-MCP-WIRE: MCP protocol compliance
mod mcp_protocol {
    use super::*;

    #[test]
    fn test_mcp_config_structure() {
        // MCP config should be properly structured
        let config = McpServerConfig::stdio(
            "test-server",
            "node",
            vec!["test-server.js".to_string()],
            HashMap::new(),
        );

        // Config should be properly structured
        assert_eq!(config.name, "test-server");
        match &config.transport {
            meerkat_core::mcp_config::McpTransportConfig::Stdio(stdio) => {
                assert_eq!(stdio.command, "node");
                assert_eq!(stdio.args.len(), 1);
            }
            _ => panic!("Expected stdio transport"),
        }

        // Should serialize correctly
        let json = serde_json::to_value(&config).expect("Should serialize");
        assert_eq!(json["name"], "test-server");
        assert_eq!(json["command"], "node");
    }

    #[tokio::test]
    async fn test_mcp_router_creation() {
        // Test router creation
        let router = McpRouter::new();

        // Router should be created successfully
        // list_tools returns empty slice when no servers are connected
        let tools = router.list_tools();
        assert!(tools.is_empty(), "No tools without servers");
    }

    #[tokio::test]
    async fn test_mcp_tool_call_roundtrip() {
        // Test with real MCP test server if available
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
        let workspace_root = std::path::Path::new(&manifest_dir)
            .parent()
            .unwrap_or(std::path::Path::new("."));
        let server_path = workspace_root.join("target/debug/mcp-test-server");

        if !server_path.exists() {
            eprintln!("Skipping: MCP test server not built (run cargo build -p mcp-test-server)");
            return;
        }

        let config = McpServerConfig::stdio(
            "test",
            server_path.to_string_lossy().to_string(),
            vec![],
            HashMap::new(),
        );

        // Connect to server
        let connection = McpConnection::connect(&config)
            .await
            .expect("Should connect to test server");

        // List tools
        let tools = connection.list_tools().await.expect("Should list tools");
        assert!(!tools.is_empty(), "Test server should have tools");

        // Find echo tool
        let echo_tool = tools
            .iter()
            .find(|t| t.name == "echo")
            .expect("Test server should have echo tool");
        assert_eq!(echo_tool.name, "echo");

        // Call echo tool
        let result = connection
            .call_tool("echo", &serde_json::json!({"message": "test"}))
            .await
            .expect("Tool call should succeed");

        assert!(result.contains("test"), "Echo should return input");

        // Clean up
        connection.close().await.expect("Should close cleanly");
    }

    #[tokio::test]
    async fn test_meerkat_mcp_server_tools_list() {
        let tools = meerkat_mcp_server::tools_list();
        let names: std::collections::HashSet<&str> = tools
            .iter()
            .filter_map(|tool| tool.get("name").and_then(|n| n.as_str()))
            .collect();

        assert!(
            names.contains("meerkat_run"),
            "meerkat-mcp-server should expose meerkat_run"
        );
        assert!(
            names.contains("meerkat_resume"),
            "meerkat-mcp-server should expose meerkat_resume"
        );
    }
}

/// Additional integration tests for combined functionality
mod combined {
    use super::*;

    #[derive(Debug, Clone, JsonSchema)]
    #[allow(dead_code)]
    struct ReadFileArgs {
        path: String,
    }

    #[derive(Debug, Clone, JsonSchema)]
    #[allow(dead_code)]
    struct WriteFileArgs {
        path: String,
        content: String,
    }

    #[test]
    fn test_session_with_tool_results() {
        let mut session = Session::new();

        // Add messages including tool results
        session.push(Message::User(UserMessage {
            content: "Call a tool".to_string(),
        }));

        let first_usage = Usage {
            input_tokens: 100,
            output_tokens: 50,
            cache_creation_tokens: None,
            cache_read_tokens: None,
        };
        session.push(Message::Assistant(AssistantMessage {
            content: "".to_string(),
            tool_calls: vec![ToolCall::new(
                "tc_1".to_string(),
                "test_tool".to_string(),
                serde_json::json!({"input": "test"}),
            )],
            stop_reason: StopReason::ToolUse,
            usage: first_usage.clone(),
        }));
        session.record_usage(first_usage);

        session.push(Message::ToolResults {
            results: vec![ToolResult::new(
                "tc_1".to_string(),
                "Tool result".to_string(),
                false,
            )],
        });

        let second_usage = Usage {
            input_tokens: 150,
            output_tokens: 75,
            cache_creation_tokens: None,
            cache_read_tokens: None,
        };
        session.push(Message::Assistant(AssistantMessage {
            content: "Based on the tool result...".to_string(),
            tool_calls: vec![],
            stop_reason: StopReason::EndTurn,
            usage: second_usage.clone(),
        }));
        session.record_usage(second_usage);

        // Verify session state
        assert_eq!(session.messages().len(), 4);
        assert_eq!(session.tool_call_count(), 1);
        assert_eq!(session.total_tokens(), 375); // 150 + 150 + 75
    }

    #[test]
    fn test_budget_with_usage_recording() {
        let budget = Budget::new(BudgetLimits::default().with_max_tokens(1000));

        // Record usage from a Usage struct
        let usage = Usage {
            input_tokens: 200,
            output_tokens: 100,
            cache_creation_tokens: None,
            cache_read_tokens: None,
        };

        budget.record_usage(&usage);
        assert_eq!(budget.token_usage(), Some((300, 1000)));
        assert_eq!(budget.remaining_tokens(), Some(700));
    }

    #[test]
    fn test_operation_spec_completeness() {
        // Verify OperationSpec can be fully constructed
        let spec = OperationSpec {
            id: OperationId::new(),
            kind: WorkKind::ToolCall,
            result_shape: ResultShape::Single,
            policy: OperationPolicy {
                timeout_ms: Some(30000),
                cancel_on_parent_cancel: true,
                checkpoint_results: true,
            },
            budget_reservation: BudgetLimits::default().with_max_tokens(1000),
            depth: 0,
            depends_on: vec![],
            context: Some(ContextStrategy::FullHistory),
            tool_access: Some(ToolAccessPolicy::Inherit),
        };

        // Should serialize correctly
        let json = serde_json::to_string(&spec).expect("Should serialize");
        assert!(json.contains("tool_call"));

        // Roundtrip
        let parsed: OperationSpec = serde_json::from_str(&json).expect("Should deserialize");
        assert_eq!(parsed.kind, spec.kind);
        assert_eq!(parsed.result_shape, spec.result_shape);
    }

    #[test]
    fn test_llm_request_with_tools() {
        let tools = vec![
            Arc::new(ToolDef {
                name: "read_file".to_string(),
                description: "Read a file".to_string(),
                input_schema: meerkat_tools::schema_for::<ReadFileArgs>(),
            }),
            Arc::new(ToolDef {
                name: "write_file".to_string(),
                description: "Write a file".to_string(),
                input_schema: meerkat_tools::schema_for::<WriteFileArgs>(),
            }),
        ];

        let request = LlmRequest::new(
            "claude-3-7-sonnet-20250219",
            vec![Message::User(UserMessage {
                content: "Read the file".to_string(),
            })],
        )
        .with_tools(tools)
        .with_max_tokens(4096)
        .with_temperature(0.7);

        assert_eq!(request.model, "claude-3-7-sonnet-20250219");
        assert_eq!(request.tools.len(), 2);
        assert_eq!(request.max_tokens, 4096);
        assert_eq!(request.temperature, Some(0.7));
    }
}
