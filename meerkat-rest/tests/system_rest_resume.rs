#![cfg(feature = "integration-real-tests")]
#![allow(unused_mut, clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use meerkat::{
    AgentFactory, Config, FactoryAgentBuilder, MemoryStore, PersistenceBundle,
    PersistentSessionService, SessionId, SessionStore,
};
use meerkat_client::TestClient;
use meerkat_core::MemoryConfigStore;
#[cfg(feature = "mob")]
use meerkat_mob_mcp::wire_mob_tools;
use meerkat_rest::{AppState, router};
use meerkat_store::StoreAdapter;
use serde_json::{Value, json};
use std::sync::Arc;
use tempfile::TempDir;
use tokio::time::{Duration, timeout};
use tower::ServiceExt;

fn skip_if_no_prereqs() -> bool {
    false
}

#[tokio::test]
#[ignore = "lane:e2e-system"]
async fn integration_real_rest_resume_metadata() {
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

    let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());

    let factory = AgentFactory::new(store_path.clone())
        .builtins(true)
        .shell(true)
        .project_root(project_root.clone());
    let provider_registry = factory.provider_runtime_registry();
    let mut builder = FactoryAgentBuilder::new(factory, config.clone());
    builder.default_llm_client = Some(Arc::new(TestClient::default()));
    let persistence = PersistenceBundle::new(
        store.clone(),
        None,
        Arc::new(meerkat_store::MemoryBlobStore::new()),
    );
    let runtime_adapter = persistence.runtime_adapter();
    builder.default_session_store = Some(Arc::new(StoreAdapter::new(persistence.session_store())));
    #[cfg(feature = "mob")]
    let builder_mob_tools_slot = Arc::clone(&builder.default_mob_tools);
    let (session_store_inner, runtime_store, blob_store) = persistence.into_parts();
    let mut session_service =
        PersistentSessionService::new(builder, 100, session_store_inner, runtime_store, blob_store);
    let session_service = Arc::new(session_service);
    #[cfg(feature = "mob")]
    let mob_state = wire_mob_tools(
        &builder_mob_tools_slot,
        session_service.clone(),
        Some(runtime_adapter.clone()),
        None,
    );
    let config_store_arc: Arc<dyn meerkat_core::ConfigStore> = Arc::new(config_store);
    let config_runtime = Arc::new(meerkat_core::ConfigRuntime::new(
        Arc::clone(&config_store_arc),
        store_path.join("config_state.json"),
    ));

    let state_run = AppState {
        store_path: store_path.clone(),
        default_model: config.agent.model.clone().into(),
        max_tokens: config.agent.max_tokens_per_turn,
        rest_host: config.rest.host.clone().into(),
        rest_port: config.rest.port,
        enable_builtins: true,
        enable_shell: true,
        project_root: Some(project_root.clone()),
        context_root: None,
        user_config_root: None,
        llm_client_override: Some(Arc::new(TestClient::default())),
        config_store: config_store_arc,
        event_tx,
        session_service,
        schedule_service: meerkat::ScheduleService::new(Arc::new(
            meerkat::MemoryScheduleStore::default(),
        )),
        webhook_auth: meerkat_rest::webhook::WebhookAuth::None,
        realm: meerkat_core::RealmId::parse("test-realm").expect("valid realm"),
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
        auth_lease: Arc::new(meerkat_runtime::RuntimeAuthLeaseHandle::new()),
        provider_registry,
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
    let provider_registry2 = factory2.provider_runtime_registry();
    let mut builder2 = FactoryAgentBuilder::new(factory2, config.clone());
    builder2.default_llm_client = Some(Arc::new(TestClient::default()));
    let persistence2 = PersistenceBundle::new(
        store.clone(),
        None,
        Arc::new(meerkat_store::MemoryBlobStore::new()),
    );
    let runtime_adapter2 = persistence2.runtime_adapter();
    builder2.default_session_store =
        Some(Arc::new(StoreAdapter::new(persistence2.session_store())));
    #[cfg(feature = "mob")]
    let builder2_mob_tools_slot = Arc::clone(&builder2.default_mob_tools);
    let (session_store2_inner, runtime_store2, blob_store2) = persistence2.into_parts();
    let mut session_service2 = PersistentSessionService::new(
        builder2,
        100,
        session_store2_inner,
        runtime_store2,
        blob_store2,
    );
    let session_service2 = Arc::new(session_service2);
    #[cfg(feature = "mob")]
    let mob_state2 = wire_mob_tools(
        &builder2_mob_tools_slot,
        session_service2.clone(),
        Some(runtime_adapter2.clone()),
        None,
    );
    let config_store_resume: Arc<dyn meerkat_core::ConfigStore> =
        Arc::new(MemoryConfigStore::new(config.clone()));
    let config_runtime_resume = Arc::new(meerkat_core::ConfigRuntime::new(
        Arc::clone(&config_store_resume),
        store_path.join("config_state.json"),
    ));

    let state_resume = AppState {
        store_path: store_path.clone(),
        default_model: "gpt-5.2".into(),
        max_tokens: 7,
        rest_host: config.rest.host.clone().into(),
        rest_port: config.rest.port,
        enable_builtins: true,
        enable_shell: true,
        project_root: Some(project_root.clone()),
        context_root: None,
        user_config_root: None,
        llm_client_override: Some(Arc::new(TestClient::default())),
        config_store: config_store_resume,
        event_tx: tokio::sync::broadcast::channel(16).0,
        session_service: session_service2,
        schedule_service: meerkat::ScheduleService::new(Arc::new(
            meerkat::MemoryScheduleStore::default(),
        )),
        webhook_auth: meerkat_rest::webhook::WebhookAuth::None,
        realm: meerkat_core::RealmId::parse("test-realm").expect("valid realm"),
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
        config_runtime: config_runtime_resume,
        realm_lease: Arc::new(tokio::sync::Mutex::new(None)),
        skill_runtime: None,
        runtime_adapter: runtime_adapter2,
        schedule_host: Arc::default(),
        request_executor: std::sync::Arc::new(meerkat::surface::SurfaceRequestExecutor::new(
            std::time::Duration::from_secs(5),
        )),
        #[cfg(feature = "mob")]
        mob_state: mob_state2,
        #[cfg(feature = "mcp")]
        mcp_sessions: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
        token_store: Arc::new(meerkat_providers::auth_store::EphemeralTokenStore::new()),
        auth_lease: Arc::new(meerkat_runtime::RuntimeAuthLeaseHandle::new()),
        provider_registry: provider_registry2,
    };

    let app = router(state_resume);
    let resume_payload = json!({
        "prompt": "Continue.",
        "session_id": session_id
    });
    let request = Request::builder()
        .method("POST")
        .uri(format!("/sessions/{session_id}/messages"))
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
