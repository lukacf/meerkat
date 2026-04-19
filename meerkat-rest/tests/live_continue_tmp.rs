#![cfg(feature = "integration-real-tests")]
#![allow(clippy::unwrap_used, clippy::expect_used)]
use axum::body::Body;
use axum::http::Request;
use http_body_util::BodyExt;
use meerkat::surface::wire_runtime_bindings;
use meerkat::{
    AgentFactory, Config, FactoryAgentBuilder, MemoryStore, PersistenceBundle,
    PersistentSessionService, SessionStore,
};
use meerkat_client::TestClient;
use meerkat_core::MemoryConfigStore;
#[cfg(feature = "mob")]
use meerkat_mob_mcp::wire_mob_tools;
use meerkat_rest::{AppState, router};
use meerkat_store::StoreAdapter;
use serde_json::json;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::time::{Duration, timeout};
use tower::ServiceExt;

#[tokio::test]
#[ignore = "lane:e2e-live"]
async fn integration_real_live_continue_hangs() {
    let temp_dir = TempDir::new().unwrap();
    let project_root = temp_dir.path().join("project");
    std::fs::create_dir_all(project_root.join(".rkat")).unwrap();

    let config = Config::default();
    let store_path = temp_dir.path().join("sessions");
    let (event_tx, _) = tokio::sync::broadcast::channel(16);

    let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());

    let factory = AgentFactory::new(store_path.clone())
        .builtins(true)
        .shell(true)
        .project_root(project_root.clone());
    let mut builder = FactoryAgentBuilder::new(factory, config.clone());
    builder.default_llm_client = Some(Arc::new(TestClient::default()));
    let persistence =
        PersistenceBundle::new(store, None, Arc::new(meerkat_store::MemoryBlobStore::new()));
    let runtime_adapter = persistence.runtime_adapter();
    builder.default_session_store = Some(Arc::new(StoreAdapter::new(persistence.session_store())));
    #[cfg(feature = "mob")]
    let builder_mob_tools_slot = Arc::clone(&builder.default_mob_tools);
    let (session_store_inner, runtime_store, blob_store) = persistence.into_parts();
    let mut session_service =
        PersistentSessionService::new(builder, 100, session_store_inner, runtime_store, blob_store);
    wire_runtime_bindings(&mut session_service, &runtime_adapter);
    let session_service = Arc::new(session_service);
    #[cfg(feature = "mob")]
    let mob_state = wire_mob_tools(
        &builder_mob_tools_slot,
        session_service.clone(),
        Some(runtime_adapter.clone()),
        None,
    );
    let config_store: Arc<dyn meerkat_core::ConfigStore> =
        Arc::new(MemoryConfigStore::new(config.clone()));
    let config_runtime = Arc::new(meerkat_core::ConfigRuntime::new(
        Arc::clone(&config_store),
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
        project_root: Some(project_root.clone()),
        llm_client_override: Some(Arc::new(TestClient::default())),
        config_store,
        event_tx,
        session_service,
        schedule_service: meerkat::ScheduleService::new(Arc::new(
            meerkat::MemoryScheduleStore::default(),
        )),
        webhook_auth: meerkat_rest::webhook::WebhookAuth::None,
        realm_id: "test-realm".to_string(),
        instance_id: None,
        backend: "sqlite".to_string(),
        resolved_paths: meerkat_core::ConfigResolvedPaths {
            root: store_path.display().to_string(),
            manifest_path: String::new(),
            config_path: String::new(),
            sessions_sqlite_path: None,
            sessions_jsonl_dir: String::new(),
        },
        expose_paths: false,
        realtime_rpc_tcp_addr: None,
        config_runtime,
        realm_lease: Arc::new(tokio::sync::Mutex::new(None)),
        skill_runtime: None,
        runtime_adapter: runtime_adapter.clone(),
        schedule_host: Arc::default(),
        request_executor: std::sync::Arc::new(meerkat::surface::SurfaceRequestExecutor::new(
            std::time::Duration::from_secs(5),
        )),
        #[cfg(feature = "mob")]
        mob_state,
        #[cfg(feature = "mcp")]
        mcp_sessions: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
        token_store: Arc::new(meerkat_providers::auth_store::EphemeralTokenStore::new()),
    };

    let app = router(state);
    let run_payload = json!({"prompt":"hi"});
    let req1 = Request::builder()
        .method("POST")
        .uri("/sessions")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&run_payload).unwrap()))
        .unwrap();
    let resp1 = timeout(Duration::from_secs(5), app.clone().oneshot(req1))
        .await
        .unwrap()
        .unwrap();
    let body1 = resp1.into_body().collect().await.unwrap().to_bytes();
    let run_json: serde_json::Value = serde_json::from_slice(&body1).unwrap();
    let sid = run_json["session_id"].as_str().unwrap().to_string();

    let continue_payload = json!({"session_id": sid, "prompt":"next"});
    let sid2 = continue_payload["session_id"].as_str().unwrap();
    let req2 = Request::builder()
        .method("POST")
        .uri(format!("/sessions/{sid2}/messages"))
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&continue_payload).unwrap()))
        .unwrap();
    let res2 = timeout(Duration::from_secs(2), app.oneshot(req2)).await;
    assert!(
        res2.is_ok(),
        "continue timed out: likely hung waiting event forwarder"
    );
}
