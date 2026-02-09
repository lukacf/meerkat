#![cfg(feature = "integration-real-tests")]
#![allow(clippy::unwrap_used, clippy::expect_used)]
use axum::body::Body;
use axum::http::Request;
use http_body_util::BodyExt;
use meerkat::{AgentFactory, Config, EphemeralSessionService, FactoryAgentBuilder};
use meerkat_client::TestClient;
use meerkat_core::MemoryConfigStore;
use meerkat_rest::{AppState, router};
use serde_json::json;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::time::{Duration, timeout};
use tower::ServiceExt;

#[tokio::test]
#[ignore = "integration-real: temporary regression reproducer (can hang)"]
async fn integration_real_live_continue_hangs() {
    let temp_dir = TempDir::new().unwrap();
    let project_root = temp_dir.path().join("project");
    std::fs::create_dir_all(project_root.join(".rkat")).unwrap();

    let config = Config::default();
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
        config_store: std::sync::Arc::new(MemoryConfigStore::new(config.clone())),
        event_tx,
        session_service,
        builder_slot,
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
