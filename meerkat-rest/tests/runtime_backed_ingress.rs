#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use meerkat::{
    AgentFactory, Config, FactoryAgentBuilder, MemoryStore, PersistentSessionService, SessionStore,
};
use meerkat_client::TestClient;
use meerkat_core::MemoryConfigStore;
use meerkat_rest::{AppState, router};
use serde_json::{Value, json};
use std::sync::Arc;
use tempfile::TempDir;
use tower::ServiceExt;

#[tokio::test]
async fn runtime_backed_ingress_red_ok_rest_session_create_uses_runtime_adapter_surface() {
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
    builder.default_llm_client = Some(Arc::new(TestClient::default()));
    let session_service = Arc::new(PersistentSessionService::new(builder, 100, store, None));
    let config_store_arc: Arc<dyn meerkat_core::ConfigStore> = Arc::new(config_store);
    let config_runtime = Arc::new(meerkat_core::ConfigRuntime::new(
        Arc::clone(&config_store_arc),
        store_path.join("config_state.json"),
    ));

    let state = AppState {
        store_path: store_path.clone(),
        default_model: config.agent.model.clone().into(),
        max_tokens: config.agent.max_tokens_per_turn,
        rest_host: config.rest.host.clone().into(),
        rest_port: config.rest.port,
        enable_builtins: true,
        enable_shell: true,
        project_root: Some(project_root),
        llm_client_override: Some(Arc::new(TestClient::default())),
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
        runtime_adapter: Arc::new(meerkat_runtime::RuntimeSessionAdapter::ephemeral()),
        #[cfg(feature = "mob")]
        mob_state: meerkat_mob_mcp::MobMcpState::new_in_memory(),
        #[cfg(feature = "comms")]
        comms_drain_handles: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
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
}
