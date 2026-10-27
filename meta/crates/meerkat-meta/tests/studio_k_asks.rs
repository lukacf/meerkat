// meerkat-studio K-asks regression suite (K1 repro + K2 transparency).
//
// K1: meta/retire_member + meta/respawn_member on members of a
// multi-member crew created via meta/ensure_member. Works on the ephemeral
// session service; on the persistent chain (studio's construction:
// FactoryAgentBuilder -> PersistentSessionService -> MobBootstrapSpec ->
// UnifiedRuntime) BOTH used to fail for never-ran members: disposal completed
// but the ArchiveSession step's authority lookup NotFounded (no runtime
// snapshot was ever committed for an idle member), meerkat-mob escalated to a
// fatal error, and the member was stranded in state=retiring — and on meerkat
// 0.7.21 the same path DEADLOCKED the whole mob instead. Fixed upstream
// across asks 20/21/21b/21c; converged in meerkat 0.7.22 (the runtime-loop
// stop-under-gate self-deadlock was the root producer of the strand state).
// Residue: the identity-first gateway construction's mob-plane workers still
// archive-NotFound on 0.7.22 (ask 21d, tolerated in the doctrine test below).
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::sync::Arc;
use std::time::Duration;

use axum::body::{Body, to_bytes};
use axum::http::Request;
use meerkat::{
    AgentFactory, Config, FactoryAgentBuilder, PersistentSessionService, build_ephemeral_service,
};
use meerkat_client::TestClient;
use meerkat_meta::{
    AuthPolicy, BigQueryNaming, ConsolePolicy, DiscoverySpec, MetaConfig, MobBootstrapOptions,
    MobBootstrapSpec, RuntimeDecisionInputs, RuntimeOpsPolicy, TrustedOidcRuntimeConfig,
    UnifiedRuntime, build_runtime_decision_state,
};
use meerkat_mob::{MobDefinition, MobStorage};
use serde_json::{Value, json};
use tower::ServiceExt;

async fn rpc(app: &axum::Router, method: &str, params: Value) -> Value {
    let request = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/console/rpc")
                .header("content-type", "application/json")
                .body(Body::from(request.to_string()))
                .expect("request"),
        )
        .await
        .expect("response");
    let body = to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("body");
    serde_json::from_slice(&body).expect("json")
}

fn studio_decision_state() -> meerkat_meta::RuntimeDecisionState {
    build_runtime_decision_state(RuntimeDecisionInputs {
        bigquery: BigQueryNaming {
            dataset: "studio_dataset".to_string(),
            table: "studio_table".to_string(),
        },
        trusted_meta_toml: r#"
[[modules]]
id = "router"
command = "router-bin"
args = []
restart_policy = "always"
"#
        .to_string(),
        auth: AuthPolicy {
            default_provider: meerkat_meta::AuthProvider::GoogleOAuth,
            email_allowlist: vec!["alice@example.com".to_string()],
        },
        trusted_oidc: TrustedOidcRuntimeConfig {
            discovery_json:
                r#"{"issuer":"https://trusted.meta.local","jwks_uri":"https://trusted.meta.local/.well-known/jwks.json"}"#
                    .to_string(),
            jwks_json: r#"{"keys":[{"kid":"kid-current","kty":"oct","alg":"HS256","k":"cGhhc2U3LXRydXN0ZWQtY3VycmVudC1zZWNyZXQ"}]}"#
                .to_string(),
            audience: "meerkat-console".to_string(),
            require_verified_email: false,
        },
        console: ConsolePolicy {
            require_app_auth: false,
            ..ConsolePolicy::default()
        },
        ops: RuntimeOpsPolicy::default(),
        release_metadata_json: include_str!("../assets/release-targets.json").to_string(),
    })
    .expect("decision state")
}

#[tokio::test]
async fn studio_k1_retire_respawn_succeed_on_ephemeral_ensure_member_crew() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let session_path = temp_dir.path().join("sessions");
    std::fs::create_dir_all(&session_path).expect("session path");
    let factory = AgentFactory::new(&session_path).comms(true);
    let session_service = Arc::new(build_ephemeral_service(factory, Config::default(), 16));
    let definition = MobDefinition::from_toml(
        r#"
[mob]
id = "studio-crew"

[profiles.general]
model = "gpt-5.5"
external_addressable = true

[profiles.general.tools]
comms = true
"#,
    )
    .expect("definition");
    let mob_spec = MobBootstrapSpec::new(definition, MobStorage::in_memory(), session_service)
        .with_options(MobBootstrapOptions {
            allow_ephemeral_sessions: true,
            notify_orchestrator_on_resume: true,
            default_llm_client: Some(Arc::new(TestClient::default())),
        });
    let module_config = MetaConfig {
        modules: vec![],
        discovery: DiscoverySpec {
            namespace: "studio".to_string(),
            modules: vec![],
        },
        pre_spawn: vec![],
    };
    let runtime = UnifiedRuntime::bootstrap(mob_spec, module_config, Duration::from_secs(2))
        .await
        .expect("bootstrap");
    let decisions = studio_decision_state();
    let app = runtime.build_reference_app_router(decisions);

    for member in ["lead", "builder", "reviewer"] {
        let response = rpc(
            &app,
            "meta/ensure_member",
            json!({"role": "general", "agent_identity": member}),
        )
        .await;
        assert!(
            response.get("error").is_none(),
            "ensure {member}: {response}"
        );
    }

    let retire = rpc(&app, "meta/retire_member", json!({"member_id": "builder"})).await;
    assert!(
        retire.get("error").is_none(),
        "retire_member on an ephemeral ensure_member crew must succeed: {retire}"
    );

    let respawn = rpc(
        &app,
        "meta/respawn_member",
        json!({"member_id": "reviewer"}),
    )
    .await;
    assert!(
        respawn.get("error").is_none(),
        "respawn_member on an ephemeral ensure_member crew must succeed: {respawn}"
    );

    let caps = rpc(&app, "meta/capabilities", json!({})).await;
    assert_eq!(
        caps["result"]["identity_first"],
        json!(false),
        "a mob-plane-only console must advertise identity_first=false: {caps}"
    );
}

/// Studio's actual construction chain: FactoryAgentBuilder →
/// PersistentSessionService → MobBootstrapSpec → UnifiedRuntime, then a
/// 3-member crew via meta/ensure_member and per-member retire/respawn.
#[tokio::test]
async fn studio_k1_retire_respawn_succeed_on_persistent_ensure_member_crew() {
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
    let mut builder = FactoryAgentBuilder::new(factory, Config::default());
    builder.default_session_store = Some(Arc::new(meerkat_store::StoreAdapter::new(
        session_store.clone(),
    )));
    builder.default_blob_store = Some(blob_store.clone());
    let service = Arc::new(PersistentSessionService::new(
        builder,
        16,
        session_store,
        runtime_store,
        blob_store,
    ));
    let definition = MobDefinition::from_toml(
        r#"
[mob]
id = "studio-crew-persistent"

[profiles.general]
model = "gpt-5.5"
external_addressable = true

[profiles.general.tools]
comms = true
"#,
    )
    .expect("definition");
    // NOTE: deliberately NOT passing a separate with_session_runtime_adapter
    // — the mob must share the concrete service's own cached machine so the
    // archive protocol and the session lifecycle run on ONE authority
    // (two-adapter discriminator for the ask-21 residue).
    let mob_spec = MobBootstrapSpec::new(definition, MobStorage::in_memory(), service)
        .with_options(MobBootstrapOptions {
            allow_ephemeral_sessions: true,
            notify_orchestrator_on_resume: true,
            default_llm_client: Some(Arc::new(TestClient::default())),
        });
    let module_config = MetaConfig {
        modules: vec![],
        discovery: DiscoverySpec {
            namespace: "studio-persistent".to_string(),
            modules: vec![],
        },
        pre_spawn: vec![],
    };
    let runtime = UnifiedRuntime::bootstrap(mob_spec, module_config, Duration::from_secs(2))
        .await
        .expect("bootstrap");
    let app = runtime.build_reference_app_router(studio_decision_state());

    for member in ["lead", "builder", "reviewer"] {
        let response = rpc(
            &app,
            "meta/ensure_member",
            json!({"role": "general", "agent_identity": member}),
        )
        .await;
        assert!(
            response.get("error").is_none(),
            "ensure {member}: {response}"
        );
    }

    let respawn = rpc(
        &app,
        "meta/respawn_member",
        json!({"member_id": "reviewer"}),
    )
    .await;
    let retire = rpc(&app, "meta/retire_member", json!({"member_id": "builder"})).await;
    assert!(
        retire.get("error").is_none(),
        "retire_member on a persistent ensure_member crew must succeed \
         (meerkat-studio K1); error: {retire}"
    );

    assert!(
        respawn.get("error").is_none(),
        "respawn_member on a persistent ensure_member crew must succeed \
         (meerkat-studio K1); error: {respawn}"
    );
}

/// meerkat-studio ask K2: a console JSON-RPC internal error must carry the
/// real failure reason on the wire — `message` and `data.detail` — never a
/// bare `{"error":"internal_error"}`. Uses an operation that fails
/// deterministically regardless of upstream fixes: respawning a member that
/// does not exist.
#[tokio::test]
async fn studio_k2_internal_errors_carry_detail_on_the_wire() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let session_path = temp_dir.path().join("sessions");
    std::fs::create_dir_all(&session_path).expect("session path");
    let factory = AgentFactory::new(&session_path).comms(true);
    let session_service = Arc::new(build_ephemeral_service(factory, Config::default(), 16));
    let definition = MobDefinition::from_toml(
        r#"
[mob]
id = "studio-k2"

[profiles.general]
model = "gpt-5.5"
external_addressable = true

[profiles.general.tools]
comms = true
"#,
    )
    .expect("definition");
    let mob_spec = MobBootstrapSpec::new(definition, MobStorage::in_memory(), session_service)
        .with_options(MobBootstrapOptions {
            allow_ephemeral_sessions: true,
            notify_orchestrator_on_resume: true,
            default_llm_client: Some(Arc::new(TestClient::default())),
        });
    let module_config = MetaConfig {
        modules: vec![],
        discovery: DiscoverySpec {
            namespace: "studio-k2".to_string(),
            modules: vec![],
        },
        pre_spawn: vec![],
    };
    let runtime = UnifiedRuntime::bootstrap(mob_spec, module_config, Duration::from_secs(2))
        .await
        .expect("bootstrap");
    let app = runtime.build_reference_app_router(studio_decision_state());

    let response = rpc(
        &app,
        "meta/respawn_member",
        json!({"member_id": "no-such-member"}),
    )
    .await;
    let error = response
        .get("error")
        .expect("respawn of unknown member errors");
    let message = error["message"].as_str().unwrap_or_default();
    assert_ne!(
        message, "internal error",
        "the wire message must carry the real reason, not the old opaque \
         placeholder: {response}"
    );
    assert!(
        !message.is_empty(),
        "error message must not be empty: {response}"
    );
    let detail = error["data"]["detail"].as_str().unwrap_or_default();
    assert!(
        !detail.is_empty(),
        "data.detail must carry the failure chain: {response}"
    );
}

/// meerkat-studio ask K0: the identity-first gateway surface. Same persistent
/// construction chain and the same three RPCs that fail on the session-owned
/// surface (the `#[ignore]`d test above) — but with the identity-first
/// substrate attached, retire and respawn of NEVER-RAN members succeed:
/// the ask-20 failure class is unreachable by construction.
#[tokio::test]
async fn studio_k0_identity_first_gateway_retire_respawn_succeed_on_idle_members() {
    use meerkat_meta::identity_first::{
        AgentRuntimeServices, DurabilityPolicy, IdentityFirstRuntimeContext, IdentityRuntime,
        IdentityRuntimeConfig, LocalContinuityStore, LocalLeaseProvider, MobSessionBridge,
        MutableRosterProvider, restore_flow,
    };

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
    let mut builder = FactoryAgentBuilder::new(factory, Config::default());
    builder.default_session_store = Some(Arc::new(meerkat_store::StoreAdapter::new(
        session_store.clone(),
    )));
    builder.default_blob_store = Some(blob_store.clone());
    let adapter = Arc::new(
        meerkat_runtime::MeerkatMachine::persistent(
            Arc::clone(&runtime_store),
            Arc::clone(&blob_store),
        )
        .expect("acquire the studio fixture runtime machine"),
    );
    let service = Arc::new(PersistentSessionService::new(
        builder,
        16,
        session_store,
        runtime_store,
        blob_store,
    ));
    let definition = MobDefinition::from_toml(
        r#"
[mob]
id = "studio-crew-identity"

[profiles.general]
model = "gpt-5.5"
external_addressable = true

[profiles.general.tools]
comms = true
"#,
    )
    .expect("definition");
    let mob_spec = MobBootstrapSpec::new(definition, MobStorage::in_memory(), service)
        .with_session_runtime_adapter(adapter.clone())
        .expect("acquire the fixture session runtime owner")
        .with_options(MobBootstrapOptions {
            allow_ephemeral_sessions: true,
            notify_orchestrator_on_resume: true,
            default_llm_client: Some(Arc::new(TestClient::default())),
        });
    let module_config = MetaConfig {
        modules: vec![],
        discovery: DiscoverySpec {
            namespace: "studio-identity".to_string(),
            modules: vec![],
        },
        pre_spawn: vec![],
    };
    let mut runtime = UnifiedRuntime::bootstrap(mob_spec, module_config, Duration::from_secs(2))
        .await
        .expect("bootstrap");

    // Mirror meta_gateway's `identity_first: true` bootstrap.
    let continuity_store =
        LocalContinuityStore::open(state.join("continuity.db")).expect("continuity store");
    let fencing_floor = continuity_store
        .max_fencing_token()
        .expect("fencing high-water");
    let mob_handle = runtime.mob_handle();
    let session_service = runtime
        .mob_runtime()
        .session_service()
        .cloned()
        .expect("session service");
    let bridge: Arc<dyn meerkat_meta::identity_first::SessionBridge> = Arc::new(
        MobSessionBridge::with_session_service(mob_handle.clone(), session_service),
    );
    let irt = Arc::new(
        IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(continuity_store),
            lease_provider: Arc::new(LocalLeaseProvider::with_floor(fencing_floor)),
            runtime_instance_id: "studio-k0-test".to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: Some(bridge),
            default_timeout: None,
        })
        .with_runtime_services(AgentRuntimeServices::new(mob_handle.clone())),
    );
    let roster = Arc::new(MutableRosterProvider::new(Vec::new()));
    restore_flow(&irt, &roster.snapshot(), None, None)
        .await
        .expect("restore_flow (empty roster)");
    let mob_definition = mob_handle.definition().clone();
    runtime.set_console_identity_roster(roster.clone());
    runtime.attach_identity_first_context(Arc::new(IdentityFirstRuntimeContext::new(
        irt,
        roster,
        None,
        None,
        Some(mob_definition),
    )));
    let app = runtime.build_reference_app_router(studio_decision_state());

    for member in ["lead", "builder", "reviewer"] {
        let response = rpc(
            &app,
            "meta/ensure_member",
            json!({"role": "general", "agent_identity": member}),
        )
        .await;
        assert!(
            response.get("error").is_none(),
            "identity ensure {member}: {response}"
        );
        assert_eq!(
            response["result"]["identity_first"],
            json!(true),
            "ensure_member must take the identity arm: {response}"
        );
        assert_eq!(
            response["result"]["state"],
            json!("active"),
            "ensured identity must be active: {response}"
        );
    }

    // The K1-class calls: retire + respawn of NEVER-RAN members. On the
    // session-owned surface these strand the member; here they succeed.
    let retire = rpc(
        &app,
        "meta/retire_member",
        json!({"member_id": "rt:builder:0"}),
    )
    .await;
    assert!(
        retire.get("error").is_none(),
        "identity retire_member of an idle member must succeed: {retire}"
    );
    assert_eq!(
        retire["result"]["identity_first"],
        json!(true),
        "a current runtime alias must route through the identity authority: {retire}"
    );
    let retired_status = rpc(&app, "meta/status_identity", json!({"identity": "builder"})).await;
    assert_eq!(
        retired_status["result"]["state"],
        json!("dormant"),
        "runtime-alias retirement must leave the identity dormant (Core retired its member): \
         {retired_status}"
    );
    let respawn = rpc(
        &app,
        "meta/respawn_member",
        json!({"member_id": "rt:reviewer:0"}),
    )
    .await;
    assert!(
        respawn.get("error").is_none(),
        "identity respawn_member of an idle member must succeed: {respawn}"
    );
    assert!(
        respawn["result"]["session_id"].is_string(),
        "respawn must report the fresh session: {respawn}"
    );
    assert_eq!(
        respawn["result"]["identity_first"],
        json!(true),
        "a current runtime alias must respawn through the identity authority: {respawn}"
    );
    assert_eq!(
        respawn["result"]["generation"],
        json!(1),
        "identity respawn must advance continuity generation: {respawn}"
    );
    let respawned_status = rpc(
        &app,
        "meta/status_identity",
        json!({"identity": "reviewer"}),
    )
    .await;
    assert_eq!(
        respawned_status["result"]["agent_runtime_id"],
        json!("rt:reviewer:1"),
        "status must bind the durable identity to the fresh runtime alias: {respawned_status}"
    );

    // retire_member also removed builder from the desired roster: another
    // ensure re-creates it cleanly.
    let re_ensure = rpc(
        &app,
        "meta/ensure_member",
        json!({"role": "general", "agent_identity": "builder"}),
    )
    .await;
    assert!(
        re_ensure.get("error").is_none(),
        "re-ensure after retire must succeed: {re_ensure}"
    );

    // Capabilities advertise the doctrine flag — consumers gate their
    // migration on it (studio contract point 5).
    let caps = rpc(&app, "meta/capabilities", json!({})).await;
    assert_eq!(
        caps["result"]["identity_first"],
        json!(true),
        "identity-first gateway must advertise identity_first: {caps}"
    );

    // plane:"worker" pins a spawn to the ephemeral mob plane even here.
    let worker = rpc(
        &app,
        "meta/ensure_member",
        json!({"role": "general", "agent_identity": "scratch-helper", "plane": "worker"}),
    )
    .await;
    assert!(
        worker.get("error").is_none(),
        "worker-plane ensure on an identity gateway must succeed: {worker}"
    );
    assert!(
        worker["result"].get("identity_first").is_none(),
        "plane:worker must NOT create a durable identity: {worker}"
    );
    let worker_respawn = rpc(
        &app,
        "meta/respawn_member",
        json!({"member_id": "scratch-helper"}),
    )
    .await;
    assert!(
        worker_respawn.get("error").is_none(),
        "worker-plane respawn must keep the classic Mob path: {worker_respawn}"
    );
    assert!(
        worker_respawn["result"].get("identity_first").is_none(),
        "worker-plane respawn must not claim durable authority: {worker_respawn}"
    );
    let worker_retire = rpc(
        &app,
        "meta/retire_member",
        json!({"member_id": "scratch-helper"}),
    )
    .await;
    assert!(
        worker_retire.get("error").is_none(),
        "worker-plane retirement must keep the classic Mob path: {worker_retire}"
    );
    assert!(
        worker_retire["result"].get("identity_first").is_none(),
        "worker-plane retirement must not claim durable authority: {worker_retire}"
    );
}

/// Doctrine reachability fix: on a runtime that carries an IdentityRuntime
/// but NO console roster slot (builder-constructed identity-first runtimes),
/// the member-scoped RPCs must still route identity-owned members through
/// the identity authority — a classic handle.retire()/respawn() would mutate
/// the member behind the IdentityRuntime's back (stale continuity binding,
/// generation drift). Worker-plane members (not identity-registered) keep
/// the classic path.
#[tokio::test]
async fn doctrine_member_rpcs_route_identity_owned_members_through_identity_authority() {
    use meerkat_meta::identity_first::{
        AgentRuntimeServices, DurabilityPolicy, IdentityFirstRuntimeContext, IdentityRuntime,
        IdentityRuntimeConfig, LocalContinuityStore, LocalLeaseProvider, MobSessionBridge,
        MutableRosterProvider, restore_flow,
    };

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
    let mut builder = FactoryAgentBuilder::new(factory, Config::default());
    builder.default_session_store = Some(Arc::new(meerkat_store::StoreAdapter::new(
        session_store.clone(),
    )));
    builder.default_blob_store = Some(blob_store.clone());
    let adapter = Arc::new(
        meerkat_runtime::MeerkatMachine::persistent(
            Arc::clone(&runtime_store),
            Arc::clone(&blob_store),
        )
        .expect("acquire the studio fixture runtime machine"),
    );
    let service = Arc::new(PersistentSessionService::new(
        builder,
        16,
        session_store,
        runtime_store,
        blob_store,
    ));
    let definition = MobDefinition::from_toml(
        r#"
[mob]
id = "doctrine-routing"

[profiles.general]
model = "gpt-5.5"
external_addressable = true

[profiles.general.tools]
comms = true
"#,
    )
    .expect("definition");
    let mob_spec = MobBootstrapSpec::new(definition, MobStorage::in_memory(), service)
        .with_session_runtime_adapter(adapter.clone())
        .expect("acquire the fixture session runtime owner")
        .with_options(MobBootstrapOptions {
            allow_ephemeral_sessions: true,
            notify_orchestrator_on_resume: true,
            default_llm_client: Some(Arc::new(TestClient::default())),
        });
    let module_config = MetaConfig {
        modules: vec![],
        discovery: DiscoverySpec {
            namespace: "doctrine-routing".to_string(),
            modules: vec![],
        },
        pre_spawn: vec![],
    };
    let mut runtime = UnifiedRuntime::bootstrap(mob_spec, module_config, Duration::from_secs(2))
        .await
        .expect("bootstrap");

    // Identity substrate with ONE durable identity in the roster — but NO
    // console roster slot (the builder-constructed shape).
    let continuity_store =
        LocalContinuityStore::open(state.join("continuity.db")).expect("continuity store");
    let mob_handle = runtime.mob_handle();
    let session_service = runtime
        .mob_runtime()
        .session_service()
        .cloned()
        .expect("session service");
    let bridge: Arc<dyn meerkat_meta::identity_first::SessionBridge> = Arc::new(
        MobSessionBridge::with_session_service(mob_handle.clone(), session_service),
    );
    let irt = Arc::new(
        IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(continuity_store),
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: "doctrine-routing-test".to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: Some(bridge),
            default_timeout: None,
        })
        .with_runtime_services(AgentRuntimeServices::new(mob_handle.clone())),
    );
    let roster = Arc::new(MutableRosterProvider::new(vec![
        meerkat_meta::identity_first::DurableAgentSpec {
            identity: meerkat_meta::identity_first::AgentIdentity::parse("personal:alice")
                .expect("identity"),
            profile: meerkat_mob::ProfileName::from("general"),
            addressability: meerkat_meta::identity_first::AgentAddressability::Addressable,
            display_name: None,
            labels: std::collections::BTreeMap::new(),
            context: None,
            additional_instructions: Vec::new(),
            initial_message: None,
            runtime_mode_override: None,
            backend: None,
            binding: None,
            placement: None,
        },
    ]));
    restore_flow(&irt, &roster.snapshot(), None, None)
        .await
        .expect("restore alice");
    let mob_definition = mob_handle.definition().clone();
    // NOTE: deliberately NOT calling set_console_identity_roster — this is
    // the no-roster-slot console shape.
    runtime.attach_identity_first_context(Arc::new(IdentityFirstRuntimeContext::new(
        irt.clone(),
        roster,
        None,
        None,
        Some(mob_definition),
    )));
    let app = runtime.build_reference_app_router(studio_decision_state());

    // A worker on the mob plane, for contrast.
    let ensure = rpc(
        &app,
        "meta/ensure_member",
        json!({"role": "general", "agent_identity": "scratch-worker"}),
    )
    .await;
    assert!(
        ensure.get("error").is_none(),
        "worker-plane ensure must stay classic and succeed: {ensure}"
    );
    assert!(
        ensure["result"].get("identity_first").is_none(),
        "no roster slot: ensure_member must NOT take the identity arm: {ensure}"
    );

    // retire_member addressed at the DURABLE identity routes to the identity
    // authority (identity_first marker), never the classic mob plane.
    let retire = rpc(
        &app,
        "meta/retire_member",
        json!({"member_id": "personal:alice"}),
    )
    .await;
    assert!(
        retire.get("error").is_none(),
        "identity-owned retire must succeed: {retire}"
    );
    assert_eq!(
        retire["result"]["identity_first"],
        json!(true),
        "identity-owned member must route through the identity authority: {retire}"
    );

    // Same for respawn: identity reset (fresh session, same identity).
    // Re-materialize alice first (retire above released her).
    restore_flow(&irt, &[], None, None).await.ok();
    let respawn = rpc(
        &app,
        "meta/respawn_member",
        json!({"member_id": "scratch-worker"}),
    )
    .await;
    // Never-ran worker respawn converges since meerkat 0.7.23 (ask 21d:
    // archive-NotFound on a terminal-phase registered runtime completes
    // disposal instead of escalating).
    assert!(
        respawn["result"].get("identity_first").is_none(),
        "worker respawn must NOT route through the identity authority: {respawn}"
    );
    assert!(
        respawn.get("error").is_none(),
        "worker respawn must succeed on the mob plane: {respawn}"
    );
}
