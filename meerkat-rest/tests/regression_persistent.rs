#![allow(clippy::unwrap_used, clippy::expect_used)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use meerkat::{
    AgentFactory, Config, FactoryAgentBuilder, MemoryStore, PersistentSessionService, SessionId,
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

/// Build an `AppState` backed by the given `MemoryStore`, with optional overrides
/// for `default_model` and `max_tokens` on the `AppState` itself.
///
/// The `enable_builtins` / `enable_shell` flags default to `true` unless
/// overridden by the caller through the returned struct.
fn build_state(
    store: Arc<dyn SessionStore>,
    store_path: &std::path::Path,
    project_root: &std::path::Path,
    config: &Config,
    default_model: Option<&str>,
    max_tokens: Option<u32>,
) -> AppState {
    let factory = AgentFactory::new(store_path.to_path_buf())
        .builtins(true)
        .shell(true)
        .project_root(project_root.to_path_buf());
    let mut builder = FactoryAgentBuilder::new(factory, config.clone());
    builder.default_llm_client = Some(Arc::new(TestClient::default()));
    let session_service = Arc::new(PersistentSessionService::new(builder, 100, store, None));

    let config_store: Arc<dyn meerkat_core::ConfigStore> =
        Arc::new(MemoryConfigStore::new(config.clone()));
    let config_runtime = Arc::new(meerkat_core::ConfigRuntime::new(
        Arc::clone(&config_store),
        store_path.join("config_state.json"),
    ));
    let (event_tx, _) = tokio::sync::broadcast::channel(16);

    AppState {
        store_path: store_path.to_path_buf(),
        default_model: default_model
            .unwrap_or(&config.agent.model)
            .to_string()
            .into(),
        max_tokens: max_tokens.unwrap_or(config.agent.max_tokens_per_turn),
        rest_host: config.rest.host.clone().into(),
        rest_port: config.rest.port,
        enable_builtins: true,
        enable_shell: true,
        project_root: Some(project_root.to_path_buf()),
        llm_client_override: Some(Arc::new(TestClient::default())),
        config_store,
        event_tx,
        session_service,
        webhook_auth: meerkat_rest::webhook::WebhookAuth::None,
        realm_id: "test-realm".to_string(),
        instance_id: None,
        backend: "redb".to_string(),
        resolved_paths: meerkat_core::ConfigResolvedPaths {
            root: store_path.display().to_string(),
            manifest_path: String::new(),
            config_path: String::new(),
            sessions_redb_path: String::new(),
            sessions_jsonl_dir: String::new(),
        },
        expose_paths: false,
        config_runtime,
        realm_lease: Arc::new(tokio::sync::Mutex::new(None)),
        skill_runtime: None,
        runtime_adapter: std::sync::Arc::new(meerkat_runtime::RuntimeSessionAdapter::ephemeral()),
        #[cfg(feature = "mob")]
        mob_state: meerkat_mob_mcp::MobMcpState::new_in_memory(),
        #[cfg(feature = "mcp")]
        mcp_sessions: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
    }
}

/// Scaffold: temp dir, project root with `.rkat/`, config, shared store.
struct Scaffold {
    store: Arc<dyn SessionStore>,
    store_path: std::path::PathBuf,
    project_root: std::path::PathBuf,
    config: Config,
    _temp_dir: TempDir,
}

impl Scaffold {
    fn new() -> Self {
        Self::with_config(Config::default())
    }

    fn with_config(config: Config) -> Self {
        let temp_dir = TempDir::new().expect("temp dir");
        let project_root = temp_dir.path().join("project");
        std::fs::create_dir_all(project_root.join(".rkat")).expect("create .rkat");
        let store_path = temp_dir.path().join("sessions");
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        Self {
            store,
            store_path,
            project_root,
            config,
            _temp_dir: temp_dir,
        }
    }

    fn state(&self) -> AppState {
        build_state(
            self.store.clone(),
            &self.store_path,
            &self.project_root,
            &self.config,
            None,
            None,
        )
    }

    fn state_with_overrides(
        &self,
        default_model: Option<&str>,
        max_tokens: Option<u32>,
    ) -> AppState {
        build_state(
            self.store.clone(),
            &self.store_path,
            &self.project_root,
            &self.config,
            default_model,
            max_tokens,
        )
    }
}

/// POST /sessions with the given payload, return (session_id, response_json).
async fn create_session(state: AppState, payload: Value) -> (String, Value) {
    let app = router(state);
    let request = Request::builder()
        .method("POST")
        .uri("/sessions")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&payload).expect("serialize")))
        .expect("build request");
    let response = timeout(Duration::from_secs(30), app.oneshot(request))
        .await
        .expect("create timed out")
        .expect("create request");
    assert_eq!(response.status(), StatusCode::OK);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("read body")
        .to_bytes();
    let json: Value = serde_json::from_slice(&body).expect("parse response");
    let session_id = json["session_id"]
        .as_str()
        .expect("session_id field")
        .to_string();
    (session_id, json)
}

/// POST /sessions/{id}/messages to resume, return (status, response_json).
async fn resume_session(state: AppState, session_id: &str, payload: Value) -> (StatusCode, Value) {
    let app = router(state);
    let request = Request::builder()
        .method("POST")
        .uri(format!("/sessions/{session_id}/messages"))
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&payload).expect("serialize")))
        .expect("build request");
    let response = timeout(Duration::from_secs(30), app.oneshot(request))
        .await
        .expect("resume timed out")
        .expect("resume request");
    let status = response.status();
    let body = response
        .into_body()
        .collect()
        .await
        .expect("read body")
        .to_bytes();
    let json: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    (status, json)
}

/// DELETE /sessions/{id} to archive, return status.
async fn archive_session(state: AppState, session_id: &str) -> StatusCode {
    let app = router(state);
    let request = Request::builder()
        .method("DELETE")
        .uri(format!("/sessions/{session_id}"))
        .body(Body::empty())
        .expect("build request");
    let response = timeout(Duration::from_secs(10), app.oneshot(request))
        .await
        .expect("archive timed out")
        .expect("archive request");
    response.status()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn resume_preserves_model_metadata() {
    let mut config = Config::default();
    config.agent.model = "claude-sonnet-4-5".to_string();
    let s = Scaffold::with_config(config);

    let (sid, _) = create_session(
        s.state(),
        json!({
            "prompt": "Say ok.",
            "model": "claude-sonnet-4-5",
            "max_tokens": 128
        }),
    )
    .await;

    // Reconstruct service with a DIFFERENT default model — resume must preserve original.
    let state2 = s.state_with_overrides(Some("gpt-5.2"), None);
    let (status, _) = resume_session(
        state2,
        &sid,
        json!({ "prompt": "Continue.", "session_id": sid }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let session = s
        .store
        .load(&SessionId::parse(&sid).expect("parse sid"))
        .await
        .expect("load")
        .expect("session exists");
    let metadata = session.session_metadata().expect("metadata present");
    assert_eq!(metadata.model, "claude-sonnet-4-5");
}

#[tokio::test]
async fn resume_preserves_max_tokens() {
    let s = Scaffold::new();

    let (sid, _) = create_session(
        s.state(),
        json!({
            "prompt": "Say ok.",
            "max_tokens": 128
        }),
    )
    .await;

    // Reconstruct with wildly different max_tokens on the server.
    let state2 = s.state_with_overrides(None, Some(9999));
    let (status, _) = resume_session(
        state2,
        &sid,
        json!({ "prompt": "Continue.", "session_id": sid }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let session = s
        .store
        .load(&SessionId::parse(&sid).expect("parse sid"))
        .await
        .expect("load")
        .expect("session exists");
    let metadata = session.session_metadata().expect("metadata present");
    assert_eq!(metadata.max_tokens, 128);
}

#[tokio::test]
async fn resume_preserves_tooling_flags() {
    let s = Scaffold::new();

    let (sid, _) = create_session(
        s.state(),
        json!({
            "prompt": "Say ok.",
            "max_tokens": 128
        }),
    )
    .await;

    // Reconstruct service — a fresh AppState from the same store.
    let state2 = s.state();
    let (status, _) = resume_session(
        state2,
        &sid,
        json!({ "prompt": "Continue.", "session_id": sid }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let session = s
        .store
        .load(&SessionId::parse(&sid).expect("parse sid"))
        .await
        .expect("load")
        .expect("session exists");
    let metadata = session.session_metadata().expect("metadata present");
    assert!(metadata.tooling.builtins, "builtins should be preserved");
    assert!(metadata.tooling.shell, "shell should be preserved");
}

#[tokio::test]
async fn resume_preserves_provider() {
    let s = Scaffold::new();

    let (sid, _) = create_session(
        s.state(),
        json!({
            "prompt": "Say ok.",
            "max_tokens": 128
        }),
    )
    .await;

    let session = s
        .store
        .load(&SessionId::parse(&sid).expect("parse sid"))
        .await
        .expect("load")
        .expect("session exists");
    let original_provider = session.session_metadata().expect("metadata").provider;

    // Reconstruct with a different default model (which implies a different provider).
    let state2 = s.state_with_overrides(Some("gpt-5.2"), None);
    let (status, _) = resume_session(
        state2,
        &sid,
        json!({ "prompt": "Continue.", "session_id": sid }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let session = s
        .store
        .load(&SessionId::parse(&sid).expect("parse sid"))
        .await
        .expect("load")
        .expect("session exists");
    let metadata = session.session_metadata().expect("metadata present");
    assert_eq!(metadata.provider, original_provider);
}

#[tokio::test]
async fn resume_message_count_increases() {
    let s = Scaffold::new();

    let (sid, _) = create_session(
        s.state(),
        json!({
            "prompt": "Say ok.",
            "max_tokens": 128
        }),
    )
    .await;

    let session_before = s
        .store
        .load(&SessionId::parse(&sid).expect("parse sid"))
        .await
        .expect("load")
        .expect("session exists");
    let count_before = session_before.messages().len();

    // Reconstruct and resume.
    let state2 = s.state();
    let (status, _) = resume_session(
        state2,
        &sid,
        json!({ "prompt": "Continue.", "session_id": sid }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let session_after = s
        .store
        .load(&SessionId::parse(&sid).expect("parse sid"))
        .await
        .expect("load")
        .expect("session exists");
    let count_after = session_after.messages().len();

    // Create adds user + assistant (2). Resume adds user + assistant (2 more). Total >= 4.
    assert!(
        count_after >= 4,
        "expected at least 4 messages after resume, got {count_after}"
    );
    assert!(
        count_after > count_before,
        "message count should increase: before={count_before}, after={count_after}"
    );
}

#[tokio::test]
async fn resume_after_archive_fails() {
    let s = Scaffold::new();

    let (sid, _) = create_session(
        s.state(),
        json!({
            "prompt": "Say ok.",
            "max_tokens": 128
        }),
    )
    .await;

    // Archive the session.
    let delete_status = archive_session(s.state(), &sid).await;
    assert_eq!(delete_status, StatusCode::OK);

    // Reconstruct and attempt to resume the archived session.
    let state2 = s.state();
    let (status, _) = resume_session(
        state2,
        &sid,
        json!({ "prompt": "Continue.", "session_id": sid }),
    )
    .await;
    assert!(
        status == StatusCode::NOT_FOUND || status == StatusCode::INTERNAL_SERVER_ERROR,
        "expected error status after archive, got {status}"
    );
}
