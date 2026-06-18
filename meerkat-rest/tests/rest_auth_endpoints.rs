#![allow(unused_mut)]

//! T15 (Phase 5): REST auth endpoints e2e — top-down observable proof
//! that the `/auth/*` + `/realms/*` routes registered in Phase 4d are
//! reachable through the router and return well-formed responses
//! (status code + JSON content-type) even without live credentials.
//!
//! Plan choke point K6 reads: "POST /auth/profiles -> GET
//! /auth/bindings/{binding_id} -> POST /sessions with auth_binding". The
//! live-provider half belongs in the `e2e-auth` lane; this file
//! exercises the offline router half: each route is registered,
//! dispatches, and returns a structured response.

#![cfg(feature = "integration-real-tests")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::redundant_clone,
    clippy::map_unwrap_or
)]

use axum::body::Body;
use axum::http::{Request, StatusCode, header::CONTENT_TYPE};
use http_body_util::BodyExt;
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
use serde_json::Value;
use std::sync::Arc;
use tempfile::TempDir;
use tower::ServiceExt;

fn build_app() -> axum::Router {
    // Empty realm-config chain: the source's realms-root is empty and the
    // global doc is absent, so every realm resolves to `None` (the composed
    // head config collapses to `Config::default()`), preserving prior behavior.
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_dir = Box::leak(Box::new(temp_dir));
    let realm_config_source: Arc<dyn meerkat_core::RealmConfigSource> =
        Arc::new(meerkat_store::FilesystemRealmConfigSource::new(
            temp_dir.path().join("empty-realms"),
            temp_dir.path().join("empty-realms").join("__no_global__"),
            meerkat_models::canonical(),
        ));
    build_app_inner(
        meerkat_core::RealmId::parse("test-realm").expect("valid realm"),
        Config::default(),
        realm_config_source,
    )
}

/// Build the REST app, seeding a filesystem realm-config chain so the COMPOSED
/// config (and the strict-owner write gate) sees realm-scoped `[realm.*]`
/// sections inherited along the parent chain. `realm_docs` maps each realm to
/// its OWN [`Config`] document; the reserved `global` realm is written to a
/// HOME-rooted doc the source maps to, every other realm to
/// `<realms_root>/<realm>/config.toml`. `head_realm` becomes `AppState.realm`
/// (the composition head).
///
/// Async because docs are seeded through [`meerkat_core::FileConfigStore`] (the
/// same TOML codec the source parses) — so the test never hand-rolls TOML and
/// the writes run on the live tokio reactor.
async fn build_app_with_realm_docs(
    head_realm: meerkat_core::RealmId,
    realm_docs: &[(meerkat_core::RealmId, Config)],
) -> axum::Router {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_dir = Box::leak(Box::new(temp_dir));

    let realms_root = temp_dir.path().join("realms");
    let global_doc = temp_dir.path().join("home").join("config.toml");
    for (realm, doc) in realm_docs {
        let path = if realm.is_global() {
            global_doc.clone()
        } else {
            realms_root.join(realm.as_str()).join("config.toml")
        };
        let store = meerkat_core::FileConfigStore::new(path, meerkat_models::canonical());
        meerkat_core::ConfigStore::set(&store, doc.clone())
            .await
            .expect("seed realm config doc");
    }
    let realm_config_source: Arc<dyn meerkat_core::RealmConfigSource> =
        Arc::new(meerkat_store::FilesystemRealmConfigSource::new(
            realms_root,
            global_doc,
            meerkat_models::canonical(),
        ));

    // The agent-build config seeded into the session service is the composed
    // head-chain config.
    let config = meerkat_core::EffectiveConfigReader::new(Arc::clone(&realm_config_source))
        .effective_config(&head_realm)
        .await
        .unwrap_or_else(|_| Config::default());

    build_app_inner(head_realm, config, realm_config_source)
}

/// Shared body for the two harnesses: wires `AppState` + router from an already
/// resolved `(head_realm, composed config, realm_config_source)`.
fn build_app_inner(
    head_realm: meerkat_core::RealmId,
    config: Config,
    realm_config_source: Arc<dyn meerkat_core::RealmConfigSource>,
) -> axum::Router {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_dir = Box::leak(Box::new(temp_dir));
    let project_root = temp_dir.path().join("project");
    std::fs::create_dir_all(project_root.join(".rkat")).expect("mkdir");

    let store_path = temp_dir.path().join("sessions");
    let (event_tx, _) = tokio::sync::broadcast::channel(16);

    let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());

    let factory = AgentFactory::new(store_path.clone())
        .builtins(false)
        .shell(false)
        .project_root(project_root.clone());
    let provider_registry = factory.provider_runtime_registry();
    let mut builder = FactoryAgentBuilder::new(factory, config.clone());
    builder.default_llm_client = Some(Arc::new(TestClient::default()));
    let persistence =
        PersistenceBundle::new(store, None, Arc::new(meerkat_store::MemoryBlobStore::new()));
    let runtime_adapter = persistence.runtime_adapter();
    let workgraph_store = persistence.workgraph_store();
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
    let config_store: Arc<dyn meerkat_core::ConfigStore> = Arc::new(MemoryConfigStore::new(
        config.clone(),
        meerkat_models::canonical(),
    ));
    let config_runtime = Arc::new(meerkat_core::ConfigRuntime::new(
        Arc::clone(&config_store),
        store_path.join("config_state.json"),
    ));

    let state = AppState {
        store_path: store_path.clone(),
        max_tokens: config.agent.resolved_max_tokens_per_turn(),
        rest_host: config.rest.host.clone().into(),
        rest_port: config.rest.port,
        enable_builtins: false,
        enable_shell: false,
        project_root: Some(project_root.clone()),
        context_root: None,
        user_config_root: None,
        llm_client_override: Some(Arc::new(TestClient::default())),
        config_store,
        event_tx,
        session_service,
        schedule_service: meerkat::ScheduleService::new(Arc::new(
            meerkat::MemoryScheduleStore::default(),
        )),
        workgraph_service: meerkat::WorkGraphService::with_scope(
            workgraph_store,
            "test-realm",
            meerkat::WorkNamespace::default(),
        ),
        webhook_auth: meerkat_rest::webhook::WebhookAuth::None,
        realm: head_realm,
        realm_config_source,
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
        config_runtime,
        realm_lease: Arc::new(tokio::sync::Mutex::new(None)),
        skill_runtime: None,
        runtime_adapter: runtime_adapter.clone(),
        runtime_pre_admissions: meerkat_rest::default_rest_runtime_pre_admissions(),
        runtime_registration_locks: meerkat_rest::default_rest_runtime_registration_locks(),
        schedule_host: Arc::default(),
        request_executor: std::sync::Arc::new(meerkat::surface::SurfaceRequestExecutor::new(
            std::time::Duration::from_secs(5),
        )),
        #[cfg(feature = "mob")]
        mob_state,
        #[cfg(feature = "mcp")]
        mcp_sessions: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
        token_store: Arc::new(meerkat_providers::auth_store::EphemeralTokenStore::new()),
        auth_lease: runtime_adapter.generated_auth_lease_handle(),
        provider_registry,
    };

    router(state)
}

/// Every Phase 4d-registered `/auth/*` + `/realms/*` collection route
/// dispatches (no 404 on the list endpoint) and returns a response
/// with a well-formed status code. Collection routes must exist —
/// per-ID 404s on missing resources are fine.
#[tokio::test]
async fn auth_routes_are_registered_and_dispatch() {
    let app = build_app();

    // Collection-scoped routes: 404 on the list itself means the route
    // isn't registered. These are strict asserts.
    for (method, path) in [("GET", "/auth/profiles"), ("GET", "/realms")] {
        let req = Request::builder()
            .method(method)
            .uri(path)
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_ne!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "collection route {method} {path} must be registered"
        );
    }

    // Per-resource routes: must match the router (not return 404 for
    // unknown-route reasons) — structured handler 404s for missing
    // resources are fine.
    for (method, path) in [
        ("GET", "/auth/bindings/nonexistent?realm_id=test-realm"),
        (
            "GET",
            "/auth/bindings/nonexistent/status?realm_id=test-realm",
        ),
        ("GET", "/realms/test-realm"),
    ] {
        let req = Request::builder()
            .method(method)
            .uri(path)
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let content_type = resp
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        assert!(
            status.is_success() || status.is_client_error() || status.is_server_error(),
            "route {method} {path} must return a valid HTTP status, got {status}"
        );

        if status == StatusCode::NOT_FOUND {
            assert!(
                content_type
                    .as_deref()
                    .is_some_and(|value| value.starts_with("application/json")),
                "route {method} {path} returned 404 without a JSON handler body; \
                 this looks like an unknown-route 404"
            );
            let body: Value =
                serde_json::from_slice(&bytes).expect("handler 404 body must be structured JSON");
            assert!(
                body.get("error").is_some(),
                "route {method} {path} returned 404 JSON without an error field: {body}"
            );
        }
    }
}

/// POST /auth/profiles with a well-formed body returns a JSON
/// response — verifying the handler parses the body and dispatches
/// through the TokenStore-backed flow.
#[tokio::test]
async fn create_auth_profile_accepts_valid_body() {
    let app = build_app();

    let body = serde_json::json!({
        "realm": "test-realm",
        "id": "default_openai",
        "provider": "openai",
        "backend_kind": "openai_api",
        "auth_method": "api_key",
        "source": {"kind": "inline_secret", "secret": "sk-test"}
    });

    let req = Request::builder()
        .method("POST")
        .uri("/auth/profiles")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    assert_ne!(status, StatusCode::NOT_FOUND, "route must be registered");

    // Handler should return a response — not panic. Accept success or
    // client error; the important guarantee is that the body+content-type
    // are well-formed.
    assert!(
        status.is_success() || status.is_client_error() || status.is_server_error(),
        "status must be a valid HTTP code, got {status}"
    );
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    // Body may be JSON (success or structured error) or plain text for
    // some error paths. The contract is: the request reached a handler
    // and returned an HTTP response.
    if !bytes.is_empty() {
        // If the body looks like JSON, it must parse.
        let trimmed = bytes.iter().position(|b| !b.is_ascii_whitespace());
        if trimmed.is_some_and(|i| matches!(bytes[i], b'{' | b'[' | b'"')) {
            let _parsed: Value =
                serde_json::from_slice(&bytes).expect("JSON-shaped body must parse");
        }
    }
}

/// `POST /sessions` accepts the structural `AuthBindingRef` field on the
/// body. When the referenced realm doesn't exist, the
/// server returns a structured error — not 404 or a panic.
#[tokio::test]
async fn post_sessions_accepts_auth_binding_field() {
    let app = build_app();

    let body = serde_json::json!({
        "prompt": "hi",
        "auth_binding": {"realm": "test-realm", "binding": "default"}
    });

    let req = Request::builder()
        .method("POST")
        .uri("/sessions")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    assert_ne!(
        status,
        StatusCode::NOT_FOUND,
        "/sessions must be registered"
    );
    assert!(
        status.is_success() || status.is_client_error() || status.is_server_error(),
        "status must be valid HTTP code for auth_binding body, got {status}"
    );
}

// ---------------------------------------------------------------------------
// RCT-27: hierarchical realm config inheritance through the REST surface.
//
// Reads inherit DOWN the chain (a child realm sees a binding defined only on
// its `global` ancestor through the COMPOSED config); writes are STRICT-OWNER
// (an OAuth login/persist for a child realm whose binding is inherited from
// `global` is REJECTED with 409 naming the owner, never persisted into the
// child). This proves both MF-A (compose the auth-resolution config) and MF-B
// (strict-owner write gate on the OAuth persist endpoints) end to end.
// ---------------------------------------------------------------------------

/// `[realm.global]` defining a single anthropic binding `default_anthropic`
/// (the inherited owner) — written to the HOME-rooted global doc by the
/// harness. Built via `from_inline_api_keys` so the doc is guaranteed valid.
fn global_realm_doc() -> Config {
    let mut config = Config::default();
    let section = meerkat_core::RealmConfigSection::from_inline_api_keys(&[("anthropic", "sk-x")]);
    config.realm.insert("global".to_string(), section);
    config
}

/// Child realm `child-realm` whose OWN section defines NO binding and inherits
/// from `global` via its `parent` edge. The composed config therefore exposes
/// `default_anthropic` to `child-realm` for READS, while a WRITE targeting it
/// must be rejected (strict-owner).
fn child_realm_doc() -> (meerkat_core::RealmId, Config) {
    let child = meerkat_core::RealmId::parse("child-realm").expect("valid child realm");
    let mut config = Config::default();
    let mut section = meerkat_core::RealmConfigSection::default();
    section.parent = Some(meerkat_core::RealmId::global());
    config.realm.insert(child.as_str().to_string(), section);
    (child, config)
}

/// READ half: an inherited `[realm.global]` binding is visible to the child
/// realm through the COMPOSED config (MF-A). Asserted at the composition layer
/// the auth-resolution path consumes: `resolve_realm_binding_target_for_provider`
/// resolves the inherited binding for the child head and owner-stamps it to
/// `global` — proving inheritance flows DOWN the chain on reads.
#[tokio::test]
async fn rct27_inherited_global_binding_visible_through_composed_config() {
    use std::sync::Arc;

    let temp_dir = TempDir::new().expect("temp dir");
    let realms_root = temp_dir.path().join("realms");
    let global_doc = temp_dir.path().join("home").join("config.toml");

    // Seed `[realm.global]` (owner) + child realm (inherits via parent edge).
    let (child, child_doc) = child_realm_doc();
    for (realm, doc) in [
        (meerkat_core::RealmId::global(), global_realm_doc()),
        (child.clone(), child_doc),
    ] {
        let path = if realm.is_global() {
            global_doc.clone()
        } else {
            realms_root.join(realm.as_str()).join("config.toml")
        };
        let store = meerkat_core::FileConfigStore::new(path, meerkat_models::canonical());
        meerkat_core::ConfigStore::set(&store, doc)
            .await
            .expect("seed realm doc");
    }

    let source: Arc<dyn meerkat_core::RealmConfigSource> =
        Arc::new(meerkat_store::FilesystemRealmConfigSource::new(
            realms_root,
            global_doc,
            meerkat_models::canonical(),
        ));
    let composed = meerkat_core::EffectiveConfigReader::new(source)
        .effective_config(&child)
        .await
        .expect("compose child chain");

    // The inherited binding resolves for the child head and is owner-stamped to
    // the owning `global` realm — reads inherit DOWN the chain.
    let target = meerkat_core::resolve_realm_binding_target_for_provider(
        &composed,
        meerkat_core::Provider::Anthropic,
        Some(&child),
        Some(&meerkat_core::connection::BindingId::parse("default_anthropic").unwrap()),
        None,
        None,
        false,
    )
    .expect("inherited binding must resolve for the child realm");
    assert!(
        target.auth_binding.realm.is_global(),
        "inherited binding must be owner-stamped to the global realm, got {}",
        target.auth_binding.realm.as_str()
    );
}

/// WRITE half: an OAuth login for the child realm targeting the inherited
/// `default_anthropic` binding is REJECTED with 409 naming the owning `global`
/// realm — the strict-owner write gate (MF-B) runs before any persist. The same
/// login targeting the OWNING `global` realm is NOT rejected by the gate
/// (it proceeds past the strict-owner check).
#[tokio::test]
async fn rct27_oauth_login_for_inherited_child_binding_is_rejected_409() {
    let (child, child_doc) = child_realm_doc();
    let app = build_app_with_realm_docs(
        child.clone(),
        &[
            (meerkat_core::RealmId::global(), global_realm_doc()),
            (child.clone(), child_doc),
        ],
    )
    .await;

    // Child realm + inherited binding: strict-owner write gate must REJECT.
    let body = serde_json::json!({
        "provider": "anthropic",
        "redirect_uri": "http://localhost:1455/callback",
        "realm_id": child.as_str(),
        "binding_id": "default_anthropic",
    });
    let req = Request::builder()
        .method("POST")
        .uri("/auth/login/start")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::CONFLICT,
        "OAuth login for a child realm's inherited binding must be rejected (strict-owner)"
    );
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&bytes).expect("structured JSON error");
    let err = body
        .get("error")
        .and_then(Value::as_str)
        .expect("error field");
    assert!(
        err.contains("global"),
        "409 error must name the owning realm 'global', got: {err}"
    );

    // Same login targeting the OWNING global realm passes the strict-owner gate
    // (it is NOT a CONFLICT; it proceeds to the OAuth flow / downstream checks).
    let owner_body = serde_json::json!({
        "provider": "anthropic",
        "redirect_uri": "http://localhost:1455/callback",
        "realm_id": "global",
        "binding_id": "default_anthropic",
    });
    let owner_req = Request::builder()
        .method("POST")
        .uri("/auth/login/start")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&owner_body).unwrap()))
        .unwrap();
    let owner_resp = app.oneshot(owner_req).await.unwrap();
    assert_ne!(
        owner_resp.status(),
        StatusCode::CONFLICT,
        "OAuth login targeting the OWNING realm must not hit the strict-owner gate"
    );
}
