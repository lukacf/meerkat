#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use meerkat::{
    AgentFactory, Config, EphemeralSessionService, FactoryAgentBuilder, JsonlStore, SessionId,
    SessionStore,
};
use meerkat_client::TestClient;
use meerkat_core::MemoryConfigStore;
use meerkat_rest::{AppState, router};
use serde_json::{Value, json};
use std::sync::Arc;
use tempfile::TempDir;
use tokio::time::{Duration, timeout};
use tower::ServiceExt;

fn skip_if_no_prereqs() -> bool {
    false
}

#[tokio::test]
async fn integration_rest_resume_metadata() {
    if skip_if_no_prereqs() {
        return;
    }
    inner_test_rest_resume_metadata().await;
}

async fn inner_test_rest_resume_metadata() {
    let temp_dir = TempDir::new().expect("temp dir");
    let project_root = temp_dir.path().join("project");
    std::fs::create_dir_all(project_root.join(".rkat")).expect("create .rkat");

    let mut config = Config::default();
    config.agent.max_tokens_per_turn = 128;
    let config_store = MemoryConfigStore::new(config.clone());

    let store_path = temp_dir.path().join("sessions");
    let (event_tx, _) = tokio::sync::broadcast::channel(16);

    let factory = AgentFactory::new(store_path.clone())
        .builtins(true)
        .shell(true)
        .project_root(project_root.clone());
    let mut builder = FactoryAgentBuilder::new(factory, config.clone());
    builder.default_llm_client = Some(Arc::new(TestClient::default()));
    let builder_slot = builder.build_config_slot.clone();
    let session_service = Arc::new(EphemeralSessionService::new(builder, 100));

    let state_run = AppState {
        store_path: store_path.clone(),
        default_model: config.agent.model.clone().into(),
        max_tokens: config.agent.max_tokens_per_turn,
        rest_host: config.rest.host.clone().into(),
        rest_port: config.rest.port,
        enable_builtins: true,
        enable_shell: true,
        project_root: Some(project_root.clone()),
        llm_client_override: Some(Arc::new(TestClient::default())),
        config_store: std::sync::Arc::new(config_store),
        event_tx,
        session_service,
        builder_slot,
    };

    let app = router(state_run);
    let run_payload = json!({
        "prompt": "Say the word 'ok' and nothing else.",
        "model": config.agent.model,
        "max_tokens": config.agent.max_tokens_per_turn
    });
    let request = Request::builder()
        .method("POST")
        .uri("/sessions")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_vec(&run_payload).expect("serialize payload"),
        ))
        .expect("build request");

    let response = timeout(Duration::from_secs(120), app.oneshot(request))
        .await
        .expect("run request timed out")
        .expect("run request");
    assert_eq!(response.status(), StatusCode::OK);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("read body")
        .to_bytes();
    let run_json: Value = serde_json::from_slice(&body).expect("parse response");
    let session_id = run_json["session_id"]
        .as_str()
        .expect("session_id")
        .to_string();

    let store = JsonlStore::new(store_path.clone());
    store.init().await.expect("init store");
    let session = store
        .load(&SessionId::parse(&session_id).expect("session id"))
        .await
        .expect("load session")
        .expect("session exists");
    let metadata = session.session_metadata().expect("metadata");

    let original_model = metadata.model.clone();
    let original_max_tokens = metadata.max_tokens;
    let original_tooling = metadata.tooling.clone();
    let original_provider = metadata.provider;

    let factory2 = AgentFactory::new(store_path.clone())
        .builtins(true)
        .shell(true)
        .project_root(project_root.clone());
    let mut builder2 = FactoryAgentBuilder::new(factory2, config.clone());
    builder2.default_llm_client = Some(Arc::new(TestClient::default()));
    let builder_slot2 = builder2.build_config_slot.clone();
    let session_service2 = Arc::new(EphemeralSessionService::new(builder2, 100));

    let state_resume = AppState {
        store_path: store_path.clone(),
        default_model: "gpt-4o-mini".into(),
        max_tokens: 7,
        rest_host: config.rest.host.clone().into(),
        rest_port: config.rest.port,
        enable_builtins: true,
        enable_shell: true,
        project_root: Some(project_root.clone()),
        llm_client_override: Some(Arc::new(TestClient::default())),
        config_store: std::sync::Arc::new(MemoryConfigStore::new(config.clone())),
        event_tx: tokio::sync::broadcast::channel(16).0,
        session_service: session_service2,
        builder_slot: builder_slot2,
    };

    let app = router(state_resume);
    let resume_payload = json!({
        "prompt": "Continue.",
        "session_id": session_id
    });
    let request = Request::builder()
        .method("POST")
        .uri(format!("/sessions/{}/messages", session_id))
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_vec(&resume_payload).expect("serialize payload"),
        ))
        .expect("build request");

    let response = timeout(Duration::from_secs(120), app.oneshot(request))
        .await
        .expect("resume request timed out")
        .expect("resume request");
    assert_eq!(response.status(), StatusCode::OK);

    let session = store
        .load(&SessionId::parse(run_json["session_id"].as_str().unwrap()).expect("session id"))
        .await
        .expect("load session")
        .expect("session exists");
    let metadata = session.session_metadata().expect("metadata");

    assert_eq!(metadata.model, original_model);
    assert_eq!(metadata.max_tokens, original_max_tokens);
    assert_eq!(metadata.tooling.builtins, original_tooling.builtins);
    assert_eq!(metadata.tooling.shell, original_tooling.shell);
    assert_eq!(metadata.provider, original_provider);
}
