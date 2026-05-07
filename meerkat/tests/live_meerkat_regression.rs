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
//!   cargo test -p meerkat --test live_meerkat_regression -- --ignored --test-threads=1

use async_trait::async_trait;
use meerkat::*;
use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
use meerkat_core::image_generation::{
    GenerateImageExecutionPlan, GenerateImageRequest, ImageContinuityTokenSupport,
    ImageFormatPreference, ImageGenerationBackendKind, ImageGenerationIntent,
    ImageGenerationPlanner, ImageGenerationResolvedPlan, ImageGenerationTargetCapabilities,
    ImageGenerationTargetPreference, ImageOperationDenialReason, ImageOperationId,
    ImageOperationTerminalClass, ImageQualityPreference, ImageSizePreference, PromptSource,
    PromptText, SessionModelRoutingStatus, ToolCallId,
};
use meerkat_core::lifecycle::run_primitive::{ModelId, RuntimeExecutionKind};
use meerkat_core::service::{InitialTurnPolicy, SessionBuildOptions};
use meerkat_core::{
    AgentToolDispatcher, AssistantBlock, BlobStore, ContentBlock, ProviderId, SessionEffect,
    ToolCallView,
};
use meerkat_runtime::{MeerkatMachine, SessionServiceRuntimeExt};
use meerkat_session::ephemeral::{SessionAgent, SessionAgentBuilder};
use meerkat_store::MemoryBlobStore;
use meerkat_tools::builtin::{BuiltinToolConfig, CompositeDispatcher, MemoryTaskStore};
use serde_json::value::RawValue;
use serde_json::{Value, json};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
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

fn live_image_request(provider: &str, model: &str) -> GenerateImageRequest {
    GenerateImageRequest::new(
        ImageGenerationIntent::Generate {
            prompt: PromptText::new(
                "Generate a simple original image: one teal square centered on a plain white background.",
            )
            .unwrap(),
            prompt_source: PromptSource::ModelDistilled {
                tool_call_id: ToolCallId::new("live-substrate-image-smoke"),
            },
            reference_images: Vec::new(),
        },
        ImageGenerationTargetPreference::Model {
            provider: ProviderId::new(provider),
            model: ModelId::new(model),
        },
        ImageSizePreference::Square1024,
        ImageQualityPreference::Low,
        ImageFormatPreference::Png,
        std::num::NonZeroU32::new(1).unwrap(),
    )
    .unwrap()
}

// ============================================================================
// IMAGE SUBSTRATE LIVE SMOKE
// ============================================================================

mod image_generation_substrate {
    use super::*;

    struct LiveImagePlanner;

    impl ImageGenerationPlanner for LiveImagePlanner {
        fn resolve_image_generation_plan(
            &self,
            status: &SessionModelRoutingStatus,
            _operation_id: ImageOperationId,
            request: &GenerateImageRequest,
        ) -> Result<ImageGenerationResolvedPlan, ImageOperationDenialReason> {
            let ImageGenerationTargetPreference::Model { provider, model } = &request.target else {
                return Err(ImageOperationDenialReason::UnsupportedTarget);
            };
            let capabilities = ImageGenerationTargetCapabilities {
                hosted_image_generation_tool: true,
                native_image_output: true,
                custom_tools: true,
                image_search_grounding: false,
                image_continuity_tokens: ImageContinuityTokenSupport::SameProviderOnly,
            };
            let (provider_model, execution_plan) = match provider.0.as_str() {
                "openai" => {
                    let images_api =
                        model.as_str().starts_with("gpt-image") && model.as_str() != "gpt-image-2";
                    let (provider_model, backend, provider_plan) = if images_api {
                        (
                            model.clone(),
                            ImageGenerationBackendKind::ProviderApi,
                            json!({
                                "endpoint": "generations",
                                "output": {
                                    "size": "square1024",
                                    "quality": "low",
                                    "output_format": "png"
                                }
                            }),
                        )
                    } else {
                        (
                            ModelId::new("gpt-5.4"),
                            ImageGenerationBackendKind::HostedTool,
                            json!({
                                "tool_name": "image_generation",
                                "model": "gpt-image-2",
                                "output": {
                                    "size": "square1024",
                                    "quality": "low",
                                    "output_format": "png"
                                }
                            }),
                        )
                    };
                    (
                        provider_model,
                        GenerateImageExecutionPlan {
                            provider: provider.clone(),
                            backend,
                            max_count: request.count,
                            capabilities,
                            requires_scoped_override: false,
                            provider_plan,
                        },
                    )
                }
                "gemini" | "google" => (
                    model.clone(),
                    GenerateImageExecutionPlan {
                        provider: provider.clone(),
                        backend: ImageGenerationBackendKind::NativeModel,
                        max_count: request.count,
                        capabilities,
                        requires_scoped_override: true,
                        provider_plan: json!({
                            "projection_snapshot_id": "00000000-0000-0000-0000-000000000001",
                            "output": {
                                "aspect_ratio": "square1x1",
                                "image_size": "one_k"
                            }
                        }),
                    },
                ),
                _ => return Err(ImageOperationDenialReason::UnsupportedTarget),
            };
            let machine_routing_model = if execution_plan.requires_scoped_override() {
                provider_model.clone()
            } else {
                status.effective_model.clone()
            };
            Ok(ImageGenerationResolvedPlan {
                provider_model,
                machine_routing_model,
                machine_routing_realtime_capable: false,
                execution_plan,
                projected_messages: Vec::new(),
            })
        }

        fn infer_provider_for_model(&self, model: &str) -> Option<ProviderId> {
            if model.starts_with("gpt-image") || model.starts_with("dall-e") {
                Some(ProviderId::new("openai"))
            } else if model.starts_with("gemini-") {
                Some(ProviderId::new("gemini"))
            } else {
                None
            }
        }
    }

    struct ScriptedImageToolCallClient {
        provider: &'static str,
        model: String,
        calls: Mutex<usize>,
    }

    #[async_trait]
    impl LlmClient for ScriptedImageToolCallClient {
        fn project_replay_messages(
            &self,
            messages: &[meerkat_core::Message],
        ) -> Result<Vec<meerkat_core::Message>, meerkat_client::LlmError> {
            Ok(messages.to_vec())
        }

        fn stream<'a>(
            &'a self,
            _request: &'a LlmRequest,
        ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
            let mut guard = self.calls.lock().expect("scripted client lock");
            let call_index = *guard;
            *guard += 1;
            drop(guard);

            let events = if call_index == 0 {
                vec![
                    LlmEvent::ToolCallComplete {
                        id: "live-agent-image-call".to_string(),
                        name: "generate_image".to_string(),
                        args: json!({
                            "request": live_image_request(self.provider, &self.model)
                        }),
                        meta: None,
                    },
                    LlmEvent::Done {
                        outcome: LlmDoneOutcome::Success {
                            stop_reason: StopReason::ToolUse,
                        },
                    },
                ]
            } else {
                vec![
                    LlmEvent::TextDelta {
                        delta: "image complete".to_string(),
                        meta: None,
                    },
                    LlmEvent::Done {
                        outcome: LlmDoneOutcome::Success {
                            stop_reason: StopReason::EndTurn,
                        },
                    },
                ]
            };
            Box::pin(futures::stream::iter(events.into_iter().map(Ok)))
        }

        fn provider(&self) -> &'static str {
            "scripted-image-tool-call"
        }

        async fn health_check(&self) -> Result<(), LlmError> {
            Ok(())
        }
    }

    async fn run_live_generate_image_substrate(
        provider: &str,
        model: &str,
        executor: Arc<dyn meerkat_llm_core::ImageGenerationExecutor>,
    ) {
        let runtime = Arc::new(MeerkatMachine::ephemeral());
        let session = Session::new();
        let session_id = session.id().clone();
        runtime
            .prepare_bindings(session_id.clone())
            .await
            .expect("runtime bindings should be prepared");
        runtime
            .configure_model_routing_baseline(&session_id, ModelId::new(model), false)
            .await
            .expect("baseline image model should be configured");

        let blob_store = Arc::new(MemoryBlobStore::default());
        let mut dispatcher = CompositeDispatcher::new(
            Arc::new(MemoryTaskStore::new()),
            &BuiltinToolConfig::default(),
            None,
            None,
            None,
            None,
            true,
        )
        .expect("builtin dispatcher should construct");
        dispatcher.register_image_generation_tool(
            meerkat_tools::builtin::image_generation::ImageGenerationToolRuntime {
                session_id,
                machine: runtime,
                planner: Arc::new(LiveImagePlanner),
                blob_store: blob_store.clone(),
                executor,
            },
            meerkat_core::ToolCategoryOverride::Enable,
        );

        let raw = RawValue::from_string(
            serde_json::to_string(&json!({
                "request": live_image_request(provider, model)
            }))
            .expect("request should serialize"),
        )
        .expect("request should become raw json");
        let outcome = dispatcher
            .dispatch(ToolCallView {
                id: "live-image-call",
                name: "generate_image",
                args: &raw,
            })
            .await
            .expect("generate_image dispatch should succeed");

        assert_eq!(
            outcome.session_effects.len(),
            1,
            "generated images should be appended to session truth"
        );
        let SessionEffect::AppendAssistantBlocks { blocks } = &outcome.session_effects[0] else {
            panic!("expected assistant image block effect");
        };
        assert_eq!(blocks.len(), 1, "live smoke should commit one image block");

        let ContentBlock::Text { text } = &outcome.result.content[0] else {
            panic!("tool result should be JSON text");
        };
        let result: meerkat_core::image_generation::ImageGenerationToolResult =
            serde_json::from_str(text).expect("tool result should decode");
        assert!(
            matches!(result.terminal, ImageOperationTerminalClass::Generated),
            "image substrate terminal should be Generated, got {:?}",
            result.terminal
        );
        assert_eq!(result.images.len(), 1);
        let payload = blob_store
            .get(&result.images[0].blob_ref.blob_id)
            .await
            .expect("committed image blob should be readable");
        assert!(
            payload.data.len() > 256,
            "committed image payload should contain real image bytes"
        );
        eprintln!(
            "[image-substrate] {provider} model={model} committed {} bytes as {}",
            payload.data.len(),
            result.images[0].media_type.as_str()
        );
    }

    #[tokio::test]
    #[ignore = "lane:e2e-smoke"]
    async fn e2e_smoke_openai_generate_image_substrate() {
        let Some(api_key) = openai_api_key() else {
            eprintln!("Skipping OpenAI substrate image smoke: missing OPENAI_API_KEY");
            return;
        };
        let model =
            std::env::var("RKAT_OPENAI_IMAGE_MODEL").unwrap_or_else(|_| "gpt-image-2".into());
        run_live_generate_image_substrate(
            "openai",
            &model,
            Arc::new(meerkat_client::OpenAiClient::new(api_key)),
        )
        .await;
    }

    #[tokio::test]
    #[ignore = "lane:e2e-smoke"]
    async fn e2e_smoke_gemini_generate_image_substrate() {
        let Some(api_key) = gemini_api_key() else {
            eprintln!(
                "Skipping Gemini substrate image smoke: missing GEMINI_API_KEY/RKAT_GEMINI_API_KEY/GOOGLE_API_KEY"
            );
            return;
        };
        let model = std::env::var("RKAT_GEMINI_IMAGE_MODEL")
            .unwrap_or_else(|_| "gemini-3.1-flash-image-preview".into());
        run_live_generate_image_substrate(
            "gemini",
            &model,
            Arc::new(meerkat_client::GeminiClient::new(api_key)),
        )
        .await;
    }

    #[tokio::test]
    #[ignore = "lane:e2e-smoke"]
    async fn e2e_smoke_openai_generate_image_factory_session_turn() {
        let Some(api_key) = openai_api_key() else {
            eprintln!("Skipping OpenAI factory/session image smoke: missing OPENAI_API_KEY");
            return;
        };
        let model =
            std::env::var("RKAT_OPENAI_IMAGE_MODEL").unwrap_or_else(|_| "gpt-image-2".into());
        let temp_dir = TempDir::new().unwrap();
        let runtime = Arc::new(MeerkatMachine::ephemeral());
        let blob_store = Arc::new(MemoryBlobStore::default());
        let factory = AgentFactory::new(temp_dir.path().join("sessions"))
            .builtins(true)
            .with_image_generation_machine(runtime.clone());
        let mut builder = FactoryAgentBuilder::new(factory, Config::default());
        builder.default_llm_client = Some(Arc::new(ScriptedImageToolCallClient {
            provider: "openai",
            model: model.clone(),
            calls: Mutex::new(0),
        }));
        builder.default_blob_store = Some(blob_store.clone());
        builder.default_image_generation_executor =
            Some(Arc::new(meerkat_client::OpenAiClient::new(api_key)));

        let session = Session::new();
        let session_id = session.id().clone();
        let bindings = runtime
            .prepare_bindings(session_id.clone())
            .await
            .expect("runtime bindings should be prepared");
        runtime
            .configure_model_routing_baseline(&session_id, ModelId::new("gpt-5.4"), false)
            .await
            .expect("baseline text model should be configured");

        let req = CreateSessionRequest {
            model: "gpt-5.4".to_string(),
            prompt: "make an image".to_string().into(),
            render_metadata: None,
            system_prompt: None,
            max_tokens: None,
            event_tx: None,
            skill_references: None,
            initial_turn: InitialTurnPolicy::RunImmediately,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
            build: Some(SessionBuildOptions {
                resume_session: Some(session),
                runtime_build_mode: meerkat_core::RuntimeBuildMode::SessionOwned(bindings),
                ..SessionBuildOptions::default()
            }),
            labels: None,
        };
        let (build_tx, _build_rx) = mpsc::channel(8);
        let mut agent = builder
            .build_agent(&req, build_tx)
            .await
            .expect("factory should build runtime-backed image agent");
        let (run_tx, _run_rx) = mpsc::channel(32);
        let result = SessionAgent::run_turn_with_events(
            &mut agent,
            "make an image".to_string().into(),
            meerkat_core::types::HandlingMode::Queue,
            None,
            Some(RuntimeExecutionKind::ContentTurn),
            run_tx,
        )
        .await
        .expect("agent turn should complete after generate_image");
        assert_eq!(result.tool_calls, 1);

        let image_ref = agent
            .session()
            .messages()
            .iter()
            .find_map(|message| match message {
                Message::BlockAssistant(blocks) => blocks.blocks.iter().find_map(|block| {
                    if let AssistantBlock::Image { blob_ref, .. } = block {
                        Some(blob_ref.clone())
                    } else {
                        None
                    }
                }),
                _ => None,
            })
            .expect("factory/session path should append assistant image block");
        let payload = blob_store
            .get(&image_ref.blob_id)
            .await
            .expect("factory/session image blob should be readable");
        assert!(
            payload.data.len() > 256,
            "factory/session path should commit real image bytes"
        );
        eprintln!(
            "[image-substrate] factory/session OpenAI model={model} committed {} bytes as {}",
            payload.data.len(),
            image_ref.media_type
        );
    }
}

// ============================================================================
// SCENARIO 22: Session service lifecycle
// ============================================================================

mod scenario_22_session_service_lifecycle {
    use super::*;

    #[tokio::test]
    #[ignore = "lane:e2e-live"]
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

                skill_references: None,
                initial_turn: InitialTurnPolicy::RunImmediately,
                deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
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
                    system_prompt: None,
                    event_tx: None,
                    runtime: meerkat_core::service::StartTurnRuntimeSemantics::default(),
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
    #[ignore = "lane:e2e-live"]
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
            let openai_model = "gpt-5.4".to_string();
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
    #[ignore = "lane:e2e-live"]
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
