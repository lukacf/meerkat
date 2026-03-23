#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use futures::stream;
use http_body_util::BodyExt;
use meerkat::{
    AgentFactory, Config, FactoryAgentBuilder, MemoryStore, PersistentSessionService, SessionStore,
};
use meerkat_client::types::LlmStream;
use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
use meerkat_core::MemoryConfigStore;
use meerkat_rest::{AppState, router};
use meerkat_runtime::SessionServiceRuntimeExt as _;
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tempfile::TempDir;
use tower::ServiceExt;

struct FirstTurnOnlyClient {
    calls: AtomicUsize,
}

impl FirstTurnOnlyClient {
    fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
        }
    }

    fn call_count(&self) -> usize {
        self.calls.load(Ordering::Acquire)
    }
}

#[async_trait]
impl LlmClient for FirstTurnOnlyClient {
    fn stream<'a>(&'a self, _request: &'a LlmRequest) -> LlmStream<'a> {
        let call = self.calls.fetch_add(1, Ordering::AcqRel);
        if call == 0 {
            Box::pin(stream::iter(vec![
                Ok(LlmEvent::TextDelta {
                    delta: "ok".to_string(),
                    meta: None,
                }),
                Ok(LlmEvent::Done {
                    outcome: LlmDoneOutcome::Success {
                        stop_reason: meerkat_core::StopReason::EndTurn,
                    },
                }),
            ]))
        } else {
            Box::pin(stream::pending())
        }
    }

    fn provider(&self) -> &'static str {
        "first-turn-only"
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

#[tokio::test]
async fn runtime_backed_external_events_stay_queued_without_waking_idle_sessions() {
    let temp_dir = TempDir::new().expect("temp dir");
    let project_root = temp_dir.path().join("project");
    std::fs::create_dir_all(project_root.join(".rkat")).expect("create .rkat");

    let config = Config::default();
    let config_store = MemoryConfigStore::new(config.clone());
    let store_path = temp_dir.path().join("sessions");
    let (event_tx, _) = tokio::sync::broadcast::channel(16);
    let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());

    let factory = AgentFactory::new(store_path.clone())
        .builtins(true)
        .shell(true)
        .project_root(project_root.clone());
    let mut builder = FactoryAgentBuilder::new(factory, config.clone());
    let llm = Arc::new(FirstTurnOnlyClient::new());
    builder.default_llm_client = Some(llm.clone());
    let session_service = Arc::new(PersistentSessionService::new(builder, 100, store, None));
    let config_store_arc: Arc<dyn meerkat_core::ConfigStore> = Arc::new(config_store);
    let config_runtime = Arc::new(meerkat_core::ConfigRuntime::new(
        Arc::clone(&config_store_arc),
        store_path.join("config_state.json"),
    ));

    let runtime_adapter = Arc::new(meerkat_runtime::RuntimeSessionAdapter::ephemeral());
    let state = AppState {
        store_path: store_path.clone(),
        default_model: config.agent.model.clone().into(),
        max_tokens: config.agent.max_tokens_per_turn,
        rest_host: config.rest.host.clone().into(),
        rest_port: config.rest.port,
        enable_builtins: true,
        enable_shell: true,
        project_root: Some(project_root),
        llm_client_override: Some(llm.clone()),
        config_store: config_store_arc,
        event_tx,
        session_service,
        webhook_auth: meerkat_rest::webhook::WebhookAuth::None,
        realm_id: "phase1-rest".to_string(),
        instance_id: None,
        backend: "redb".to_string(),
        resolved_paths: meerkat_core::ConfigResolvedPaths {
            root: store_path.display().to_string(),
            manifest_path: String::new(),
            config_path: String::new(),
            sessions_redb_path: String::new(),
            sessions_sqlite_path: None,
            sessions_jsonl_dir: String::new(),
        },
        expose_paths: false,
        config_runtime,
        realm_lease: Arc::new(tokio::sync::Mutex::new(None)),
        skill_runtime: None,
        runtime_adapter: runtime_adapter.clone(),
        #[cfg(feature = "mob")]
        mob_state: meerkat_mob_mcp::MobMcpState::new_in_memory(),
        #[cfg(feature = "mcp")]
        mcp_sessions: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
    };

    let app = router(state);
    let request = Request::builder()
        .method("POST")
        .uri("/sessions")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_vec(&json!({
                "prompt": "say ok",
                "model": config.agent.model,
                "max_tokens": config.agent.max_tokens_per_turn
            }))
            .expect("serialize payload"),
        ))
        .expect("build request");

    let response = app.clone().oneshot(request).await.expect("run request");
    assert_eq!(response.status(), StatusCode::OK);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("read body")
        .to_bytes();
    let payload: Value = serde_json::from_slice(&body).expect("json body");
    let session_id = payload["session_id"]
        .as_str()
        .expect("session_id")
        .to_string();
    let parsed_session_id = meerkat::SessionId::parse(&session_id).expect("parse session id");
    assert_eq!(
        llm.call_count(),
        1,
        "initial create-session turn should consume exactly one model call"
    );

    let event_request = Request::builder()
        .method("POST")
        .uri(format!("/sessions/{session_id}/external-events"))
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_vec(&json!({
                "source": "phase8",
                "body": "runtime-backed external event"
            }))
            .expect("serialize event payload"),
        ))
        .expect("build event request");

    let event_response = app.oneshot(event_request).await.expect("run event request");
    assert_eq!(event_response.status(), StatusCode::ACCEPTED);
    let event_body = event_response
        .into_body()
        .collect()
        .await
        .expect("read event body")
        .to_bytes();
    let event_payload: Value = serde_json::from_slice(&event_body).expect("event json body");
    assert_eq!(event_payload, json!({ "queued": true }));

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    assert_eq!(
        llm.call_count(),
        1,
        "queued external events must not wake a fresh runtime-backed turn immediately"
    );
    assert_eq!(
        runtime_adapter
            .runtime_state(&parsed_session_id)
            .await
            .expect("runtime state"),
        meerkat_runtime::RuntimeState::Attached,
        "queued external events should stage work without starting a new run"
    );
    assert!(
        !runtime_adapter
            .list_active_inputs(&parsed_session_id)
            .await
            .expect("active inputs")
            .is_empty(),
        "queued external events should remain staged for a later boundary"
    );
}
