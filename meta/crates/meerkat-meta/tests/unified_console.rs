#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::uninlined_format_args,
    clippy::collapsible_if,
    clippy::redundant_clone,
    clippy::needless_raw_string_hashes,
    clippy::single_match,
    clippy::redundant_closure_for_method_calls,
    clippy::redundant_pattern_matching,
    clippy::ignored_unit_patterns,
    clippy::clone_on_copy,
    clippy::manual_assert,
    clippy::unwrap_in_result,
    clippy::useless_vec
)]
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::body::to_bytes;
use axum::http::{Request, StatusCode, header};
use futures::stream;
use meerkat::{AgentFactory, Config, build_ephemeral_service};
use meerkat_client::{LlmClient, LlmError, LlmEvent, LlmRequest, TestClient};
use meerkat_core::SessionId;
use meerkat_core::{Message, Provider, StopReason};
// meerkat 0.7: the MeerkatId alias was deleted; member ids are AgentIdentity.
use meerkat_meta::{
    AccessControlConfig, AccessController, AccessRule, AuthPolicy, BigQueryNaming, ConsolePolicy,
    ConsoleRestJsonRequest, DiscoverySpec, MetaConfig, MobBootstrapOptions, MobBootstrapSpec,
    MobRuntimeError, RuntimeDecisionInputs, RuntimeOpsPolicy, TrustedOidcRuntimeConfig,
    UnifiedRuntime, build_runtime_decision_state, console_json_router,
    handle_console_rest_json_route,
};
use meerkat_mob::ids::AgentIdentity as MobMemberId;
use meerkat_mob::{MobDefinition, MobRuntimeMode, MobStorage, SpawnMemberSpec};
use serde_json::{Value, json};
use tempfile::TempDir;
use tower::ServiceExt;

/// Per-test mob id counter: 0.8.23's fail-closed in-proc registration
/// means concurrently running tests must not share a supervisor route.
static NEXT_TEST_MOB_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The one definition of the normalized-provider-accounting contract every
/// Meta LLM double must satisfy under meerkat 0.8.22. See the module docs.
#[path = "support/llm_usage.rs"]
mod llm_usage;

struct RuntimeFixture {
    _temp_dir: TempDir,
    runtime: UnifiedRuntime,
}

fn trusted_toml() -> String {
    r#"
[[modules]]
id = "router"
command = "router-bin"
args = ["--mode", "fast"]
restart_policy = "always"

[[modules]]
id = "delivery"
command = "delivery-bin"
args = ["--sink", "test"]
restart_policy = "on_failure"
"#
    .to_string()
}

fn release_json() -> String {
    include_str!("../assets/release-targets.json").to_string()
}

fn trusted_oidc() -> TrustedOidcRuntimeConfig {
    TrustedOidcRuntimeConfig {
        discovery_json:
            r#"{"issuer":"https://trusted.meta.local","jwks_uri":"https://trusted.meta.local/.well-known/jwks.json"}"#
                .to_string(),
        jwks_json: r#"{"keys":[{"kid":"kid-current","kty":"oct","alg":"HS256","k":"cGhhc2U3LXRydXN0ZWQtY3VycmVudC1zZWNyZXQ"}]}"#
            .to_string(),
        audience: "meerkat-console".to_string(),
        require_verified_email: false,
    }
}

fn decision_state(require_app_auth: bool) -> meerkat_meta::RuntimeDecisionState {
    build_runtime_decision_state(RuntimeDecisionInputs {
        bigquery: BigQueryNaming {
            dataset: "phase_h1_dataset".to_string(),
            table: "phase_h1_table".to_string(),
        },
        trusted_meta_toml: trusted_toml(),
        auth: AuthPolicy {
            default_provider: meerkat_meta::AuthProvider::GoogleOAuth,
            email_allowlist: vec![
                "alice@example.com".to_string(),
                "svc:deploy-bot".to_string(),
            ],
        },
        trusted_oidc: trusted_oidc(),
        console: ConsolePolicy {
            require_app_auth,
            ..ConsolePolicy::default()
        },
        ops: RuntimeOpsPolicy::default(),
        release_metadata_json: release_json(),
    })
    .expect("decision state builds")
}

fn assert_mob_stop_allows_boundary_cancel(stop: Result<(), MobRuntimeError>) {
    if let Err(err) = stop {
        assert!(
            err.to_string().contains("cancel_after_boundary"),
            "shutdown failed: {err:?}"
        );
    }
}

async fn build_runtime_fixture() -> RuntimeFixture {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let session_path = temp_dir.path().join("sessions");
    std::fs::create_dir_all(&session_path).expect("session path");

    let factory = AgentFactory::new(&session_path).comms(true);
    let session_service = Arc::new(build_ephemeral_service(factory, Config::default(), 16));

    let definition = MobDefinition::from_toml(&format!(
        r#"
[mob]
id = "phase-h1-console-mob-{}"

[profiles.lead]
model = "gpt-5.5"
external_addressable = true

[profiles.lead.tools]
comms = true
"#,
        NEXT_TEST_MOB_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ))
    .expect("parse console mob definition");

    let mob_spec = MobBootstrapSpec::new(definition, MobStorage::in_memory(), session_service)
        .with_options(MobBootstrapOptions {
            allow_ephemeral_sessions: true,
            notify_orchestrator_on_resume: true,
            // `TestClient::default()` DOES synthesize accounting under 0.8.22,
            // but under `Provider::Other` - and every profile here is
            // `gpt-5.5`, whose canonical owner is `Provider::OpenAI`, so the
            // turn would fail closed with
            // `normalized_provider_accounting_identity_mismatch`. See the rule
            // in `tests/support/llm_usage.rs`.
            default_llm_client: Some(Arc::new(TestClient::for_provider(Provider::OpenAI))),
        });
    let module_config = MetaConfig {
        modules: vec![],
        discovery: DiscoverySpec {
            namespace: "phase-h1".to_string(),
            modules: vec![],
        },
        pre_spawn: vec![],
    };
    let runtime = UnifiedRuntime::bootstrap(mob_spec, module_config, Duration::from_secs(2))
        .await
        .expect("bootstrap unified runtime");

    RuntimeFixture {
        _temp_dir: temp_dir,
        runtime,
    }
}

/// [`build_runtime_fixture`] over a runtime with a store: the persistent
/// session service and a persistent runtime authority over SQLite stores in
/// the fixture's temp dir. Identity adoption needs one: Core refuses it on a
/// runtime without a store
/// (`mob_identity_adoption_requires_persistent_runtime`).
async fn build_store_backed_runtime_fixture() -> RuntimeFixture {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let state = temp_dir.path().join("state");
    std::fs::create_dir_all(&state).expect("state dir");
    let session_store: Arc<dyn meerkat::SessionStore> = Arc::new(
        meerkat_store::SqliteSessionStore::open(state.join("sessions.db")).expect("session store"),
    );
    let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> = Arc::new(
        meerkat_runtime::store::SqliteRuntimeStore::new(state.join("runtime.sqlite"))
            .expect("runtime store"),
    );
    let blob_store: Arc<dyn meerkat_core::BlobStore> =
        Arc::new(meerkat_store::MemoryBlobStore::new());
    let factory = AgentFactory::new(&state).comms(true);
    let mut builder = meerkat::FactoryAgentBuilder::new(factory, Config::default());
    builder.default_session_store = Some(Arc::new(meerkat_store::StoreAdapter::new(
        session_store.clone(),
    )));
    builder.default_blob_store = Some(blob_store.clone());
    let machine = Arc::new(
        meerkat_runtime::MeerkatMachine::persistent(
            Arc::clone(&runtime_store),
            Arc::clone(&blob_store),
        )
        .expect("acquire the store-backed console fixture runtime machine"),
    );
    let session_service = Arc::new(meerkat_session::PersistentSessionService::new(
        builder,
        16,
        session_store,
        runtime_store,
        blob_store,
    ));

    let definition = MobDefinition::from_toml(&format!(
        r#"
[mob]
id = "phase-h1-store-console-mob-{}"

[profiles.lead]
model = "gpt-5.5"
external_addressable = true

[profiles.lead.tools]
comms = true
"#,
        NEXT_TEST_MOB_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ))
    .expect("parse store-backed console mob definition");

    let mob_spec = MobBootstrapSpec::new(definition, MobStorage::in_memory(), session_service)
        .with_session_runtime_adapter(machine)
        .expect("acquire the fixture session runtime owner")
        .with_options(MobBootstrapOptions {
            allow_ephemeral_sessions: true,
            notify_orchestrator_on_resume: true,
            default_llm_client: Some(Arc::new(TestClient::for_provider(Provider::OpenAI))),
        });
    let module_config = MetaConfig {
        modules: vec![],
        discovery: DiscoverySpec {
            namespace: "phase-h1".to_string(),
            modules: vec![],
        },
        pre_spawn: vec![],
    };
    let runtime = UnifiedRuntime::bootstrap(mob_spec, module_config, Duration::from_secs(2))
        .await
        .expect("bootstrap store-backed unified runtime");

    RuntimeFixture {
        _temp_dir: temp_dir,
        runtime,
    }
}

fn console_member_spec(member_id: &str) -> SpawnMemberSpec {
    SpawnMemberSpec::from_wire(
        "lead".to_string(),
        MobMemberId::from(member_id).to_string(),
        Some(format!("You are {member_id}. Keep responses concise.").into()),
        None,
        None,
    )
}

async fn spawn_console_members(runtime: &UnifiedRuntime) {
    for member_id in ["router", "delivery"] {
        runtime
            .spawn(console_member_spec(member_id))
            .await
            .expect("spawn console member");
    }
}

async fn spawn_named_members(runtime: &UnifiedRuntime, member_ids: &[&str]) {
    for member_id in member_ids {
        runtime
            .spawn(console_member_spec(member_id))
            .await
            .expect("spawn named console member");
    }
}

async fn get_console_experience(app: &Router) -> Value {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/console/experience")
                .body(Body::empty())
                .expect("console request"),
        )
        .await
        .expect("console response");
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("console body");
    let experience: Value = serde_json::from_slice(&body).expect("console json");
    assert!(
        experience["storage_scope"]
            .as_str()
            .is_some_and(|scope| scope.starts_with("console-storage:v1:")),
        "default embedded experience must provide its authorized draft scope"
    );
    experience
}

async fn query_console_timeline_frames(app: &Router, identity: &str) -> Vec<Value> {
    let query_payload = json!({
        "jsonrpc": "2.0",
        "id": "query-timeline",
        "method": "meta/console/query_timeline",
        "params": { "identity": identity }
    });
    let query_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/console/rpc")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(query_payload.to_string()))
                .expect("query request"),
        )
        .await
        .expect("query response");
    assert_eq!(query_response.status(), StatusCode::OK);
    let query_body = to_bytes(query_response.into_body(), 1024 * 1024)
        .await
        .expect("query body");
    let query_json: Value = serde_json::from_slice(&query_body).expect("query json");
    query_json["result"]["frames"]
        .as_array()
        .expect("console timeline frames")
        .clone()
}

#[tokio::test]
#[ignore]
async fn phase_h1_req_001_reference_style_router_mounts_console_and_sse() {
    let fixture = build_runtime_fixture().await;
    spawn_console_members(&fixture.runtime).await;

    let app = fixture
        .runtime
        .build_reference_app_router(decision_state(false));
    let health_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/healthz")
                .body(Body::empty())
                .expect("health request"),
        )
        .await
        .expect("health response");
    assert_eq!(health_response.status(), StatusCode::OK);

    let console_entry_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/console")
                .body(Body::empty())
                .expect("console entry request"),
        )
        .await
        .expect("console entry response");
    let console_entry_status = console_entry_response.status();
    let console_entry_content_type = console_entry_response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let console_entry_body = to_bytes(console_entry_response.into_body(), 1024 * 1024)
        .await
        .expect("console entry body");
    let console_entry_text = String::from_utf8(console_entry_body.to_vec()).expect("console html");
    assert_eq!(console_entry_status, StatusCode::OK);
    assert!(
        console_entry_content_type.starts_with("text/html"),
        "expected text/html content-type, got: {console_entry_content_type}"
    );
    assert!(console_entry_text.contains("<div id=\"root\"></div>"));
    assert!(console_entry_text.contains("/console/assets/console-app.js"));

    let console_asset_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/console/assets/console-app.js")
                .body(Body::empty())
                .expect("console asset request"),
        )
        .await
        .expect("console asset response");
    let console_asset_status = console_asset_response.status();
    let console_asset_content_type = console_asset_response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let console_asset_body = to_bytes(console_asset_response.into_body(), 1024 * 1024)
        .await
        .expect("console asset body");
    let console_asset_text = String::from_utf8(console_asset_body.to_vec()).expect("console js");
    assert_eq!(console_asset_status, StatusCode::OK);
    assert!(
        console_asset_content_type.starts_with("application/javascript"),
        "expected application/javascript content-type, got: {console_asset_content_type}"
    );
    assert!(console_asset_text.contains("createConsoleApp"));

    let console_json = get_console_experience(&app).await;
    assert_eq!(
        console_json["agent_sidebar"]["panel_id"],
        json!("console.agent_sidebar")
    );
    assert_eq!(
        console_json["chat_inspector"]["panel_id"],
        json!("console.chat_inspector")
    );
    assert_eq!(
        console_json["topology"]["panel_id"],
        json!("console.topology")
    );
    assert_eq!(
        console_json["health_overview"]["panel_id"],
        json!("console.health_overview")
    );
    assert_eq!(
        console_json["agent_sidebar"]["live_snapshot"]["agents"],
        json!([
            {
                "agent_id": "delivery",
                "member_id": "delivery",
                "label": "delivery",
                "kind": "mob_agent",
                "profile": "lead",
                "state": "active",
                "wired_to": [],
                "labels": {},
                "group": "lead",
                "addressable": true,
                "affordances": {
                    "addressable": true,
                    "can_send_message": true,
                    "can_retire": true,
                    "can_respawn": true,
                    "runtime_mode": "mob_agent"
                }
            },
            {
                "agent_id": "router",
                "member_id": "router",
                "label": "router",
                "kind": "mob_agent",
                "profile": "lead",
                "state": "active",
                "wired_to": [],
                "labels": {},
                "group": "lead",
                "addressable": true,
                "affordances": {
                    "addressable": true,
                    "can_send_message": true,
                    "can_retire": true,
                    "can_respawn": true,
                    "runtime_mode": "mob_agent"
                }
            }
        ])
    );

    assert_eq!(console_json["contract_version"], json!("0.5.0"));
    assert_eq!(
        console_json["runtime_capabilities"],
        json!({
            "can_spawn_members": true,
            "can_send_messages": true,
            "can_wire_members": true,
            "can_retire_members": true,
            "available_spawn_modes": ["module", "profile"],
            "profile_capabilities": {
                "lead": {
                    "instance_count": 2,
                    "addressable": true,
                    "has_wiring": false,
                }
            }
        })
    );

    let shutdown = fixture.runtime.shutdown().await;
    assert_mob_stop_allows_boundary_cancel(shutdown.mob_stop);
}

#[tokio::test]
async fn live_snapshot_keeps_configured_modules_even_when_runtime_members_differ() {
    let fixture = build_runtime_fixture().await;
    spawn_named_members(&fixture.runtime, &["triage", "billing"]).await;

    let app = fixture
        .runtime
        .build_reference_app_router(decision_state(false));
    let console_json = get_console_experience(&app).await;

    // Topology now shows identity-native nodes from mob members, not config modules.
    let topology_nodes = console_json["topology"]["live_snapshot"]["nodes"]
        .as_array()
        .expect("topology nodes array");
    assert_eq!(topology_nodes.len(), 2);
    assert!(
        topology_nodes.iter().any(|n| n["identity"] == "billing"),
        "expected billing in topology nodes"
    );
    assert!(
        topology_nodes.iter().any(|n| n["identity"] == "triage"),
        "expected triage in topology nodes"
    );
    assert_eq!(
        console_json["topology"]["live_snapshot"]["node_count"],
        json!(2)
    );
    // Health overview loaded_modules still comes from config.
    assert_eq!(
        console_json["health_overview"]["live_snapshot"]["loaded_modules"],
        json!(["delivery", "router"])
    );
    assert_eq!(
        console_json["health_overview"]["live_snapshot"]["loaded_module_count"],
        json!(2)
    );
    assert_eq!(
        console_json["agent_sidebar"]["live_snapshot"]["agents"]
            .as_array()
            .expect("agents array")
            .len(),
        2
    );

    let shutdown = fixture.runtime.shutdown().await;
    assert_mob_stop_allows_boundary_cancel(shutdown.mob_stop);
}

#[tokio::test]
async fn reconcile_spawns_colon_identities_and_member_rows_keep_public_alias() {
    // meerkat 0.7's MemberCommsName is fail-closed: roster member ids must not
    // contain ':'. Identity-first identities like "domain:billing" are encoded
    // into comms-safe roster ids at the meta→meerkat-mob boundary and must
    // decode back to the public alias on every projection surface.
    let fixture = build_runtime_fixture().await;

    let report = fixture
        .runtime
        .reconcile(vec![
            console_member_spec("triage:main"),
            console_member_spec("domain:billing"),
        ])
        .await
        .expect("reconcile of ':'-bearing identities must spawn, not fail comms-name validation");

    assert!(
        report.mob.failures.is_empty(),
        "no per-identity reconcile failures expected: {:?}",
        report.mob.failures
    );
    let mut spawned: Vec<&str> = report
        .mob
        .spawned
        .iter()
        .map(|receipt| receipt.agent_identity.as_str())
        .collect();
    spawned.sort_unstable();
    assert_eq!(spawned, vec!["domain:billing", "triage:main"]);
    let mut desired = report.mob.desired.clone();
    desired.sort_unstable();
    assert_eq!(desired, vec!["domain:billing", "triage:main"]);
    assert!(
        !format!("{report:?}").contains("mk--"),
        "reconcile report must speak the public alias space, got: {report:?}"
    );

    // Member rows project the original identity string, not the encoded
    // roster id.
    let app = fixture
        .runtime
        .build_reference_app_router(decision_state(false));
    let list_payload = json!({
        "jsonrpc": "2.0",
        "id": "list-members",
        "method": "meta/list_members",
        "params": {}
    });
    let list_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/console/rpc")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(list_payload.to_string()))
                .expect("list_members request"),
        )
        .await
        .expect("list_members response");
    assert_eq!(list_response.status(), StatusCode::OK);
    let list_body = to_bytes(list_response.into_body(), 1024 * 1024)
        .await
        .expect("list_members body");
    let list_json: Value = serde_json::from_slice(&list_body).expect("list_members json");
    let mut row_identities: Vec<&str> = list_json["result"]
        .as_array()
        .expect("member rows array")
        .iter()
        .map(|row| row["agent_identity"].as_str().expect("agent_identity"))
        .collect();
    row_identities.sort_unstable();
    assert_eq!(row_identities, vec!["domain:billing", "triage:main"]);
    assert!(
        !list_json.to_string().contains("mk--"),
        "member rows must not leak encoded roster ids: {list_json}"
    );

    let shutdown = fixture.runtime.shutdown().await;
    assert_mob_stop_allows_boundary_cancel(shutdown.mob_stop);
}

#[tokio::test]
async fn multipart_blob_upload_round_trips_through_reference_router() {
    let definition = MobDefinition::from_toml(&format!(
        r#"
[mob]
id = "multipart-console-mob-{}"

[profiles.lead]
model = "gpt-5.5"
external_addressable = true

[profiles.lead.tools]
comms = true
"#,
        NEXT_TEST_MOB_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ))
    .expect("parse multipart mob definition");
    let runtime = Box::pin(
        UnifiedRuntime::builder()
            .definition(definition)
            .module_config(MetaConfig {
                modules: vec![],
                discovery: DiscoverySpec {
                    namespace: "multipart-console".to_string(),
                    modules: vec![],
                },
                pre_spawn: vec![],
            })
            .default_llm_client(Arc::new(TestClient::for_provider(Provider::OpenAI)))
            .timeout(Duration::from_secs(2))
            .build(),
    )
    .await
    .expect("build multipart runtime");

    let app = runtime.build_reference_app_router(decision_state(false));
    let boundary = "meta-test-boundary";
    let payload = json!({
        "jsonrpc": "2.0",
        "id": "upload-1",
        "method": "meta/blob/upload",
        "params": {
            "upload": {
                "type": "image_upload",
                "upload_id": "upload-1",
                "media_type": "image/png"
            }
        }
    });
    let mut body = Vec::new();
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(b"Content-Disposition: form-data; name=\"payload\"\r\n");
    body.extend_from_slice(b"Content-Type: application/json\r\n\r\n");
    body.extend_from_slice(payload.to_string().as_bytes());
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        b"Content-Disposition: form-data; name=\"file:upload-1\"; filename=\"tiny.png\"\r\n",
    );
    body.extend_from_slice(b"Content-Type: image/png\r\n\r\n");
    body.extend_from_slice(b"tiny-png");
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());

    let upload_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/console/rpc/multipart")
                .header(
                    header::CONTENT_TYPE,
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(body))
                .expect("multipart request"),
        )
        .await
        .expect("multipart response");
    assert_eq!(upload_response.status(), StatusCode::OK);
    let upload_body = to_bytes(upload_response.into_body(), 1024 * 1024)
        .await
        .expect("multipart body");
    let upload_json: Value = serde_json::from_slice(&upload_body).expect("upload json");
    let blob_id = upload_json["result"]["blob_id"]
        .as_str()
        .expect("blob id")
        .to_string();
    assert_eq!(upload_json["result"]["media_type"], json!("image/png"));
    assert_eq!(upload_json["result"]["size"], json!(8));

    let blob_response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/blobs/{blob_id}"))
                .body(Body::empty())
                .expect("blob request"),
        )
        .await
        .expect("blob response");
    assert_eq!(blob_response.status(), StatusCode::OK);
    assert_eq!(
        blob_response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("image/png")
    );
    let blob_body = to_bytes(blob_response.into_body(), 1024 * 1024)
        .await
        .expect("blob body");
    assert_eq!(blob_body.as_ref(), b"tiny-png");

    // The same blob behind `require_app_auth = true`: the route serves it to
    // the console's authenticated request and refuses a bare or forged one.
    // The Console reads stored images with its own fetch for exactly this.
    let mut protected_decisions = decision_state(true);
    // HS256 test keys are trusted only from a development issuer.
    protected_decisions.trusted_oidc.discovery_json = json!({
        "issuer": DEVELOPMENT_ISSUER,
        "jwks_uri": format!("{DEVELOPMENT_ISSUER}/.well-known/jwks.json"),
    })
    .to_string();
    let protected = runtime.build_reference_app_router(protected_decisions);
    for (label, credential) in [
        ("no credential", None),
        (
            "a token signed with another key",
            Some(console_bearer(b"some-other-secret")),
        ),
    ] {
        let mut request = Request::builder()
            .method("GET")
            .uri(format!("/blobs/{blob_id}"));
        if let Some(credential) = credential {
            request = request.header(header::AUTHORIZATION, format!("Bearer {credential}"));
        }
        let refused = protected
            .clone()
            .oneshot(request.body(Body::empty()).expect("blob request"))
            .await
            .expect("blob response");
        assert_eq!(refused.status(), StatusCode::UNAUTHORIZED, "{label}");
        let refused_body = to_bytes(refused.into_body(), 1024 * 1024)
            .await
            .expect("refused body");
        assert_ne!(refused_body.as_ref(), b"tiny-png", "{label}");
    }
    let served = protected
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/blobs/{blob_id}"))
                .header(
                    header::AUTHORIZATION,
                    format!(
                        "Bearer {}",
                        console_bearer(b"phase7-trusted-current-secret")
                    ),
                )
                .body(Body::empty())
                .expect("authenticated blob request"),
        )
        .await
        .expect("authenticated blob response");
    assert_eq!(served.status(), StatusCode::OK);
    assert_eq!(
        served
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("image/png")
    );
    let served_body = to_bytes(served.into_body(), 1024 * 1024)
        .await
        .expect("authenticated blob body");
    assert_eq!(served_body.as_ref(), b"tiny-png");

    let shutdown = runtime.shutdown().await;
    assert_mob_stop_allows_boundary_cancel(shutdown.mob_stop);
}

const DEVELOPMENT_ISSUER: &str = "https://trusted.meta.localhost";

/// A console bearer for `alice@example.com`, an allowlisted principal of
/// [`trusted_oidc`], issued by [`DEVELOPMENT_ISSUER`] and signed with
/// `secret` (the trusted key is `phase7-trusted-current-secret`).
fn console_bearer(secret: &[u8]) -> String {
    let mut jwt_header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256);
    jwt_header.kid = Some("kid-current".to_string());
    jsonwebtoken::encode(
        &jwt_header,
        &json!({
            "iss": DEVELOPMENT_ISSUER,
            "aud": "meerkat-console",
            "sub": "alice@example.com",
            "email": "alice@example.com",
            "provider": "google_oauth",
            "exp": chrono::Utc::now().timestamp() + 300,
        }),
        &jsonwebtoken::EncodingKey::from_secret(secret),
    )
    .expect("signed console token")
}

#[tokio::test]
async fn multipart_send_message_projects_text_and_image_into_console_events() {
    let definition = MobDefinition::from_toml(
        r#"
[mob]
id = "multipart-send-console-mob"

[profiles.lead]
model = "gpt-5.5"
external_addressable = true

[profiles.lead.tools]
comms = true
"#,
    )
    .expect("parse multipart send mob definition");
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let session_path = temp_dir.path().join("sessions");
    std::fs::create_dir_all(&session_path).expect("session path");
    let factory = AgentFactory::new(&session_path).comms(true);
    let session_service = Arc::new(build_ephemeral_service(factory, Config::default(), 16));
    let binary_blob_store: Arc<dyn meerkat_meta::BinaryBlobStore> =
        Arc::new(meerkat_meta::ObjectStoreBlobStore::memory());
    let mut mob_spec = MobBootstrapSpec::new(definition, MobStorage::in_memory(), session_service)
        .with_options(MobBootstrapOptions {
            allow_ephemeral_sessions: true,
            notify_orchestrator_on_resume: true,
            default_llm_client: Some(Arc::new(TestClient::for_provider(Provider::OpenAI))),
        });
    mob_spec.binary_blob_store = Some(binary_blob_store);
    let runtime = UnifiedRuntime::bootstrap(
        mob_spec,
        MetaConfig {
            modules: vec![],
            discovery: DiscoverySpec {
                namespace: "multipart-send-console".to_string(),
                modules: vec![],
            },
            pre_spawn: vec![],
        },
        Duration::from_secs(2),
    )
    .await
    .expect("build multipart send runtime");
    runtime
        .spawn(console_member_spec("analyst"))
        .await
        .expect("spawn analyst member");

    let app = runtime.build_reference_app_router(decision_state(false));
    let boundary = "meta-send-boundary";
    let payload = json!({
        "jsonrpc": "2.0",
        "id": "send-1",
        "method": "meta/console/send",
        "params": {
            "identity": "analyst",
            "origin": "test",
            "idempotency_key": "multipart-send-image-1",
            "content": [
                { "type": "text", "text": "Describe this inline image." },
                {
                    "type": "image_upload",
                    "part_name": "image-field",
                    "media_type": "image/png"
                }
            ]
        }
    });
    let mut body = Vec::new();
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(b"Content-Disposition: form-data; name=\"payload\"\r\n");
    body.extend_from_slice(b"Content-Type: application/json\r\n\r\n");
    body.extend_from_slice(payload.to_string().as_bytes());
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        b"Content-Disposition: form-data; name=\"file:image-field\"; filename=\"tiny.png\"\r\n",
    );
    body.extend_from_slice(b"Content-Type: image/png\r\n\r\n");
    body.extend_from_slice(b"tiny-png");
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());

    let send_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/console/rpc/multipart")
                .header(
                    header::CONTENT_TYPE,
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(body))
                .expect("multipart send request"),
        )
        .await
        .expect("multipart send response");
    assert_eq!(send_response.status(), StatusCode::OK);
    let send_body = to_bytes(send_response.into_body(), 1024 * 1024)
        .await
        .expect("multipart send body");
    let send_json: Value = serde_json::from_slice(&send_body).expect("send json");
    assert!(
        send_json.get("error").is_none() || send_json["error"].is_null(),
        "send failed: {send_json}"
    );
    assert_eq!(send_json["result"]["status"], json!("accepted"));
    assert_eq!(send_json["result"]["identity"], json!("analyst"));
    assert!(send_json["result"]["interaction_id"].as_str().is_some());

    let frames = query_console_timeline_frames(&app, "analyst").await;
    let started = frames
        .iter()
        .find(|frame| frame["kind"] == "user_input" || frame["event"] == "interaction_started")
        .expect("user input timeline frame");
    assert_eq!(
        started["payload"]["content"][0]["text"],
        json!("Describe this inline image.")
    );
    assert_eq!(started["payload"]["content"][1]["type"], json!("image"));
    assert_eq!(started["payload"]["content"][1]["source"], json!("blob"));
    assert_eq!(
        started["payload"]["content"][1]["media_type"],
        json!("image/png")
    );
    assert!(
        started["payload"]["content"][1]["blob_id"]
            .as_str()
            .is_some_and(|value| value.starts_with("sha256:"))
    );

    let shutdown = runtime.shutdown().await;
    assert_mob_stop_allows_boundary_cancel(shutdown.mob_stop);
}

#[tokio::test]
#[ignore]
async fn phase_h1_live_snapshot_tracks_runtime_drift() {
    let fixture = build_runtime_fixture().await;
    spawn_console_members(&fixture.runtime).await;

    let app = fixture
        .runtime
        .build_reference_app_router(decision_state(false));
    let initial = get_console_experience(&app).await;

    assert_eq!(
        initial["health_overview"]["live_snapshot"]["running"],
        json!(true)
    );
    assert_eq!(
        initial["health_overview"]["live_snapshot"]["loaded_modules"],
        json!(["delivery", "router"])
    );

    let reconcile = fixture
        .runtime
        .reconcile(vec![console_member_spec("router")])
        .await
        .expect("reconcile delivery retirement");
    assert_eq!(reconcile.mob.retired, vec!["delivery".to_string()]);

    let after_retire = get_console_experience(&app).await;
    assert_eq!(
        after_retire["agent_sidebar"]["live_snapshot"]["agents"],
        json!([
            {
                "agent_id": "router",
                "member_id": "router",
                "label": "router",
                "kind": "mob_agent",
                "profile": "lead",
                "state": "active",
                "wired_to": [],
                "labels": {},
                "group": "lead",
                "addressable": true,
                "affordances": {
                    "addressable": true,
                    "can_respawn": true,
                    "can_retire": true,
                    "can_send_message": true,
                    "runtime_mode": "mob_agent"
                }
            }
        ])
    );
    assert_eq!(
        after_retire["topology"]["live_snapshot"]["nodes"],
        json!(["delivery", "router"])
    );
    assert_eq!(
        after_retire["topology"]["live_snapshot"]["node_count"],
        json!(2)
    );
    assert_eq!(
        after_retire["health_overview"]["live_snapshot"]["loaded_modules"],
        json!(["delivery", "router"])
    );
    assert_eq!(
        after_retire["health_overview"]["live_snapshot"]["loaded_module_count"],
        json!(2)
    );

    let shutdown = fixture.runtime.shutdown().await;
    assert_mob_stop_allows_boundary_cancel(shutdown.mob_stop);
    let after_stop = get_console_experience(&app).await;
    assert_eq!(
        after_stop["health_overview"]["live_snapshot"]["running"],
        json!(false)
    );
}

#[tokio::test]
#[ignore]
async fn phase_h1_console_modules_route_honors_auth_mode() {
    let open_state = decision_state(false);
    let direct_open = handle_console_rest_json_route(
        &open_state,
        &ConsoleRestJsonRequest {
            method: "GET".to_string(),
            path: "/console/modules".to_string(),
            auth: None,
        },
    );
    let open_app = console_json_router(open_state);
    let open_response = open_app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/console/modules")
                .body(Body::empty())
                .expect("open request"),
        )
        .await
        .expect("open response");
    let open_status = open_response.status();
    let open_body = to_bytes(open_response.into_body(), 1024 * 1024)
        .await
        .expect("open body");
    let open_json: Value = serde_json::from_slice(&open_body).expect("open json");

    assert_eq!(open_status, StatusCode::OK);
    assert_eq!(direct_open.status, 200);
    assert_eq!(open_json, direct_open.body);
    assert_eq!(open_json["modules"], json!(["router", "delivery"]));

    let state = decision_state(true);
    let direct = handle_console_rest_json_route(
        &state,
        &ConsoleRestJsonRequest {
            method: "GET".to_string(),
            path: "/console/modules".to_string(),
            auth: None,
        },
    );
    let app = console_json_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/console/modules")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    let response_status = response.status();
    let body = to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("body");
    let json_body: Value = serde_json::from_slice(&body).expect("json body");

    assert_eq!(response_status, StatusCode::UNAUTHORIZED);
    assert_eq!(direct.status, 401);
    assert_eq!(
        StatusCode::from_u16(direct.status).expect("status code"),
        response_status
    );
    assert_eq!(
        direct.body,
        json!({"error":"unauthorized","reason":"missing_credentials"})
    );
    assert_eq!(json_body, direct.body);
}

#[tokio::test]
#[ignore]
async fn phase_h1_cross_panel_sidebar_agent_streams_and_unknown_member_rejected() {
    let fixture = build_runtime_fixture().await;
    spawn_console_members(&fixture.runtime).await;

    let app = fixture
        .runtime
        .build_reference_app_router(decision_state(false));
    let console_json = get_console_experience(&app).await;

    let selected_agent_id = console_json["agent_sidebar"]["live_snapshot"]["agents"]
        .as_array()
        .expect("agents array")
        .first()
        .and_then(|agent| agent.get("agent_id"))
        .and_then(Value::as_str)
        .expect("selected agent_id");
    assert_eq!(
        console_json["agent_sidebar"]["live_snapshot"]["agents"][0]["member_id"],
        json!(selected_agent_id)
    );

    // Sending to a known agent should succeed via send_message.
    let session_id = meerkat_meta::send_message_on_mob(
        &fixture.runtime.mob_handle(),
        selected_agent_id,
        "cross-panel hello".to_string(),
    )
    .await
    .expect("send_message to known agent should succeed");
    SessionId::parse(&session_id).expect("send_message should return a valid session_id");

    // Sending to an unknown agent should fail.
    let unknown_result = meerkat_meta::send_message_on_mob(
        &fixture.runtime.mob_handle(),
        "unknown-member-id",
        "should fail".to_string(),
    )
    .await;
    assert!(
        unknown_result.is_err(),
        "send_message to unknown agent should fail"
    );

    let shutdown = fixture.runtime.shutdown().await;
    assert_mob_stop_allows_boundary_cancel(shutdown.mob_stop);
}

#[tokio::test]
#[ignore]
async fn phase_h1_multi_instance_profile_sidebar_enumerates_individual_agents() {
    let fixture = build_runtime_fixture().await;

    // Spawn two agents from the same profile with colon-namespaced IDs,
    // display_name labels, and one non-addressable singleton.
    let mut identity_luka_labels = std::collections::BTreeMap::new();
    identity_luka_labels.insert("display_name".to_string(), "Luka".to_string());
    identity_luka_labels.insert("group".to_string(), "Identity".to_string());

    let mut identity_parent_labels = std::collections::BTreeMap::new();
    identity_parent_labels.insert("display_name".to_string(), "Parent".to_string());
    identity_parent_labels.insert("group".to_string(), "Identity".to_string());

    let mut gate_labels = std::collections::BTreeMap::new();
    gate_labels.insert("addressable".to_string(), "false".to_string());
    gate_labels.insert("singleton".to_string(), "true".to_string());
    gate_labels.insert("group".to_string(), "Internal".to_string());

    let identity_luka = SpawnMemberSpec::new(
        meerkat_mob::ProfileName::from("lead"),
        MobMemberId::from("identity:luka"),
    )
    .with_labels(identity_luka_labels);
    let identity_parent = SpawnMemberSpec::new(
        meerkat_mob::ProfileName::from("lead"),
        MobMemberId::from("identity:parent"),
    )
    .with_labels(identity_parent_labels);
    let gate = SpawnMemberSpec::new(
        meerkat_mob::ProfileName::from("lead"),
        MobMemberId::from("gate:main"),
    )
    .with_labels(gate_labels);

    fixture
        .runtime
        .spawn(identity_luka)
        .await
        .expect("spawn identity:luka");
    fixture
        .runtime
        .spawn(identity_parent)
        .await
        .expect("spawn identity:parent");
    fixture.runtime.spawn(gate).await.expect("spawn gate:main");

    let app = fixture
        .runtime
        .build_reference_app_router(decision_state(false));
    let console_json = get_console_experience(&app).await;

    let agents = console_json["agent_sidebar"]["live_snapshot"]["agents"]
        .as_array()
        .expect("agents array");

    // All 3 individual agents should appear (not collapsed by profile).
    assert_eq!(agents.len(), 3, "expected 3 agents, got {}", agents.len());

    // Verify gate:main — non-addressable singleton.
    let gate_agent = agents
        .iter()
        .find(|a| a["agent_id"] == "gate:main")
        .expect("gate:main should be in sidebar");
    assert_eq!(gate_agent["label"], json!("gate:main")); // no display_name
    assert_eq!(gate_agent["addressable"], json!(false));
    assert_eq!(gate_agent["group"], json!("Internal"));
    assert_eq!(gate_agent["affordances"]["can_send_message"], json!(false));
    assert_eq!(gate_agent["affordances"]["can_retire"], json!(false));
    assert_eq!(gate_agent["affordances"]["can_respawn"], json!(true));

    // Verify identity:luka — display_name label renders as label.
    let luka_agent = agents
        .iter()
        .find(|a| a["agent_id"] == "identity:luka")
        .expect("identity:luka should be in sidebar");
    assert_eq!(luka_agent["label"], json!("Luka"));
    assert_eq!(luka_agent["group"], json!("Identity"));
    assert_eq!(luka_agent["addressable"], json!(true));
    assert_eq!(luka_agent["affordances"]["can_retire"], json!(true));

    // Verify identity:parent — second instance of same profile.
    let parent_agent = agents
        .iter()
        .find(|a| a["agent_id"] == "identity:parent")
        .expect("identity:parent should be in sidebar");
    assert_eq!(parent_agent["label"], json!("Parent"));
    assert_eq!(parent_agent["profile"], json!("lead"));

    // Verify profile_capabilities aggregation.
    let profile_caps = &console_json["runtime_capabilities"]["profile_capabilities"];
    assert_eq!(profile_caps["lead"]["instance_count"], json!(3));

    let shutdown = fixture.runtime.shutdown().await;
    assert_mob_stop_allows_boundary_cancel(shutdown.mob_stop);
}

// ──────────────────────────────────────────────────────────────────────────
// Sender-side comms projection: a member that SENDS peer communications must
// see those outgoing items in its OWN conversation timeline, and the
// recipient must see the arrivals. Regression for the console defect where
// the sender's conversation view showed only the text reply while the
// recipient rendered both communications.
// ──────────────────────────────────────────────────────────────────────────

const SENDER_MEMBER: &str = "console-sender";
const RECEIVER_MEMBER: &str = "console-receiver";
const OUTGOING_MESSAGE_BODY: &str = "Direct peer message: lights schedule updated.";
const OUTGOING_REQUEST_BODY: &str = "Structured request: report current device status.";
const SENDER_REPLY_TEXT: &str = "Reply to operator: home domain updated.";

/// Scripted LLM shared by both members. The sender's first real turn emits a
/// plain peer message AND a structured request to the receiver via the
/// agent-facing comms tools, then closes with a text reply; the receiver
/// acknowledges everything with text. The receiver's peer id is resolved
/// after spawn (the shared client is constructed before any member exists),
/// so it arrives through a set-once slot that is filled strictly before the
/// sender's scripted turn is triggered.
struct CommsScriptClient {
    receiver_peer_id: Arc<std::sync::OnceLock<String>>,
    sender_turn_calls: AtomicUsize,
}

/// meerkat 0.8.22 rejects a turn whose stream carried no normalized provider
/// accounting, so the terminal `Done` never travels alone. Taking the request
/// is what makes that possible here: the accounting identity is the request's
/// model, not a literal this fixture could restate.
fn text_only_turn(
    request: &LlmRequest,
    provider: Provider,
    text: &str,
) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'static>> {
    let [usage, done] = llm_usage::usage_then_done(request, provider, StopReason::EndTurn);
    Box::pin(stream::iter(vec![
        Ok(LlmEvent::TextDelta {
            delta: text.to_string(),
            meta: None,
        }),
        Ok(usage),
        Ok(done),
    ]))
}

impl LlmClient for CommsScriptClient {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        // meerkat 0.8.22 rejects a turn whose stream carried no normalized
        // provider accounting, so the terminal `Done` never travels alone on
        // ANY branch below - the silent spawn turn included.
        let provider = LlmClient::provider(self);
        if matches!(
            request.messages.last(),
            Some(Message::User(user)) if user.text_content().contains("You have been spawned as")
        ) {
            let [usage, done] = llm_usage::usage_then_done(request, provider, StopReason::EndTurn);
            return Box::pin(stream::iter(vec![Ok(usage), Ok(done)]));
        }
        let transcript = serde_json::to_string(&request.messages).unwrap_or_default();
        if !transcript.contains(&format!("You are {SENDER_MEMBER}")) {
            return text_only_turn(request, provider, "Acknowledged.");
        }
        let call = self.sender_turn_calls.fetch_add(1, Ordering::SeqCst);
        if call == 0 {
            let peer_id = self
                .receiver_peer_id
                .get()
                .cloned()
                .unwrap_or_else(|| "unresolved-peer".to_string());
            let [usage, done] = llm_usage::usage_then_done(request, provider, StopReason::ToolUse);
            Box::pin(stream::iter(vec![
                Ok(LlmEvent::ToolCallComplete {
                    id: "outgoing-peer-message".to_string(),
                    name: "send_message".to_string(),
                    args: json!({
                        "peer_id": peer_id,
                        "body": OUTGOING_MESSAGE_BODY,
                        "handling_mode": "queue",
                    }),
                    meta: None,
                }),
                Ok(LlmEvent::ToolCallComplete {
                    id: "outgoing-peer-request".to_string(),
                    name: "send_request".to_string(),
                    // The typed request vocabulary is closed
                    // (supervisor.bridge / checksum_token); a structured
                    // status request rides the checksum_token contract.
                    args: json!({
                        "peer_id": peer_id,
                        "intent": "checksum_token",
                        "params": { "subject": "device_status_report" },
                        "blocks": [{ "type": "text", "text": OUTGOING_REQUEST_BODY }],
                        "handling_mode": "queue",
                    }),
                    meta: None,
                }),
                Ok(usage),
                Ok(done),
            ]))
        } else {
            text_only_turn(request, provider, SENDER_REPLY_TEXT)
        }
    }

    fn provider(&self) -> Provider {
        Provider::Other
    }

    fn health_check<'life0, 'async_trait>(
        &'life0 self,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), LlmError>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async { Ok(()) })
    }
}

async fn post_console_rpc(app: &Router, payload: &Value) -> Value {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/console/rpc")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(payload.to_string()))
                .expect("rpc request"),
        )
        .await
        .expect("rpc response");
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("rpc body");
    serde_json::from_slice(&body).expect("rpc json")
}

fn assert_no_reserved_member_identity(value: &Value, path: &str) {
    match value {
        Value::String(text) => assert!(
            !text.contains("mk--"),
            "{path} leaks the reserved roster namespace: {text}"
        ),
        Value::Array(values) => {
            for (index, value) in values.iter().enumerate() {
                assert_no_reserved_member_identity(value, &format!("{path}[{index}]"));
            }
        }
        Value::Object(values) => {
            for (key, value) in values {
                assert_no_reserved_member_identity(value, &format!("{path}.{key}"));
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

/// meta/member_status must accept the runtime alias our OWN surfaces hand out.
///
/// This is the console sibling of the meta/get_member defect fixed in 0.8.23:
/// that one decoded the comms marker but never reduced an
/// rt:{identity}:{generation} alias to the durable identity before encoding it
/// into a roster key, producing a well-formed key for an identity that does not
/// exist - "not found" about a healthy member.
///
/// The exposure here is an operator, not a program: status and identities
/// responses hand out the alias, and the obvious next move is to paste it into
/// member_status. That path had NO alias coverage at all, which is the condition
/// under which the get_member defect survived a full green suite. It also reaches
/// further than a gateway binary - any embedder merging meta_console_router
/// exposes this method, which a production embedder pointed out after measuring their own tree.
/// `meta/identity/routing_status` must be REACHABLE on both planes, and its
/// negative must be CLASSIFIABLE.
///
/// Two separate properties, and neither is provable by a serialization test:
///
///  1. Meta carries two independent dispatchers - the stdin JSON-RPC router
///     in `rpc.rs` and the HTTP console's own match in `http_console.rs`. A
///     method wired in one is silently absent from the other, which is how
///     Meta has shipped unreachable surfaces before. `-32601` here is the
///     failure this test exists to catch, so it is asserted directly rather
///     than via `error.is_null()`: this fixture's member has no live session,
///     so an ERROR is the expected answer and "no error" would be the wrong
///     property to demand.
///
///  2. That error must carry a machine-readable `reason`. A production embedder sweeps a fleet
///     of materialized identities, where "no session yet" is the EXPECTED
///     state at boot and "the machine does not hold a resolved session" is a
///     real defect. A bare -32000 collapses those into one unusable verdict.
#[tokio::test]
async fn routing_status_is_reachable_on_both_planes_and_its_failure_is_classifiable()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let fixture = build_runtime_fixture().await;
    let durable = "gate:main";
    let encoded = meerkat_meta::member_comms_id::mob_member_id(durable);

    let handle = fixture.runtime.mob_handle();
    let mut spec = SpawnMemberSpec::from_wire(
        "lead".to_string(),
        encoded.to_string(),
        Some("Routing-status fixture.".into()),
        None,
        None,
    );
    spec.runtime_mode = Some(MobRuntimeMode::TurnDriven);
    handle.spawn_spec(spec).await?;

    let app = fixture
        .runtime
        .build_reference_app_router(decision_state(false));
    let rpc = |id: &str, method: &str, params: Value| json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});

    let reasons = [
        "runtime_unsupported",
        "no_current_session",
        "member_lookup_failed",
        "session_not_held",
        "upstream_read_failed",
        "invalid_identity",
    ];
    let assert_typed_refusal = |plane: &str, response: &Value, expect_identity: &str| {
        assert_eq!(
            response["error"]["data"]["kind"],
            json!("routing_status_unavailable"),
            "{plane} plane refused without the typed discriminator, so a caller cannot tell \
             'not addressed yet' from a real defect: {response:#?}"
        );
        let reason = response["error"]["data"]["reason"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            reasons.contains(&reason.as_str()),
            "{plane} plane returned an unknown reason {reason:?}; the discriminator is a \
             closed set callers branch on: {response:#?}"
        );
        assert_eq!(
            response["error"]["data"]["identity"],
            json!(expect_identity),
            "{plane} plane must echo the identity it refused for: {response:#?}"
        );
        reason
    };

    let both_planes = async |request: &Value| -> Result<
        (Value, Value),
        Box<dyn std::error::Error + Send + Sync>,
    > {
        let console = post_console_rpc(&app, request).await;
        let stdin_raw = meerkat_meta::rpc::handle_unified_rpc_json(
            &fixture.runtime,
            &request.to_string(),
            Duration::from_secs(5),
            None,
            None,
        )
        .await;
        Ok((console, serde_json::from_str(&stdin_raw)?))
    };

    // ---- part A: a member the mob actually spawned ----
    let (console, stdin) = both_planes(&rpc(
        "routing",
        "meta/identity/routing_status",
        json!({ "identity": durable }),
    ))
    .await?;

    for (plane, response) in [("console", &console), ("stdin", &stdin)] {
        assert_ne!(
            response["error"]["code"],
            json!(-32601),
            "{plane} plane does not dispatch meta/identity/routing_status at all. The two \
             planes carry separate matches; wiring only one ships a method the other surface \
             cannot reach: {response:#?}"
        );
        // Deliberately tolerant about WHICH of the two happens: whether this
        // fixture's member has a hydrated session is not this test's subject,
        // and hard-asserting success would make it fail under load for a
        // reason unrelated to the property. What is NOT tolerated is an
        // untyped refusal.
        if response["error"].is_null() {
            assert_eq!(
                response["result"]["identity"],
                json!(durable),
                "{response:#?}"
            );
            assert!(
                response["result"]["session_id"]
                    .as_str()
                    .is_some_and(|id| !id.is_empty()),
                "{plane} plane answered without naming the session it read: {response:#?}"
            );
            assert!(
                response["result"]["baseline_model"].is_string(),
                "{plane} plane answered without a baseline model: {response:#?}"
            );
        } else {
            assert_typed_refusal(plane, response, durable);
        }
    }

    // ---- part B: an identity the mob has never heard of ----
    //
    // This half is why the test is worth running. Part A's member resolves a
    // real status, so on its own it exercises ONLY the success branch and a
    // mutation stripping the typed error payload passes unnoticed - which is
    // exactly what a mutation sweep caught here. An unknown identity cannot
    // resolve a session, so this drives the refusal path deterministically.
    let unknown = "gate:no-such-member";
    let (console_unknown, stdin_unknown) = both_planes(&rpc(
        "routing-unknown",
        "meta/identity/routing_status",
        json!({ "identity": unknown }),
    ))
    .await?;

    let mut refusals = Vec::new();
    for (plane, response) in [("console", &console_unknown), ("stdin", &stdin_unknown)] {
        assert_ne!(response["error"]["code"], json!(-32601), "{response:#?}");
        assert!(
            !response["error"].is_null(),
            "{plane} plane produced a routing status for an identity the mob never had. A \
             plausible status for a member nobody looked up is worse than a refusal: \
             {response:#?}"
        );
        refusals.push(assert_typed_refusal(plane, response, unknown));
    }

    // Pinned exactly, not just as set membership. Meta's roster answers
    // `member_status` for an unknown member with a WELL-FORMED "unknown"
    // status carrying no session rather than an error, so an identity that
    // does not exist is indistinguishable here from one that exists and has
    // never been addressed. Both report `no_current_session`. That is a
    // real limitation of the roster read, recorded here so it is a documented
    // property rather than a surprise to a caller sweeping a fleet: a typo'd
    // identity reports the same reason as a legitimately unaddressed one.
    for (plane, reason) in [("console", &refusals[0]), ("stdin", &refusals[1])] {
        assert_eq!(
            reason, "no_current_session",
            "{plane} plane: an identity with no session must report no_current_session"
        );
    }
    assert_eq!(
        refusals[0], refusals[1],
        "the planes disagree about why routing status is unavailable:\n  console: \
         {console_unknown:#?}\n  stdin: {stdin_unknown:#?}"
    );

    // ---- part C: the CURRENT runtime alias must reach the same member ----
    //
    // Since the stable-identity lowering the roster is keyed by the encoded
    // DURABLE identity, so handing a current `rt:{identity}:{generation}` alias
    // straight to `mob_member_id` builds a roster key nobody owns. That does not
    // fail loudly - `member_status` answers with a well-formed "unknown" status
    // carrying no session - so the surface reports `no_current_session` for a
    // healthy addressed member. A false absence is worse than an error here,
    // because a fleet sweep counts it as "not addressed yet" and moves on.
    //
    // Asserted against the DURABLE spelling's own answer rather than in
    // isolation: the property is that both spellings reach one member, which a
    // standalone assertion on the alias cannot express.
    let (console_alias, stdin_alias) = both_planes(&rpc(
        "routing-alias",
        "meta/identity/routing_status",
        json!({ "identity": format!("rt:{durable}:0") }),
    ))
    .await?;

    for (plane, aliased, direct) in [
        ("console", &console_alias, &console),
        ("stdin", &stdin_alias, &stdin),
    ] {
        let direct_reason = direct["error"]["data"]["reason"].as_str();
        let alias_reason = aliased["error"]["data"]["reason"].as_str();
        assert_eq!(
            alias_reason, direct_reason,
            "{plane} plane answers the runtime alias differently from the durable identity \
             it denotes. If the durable spelling resolves and the alias reports \
             no_current_session, the lookup was not normalized through \
             roster_member_id_for_supplied_id and a healthy member reads as unaddressed:\n  \
             alias:  {aliased:#?}\n  direct: {direct:#?}"
        );
        if aliased["error"].is_null() {
            assert_eq!(
                aliased["result"]["session_id"], direct["result"]["session_id"],
                "{plane} plane resolved the alias to a DIFFERENT session than the durable \
                 identity: {aliased:#?}"
            );
            // The caller gets back the spelling it asked with; normalization is
            // a lookup detail and must not rewrite the answer.
            assert_eq!(
                aliased["result"]["identity"],
                json!(format!("rt:{durable}:0")),
                "{plane} plane must echo the supplied identity, not the normalized one: \
                 {aliased:#?}"
            );
        }
    }

    // ---- part D: malformed ingress is typed, and identical on both planes ----
    //
    // `invalid_identity` was documented as a member of the closed reason set
    // while being UNREACHABLE: each plane refused a missing or empty identity
    // inline with an untyped -32602 carrying no data, and the two planes did not
    // agree with each other. A reason a caller can never observe is not part of
    // a contract, so both cases are pinned here on both planes.
    for (case, params) in [("empty", json!({ "identity": "" })), ("missing", json!({}))] {
        let (console_bad, stdin_bad) =
            both_planes(&rpc("routing-bad", "meta/identity/routing_status", params)).await?;
        for (plane, response) in [("console", &console_bad), ("stdin", &stdin_bad)] {
            assert!(
                !response["error"].is_null(),
                "{plane} plane accepted a {case} identity: {response:#?}"
            );
            assert_eq!(
                response["error"]["code"],
                json!(-32602),
                "a malformed request is invalid-params, not a server error \
                 ({plane}, {case}): {response:#?}"
            );
            assert_eq!(
                response["error"]["data"]["kind"],
                json!("routing_status_unavailable"),
                "{plane} plane refused a {case} identity untyped, so a caller cannot branch \
                 on it the way it branches on every other refusal: {response:#?}"
            );
            assert_eq!(
                response["error"]["data"]["reason"],
                json!("invalid_identity"),
                "{plane}/{case} must surface the documented reason: {response:#?}"
            );
        }
        assert_eq!(
            console_bad["error"]["data"], stdin_bad["error"]["data"],
            "the planes disagree about a {case} identity:\n  console: {console_bad:#?}\n  \
             stdin: {stdin_bad:#?}"
        );
    }

    // Advertisement, on both planes. A dispatchable method nothing advertises
    // is discoverable only by guessing.
    let console_caps = post_console_rpc(&app, &rpc("caps", "meta/capabilities", json!({}))).await;
    let stdin_caps_raw = meerkat_meta::rpc::handle_unified_rpc_json(
        &fixture.runtime,
        &rpc("caps", "meta/capabilities", json!({})).to_string(),
        Duration::from_secs(5),
        None,
        None,
    )
    .await;
    let stdin_caps: Value = serde_json::from_str(&stdin_caps_raw)?;
    for (plane, caps) in [("console", &console_caps), ("stdin", &stdin_caps)] {
        let methods = caps["result"]["methods"]
            .as_array()
            .unwrap_or_else(|| panic!("{plane} capabilities must list methods: {caps:#?}"));
        assert!(
            methods
                .iter()
                .any(|method| method == "meta/identity/routing_status"),
            "{plane} plane dispatches routing_status but does not advertise it: {methods:#?}"
        );
    }

    Ok(())
}

#[tokio::test]
async fn member_status_accepts_the_runtime_alias_its_own_surfaces_hand_out()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let fixture = build_runtime_fixture().await;
    // A POST-LOWERING member: the roster is keyed by the DURABLE identity.
    let durable = "gate:main";
    let encoded = meerkat_meta::member_comms_id::mob_member_id(durable);

    let handle = fixture.runtime.mob_handle();
    let mut spec = SpawnMemberSpec::from_wire(
        "lead".to_string(),
        encoded.to_string(),
        Some("Alias-input fixture.".into()),
        None,
        None,
    );
    spec.runtime_mode = Some(MobRuntimeMode::TurnDriven);
    handle.spawn_spec(spec).await?;

    let app = fixture
        .runtime
        .build_reference_app_router(decision_state(false));
    let rpc = |id: &str, method: &str, params: Value| json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});

    // Control: the durable spelling resolves. If this fails the fixture is
    // wrong and the alias result below would mean nothing.
    let by_identity = post_console_rpc(
        &app,
        &rpc(
            "by-identity",
            "meta/member_status",
            json!({ "member_id": durable }),
        ),
    )
    .await;
    assert!(
        by_identity["error"].is_null(),
        "control failed - the durable identity must resolve, so the alias case below is \
         interpretable: {by_identity:#?}"
    );

    // The actual property: the alias our own surfaces emit must reach the same
    // member. Generation is incarnation detail and must not decide the answer.
    let by_alias = post_console_rpc(
        &app,
        &rpc(
            "by-alias",
            "meta/member_status",
            json!({ "member_id": format!("rt:{durable}:0") }),
        ),
    )
    .await;
    // NOT `error.is_null()`. The failure mode here is not a refusal - it is a
    // WELL-FORMED status for a member that was never found: is_final true,
    // status "unknown", no session. An error would at least tell the operator
    // something was wrong; this reports a healthy live member as finished. So
    // the assertion has to compare the ANSWER, not the absence of an error.
    assert_eq!(
        by_alias["result"]["current_session_id"], by_identity["result"]["current_session_id"],
        "the alias must resolve to the SAME member's session. A null session here means the \
         alias was encoded into a roster key nothing owns, and the operator is handed a \
         plausible status for a member that was never looked up: {by_alias:#?}"
    );
    assert_eq!(
        by_alias["result"]["is_final"], by_identity["result"]["is_final"],
        "a live member must not report is_final through one spelling and not the other: \
         {by_alias:#?}"
    );

    Ok(())
}

#[tokio::test]
async fn member_declarations_round_trip_public_aliases_over_the_http_console()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Identity adoption needs a runtime with a store.
    let mut fixture = build_store_backed_runtime_fixture().await;
    let alias = "gate:main:0";
    let encoded = meerkat_meta::member_comms_id::mob_member_id(alias);
    assert!(
        encoded.as_str().starts_with("mk--"),
        "the fixture must use a genuinely encoded roster identity: {encoded}"
    );

    let handle = fixture.runtime.mob_handle();
    let mut spec = SpawnMemberSpec::from_wire(
        "lead".to_string(),
        encoded.to_string(),
        Some("Identity-first member declaration fixture.".into()),
        None,
        None,
    );
    spec.runtime_mode = Some(MobRuntimeMode::TurnDriven);
    handle.spawn_spec(spec).await?;

    let mob_id = handle.mob_id().to_string();
    let session_id = handle
        .resolve_bridge_session_id(&encoded)
        .await
        .ok_or("spawned member must have a bridge session")?;
    let app = fixture
        .runtime
        .build_reference_app_router(decision_state(false));

    let rpc = |id: &str, method: &str, params: Value| {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        })
    };

    let capabilities = post_console_rpc(&app, &rpc("caps", "meta/capabilities", json!({}))).await;
    let methods = capabilities["result"]["methods"]
        .as_array()
        .ok_or("capability methods must be an array")?;
    for method in [
        "mob/adopt_member_identity_declaration",
        "mob/apply_member_tool_declaration",
        "mob/member_tool_declaration",
    ] {
        assert!(
            methods.iter().any(|candidate| candidate == method),
            "the full-runtime HTTP console must advertise {method}: {methods:#?}"
        );
    }

    let adoption = post_console_rpc(
        &app,
        &rpc(
            "adopt",
            "mob/adopt_member_identity_declaration",
            json!({
                "mob_id": mob_id,
                "agent_identity": alias,
                "request_id": "adopt-http-public-alias-1",
                "precondition": "expected_absent",
                "declaration_scope": "http-public-alias-test",
                "declaration_revision": 1,
                "session": {
                    "session_id": session_id.to_string(),
                    "lineage_id": format!("session:{session_id}"),
                    "lineage_generation": 0,
                    "authority_policy": "require_existing"
                },
                "member": {
                    "profile_name": "lead",
                    "runtime_mode": "turn_driven",
                    // NO system_prompt_override: meerkat 0.8.28 refuses an adoption that
                    // restates an existing session's durable prompt, because the
                    // transcript is authoritative. Omitting it is now the contract.
                    "execution": { "execution": "controlling_session" }
                },
                "owned_wiring": [],
                "convergence": { "kind": "drain", "max_wait_ms": 5000 }
            }),
        ),
    )
    .await;
    assert_eq!(adoption["error"], Value::Null, "{adoption:#?}");
    assert_eq!(adoption["result"]["adoption"]["outcome"], json!("adopted"));
    assert_eq!(adoption["result"]["adoption"]["desired_revision"], json!(1));
    assert_eq!(
        adoption["result"]["convergence"]["agent_identity"],
        json!(alias)
    );
    assert_no_reserved_member_identity(&adoption, "adoption");

    let read_revision_one = post_console_rpc(
        &app,
        &rpc(
            "read-1",
            "mob/member_tool_declaration",
            json!({ "mob_id": mob_id, "agent_identity": alias }),
        ),
    )
    .await;
    assert_eq!(
        read_revision_one["error"],
        Value::Null,
        "{read_revision_one:#?}"
    );
    assert_eq!(read_revision_one["result"]["agent_identity"], json!(alias));
    assert_eq!(
        read_revision_one["result"]["desired_intent_revision"],
        json!(1)
    );
    assert!(
        read_revision_one["result"]["declaration"].is_object(),
        "adoption must create a readable declaration: {read_revision_one:#?}"
    );
    assert_no_reserved_member_identity(&read_revision_one, "read_revision_one");

    let declaration = json!({
        "category_overrides": {
            "builtins": "inherit",
            "shell": "enable",
            "comms": "inherit",
            "mob": "inherit",
            "memory": "inherit",
            "schedule": "inherit",
            "workgraph": "inherit",
            "image_generation": "inherit",
            "web_search": "inherit"
        },
        "callback_tools": { "kind": "set", "tools": [] },
        "execution": { "kind": "unrestricted" },
        "application_policy": { "kind": "unmanaged" }
    });
    let apply = post_console_rpc(
        &app,
        &rpc(
            "apply",
            "mob/apply_member_tool_declaration",
            json!({
                "mob_id": mob_id,
                "agent_identity": alias,
                "request_id": "apply-http-public-alias-1",
                "expected_intent_revision": 1,
                "declaration": declaration,
                "convergence": { "kind": "drain", "max_wait_ms": 5000 }
            }),
        ),
    )
    .await;
    assert_eq!(apply["error"], Value::Null, "{apply:#?}");
    assert_eq!(apply["result"]["commit"]["outcome"], json!("committed"));
    assert_eq!(apply["result"]["commit"]["desired_revision"], json!(2));
    assert_eq!(
        apply["result"]["convergence"]["agent_identity"],
        json!(alias)
    );
    assert_no_reserved_member_identity(&apply, "apply");

    let read_revision_two_request = rpc(
        "read-2",
        "mob/member_tool_declaration",
        json!({ "mob_id": mob_id, "agent_identity": alias }),
    );
    let read_revision_two = post_console_rpc(&app, &read_revision_two_request).await;
    assert_eq!(
        read_revision_two["error"],
        Value::Null,
        "{read_revision_two:#?}"
    );
    assert_eq!(read_revision_two["result"]["agent_identity"], json!(alias));
    assert_eq!(
        read_revision_two["result"]["desired_intent_revision"],
        json!(2)
    );
    assert_eq!(read_revision_two["result"]["declaration"], declaration);
    assert_no_reserved_member_identity(&read_revision_two, "read_revision_two");

    let stdin_raw = meerkat_meta::rpc::handle_unified_rpc_json(
        &fixture.runtime,
        &read_revision_two_request.to_string(),
        Duration::from_secs(5),
        None,
        None,
    )
    .await;
    let stdin: Value = serde_json::from_str(&stdin_raw)?;
    assert_eq!(
        stdin["result"], read_revision_two["result"],
        "stdin and HTTP must project the shared handler identically"
    );

    let reserved = post_console_rpc(
        &app,
        &rpc(
            "reserved",
            "mob/member_tool_declaration",
            json!({ "mob_id": mob_id, "agent_identity": encoded.as_str() }),
        ),
    )
    .await;
    assert_eq!(reserved["error"]["code"], json!(-32602), "{reserved:#?}");

    let mut read_only_decisions = decision_state(false);
    read_only_decisions.console.read_only = true;
    let read_only_app = fixture
        .runtime
        .build_reference_app_router(read_only_decisions);
    let read_only_capabilities = post_console_rpc(
        &read_only_app,
        &rpc("read-only-caps", "meta/capabilities", json!({})),
    )
    .await;
    let read_only_methods = read_only_capabilities["result"]["methods"]
        .as_array()
        .ok_or("read-only capability methods must be an array")?;
    assert!(
        read_only_methods
            .iter()
            .any(|method| method == "mob/member_tool_declaration")
    );
    for method in [
        "mob/adopt_member_identity_declaration",
        "mob/apply_member_tool_declaration",
    ] {
        assert!(
            read_only_methods
                .iter()
                .all(|candidate| candidate != method),
            "read-only capabilities must omit {method}: {read_only_methods:#?}"
        );
    }
    let read_only_read = post_console_rpc(&read_only_app, &read_revision_two_request).await;
    assert_eq!(
        read_only_read["result"], read_revision_two["result"],
        "read-only mode must retain declaration reads"
    );
    let read_only_apply = post_console_rpc(
        &read_only_app,
        &rpc(
            "read-only-apply",
            "mob/apply_member_tool_declaration",
            json!({
                "mob_id": mob_id,
                "agent_identity": alias,
                "request_id": "apply-http-public-alias-read-only",
                "expected_intent_revision": 2,
                "declaration": declaration,
                "convergence": { "kind": "drain", "max_wait_ms": 5000 }
            }),
        ),
    )
    .await;
    assert_eq!(read_only_apply["error"]["code"], json!(-32010));
    assert_eq!(read_only_apply["error"]["data"]["kind"], json!("read_only"));

    let controller = AccessController::new(AccessControlConfig {
        enabled: true,
        admins: vec!["root@example.test".to_string()],
        rules: vec![AccessRule {
            id: "anonymous-views-declaration-member".to_string(),
            actions: vec!["agent.view".to_string()],
            agents: vec![alias.to_string()],
            ..AccessRule::default()
        }],
        ..AccessControlConfig::default()
    })?;
    fixture.runtime.set_access_controller(controller);
    let scoped_app = fixture
        .runtime
        .build_reference_app_router(decision_state(false));
    let allowed_read = post_console_rpc(&scoped_app, &read_revision_two_request).await;
    assert_eq!(allowed_read["result"], read_revision_two["result"]);
    let denied_other_read = post_console_rpc(
        &scoped_app,
        &rpc(
            "denied-other-read",
            "mob/member_tool_declaration",
            json!({ "mob_id": mob_id, "agent_identity": "rt:other:0" }),
        ),
    )
    .await;
    assert_eq!(denied_other_read["error"]["code"], json!(-32030));
    let denied_apply = post_console_rpc(
        &scoped_app,
        &rpc(
            "denied-apply",
            "mob/apply_member_tool_declaration",
            json!({
                "mob_id": mob_id,
                "agent_identity": alias,
                "request_id": "apply-http-public-alias-abac",
                "expected_intent_revision": 2,
                "declaration": declaration,
                "convergence": { "kind": "drain", "max_wait_ms": 5000 }
            }),
        ),
    )
    .await;
    assert_eq!(denied_apply["error"]["code"], json!(-32030));
    assert_eq!(
        denied_apply["error"]["data"]["kind"],
        json!("access_denied")
    );

    let shutdown = fixture.runtime.shutdown().await;
    assert_mob_stop_allows_boundary_cancel(shutdown.mob_stop);
    Ok(())
}

/// Query an identity's conversation timeline the way the console front-end
/// loads a chat pane (recent window with an explicit limit).
async fn query_conversation_frames(app: &Router, identity: &str) -> Vec<Value> {
    let query_json = post_console_rpc(
        app,
        &json!({
            "jsonrpc": "2.0",
            "id": "query-conversation",
            "method": "meta/console/query_timeline",
            "params": { "identity": identity, "mode": "recent", "limit": 400 }
        }),
    )
    .await;
    query_json["result"]["frames"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// Poll an identity's conversation timeline until `predicate` matches, or
/// panic with a frame dump after ~30s.
async fn wait_for_timeline(
    app: &Router,
    identity: &str,
    label: &str,
    predicate: impl Fn(&[Value]) -> bool,
) -> Vec<Value> {
    let mut frames = Vec::new();
    for _ in 0..300 {
        frames = query_conversation_frames(app, identity).await;
        if predicate(&frames) {
            return frames;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!(
        "timed out waiting for {label} in {identity} timeline; frames:\n{}",
        serde_json::to_string_pretty(&frames).unwrap_or_default()
    );
}

fn frames_contain_text(frames: &[Value], needle: &str) -> bool {
    frames
        .iter()
        .any(|frame| frame.to_string().contains(needle))
}

/// True when a frame is renderable by the console conversation view as an
/// OUTGOING comms item for the given send tool: a tool-call frame carrying
/// the tool name + args (the view derives peer/body/intent from the args), or
/// a typed comms system notice with direction "outgoing".
fn is_outgoing_comms_frame(frame: &Value, tool_name: &str, body: &str) -> bool {
    let kind = frame["kind"].as_str().unwrap_or_default();
    let payload = &frame["payload"];
    match kind {
        "tool_call_requested" | "tool_call" | "tool_execution_started" => {
            payload["name"] == json!(tool_name) && payload["args"].to_string().contains(body)
        }
        "system_notice" => {
            let message = &payload["message"];
            message["kind"] == json!("comms")
                && message["blocks"].as_array().is_some_and(|blocks| {
                    blocks.iter().any(|block| {
                        block["type"] == json!("comms")
                            && block["direction"] == json!("outgoing")
                            && block.to_string().contains(body)
                    })
                })
        }
        _ => false,
    }
}

/// Two-member comms scenario: the sender emits a plain peer message AND a
/// structured request to the receiver, then replies with text. Asserts the
/// receiver's conversation shows both arrivals and the sender's OWN
/// conversation shows both outgoing items.
///
/// `drain_live_events` selects the projection source under test:
/// - `true`: the embedding host drains mob agent events (reference gateway
///   mode) — the conversation view is fed by live console events.
/// - `false`: no live projection ever runs, so the conversation view is
///   rebuilt purely from session-history backfill. This is the console-log
///   loss / restart mode where the sender-side omission was observed: the
///   recipient's arrivals survive (incoming typed comms notices live in its
///   transcript) while the sender's outgoing comms exist only as assistant
///   tool calls the history projection used to drop.
async fn run_outgoing_comms_console_scenario(drain_live_events: bool) {
    let definition = MobDefinition::from_toml(&format!(
        r#"
[mob]
id = "comms-console-mob-{}"

[profiles.lead]
model = "gpt-5.5"
external_addressable = true

[profiles.lead.tools]
comms = true

[wiring]
auto_wire_orchestrator = false

[[wiring.role_wiring]]
a = "lead"
b = "lead"
"#,
        NEXT_TEST_MOB_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ))
    .expect("parse comms console mob definition");

    let temp_dir = tempfile::tempdir().expect("temp dir");
    let session_path = temp_dir.path().join("sessions");
    std::fs::create_dir_all(&session_path).expect("session path");
    let factory = AgentFactory::new(&session_path).comms(true);
    let session_service = Arc::new(build_ephemeral_service(factory, Config::default(), 16));

    let peer_id_slot = Arc::new(std::sync::OnceLock::new());
    let script_client = Arc::new(CommsScriptClient {
        receiver_peer_id: peer_id_slot.clone(),
        sender_turn_calls: AtomicUsize::new(0),
    });
    let mob_spec = MobBootstrapSpec::new(definition, MobStorage::in_memory(), session_service)
        .with_options(MobBootstrapOptions {
            allow_ephemeral_sessions: true,
            notify_orchestrator_on_resume: true,
            default_llm_client: Some(script_client),
        });
    let runtime = UnifiedRuntime::bootstrap(
        mob_spec,
        MetaConfig {
            modules: vec![],
            discovery: DiscoverySpec {
                namespace: "comms-console".to_string(),
                modules: vec![],
            },
            pre_spawn: vec![],
        },
        Duration::from_secs(2),
    )
    .await
    .expect("bootstrap comms console runtime");
    let runtime = Arc::new(runtime);
    let event_drain = drain_live_events.then(|| runtime.clone().spawn_event_drain_task());

    runtime
        .spawn(console_member_spec(RECEIVER_MEMBER))
        .await
        .expect("spawn receiver member");
    let (receiver_peer_id, _, _) = runtime
        .local_member_peer_info(RECEIVER_MEMBER)
        .await
        .expect("receiver peer info");
    peer_id_slot
        .set(receiver_peer_id)
        .expect("peer id slot set once");
    runtime
        .spawn(console_member_spec(SENDER_MEMBER))
        .await
        .expect("spawn sender member");

    let app = runtime.build_reference_app_router(decision_state(false));

    // Trigger the sender's scripted turn through the console send pipeline.
    let send_json = post_console_rpc(
        &app,
        &json!({
            "jsonrpc": "2.0",
            "id": "send-comms-1",
            "method": "meta/console/send",
            "params": {
                "identity": SENDER_MEMBER,
                "origin": "test",
                "idempotency_key": "sender-outgoing-comms-1",
                "content": "Update the home domain and report back."
            }
        }),
    )
    .await;
    assert!(
        send_json.get("error").is_none() || send_json["error"].is_null(),
        "console send failed: {send_json}"
    );

    // Sender finishes a turn with the text reply.
    wait_for_timeline(&app, SENDER_MEMBER, "sender text reply", |frames| {
        frames_contain_text(frames, SENDER_REPLY_TEXT)
    })
    .await;

    // The recipient's conversation shows BOTH communications arriving.
    wait_for_timeline(&app, RECEIVER_MEMBER, "peer message arrival", |frames| {
        frames_contain_text(frames, OUTGOING_MESSAGE_BODY)
    })
    .await;
    wait_for_timeline(&app, RECEIVER_MEMBER, "peer request arrival", |frames| {
        frames_contain_text(frames, OUTGOING_REQUEST_BODY)
    })
    .await;

    // THE REGRESSION: the sender's OWN conversation view must contain both
    // outgoing communications as renderable items (not just the text reply).
    let sender_frames = wait_for_timeline(
        &app,
        SENDER_MEMBER,
        "outgoing peer message item",
        |frames| {
            frames
                .iter()
                .any(|frame| is_outgoing_comms_frame(frame, "send_message", OUTGOING_MESSAGE_BODY))
        },
    )
    .await;
    assert!(
        sender_frames.iter().any(|frame| is_outgoing_comms_frame(
            frame,
            "send_request",
            OUTGOING_REQUEST_BODY
        )),
        "sender timeline must render the outgoing structured request; frames:\n{}",
        serde_json::to_string_pretty(&sender_frames).unwrap_or_default()
    );

    if let Some(event_drain) = event_drain {
        event_drain.abort();
    }
    let shutdown = runtime.shutdown().await;
    assert_mob_stop_allows_boundary_cancel(shutdown.mob_stop);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sender_conversation_view_renders_outgoing_peer_comms_live() {
    run_outgoing_comms_console_scenario(true).await;
}

/// The observed defect: with the conversation rebuilt from session history
/// (console log lost/reset, live projection never attached), the sender's
/// view kept only the text reply while the recipient rendered both arrivals.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sender_conversation_view_renders_outgoing_peer_comms_after_history_rebuild() {
    run_outgoing_comms_console_scenario(false).await;
}

const TOOL_STEP_MEMBER: &str = "console-tool-step";

/// Scripted LLM whose every turn is a tool-only step (one `peers` call, no
/// text) followed by a text reply that names the prompt it answers. This is
/// the shape the demo `TestClient` cannot produce, and the shape that minted
/// the empty `interaction_complete` frames in the R6 repro: a tool-only
/// assistant step has no live text twin, so its history projection survived
/// dedupe as an empty completion under the live turn's interaction id.
struct ToolThenTextClient {
    tool_calls: AtomicUsize,
}

fn last_user_prompt(request: &LlmRequest) -> String {
    request
        .messages
        .iter()
        .rev()
        .find_map(|message| match message {
            Message::User(user) => Some(user.text_content()),
            _ => None,
        })
        .unwrap_or_default()
}

impl LlmClient for ToolThenTextClient {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        let provider = LlmClient::provider(self);
        let prompt = last_user_prompt(request);
        if !prompt.starts_with("turn-") {
            // Any host-minted turn gets a plain text answer (an empty
            // completion is a retryable provider error to meerkat and would
            // stall the member's queue), so the scripted tool-then-text turns
            // are exactly the two console sends.
            return text_only_turn(request, provider, "Acknowledged.");
        }
        if matches!(request.messages.last(), Some(Message::ToolResults { .. })) {
            return text_only_turn(request, provider, &format!("Reply to {prompt}"));
        }
        let call = self.tool_calls.fetch_add(1, Ordering::SeqCst);
        let [usage, done] = llm_usage::usage_then_done(request, provider, StopReason::ToolUse);
        Box::pin(stream::iter(vec![
            Ok(LlmEvent::ToolCallComplete {
                id: format!("peers-call-{call}"),
                name: "peers".to_string(),
                args: json!({}),
                meta: None,
            }),
            Ok(usage),
            Ok(done),
        ]))
    }

    fn provider(&self) -> Provider {
        Provider::Other
    }

    fn health_check<'life0, 'async_trait>(
        &'life0 self,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), LlmError>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async { Ok(()) })
    }
}

/// Sequence number of a frame's `console:N` cursor; the timeline's order.
fn frame_cursor_seq(frame: &Value) -> u64 {
    frame["cursor"]
        .as_str()
        .and_then(|cursor| cursor.strip_prefix("console:"))
        .and_then(|seq| seq.parse().ok())
        .unwrap_or_else(|| panic!("frame without a console cursor: {frame:#?}"))
}

fn frame_result_text(frame: &Value) -> &str {
    frame["payload"]["result"].as_str().unwrap_or_default()
}

/// One turn_driven member whose every turn is a tool-only step and then a
/// text reply, driven by two console sends with distinct idempotency keys.
/// Returns the member's conversation timeline after both turns completed and
/// the session-history backfill had a chance to run behind the last one,
/// together with the interaction id each send was accepted under.
///
/// `drain_live_events` selects the projection source under test exactly as
/// in `run_outgoing_comms_console_scenario`: `true` is the gateway mode (live
/// console events, history frames dedupe against their live twins); `false`
/// rebuilds the conversation purely from session-history backfill.
async fn run_tool_step_console_scenario(drain_live_events: bool) -> (Vec<Value>, Vec<String>) {
    let definition = MobDefinition::from_toml(&format!(
        r#"
[mob]
id = "tool-step-console-mob-{}"

[profiles.lead]
model = "gpt-5.5"
external_addressable = true

[profiles.lead.tools]
comms = true
"#,
        NEXT_TEST_MOB_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ))
    .expect("parse tool-step console mob definition");

    let temp_dir = tempfile::tempdir().expect("temp dir");
    let session_path = temp_dir.path().join("sessions");
    std::fs::create_dir_all(&session_path).expect("session path");
    let factory = AgentFactory::new(&session_path).comms(true);
    let session_service = Arc::new(build_ephemeral_service(factory, Config::default(), 16));
    let script_client = Arc::new(ToolThenTextClient {
        tool_calls: AtomicUsize::new(0),
    });
    let mob_spec = MobBootstrapSpec::new(definition, MobStorage::in_memory(), session_service)
        .with_options(MobBootstrapOptions {
            allow_ephemeral_sessions: true,
            notify_orchestrator_on_resume: true,
            default_llm_client: Some(script_client),
        });
    let runtime = UnifiedRuntime::bootstrap(
        mob_spec,
        MetaConfig {
            modules: vec![],
            discovery: DiscoverySpec {
                namespace: "tool-step-console".to_string(),
                modules: vec![],
            },
            pre_spawn: vec![],
        },
        Duration::from_secs(2),
    )
    .await
    .expect("bootstrap tool-step console runtime");
    let runtime = Arc::new(runtime);
    let event_drain = drain_live_events.then(|| runtime.clone().spawn_event_drain_task());

    // No initial message: a spawn-time turn would race the first send for
    // the console's pending interaction and add a third terminal, and the
    // two console sends are the whole subject here.
    let mut spec = SpawnMemberSpec::from_wire(
        "lead".to_string(),
        MobMemberId::from(TOOL_STEP_MEMBER).to_string(),
        None,
        None,
        None,
    );
    spec.runtime_mode = Some(MobRuntimeMode::TurnDriven);
    runtime.spawn(spec).await.expect("spawn turn_driven member");

    let app = runtime.build_reference_app_router(decision_state(false));
    let mut interaction_ids = Vec::new();
    for turn in ["turn-1", "turn-2"] {
        let send_json = post_console_rpc(
            &app,
            &json!({
                "jsonrpc": "2.0",
                "id": format!("send-{turn}"),
                "method": "meta/console/send",
                "params": {
                    "identity": TOOL_STEP_MEMBER,
                    "origin": "test",
                    "idempotency_key": format!("tool-step-{turn}"),
                    "content": turn
                }
            }),
        )
        .await;
        assert!(
            send_json.get("error").is_none() || send_json["error"].is_null(),
            "console send failed: {send_json}"
        );
        interaction_ids.push(
            send_json["result"]["interaction_id"]
                .as_str()
                .unwrap_or_else(|| panic!("an accepted send names its interaction: {send_json}"))
                .to_string(),
        );
        let reply = format!("Reply to {turn}");
        wait_for_timeline(&app, TOOL_STEP_MEMBER, &reply, |frames| {
            frames_contain_text(frames, &reply)
        })
        .await;
    }
    // Every session-bearing console event and every identity query forces a
    // history refresh; keep querying so the refresh behind the last turn has
    // landed before the timeline is read.
    let mut frames = Vec::new();
    for _ in 0..10 {
        frames = query_conversation_frames(&app, TOOL_STEP_MEMBER).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    if let Some(event_drain) = event_drain {
        event_drain.abort();
    }
    let shutdown = runtime.shutdown().await;
    assert_mob_stop_allows_boundary_cancel(shutdown.mob_stop);
    (frames, interaction_ids)
}

/// R6: two console sends to one turn_driven member each close with their
/// own terminal. Each accepted interaction has exactly one live
/// `interaction_complete`, carrying that turn's answer and following its own
/// `run_started`, and no text-less `interaction_complete` from
/// session-history backfill appears for the identity (before the fix each
/// tool-only step surfaced as one, stamped with the live turn's interaction
/// id).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_console_sends_to_a_turn_driven_member_each_close_with_their_own_terminal() {
    let (frames, interaction_ids) = run_tool_step_console_scenario(true).await;
    let pretty = || serde_json::to_string_pretty(&frames).unwrap_or_default();

    assert_eq!(interaction_ids.len(), 2);
    assert_ne!(
        interaction_ids[0], interaction_ids[1],
        "distinct idempotency keys reserve distinct interactions"
    );
    for (turn_idx, interaction_id) in interaction_ids.iter().enumerate() {
        let terminals: Vec<&Value> = frames
            .iter()
            .filter(|frame| {
                frame["kind"] == json!("interaction_complete")
                    && frame["source"]["kind"] == json!("console_event")
                    && frame["interaction_id"] == json!(interaction_id)
            })
            .collect();
        assert_eq!(
            terminals.len(),
            1,
            "exactly one live terminal for interaction {interaction_id}; frames:\n{}",
            pretty()
        );
        let terminal = terminals[0];
        assert_eq!(
            frame_result_text(terminal),
            format!("Reply to turn-{}", turn_idx + 1),
            "a live terminal carries its own turn's answer: {terminal:#?}"
        );
        let run_started = frames
            .iter()
            .find(|frame| {
                frame["kind"] == json!("run_started")
                    && frame["interaction_id"] == json!(interaction_id)
            })
            .unwrap_or_else(|| {
                panic!(
                    "interaction {interaction_id} has no run_started; frames:\n{}",
                    pretty()
                )
            });
        assert!(
            frame_cursor_seq(run_started) < frame_cursor_seq(terminal),
            "a terminal must follow its own run_started; frames:\n{}",
            pretty()
        );
    }

    // Positive observable first: counting EVERY interaction_complete for the
    // identity, from any source, pins exactly one terminal per send. Saved
    // text steps are text_complete, while only live run completion is a
    // terminal. The history assertions below prove backfill has landed.
    let all_terminals = frames
        .iter()
        .filter(|frame| frame["kind"] == json!("interaction_complete"))
        .count();
    assert_eq!(
        all_terminals,
        2,
        "exactly one interaction_complete per send from any source; frames:\n{}",
        pretty()
    );
    let history_replies: Vec<_> = frames
        .iter()
        .filter(|frame| {
            frame["kind"] == json!("text_complete")
                && frame["source"]["kind"] == json!("session_history")
        })
        .collect();
    assert_eq!(
        history_replies.len(),
        2,
        "both authored replies survive backfill: {}",
        pretty()
    );
    for (index, interaction_id) in interaction_ids.iter().enumerate() {
        let reply = history_replies
            .iter()
            .find(|frame| frame["interaction_id"] == json!(interaction_id))
            .expect("canonical reply retains its exact interaction");
        assert_eq!(
            frame_result_text(reply),
            format!("Reply to turn-{}", index + 1)
        );
        let terminal = frames
            .iter()
            .find(|frame| {
                frame["kind"] == json!("interaction_complete")
                    && frame["interaction_id"] == json!(interaction_id)
            })
            .unwrap();
        assert_eq!(reply["run_id"], terminal["run_id"]);
        assert_eq!(reply["session_id"], terminal["session_id"]);
        assert!(reply["source"]["source_cursor"].as_str().is_some());
    }
    assert_ne!(
        history_replies[0]["source"]["source_cursor"],
        history_replies[1]["source"]["source_cursor"]
    );
    // Diagnostic twin: name the offending frames when the count is wrong for
    // the reason this test exists.
    let empty_history_terminals: Vec<&Value> = frames
        .iter()
        .filter(|frame| {
            frame["kind"] == json!("interaction_complete")
                && frame["source"]["kind"] == json!("session_history")
                && frame_result_text(frame).is_empty()
        })
        .collect();
    assert!(
        empty_history_terminals.is_empty(),
        "a tool-only step must not surface as an empty completion: {empty_history_terminals:#?}"
    );
}

/// The history-only rebuild of the same two turns: a tool-only step projects
/// as its `tool_call_requested` frame (call and result render as a pair, like
/// a live turn), and the two text replies remain authored `text_complete`
/// messages without fabricating any run terminals.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn history_rebuild_projects_a_tool_only_step_as_a_tool_call_not_an_empty_completion() {
    let (frames, _) = run_tool_step_console_scenario(false).await;
    let pretty = || serde_json::to_string_pretty(&frames).unwrap_or_default();

    let mut results: Vec<&str> = frames
        .iter()
        .filter(|frame| frame["kind"] == json!("text_complete"))
        .map(frame_result_text)
        .collect();
    results.sort_unstable();
    assert_eq!(
        results,
        vec!["Reply to turn-1", "Reply to turn-2"],
        "one authored text message per turn and none for the tool-only steps; frames:\n{}",
        pretty()
    );
    assert!(
        frames
            .iter()
            .all(|frame| frame["kind"] != json!("interaction_complete")),
        "history messages never fabricate run terminals: {}",
        pretty()
    );
    let peers_calls = frames
        .iter()
        .filter(|frame| {
            frame["kind"] == json!("tool_call_requested")
                && frame["payload"]["name"] == json!("peers")
                && frame["source"]["kind"] == json!("session_history")
        })
        .count();
    assert_eq!(
        peers_calls,
        2,
        "each tool-only step projects its tool call; frames:\n{}",
        pretty()
    );
}

const PARKED_TOOL_MEMBER: &str = "console-parked-tool";
const PARKED_TOOL: &str = "parked_probe";
/// Failure bound for each wait of the parked-turn shutdown scenario.
const PARKED_TURN_FAILURE_BOUND: Duration = Duration::from_mins(1);

/// A host tool whose call stays in flight until the mob has stopped. A turn
/// that calls it is parked inside the call when shutdown begins, and its run
/// can end only after the mob stop, which shutdown starts after its first
/// event drain.
struct ParkedToolBundle {
    entered: Arc<tokio::sync::Notify>,
    mob: Arc<std::sync::OnceLock<meerkat_mob::MobHandle>>,
}

#[async_trait::async_trait]
impl meerkat_core::AgentToolDispatcher for ParkedToolBundle {
    fn tools(&self) -> Arc<[Arc<meerkat_core::types::ToolDef>]> {
        vec![Arc::new(meerkat_core::types::ToolDef::new(
            PARKED_TOOL,
            "Returns once the mob has stopped.",
            json!({"type": "object", "properties": {}}),
        ))]
        .into()
    }

    async fn dispatch(
        &self,
        call: meerkat_core::types::ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
        self.entered.notify_one();
        if let Some(mob) = self.mob.get().cloned() {
            // Bounded: a stop that never comes ends the call, and the
            // scenario then fails on its own assertions. Only an observed
            // state other than Running releases the call early.
            let _ = tokio::time::timeout(PARKED_TURN_FAILURE_BOUND, async {
                while !matches!(
                    mob.status().await,
                    Ok(state) if state != meerkat_mob::MobState::Running
                ) {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
            .await;
        }
        Ok(meerkat_core::ToolDispatchOutcome::sync_result(
            meerkat_core::types::ToolResult::new(
                call.id.to_string(),
                "released by the mob stop".to_string(),
                false,
            ),
        ))
    }

    fn capabilities(&self) -> meerkat_core::agent::DispatcherCapabilities {
        meerkat_core::agent::DispatcherCapabilities::default()
    }
}

/// Scripted LLM for the parked-turn scenario: a `parked-` prompt calls the
/// parked tool and answers once the call returned; any host-minted turn gets
/// a plain text answer.
struct ParkedTurnClient;

impl LlmClient for ParkedTurnClient {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        let provider = LlmClient::provider(self);
        if !last_user_prompt(request).starts_with("parked-") {
            return text_only_turn(request, provider, "Acknowledged.");
        }
        if matches!(request.messages.last(), Some(Message::ToolResults { .. })) {
            return text_only_turn(request, provider, "The parked call returned.");
        }
        let [usage, done] = llm_usage::usage_then_done(request, provider, StopReason::ToolUse);
        Box::pin(stream::iter(vec![
            Ok(LlmEvent::ToolCallComplete {
                id: "parked-call".to_string(),
                name: PARKED_TOOL.to_string(),
                args: json!({}),
                meta: None,
            }),
            Ok(usage),
            Ok(done),
        ]))
    }

    fn provider(&self) -> Provider {
        Provider::Other
    }

    fn health_check<'life0, 'async_trait>(
        &'life0 self,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), LlmError>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async { Ok(()) })
    }
}

/// A turn parked inside a tool call when the gateway shuts down gracefully
/// ends with a terminal frame in the durable console log.
///
/// The mob stop ends the parked turn's run after shutdown's first event
/// drain, so the run's terminal reached the event router only after that
/// drain and was dropped when the router closed: read from the send's
/// accepted cursor after a restart, the log had no terminal for the
/// interaction. The scenario runs the persistent gateway's shape: a SQLite
/// console store, the gateway's own drain task stopped before the runtime
/// shutdown, and then a fresh store over the same file, which is what the
/// next boot reads.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_turn_parked_in_a_tool_call_at_shutdown_keeps_a_durable_terminal_frame() {
    use meerkat_meta::ConsoleLogStore as _;

    let mob_id = format!(
        "parked-turn-shutdown-{}",
        NEXT_TEST_MOB_ID.fetch_add(1, Ordering::Relaxed)
    );
    let definition = MobDefinition::from_toml(&format!(
        r#"
[mob]
id = "{mob_id}"

[profiles.lead]
model = "gpt-5.5"
external_addressable = true

[profiles.lead.tools]
comms = true
rust_bundles = ["parked"]
"#
    ))
    .expect("parse parked-turn mob definition");

    let temp_dir = tempfile::tempdir().expect("temp dir");
    let session_path = temp_dir.path().join("sessions");
    std::fs::create_dir_all(&session_path).expect("session path");
    let console_db = temp_dir.path().join("console.sqlite3");
    let factory = AgentFactory::new(&session_path).comms(true);
    let session_service = Arc::new(build_ephemeral_service(factory, Config::default(), 16));
    let entered = Arc::new(tokio::sync::Notify::new());
    let mob_slot = Arc::new(std::sync::OnceLock::new());
    let mob_spec = MobBootstrapSpec::new(definition, MobStorage::in_memory(), session_service)
        .with_options(MobBootstrapOptions {
            allow_ephemeral_sessions: true,
            notify_orchestrator_on_resume: true,
            default_llm_client: Some(Arc::new(ParkedTurnClient)),
        })
        .register_tool_bundle(
            "parked",
            Arc::new(ParkedToolBundle {
                entered: Arc::clone(&entered),
                mob: Arc::clone(&mob_slot),
            }),
        );
    let mut runtime = UnifiedRuntime::bootstrap(
        mob_spec,
        MetaConfig {
            modules: vec![],
            discovery: DiscoverySpec {
                namespace: mob_id.clone(),
                modules: vec![],
            },
            pre_spawn: vec![],
        },
        Duration::from_secs(2),
    )
    .await
    .expect("bootstrap parked-turn runtime");
    // The persistent gateway's console store: the log a restart reads.
    runtime.set_console_log_store(Arc::new(
        meerkat_meta::SqliteConsoleLogStore::open(&console_db).expect("open console store"),
    ));
    let _ = mob_slot.set(runtime.mob_handle());
    let runtime = Arc::new(runtime);
    // The gateways drain runtime events on their own task and stop it before
    // the runtime shutdown, as `GatewayComposition::shutdown` does.
    let event_drain = Arc::clone(&runtime).spawn_event_drain_task();

    let mut spec = SpawnMemberSpec::from_wire(
        "lead".to_string(),
        MobMemberId::from(PARKED_TOOL_MEMBER).to_string(),
        None,
        None,
        None,
    );
    spec.runtime_mode = Some(MobRuntimeMode::TurnDriven);
    runtime.spawn(spec).await.expect("spawn turn_driven member");

    let app = runtime.build_reference_app_router(decision_state(false));
    let send_json = post_console_rpc(
        &app,
        &json!({
            "jsonrpc": "2.0",
            "id": "send-parked",
            "method": "meta/console/send",
            "params": {
                "identity": PARKED_TOOL_MEMBER,
                "origin": "test",
                "idempotency_key": "parked-turn",
                "content": "parked-turn"
            }
        }),
    )
    .await;
    assert!(
        send_json.get("error").is_none() || send_json["error"].is_null(),
        "console send failed: {send_json}"
    );
    let interaction_id = send_json["result"]["interaction_id"]
        .as_str()
        .expect("an accepted send names its interaction")
        .to_string();
    let accepted_cursor = send_json["result"]["cursor"]
        .as_str()
        .expect("an accepted send names its cursor")
        .to_string();

    tokio::time::timeout(PARKED_TURN_FAILURE_BOUND, entered.notified())
        .await
        .expect("the turn entered the parked tool call");

    event_drain.abort();
    let _ = event_drain.await;
    let shutdown = runtime.shutdown().await;
    assert_mob_stop_allows_boundary_cancel(shutdown.mob_stop);
    drop(app);
    drop(runtime);

    let store =
        meerkat_meta::SqliteConsoleLogStore::open(&console_db).expect("reopen console store");
    let mut frames = Vec::new();
    let mut after = Some(meerkat_meta::ConsoleCursor::from(accepted_cursor.as_str()));
    loop {
        let page = store
            .query_frames(meerkat_meta::ConsoleTimelineQuery {
                identity: None,
                conversation_id: None,
                after: after.clone(),
                limit: 500,
            })
            .await
            .expect("query the durable console log");
        let Some(last) = page.frames.last() else {
            break;
        };
        after = Some(last.cursor.clone());
        frames.extend(page.frames);
    }
    let interaction_frames: Vec<(String, String)> = frames
        .iter()
        .filter(|frame| frame.interaction_id.as_deref() == Some(interaction_id.as_str()))
        .map(|frame| (frame.kind.clone(), format!("{:?}", frame.source.kind)))
        .collect();
    let live_terminals: Vec<&str> = frames
        .iter()
        .filter(|frame| {
            frame.interaction_id.as_deref() == Some(interaction_id.as_str())
                && frame.source.kind != meerkat_meta::ConsoleFrameSourceKind::SessionHistory
                && matches!(
                    frame.kind.as_str(),
                    "interaction_complete" | "interaction_failed"
                )
        })
        .map(|frame| frame.kind.as_str())
        .collect();
    eprintln!("parked turn {interaction_id} live terminals after restart: {live_terminals:?}");
    assert!(
        !live_terminals.is_empty(),
        "the parked turn has no terminal frame after the accepted cursor {accepted_cursor}; \
         its frames (kind, source): {interaction_frames:?}"
    );
}
