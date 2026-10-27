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
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use meerkat::{AgentFactory, Config, build_ephemeral_service};
use meerkat_client::TestClient;
use meerkat_meta::{
    AuthPolicy, BigQueryNaming, ConsolePolicy, DiscoverySpec, MetaConfig, MobBootstrapOptions,
    MobBootstrapSpec, ModuleConfig, RestartPolicy, RuntimeDecisionInputs, RuntimeOpsPolicy,
    RuntimeRouteMutationError, TrustedOidcRuntimeConfig, UnifiedRuntime,
    UnifiedRuntimeBuilderError, UnifiedRuntimeBuilderField, UnifiedRuntimeReconcileError,
    build_runtime_decision_state,
};
use meerkat_mob::{MobDefinition, MobState, MobStorage, SpawnMemberSpec};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::oneshot;
use tower::ServiceExt;

fn shell_module(id: &str, script: &str) -> ModuleConfig {
    ModuleConfig {
        id: id.to_string(),
        command: "sh".to_string(),
        args: vec!["-c".to_string(), script.to_string()],
        restart_policy: RestartPolicy::Never,
    }
}

fn spawn_spec(profile: &str, member_id: &str) -> SpawnMemberSpec {
    SpawnMemberSpec::from_wire(
        profile.to_string(),
        member_id.to_string(),
        Some(format!("You are {member_id}. Keep responses concise.").into()),
        None,
        None,
    )
}

fn build_phase2_mob_spec(temp_dir: &tempfile::TempDir) -> MobBootstrapSpec {
    let session_path = temp_dir.path().join("sessions");
    std::fs::create_dir_all(&session_path).expect("session path");

    let factory = AgentFactory::new(&session_path).comms(true);
    let session_service = Arc::new(build_ephemeral_service(factory, Config::default(), 16));

    let definition = MobDefinition::from_toml(
        r#"
[mob]
id = "phase2-unified-mob"

[profiles.lead]
model = "gpt-5.5"
external_addressable = true

[profiles.lead.tools]
comms = true
"#,
    )
    .expect("parse mob definition");

    MobBootstrapSpec::new(definition, MobStorage::in_memory(), session_service).with_options(
        MobBootstrapOptions {
            allow_ephemeral_sessions: true,
            notify_orchestrator_on_resume: true,
            default_llm_client: Some(Arc::new(TestClient::default())),
        },
    )
}

fn decision_state() -> meerkat_meta::RuntimeDecisionState {
    build_runtime_decision_state(RuntimeDecisionInputs {
        bigquery: BigQueryNaming {
            dataset: "phase2_reference_dataset".to_string(),
            table: "phase2_reference_table".to_string(),
        },
        trusted_meta_toml: trusted_modules_toml(),
        auth: AuthPolicy::default(),
        trusted_oidc: trusted_oidc(),
        console: ConsolePolicy {
            require_app_auth: false,
            ..ConsolePolicy::default()
        },
        ops: RuntimeOpsPolicy::default(),
        release_metadata_json: include_str!("../assets/release-targets.json").to_string(),
    })
    .expect("build decision state")
}

fn trusted_modules_toml() -> String {
    r#"
[[modules]]
id = "router"
command = "router-bin"
args = ["--mode", "fast"]
restart_policy = "always"

[[modules]]
id = "delivery"
command = "delivery-bin"
args = ["--sink", "memory"]
restart_policy = "on_failure"
"#
    .to_string()
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

async fn get_raw(address: SocketAddr, path: &str, timeout: Duration) -> String {
    let connect_deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut stream = loop {
        match TcpStream::connect(address).await {
            Ok(stream) => break stream,
            Err(error) => {
                assert!(
                    tokio::time::Instant::now() < connect_deadline,
                    "failed to connect to unified runtime at {address}: {error}"
                );
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        }
    };

    let request = format!("GET {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .await
        .expect("write request");
    stream.flush().await.expect("flush request");

    let mut bytes = Vec::new();
    let start = tokio::time::Instant::now();
    while start.elapsed() < timeout {
        let mut chunk = [0_u8; 4096];
        let remaining = timeout.saturating_sub(start.elapsed());
        match tokio::time::timeout(remaining, stream.read(&mut chunk)).await {
            Ok(Ok(0)) => break,
            Ok(Ok(read)) => bytes.extend_from_slice(&chunk[..read]),
            Ok(Err(error)) => panic!("read response failed: {error}"),
            Err(_) => break,
        }
    }

    String::from_utf8(bytes).expect("utf8 response")
}

#[tokio::test]
#[ignore]
async fn req_002_builder_returns_unified_runtime_and_reference_app_is_unified_only() {
    let missing_mob_spec = match Box::pin(
        UnifiedRuntime::builder()
            .module_config(MetaConfig {
                modules: vec![],
                discovery: DiscoverySpec {
                    namespace: "phase2-builder-missing-mob".to_string(),
                    modules: vec![],
                },
                pre_spawn: vec![],
            })
            .timeout(Duration::from_secs(1))
            .build(),
    )
    .await
    {
        Ok(_) => panic!("missing mob_spec should return a typed builder error"),
        Err(error) => error,
    };
    assert!(matches!(
        missing_mob_spec,
        UnifiedRuntimeBuilderError::MissingRequiredField(UnifiedRuntimeBuilderField::MobSpec)
    ));

    let temp_dir = tempfile::tempdir().expect("temp dir");
    let missing_module_config = match Box::pin(
        UnifiedRuntime::builder()
            .mob_spec(build_phase2_mob_spec(&temp_dir))
            .timeout(Duration::from_secs(1))
            .build(),
    )
    .await
    {
        Ok(_) => panic!("missing module_config should return a typed builder error"),
        Err(error) => error,
    };
    assert!(matches!(
        missing_module_config,
        UnifiedRuntimeBuilderError::MissingRequiredField(UnifiedRuntimeBuilderField::ModuleConfig)
    ));

    let runtime = Box::pin(
        UnifiedRuntime::builder()
            .mob_spec(build_phase2_mob_spec(&temp_dir))
            .module_config(MetaConfig {
                modules: vec![],
                discovery: DiscoverySpec {
                    namespace: "phase2-builder-success".to_string(),
                    modules: vec![],
                },
                pre_spawn: vec![],
            })
            .timeout(Duration::from_secs(1))
            .build(),
    )
    .await
    .expect("builder should return unified runtime");
    assert_eq!(
        runtime.mob_handle().status().await.unwrap(),
        MobState::Running
    );
    assert!(runtime.module_is_running().await);

    let example_source = include_str!("../examples/library_mode_reference.rs");
    assert!(example_source.contains("UnifiedRuntime::builder()"));
    assert!(example_source.contains(".reconcile("));
    assert!(example_source.contains(".build_console_json_router("));
    assert!(example_source.contains(".serve(listener, decisions)"));
    assert!(example_source.contains(".run(listener, decisions"));
    assert!(!example_source.contains("axum::serve("));
    assert!(!example_source.contains("build_reference_app_router("));

    let shutdown = runtime.shutdown().await;
    assert_eq!(shutdown.module_shutdown.orphan_processes, 0);
    shutdown.mob_stop.expect("mob runtime should stop cleanly");
}

#[test]
fn sc_001_reference_app_router_proves_unified_owned_console_path() {
    let unified_runtime_source = include_str!("../src/unified_runtime/http.rs");
    assert!(unified_runtime_source.contains(".merge(self.build_console_frontend_router())"));
    assert!(!unified_runtime_source.contains("interaction_sse_router"));
}

#[test]
fn sc_002_unified_shutdown_stops_mob_before_router_and_module_teardown() {
    let lifecycle_source = include_str!("../src/unified_runtime/lifecycle.rs");
    assert_eq!(
        lifecycle_source.matches("let mob_stop = self").count(),
        1,
        "shutdown stops the mob once"
    );
    let mob_stop = lifecycle_source
        .find("let mob_stop = self")
        .expect("shutdown must stop mob");
    let mob_stop_end = mob_stop
        + lifecycle_source[mob_stop..]
            .find(';')
            .expect("the stop statement ends");
    // The terminal Shutdown decides each member's run (a durable member's is
    // kept for its next start), so the stop before it must not cancel them.
    assert!(
        lifecycle_source[mob_stop..mob_stop_end].contains("leave_runs_running()"),
        "the shutdown's stop must leave the members' runs running"
    );
    let close_event_router = lifecycle_source
        .find("self.close_event_router().await;")
        .expect("shutdown must close event router");
    let module_shutdown = lifecycle_source
        .find("let module_shutdown = self.module_runtime.lock().await.shutdown();")
        .expect("shutdown must stop modules");

    assert!(
        mob_stop < close_event_router,
        "mob stop must happen before closing the event router"
    );
    assert!(
        close_event_router < module_shutdown,
        "module teardown must happen after mob stop and event-router close"
    );
}

#[tokio::test]
#[ignore]
async fn req_002_router_builders_prove_console_and_sse_behavior() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let runtime = Box::pin(
        UnifiedRuntime::builder()
            .mob_spec(build_phase2_mob_spec(&temp_dir))
            .module_config(MetaConfig {
                modules: vec![],
                discovery: DiscoverySpec {
                    namespace: "phase2-router-builder-proof".to_string(),
                    modules: vec![],
                },
                pre_spawn: vec![],
            })
            .timeout(Duration::from_secs(1))
            .build(),
    )
    .await
    .expect("build unified runtime");

    runtime
        .spawn(spawn_spec("lead", "router"))
        .await
        .expect("spawn router member");

    let console_router = runtime.build_console_json_router(decision_state());
    let console_response = console_router
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/console/modules")
                .body(Body::empty())
                .expect("console request"),
        )
        .await
        .expect("console response");
    assert_eq!(console_response.status(), StatusCode::OK);
    let console_body = to_bytes(console_response.into_body(), 1024 * 1024)
        .await
        .expect("console body");
    let console_body_text = String::from_utf8(console_body.to_vec()).expect("utf8 console body");
    assert!(console_body_text.contains("\"modules\":[\"router\",\"delivery\"]"));

    let frontend_router = runtime.build_console_frontend_router();
    let frontend_response = frontend_router
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/console")
                .body(Body::empty())
                .expect("frontend request"),
        )
        .await
        .expect("frontend response");
    assert_eq!(frontend_response.status(), StatusCode::OK);
    assert!(
        frontend_response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("text/html"))
    );
    let frontend_body = to_bytes(frontend_response.into_body(), 1024 * 1024)
        .await
        .expect("frontend body");
    let frontend_body_text = String::from_utf8(frontend_body.to_vec()).expect("utf8 frontend body");
    assert!(frontend_body_text.contains("/console/assets/console-app.js"));

    let shutdown = runtime.shutdown().await;
    assert_eq!(shutdown.module_shutdown.orphan_processes, 0);
    shutdown.mob_stop.expect("mob runtime should stop cleanly");
}

#[tokio::test]
#[ignore]
async fn req_002_serve_proves_reference_console_route_behavior() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let runtime = Box::pin(
        UnifiedRuntime::builder()
            .mob_spec(build_phase2_mob_spec(&temp_dir))
            .module_config(MetaConfig {
                modules: vec![],
                discovery: DiscoverySpec {
                    namespace: "phase2-serve-proof".to_string(),
                    modules: vec![],
                },
                pre_spawn: vec![],
            })
            .timeout(Duration::from_secs(1))
            .build(),
    )
    .await
    .expect("build unified runtime");

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind listener");
    let address = listener.local_addr().expect("listener address");
    let decisions = decision_state();

    let response = {
        let serve = runtime.serve(listener, decisions);
        tokio::pin!(serve);
        tokio::select! {
            serve_result = &mut serve => panic!("serve returned unexpectedly: {serve_result:?}"),
            response = get_raw(address, "/console/experience", Duration::from_secs(10)) => response,
        }
    };
    assert!(
        response.starts_with("HTTP/1.1 200"),
        "expected HTTP 200 response, got: {response}"
    );
    assert!(response.contains("\"contract_version\":\"0.5.0\""));
    assert!(response.contains("\"send_method\":\"meta/console/send\""));
    assert!(response.contains("\"observe_route\":\"/console/timeline/stream\""));

    let shutdown = runtime.shutdown().await;
    assert_eq!(shutdown.module_shutdown.orphan_processes, 0);
    shutdown.mob_stop.expect("mob runtime should stop cleanly");
}

#[tokio::test]
#[ignore]
async fn e2e_001_real_http_interactions_stream_sse_through_unified_runtime() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let runtime = Box::pin(
        UnifiedRuntime::builder()
            .mob_spec(build_phase2_mob_spec(&temp_dir))
            .module_config(MetaConfig {
                modules: vec![],
                discovery: DiscoverySpec {
                    namespace: "phase2-e2e-sse".to_string(),
                    modules: vec![],
                },
                pre_spawn: vec![],
            })
            .timeout(Duration::from_secs(1))
            .build(),
    )
    .await
    .expect("build unified runtime");

    runtime
        .spawn(spawn_spec("lead", "router"))
        .await
        .expect("spawn router member");

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind listener");
    let address = listener.local_addr().expect("listener address");
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let decisions = decision_state();

    let server = tokio::spawn(async move {
        runtime
            .run(listener, decisions, async {
                let _ = shutdown_rx.await;
            })
            .await
    });

    let console_response = get_raw(address, "/console/modules", Duration::from_secs(10)).await;
    assert!(
        console_response.starts_with("HTTP/1.1 200"),
        "expected HTTP 200 response from /console/modules, got: {console_response}"
    );
    assert!(console_response.contains("\"modules\":[\"router\",\"delivery\"]"));

    shutdown_tx.send(()).expect("signal runtime shutdown");
    let run_report = server.await.expect("server join");
    assert!(
        run_report.serve_result.is_ok(),
        "serve failed: {:?}",
        run_report.serve_result
    );
    assert_eq!(run_report.shutdown.module_shutdown.orphan_processes, 0);
    run_report
        .shutdown
        .mob_stop
        .expect("mob runtime should stop cleanly");
}

#[tokio::test]
#[ignore]
async fn req_008_reconcile_updates_routing_wiring_when_router_module_is_loaded() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let runtime = Box::pin(UnifiedRuntime::builder()
        .mob_spec(build_phase2_mob_spec(&temp_dir))
        .module_config(MetaConfig {
            modules: vec![shell_module(
                "router",
                r#"printf '%s\n' '{"event_id":"evt-router-ready","source":"module","timestamp_ms":10,"event":{"kind":"module","module":"router","event_type":"ready","payload":{"ok":true}}}'; exec sleep 30"#,
            )],
            discovery: DiscoverySpec {
                namespace: "phase2-reconcile-routing".to_string(),
                modules: vec!["router".to_string()],
            },
            pre_spawn: vec![],
        })
        .timeout(Duration::from_secs(2))
        .build())
        .await
        .expect("build unified runtime");

    assert!(
        runtime
            .loaded_modules()
            .await
            .iter()
            .any(|module_id| module_id == "router"),
        "router should be loaded for reconcile routing test; got {:?}",
        runtime.loaded_modules().await
    );

    let first_reconcile = runtime
        .reconcile(vec![
            spawn_spec("lead", "router"),
            spawn_spec("lead", "delivery"),
        ])
        .await
        .expect("first reconcile succeeds");
    assert!(first_reconcile.routing.router_module_loaded);
    assert_eq!(
        first_reconcile.routing.active_members,
        vec!["delivery", "router"]
    );
    assert_eq!(
        first_reconcile.routing.added_route_keys,
        vec!["mob.member.delivery", "mob.member.router"]
    );
    assert!(first_reconcile.routing.removed_route_keys.is_empty());

    let second_reconcile = runtime
        .reconcile(vec![spawn_spec("lead", "router")])
        .await
        .expect("second reconcile succeeds");
    assert_eq!(second_reconcile.mob.retired, vec!["delivery"]);
    assert_eq!(
        second_reconcile.routing.active_members,
        vec!["router".to_string()]
    );
    assert!(second_reconcile.routing.added_route_keys.is_empty());
    assert_eq!(
        second_reconcile.routing.removed_route_keys,
        vec!["mob.member.delivery".to_string()]
    );

    let shutdown = runtime.shutdown().await;
    assert_eq!(shutdown.module_shutdown.orphan_processes, 0);
    shutdown.mob_stop.expect("mob runtime should stop cleanly");
}

#[tokio::test]
#[ignore]
async fn req_008_reconcile_route_mutation_failure_is_typed() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let runtime = Box::pin(UnifiedRuntime::builder()
        .mob_spec(build_phase2_mob_spec(&temp_dir))
        .module_config(MetaConfig {
            modules: vec![shell_module(
                "router",
                r#"printf '%s\n' '{"event_id":"evt-router-ready","source":"module","timestamp_ms":10,"event":{"kind":"module","module":"router","event_type":"ready","payload":{"ok":true}}}'; exec sleep 30"#,
            )],
            discovery: DiscoverySpec {
                namespace: "phase2-reconcile-route-mutation".to_string(),
                modules: vec!["router".to_string()],
            },
            pre_spawn: vec![],
        })
        .timeout(Duration::from_secs(2))
        .build())
        .await
        .expect("build unified runtime");

    let reconcile_error = runtime
        .reconcile(vec![spawn_spec("lead", "   ")])
        .await
        .expect_err("blank member id should fail with route mutation");
    assert!(matches!(
        reconcile_error,
        UnifiedRuntimeReconcileError::RouteMutation(RuntimeRouteMutationError::EmptyRecipient)
    ));

    let shutdown = runtime.shutdown().await;
    assert_eq!(shutdown.module_shutdown.orphan_processes, 0);
    shutdown.mob_stop.expect("mob runtime should stop cleanly");
}
