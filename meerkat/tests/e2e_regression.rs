#![cfg(feature = "integration-real-tests")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//!
//! E2E regression tests for the Meerkat native Rust SDK.
//!
//! These tests cover session service lifecycle, structured output across
//! providers, and event capture through the AgentFactory path.
//!
//! Each test requires API keys and makes real API calls. When keys are missing,
//! tests skip gracefully with an informational message.
//!
//! Run with:
//!   cargo test -p meerkat --test e2e_regression -- --ignored --test-threads=1

use meerkat::*;
use meerkat_core::service::InitialTurnPolicy;
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::sync::mpsc;

// ============================================================================
// HELPER FUNCTIONS
// ============================================================================

fn first_env(vars: &[&str]) -> Option<String> {
    for name in vars {
        if let Ok(value) = std::env::var(name) {
            return Some(value);
        }
    }
    None
}

fn anthropic_api_key() -> Option<String> {
    first_env(&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"])
}

#[allow(dead_code)]
fn openai_api_key() -> Option<String> {
    first_env(&["RKAT_OPENAI_API_KEY", "OPENAI_API_KEY"])
}

#[allow(dead_code)]
fn gemini_api_key() -> Option<String> {
    first_env(&["RKAT_GEMINI_API_KEY", "GEMINI_API_KEY", "GOOGLE_API_KEY"])
}

/// Get the model to use for regression tests (cheaper model by default).
fn smoke_model() -> String {
    std::env::var("SMOKE_MODEL").unwrap_or_else(|_| "claude-sonnet-4-5".to_string())
}

// ============================================================================
// SCENARIO 22: Session service lifecycle
// ============================================================================

mod scenario_22_session_service_lifecycle {
    use super::*;

    #[tokio::test]
    #[ignore = "integration-real: live API"]
    async fn e2e_scenario_22_session_service_lifecycle() {
        let Some(_api_key) = anthropic_api_key() else {
            eprintln!("Skipping scenario 22: missing ANTHROPIC_API_KEY");
            return;
        };

        let temp_dir = TempDir::new().unwrap();
        let factory = AgentFactory::new(temp_dir.path().join("sessions")).builtins(true);
        let config = Config::default();

        let service = build_ephemeral_service(factory, config, 10);

        // --- create_session ---
        eprintln!("[scenario 22] Creating session...");
        let create_result = service
            .create_session(CreateSessionRequest {
                model: smoke_model(),
                prompt: "I am EphBot. Remember this name.".to_string().into(),
                render_metadata: None,
                system_prompt: None,
                max_tokens: None,
                event_tx: None,
                host_mode: false,

                skill_references: None,
                initial_turn: InitialTurnPolicy::RunImmediately,
                build: None,
                labels: None,
            })
            .await
            .expect("create_session should succeed");

        assert!(
            !create_result.text.is_empty(),
            "create_session response text should not be empty"
        );
        let session_id = create_result.session_id.clone();
        eprintln!(
            "[scenario 22] Created session {}, text: {}",
            session_id,
            create_result.text.trim()
        );

        // --- start_turn ---
        eprintln!("[scenario 22] Starting second turn...");
        let turn_result = service
            .start_turn(
                &session_id,
                StartTurnRequest {
                    prompt: "What is my name? Reply in one sentence.".to_string().into(),
                    render_metadata: None,
                    handling_mode: meerkat_core::types::HandlingMode::Queue,
                    event_tx: None,
                    host_mode: false,

                    skill_references: None,
                    flow_tool_overlay: None,
                    additional_instructions: None,
                },
            )
            .await
            .expect("start_turn should succeed");

        assert!(
            turn_result.text.to_lowercase().contains("ephbot"),
            "Second turn should mention EphBot, got: {}",
            turn_result.text
        );
        eprintln!("[scenario 22] Turn 2 OK: {}", turn_result.text.trim());

        // --- read ---
        eprintln!("[scenario 22] Reading session...");
        let view = service
            .read(&session_id)
            .await
            .expect("read should succeed");

        assert!(
            view.state.message_count >= 4,
            "Should have at least 4 messages (user+assistant x2), got {}",
            view.state.message_count
        );
        // is_active means a turn is currently running; after completion it's false
        assert!(
            !view.state.is_active,
            "Session should be idle after turn completes"
        );
        eprintln!(
            "[scenario 22] Read OK: {} messages, active={}",
            view.state.message_count, view.state.is_active
        );

        // --- list ---
        eprintln!("[scenario 22] Listing sessions...");
        let summaries = service
            .list(SessionQuery::default())
            .await
            .expect("list should succeed");

        assert!(
            summaries.iter().any(|s| s.session_id == session_id),
            "Session {session_id} should appear in list results"
        );
        eprintln!("[scenario 22] List OK: {} sessions", summaries.len());

        // --- archive ---
        eprintln!("[scenario 22] Archiving session...");
        service
            .archive(&session_id)
            .await
            .expect("archive should succeed");
        eprintln!("[scenario 22] Archive OK");

        // --- read after archive returns archived view (not error) ---
        let archived_view = service
            .read(&session_id)
            .await
            .expect("archived session should still be readable");
        assert!(
            !archived_view.state.is_active,
            "Archived session should not be active"
        );
        eprintln!("[scenario 22] Post-archive read correctly returned inactive view");
    }
}

// ============================================================================
// SCENARIO 23: Structured output
// ============================================================================

mod scenario_23_structured_output {
    use super::*;

    #[tokio::test]
    #[ignore = "integration-real: live API"]
    async fn e2e_scenario_23_structured_output() {
        let mut ran_any = false;

        let schema_json = json!({
            "type": "object",
            "properties": {
                "answer": { "type": "string" }
            },
            "required": ["answer"]
        });

        // --- Anthropic ---
        if let Some(_api_key) = anthropic_api_key() {
            eprintln!(
                "[scenario 23] Testing structured output with Anthropic ({})",
                smoke_model()
            );
            let temp_dir = TempDir::new().unwrap();
            let factory = AgentFactory::new(temp_dir.path().join("sessions"));
            let config = Config::default();
            let mut build_config = AgentBuildConfig::new(smoke_model());
            build_config.output_schema = Some(
                OutputSchema::new(schema_json.clone())
                    .expect("schema should be valid")
                    .with_name("answer_schema"),
            );

            let mut agent = factory
                .build_agent(build_config, &config)
                .await
                .expect("Anthropic structured output agent should build");

            let result = agent
                .run("What is the capital of France? Respond in the required JSON format.".into())
                .await
                .expect("Anthropic structured output run should succeed");

            assert!(
                result.structured_output.is_some(),
                "structured_output should be Some for Anthropic"
            );
            let parsed: Value = result.structured_output.unwrap();
            assert!(
                parsed.get("answer").is_some(),
                "Structured output should have 'answer' key, got: {parsed}"
            );
            let answer = parsed["answer"].as_str().unwrap_or("");
            assert!(
                answer.to_lowercase().contains("paris"),
                "Answer should contain Paris, got: {answer}"
            );
            eprintln!("[scenario 23] Anthropic OK: {parsed}");
            ran_any = true;
        } else {
            eprintln!("[scenario 23] Skipping Anthropic: no API key");
        }

        // --- OpenAI ---
        #[cfg(feature = "openai")]
        if let Some(_api_key) = openai_api_key() {
            let openai_model = "gpt-5.2".to_string();
            eprintln!("[scenario 23] Testing structured output with OpenAI ({openai_model})");
            let temp_dir = TempDir::new().unwrap();
            let factory = AgentFactory::new(temp_dir.path().join("sessions"));
            let config = Config::default();
            let mut build_config = AgentBuildConfig::new(openai_model);
            build_config.provider = Some(Provider::OpenAI);
            build_config.output_schema = Some(
                OutputSchema::new(schema_json.clone())
                    .expect("schema should be valid")
                    .with_name("answer_schema"),
            );

            let mut agent = factory
                .build_agent(build_config, &config)
                .await
                .expect("OpenAI structured output agent should build");

            let result = agent
                .run("What is the capital of France? Respond in the required JSON format.".into())
                .await
                .expect("OpenAI structured output run should succeed");

            assert!(
                result.structured_output.is_some(),
                "structured_output should be Some for OpenAI"
            );
            let parsed: Value = result.structured_output.unwrap();
            assert!(
                parsed.get("answer").is_some(),
                "Structured output should have 'answer' key, got: {parsed}"
            );
            let answer = parsed["answer"].as_str().unwrap_or("");
            assert!(
                answer.to_lowercase().contains("paris"),
                "Answer should contain Paris, got: {answer}"
            );
            eprintln!("[scenario 23] OpenAI OK: {parsed}");
            ran_any = true;
        } else {
            eprintln!("[scenario 23] Skipping OpenAI: no API key");
        }

        // --- Gemini ---
        #[cfg(feature = "gemini")]
        if let Some(_api_key) = gemini_api_key() {
            let gemini_model = "gemini-3-flash-preview".to_string();
            eprintln!("[scenario 23] Testing structured output with Gemini ({gemini_model})");
            let temp_dir = TempDir::new().unwrap();
            let factory = AgentFactory::new(temp_dir.path().join("sessions"));
            let config = Config::default();
            let mut build_config = AgentBuildConfig::new(gemini_model);
            build_config.provider = Some(Provider::Gemini);
            build_config.output_schema = Some(
                OutputSchema::new(schema_json.clone())
                    .expect("schema should be valid")
                    .with_name("answer_schema"),
            );

            let mut agent = factory
                .build_agent(build_config, &config)
                .await
                .expect("Gemini structured output agent should build");

            let result = agent
                .run("What is the capital of France? Respond in the required JSON format.".into())
                .await
                .expect("Gemini structured output run should succeed");

            assert!(
                result.structured_output.is_some(),
                "structured_output should be Some for Gemini"
            );
            let parsed: Value = result.structured_output.unwrap();
            assert!(
                parsed.get("answer").is_some(),
                "Structured output should have 'answer' key, got: {parsed}"
            );
            let answer = parsed["answer"].as_str().unwrap_or("");
            assert!(
                answer.to_lowercase().contains("paris"),
                "Answer should contain Paris, got: {answer}"
            );
            eprintln!("[scenario 23] Gemini OK: {parsed}");
            ran_any = true;
        } else {
            eprintln!("[scenario 23] Skipping Gemini: no API key");
        }

        assert!(
            ran_any,
            "At least one provider API key must be available for scenario 23"
        );
    }
}

// ============================================================================
// SCENARIO 24: Event capture
// ============================================================================

mod scenario_24_event_capture {
    use super::*;

    #[tokio::test]
    #[ignore = "integration-real: live API"]
    async fn e2e_scenario_24_event_capture() {
        let Some(_api_key) = anthropic_api_key() else {
            eprintln!("Skipping scenario 24: missing ANTHROPIC_API_KEY");
            return;
        };

        let (event_tx, mut event_rx) = mpsc::channel::<AgentEvent>(256);

        let temp_dir = TempDir::new().unwrap();
        let factory = AgentFactory::new(temp_dir.path().join("sessions"));
        let config = Config::default();
        let mut build_config = AgentBuildConfig::new(smoke_model());
        build_config.event_tx = Some(event_tx);

        let mut agent = factory
            .build_agent(build_config, &config)
            .await
            .expect("Event capture agent should build");

        eprintln!("[scenario 24] Running agent with event capture...");
        let result = agent
            .run("Write one sentence about Rust programming.".into())
            .await
            .expect("Event capture run should succeed");

        // Collect all events from the channel
        let mut events = Vec::new();
        while let Ok(event) = event_rx.try_recv() {
            events.push(event);
        }
        eprintln!("[scenario 24] Captured {} events", events.len());

        // Assert at least one RunStarted event
        let has_run_started = events
            .iter()
            .any(|e| matches!(e, AgentEvent::RunStarted { .. }));
        assert!(
            has_run_started,
            "Should have at least one RunStarted event among {} captured events",
            events.len()
        );

        // Assert at least one TextDelta event
        let has_text_delta = events
            .iter()
            .any(|e| matches!(e, AgentEvent::TextDelta { .. }));
        assert!(
            has_text_delta,
            "Should have at least one TextDelta event among {} captured events",
            events.len()
        );

        // Assert result is non-empty with at least one turn
        assert!(!result.text.is_empty(), "Result text should not be empty");
        assert!(
            result.turns >= 1,
            "Should have at least 1 turn, got {}",
            result.turns
        );

        eprintln!(
            "[scenario 24] OK: {} events, {} turns, text={}",
            events.len(),
            result.turns,
            result.text.trim()
        );
    }
}
