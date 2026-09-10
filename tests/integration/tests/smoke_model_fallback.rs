//! Real provider adapters and SQLite authority across fresh OS processes.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use axum::Json;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response, Sse, sse::Event};
use axum::routing::post;
use futures::StreamExt;
use meerkat::surface::{
    build_runtime_backed_service, default_persistent_executor, materialize_session,
};
use meerkat::{AgentFactory, Config, CreateSessionRequest, FactoryAgentBuilder, Session};
use meerkat_core::config::ModelFallbackTarget;
use meerkat_core::model_fallback::ModelFallbackSkipReason;
use meerkat_core::service::{DeferredPromptPolicy, InitialTurnPolicy};
use meerkat_core::{AgentEvent, Provider, SessionBuildOptions};
use meerkat_runtime::completion::CompletionOutcome;
use meerkat_runtime::{Input, PromptInput};
use serde::Deserialize;
use serde_json::{Value, json};

const PRIMARY: &str = "gpt-6-astra";
const TARGET: &str = "claude-sonnet-4-5";
const CHILD: &str = "fallback_process_child";
const LARGE_WORDS: usize = 650_000;

#[derive(Clone)]
struct ServerState {
    scenario: String,
    read_phase: Arc<AtomicBool>,
    primary: Arc<AtomicUsize>,
    target: Arc<AtomicUsize>,
    large_requests: Arc<AtomicUsize>,
}

#[derive(Deserialize)]
struct OpenAiRequest {
    model: String,
    input: Box<serde_json::value::RawValue>,
}

#[derive(Deserialize)]
struct AnthropicRequest {
    model: String,
    messages: Box<serde_json::value::RawValue>,
}

fn event(name: &str, data: Value) -> Event {
    Event::default().event(name).json_data(data).unwrap()
}

async fn openai(State(state): State<ServerState>, Json(request): Json<OpenAiRequest>) -> Response {
    assert_eq!(request.model, PRIMARY);
    let count = state.primary.fetch_add(1, Ordering::SeqCst);
    if state.scenario == "context" {
        assert!(request.input.get().contains(&"word".repeat(LARGE_WORDS)));
        state.large_requests.fetch_add(1, Ordering::SeqCst);
    }
    if count < 3 {
        if state.scenario.contains("transport") {
            let stream = futures::stream::once(async {
                Ok::<_, std::io::Error>(Event::default().comment("headers established"))
            })
            .chain(futures::stream::once(async {
                tokio::time::sleep(Duration::from_millis(40)).await;
                Err(std::io::Error::new(
                    std::io::ErrorKind::ConnectionReset,
                    "scripted interrupted body",
                ))
            }));
            return Sse::new(stream).into_response();
        }
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "error": {"type":"server_error", "message":"scripted capacity unavailable"}
            })),
        )
            .into_response();
    }
    let text = if state.read_phase.load(Ordering::SeqCst) {
        "READ_PRIMARY"
    } else {
        "WRITE_PRIMARY"
    };
    Sse::new(futures::stream::iter([Ok::<_, Infallible>(event("response.completed", json!({
        "type":"response.completed",
        "response": {
            "id":"resp-local", "status":"completed", "model":PRIMARY,
            "output":[{"type":"message","id":"msg-local","role":"assistant","status":"completed",
                "content":[{"type":"output_text","text":text,"annotations":[]}]}],
            "usage":{"input_tokens":10,"output_tokens":3,"total_tokens":13}
        }
    })))] )).into_response()
}

async fn anthropic(
    State(state): State<ServerState>,
    Json(request): Json<AnthropicRequest>,
) -> Response {
    assert_eq!(request.model, TARGET);
    assert!(!request.messages.get().is_empty());
    state.target.fetch_add(1, Ordering::SeqCst);
    let text = if state.read_phase.load(Ordering::SeqCst) {
        "READ_TARGET"
    } else {
        "WRITE_TARGET"
    };
    let events = [
        event(
            "message_start",
            json!({"type":"message_start","message":{
            "id":"msg-local","type":"message","role":"assistant","model":TARGET,"content":[],
            "usage":{"input_tokens":10,"output_tokens":0}}}),
        ),
        event(
            "content_block_start",
            json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
        ),
        event(
            "content_block_delta",
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":text}}),
        ),
        event(
            "content_block_stop",
            json!({"type":"content_block_stop","index":0}),
        ),
        event(
            "message_delta",
            json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":3}}),
        ),
        event("message_stop", json!({"type":"message_stop"})),
    ];
    Sse::new(futures::stream::iter(
        events.into_iter().map(Ok::<_, Infallible>),
    ))
    .into_response()
}

async fn run_scenario(scenario: &str) {
    let state = ServerState {
        scenario: scenario.into(),
        read_phase: Arc::new(AtomicBool::new(false)),
        primary: Arc::new(AtomicUsize::new(0)),
        target: Arc::new(AtomicUsize::new(0)),
        large_requests: Arc::new(AtomicUsize::new(0)),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let app = axum::Router::new()
        .route("/v1/responses", post(openai))
        .route("/v1/messages", post(anthropic))
        .layer(DefaultBodyLimit::disable())
        .with_state(state.clone());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    std::fs::create_dir(&home).unwrap();
    let phases: &[&str] = if scenario == "allowed" {
        &["write", "read", "hold", "override"]
    } else {
        &["write", "read"]
    };
    for &phase in phases {
        state.read_phase.store(phase != "write", Ordering::SeqCst);
        let before_primary = state.primary.load(Ordering::SeqCst);
        let before_target = state.target.load(Ordering::SeqCst);
        let mut child = tokio::process::Command::new(std::env::current_exe().unwrap());
        child
            .args(["--exact", CHILD, "--ignored", "--nocapture"])
            .env("FALLBACK_PHASE", phase)
            .env("FALLBACK_SCENARIO", scenario)
            .env("FALLBACK_ROOT", root.path())
            .env("FALLBACK_URL", &base)
            .env("HOME", &home)
            .env("MEERKAT_DISABLE_GRAPH_DECODE_MEMO", "1")
            .env("RUST_MIN_STACK", "33554432")
            .kill_on_drop(true);
        for (key, _) in std::env::vars_os() {
            let name = key.to_string_lossy();
            if name.contains("API_KEY")
                || name.starts_with("RKAT_")
                || name.contains("ANTHROPIC_AUTH")
            {
                child.env_remove(key);
            }
        }
        let output = tokio::time::timeout(Duration::from_secs(180), child.output())
            .await
            .expect("bounded child")
            .unwrap();
        assert!(
            output.status.success(),
            "{scenario}/{phase}: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let switched = scenario == "allowed";
        if phase == "hold" {
            assert_eq!(state.primary.load(Ordering::SeqCst), before_primary);
            assert_eq!(state.target.load(Ordering::SeqCst), before_target);
            continue;
        }
        assert_eq!(
            state.primary.load(Ordering::SeqCst) - before_primary,
            if phase == "write" {
                if switched { 3 } else { 4 }
            } else {
                usize::from(!switched)
            }
        );
        assert_eq!(
            state.target.load(Ordering::SeqCst) - before_target,
            usize::from(switched)
        );
    }
    if scenario == "context" {
        assert_eq!(state.large_requests.load(Ordering::SeqCst), 5);
    }
    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "Turbo S loopback provider and process-boundary contract"]
async fn model_fallback_boundaries() {
    let mut invalid = Config::default();
    invalid.model_fallback.enabled = Some(true);
    assert!(invalid.validate(meerkat_models::canonical()).is_err());
    invalid.model_fallback.enabled = Some(false);
    assert!(invalid.validate(meerkat_models::canonical()).is_ok());
    assert!(
        serde_json::from_value::<Config>(json!({
            "model_fallback":{"policy":{"scope":"turn"}}
        }))
        .is_err()
    );
    for scenario in [
        "default-transport",
        "default-capacity",
        "chain-transport",
        "boundary",
        "allowed",
    ] {
        run_scenario(scenario).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "Turbo S materialized 650k-token forecast and process-boundary contract"]
async fn model_fallback_context() {
    run_scenario("context").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "invoked only by the loopback process-boundary suites"]
async fn fallback_process_child() {
    let phase = std::env::var("FALLBACK_PHASE").expect("parent supplies phase");
    let scenario = std::env::var("FALLBACK_SCENARIO").unwrap();
    let root = std::path::PathBuf::from(std::env::var_os("FALLBACK_ROOT").unwrap());
    let url = std::env::var("FALLBACK_URL").unwrap();
    let mut config = Config::default();
    let mut realm = meerkat_core::RealmConfigSection::from_inline_api_keys(&[
        ("openai", "local-dummy-openai"),
        ("anthropic", "local-dummy-anthropic"),
    ]);
    for backend in realm.backend.values_mut() {
        backend.base_url = Some(url.clone());
    }
    config.realm.insert("fallback".into(), realm);
    config.max_tokens = Some(1024);
    config.retry.max_retries = 4;
    config.retry.initial_delay = Duration::from_millis(1);
    config.retry.max_delay = Duration::from_millis(2);
    config.compaction.auto_compact_threshold = 2_000_000;
    config.provider_tools.openai.web_search = false;
    config.provider_tools.anthropic.web_search = false;
    if !scenario.starts_with("default-") {
        config.model_fallback.enabled = Some(true);
        config.model_fallback.chain.push(ModelFallbackTarget {
            model: TARGET.into(),
            provider: Some(Provider::Anthropic),
            auth_binding: None,
        });
        config.model_fallback.policy.cross_provider = scenario != "boundary";
    }
    config.validate(meerkat_models::canonical()).unwrap();
    let (_manifest, persistence) = meerkat::open_realm_persistence_in(
        &root,
        "fallback",
        Some(meerkat_store::RealmBackend::Sqlite),
        Some(meerkat_store::RealmOrigin::Explicit),
    )
    .await
    .unwrap();
    let factory = AgentFactory::new(root.join("sessions")).builtins(false);
    let builder = FactoryAgentBuilder::new(factory, config);
    let (service, runtime) = build_runtime_backed_service(builder, 4, persistence);
    let service = Arc::new(service);
    let session = if phase == "write" {
        Session::new()
    } else {
        let id =
            meerkat::SessionId::parse(std::fs::read_to_string(root.join("id")).unwrap().trim())
                .unwrap();
        service
            .load_authoritative_session(&id)
            .await
            .unwrap()
            .unwrap()
    };
    let id = session.id().clone();
    let expected_model = if scenario == "allowed" {
        TARGET
    } else {
        PRIMARY
    };
    if phase != "write" {
        assert_eq!(session.session_metadata().unwrap().model, expected_model);
    }
    let request = CreateSessionRequest {
        model: if phase == "override" { TARGET } else { PRIMARY }.into(),
        injected_context: Vec::new(),
        prompt: "".into(),
        system_prompt: if phase == "write" {
            meerkat::SystemPromptOverride::Set("Loopback fallback regression.".into())
        } else {
            meerkat::SystemPromptOverride::Inherit
        },
        max_tokens: Some(1024),
        event_tx: None,
        initial_turn: InitialTurnPolicy::Defer,
        deferred_prompt_policy: DeferredPromptPolicy::Discard,
        build: Some(SessionBuildOptions {
            realm_id: Some(meerkat_core::RealmId::parse("fallback").unwrap()),
            override_builtins: meerkat_core::ToolCategoryOverride::Disable,
            resume_override_mask: meerkat_core::service::ResumeOverrideMask {
                model: phase == "override",
                ..Default::default()
            },
            ..Default::default()
        }),
        labels: None,
    };
    let service_for_executor = Arc::clone(&service);
    let runtime_for_executor = Arc::clone(&runtime);
    Box::pin(materialize_session(
        &service,
        &runtime,
        session,
        request,
        move |id| default_persistent_executor(service_for_executor, runtime_for_executor, id),
    ))
    .await
    .unwrap();
    let mut events = service.subscribe_session_events(&id).await.unwrap();
    let reader = tokio::spawn(async move {
        let mut result = Vec::new();
        while let Some(envelope) = events.next().await {
            let done = matches!(
                envelope.payload,
                AgentEvent::RunCompleted { .. } | AgentEvent::RunFailed { .. }
            );
            result.push(envelope.payload);
            if done {
                break;
            }
        }
        result
    });
    let prompt = if (scenario == "context" && phase == "write") || phase == "hold" {
        "word".repeat(LARGE_WORDS)
    } else {
        format!("{phase} request")
    };
    let (_, completion) = runtime
        .accept_input_with_completion(&id, Input::Prompt(PromptInput::new(prompt, None)))
        .await
        .unwrap();
    let outcome = tokio::time::timeout(Duration::from_secs(90), completion.unwrap().wait())
        .await
        .unwrap()
        .unwrap();
    if phase == "hold" {
        let error = outcome
            .error_metadata()
            .expect("unsafe new fallback-origin resume must hold");
        assert!(
            matches!(error.reason.as_ref(), Some(meerkat_core::event::AgentErrorReason::ModelFallbackResumeHeld {
            provider: Provider::Anthropic, model, reason: ModelFallbackSkipReason::ContextFit,
        }) if model == TARGET),
            "{error:?}"
        );
        let _events = tokio::time::timeout(Duration::from_secs(10), reader)
            .await
            .unwrap()
            .unwrap();
        let durable = service
            .load_authoritative_session(&id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(durable.session_metadata().unwrap().model, TARGET);
        assert!(durable.session_metadata().unwrap().model_fallback.is_some());
        return;
    }
    let CompletionOutcome::Completed(result) = outcome else {
        panic!("current turn failed: {outcome:?}")
    };
    let marker = format!(
        "{}_{}",
        if phase == "write" { "WRITE" } else { "READ" },
        if scenario == "allowed" {
            "TARGET"
        } else {
            "PRIMARY"
        }
    );
    assert_eq!(result.text, marker);
    let events = tokio::time::timeout(Duration::from_secs(10), reader)
        .await
        .unwrap()
        .unwrap();
    let committed = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::ModelFallbackCommitted { .. }))
        .count();
    let staged = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::ModelFallbackStaged { .. }))
        .count();
    assert_eq!(
        committed,
        usize::from(scenario == "allowed" && phase == "write")
    );
    assert_eq!(staged, committed);
    if phase == "write" {
        let retries: Vec<_> = events
            .iter()
            .filter_map(|e| {
                if let AgentEvent::Retrying { retry } = e {
                    Some(retry)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(retries.len(), 3);
        for (index, retry) in retries.iter().enumerate() {
            assert_eq!(retry.plan.attempt, index as u32 + 1);
            assert_eq!(retry.failure.provider, "openai");
            assert_eq!(
                retry.failure.kind,
                meerkat_core::retry::LlmRetryFailureKind::RetryableProviderError
            );
        }
        if scenario == "boundary" || scenario == "context" {
            let reason = if scenario == "boundary" {
                ModelFallbackSkipReason::ProviderBoundary
            } else {
                ModelFallbackSkipReason::ContextFit
            };
            assert!(events.iter().any(|e| matches!(e, AgentEvent::ModelFallbackSkipped { target, .. } if target.reason == reason)));
            if scenario == "context" {
                let fact = events
                    .iter()
                    .find_map(|e| match e {
                        AgentEvent::ModelFallbackSkipped { target, .. } => target.context.as_ref(),
                        _ => None,
                    })
                    .unwrap();
                assert!(fact.effective_input_tokens() >= LARGE_WORDS as u64);
                assert_eq!(fact.context_window_tokens, 200_000);
            }
        }
    }
    let durable = service
        .load_authoritative_session(&id)
        .await
        .unwrap()
        .unwrap();
    if phase == "override" {
        assert!(
            durable.session_metadata().unwrap().model_fallback.is_none(),
            "explicit same-model operator intent must obsolete the fallback marker"
        );
    }
    assert_eq!(durable.session_metadata().unwrap().model, expected_model);
    assert!(durable.messages().iter().any(|message| matches!(message,
        meerkat_core::Message::BlockAssistant(assistant) if assistant.text_blocks().any(|text| text.contains(&marker)))));
    std::fs::write(root.join("id"), id.to_string()).unwrap();
}
