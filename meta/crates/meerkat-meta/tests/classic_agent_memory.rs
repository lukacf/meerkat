//! A2 decouple: the BASIC agent-memory surface (recorder `memory` tool +
//! build-time injection + Memory panel store) works on the classic mob path
//! for any member keyed on its meerkat `AgentIdentity`, WITHOUT a roster
//! provider or an `IdentityRuntime`. The identity-first path (roster present)
//! is untouched — its own suites cover it.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
use meerkat_core::types::StopReason;
use meerkat_meta::memory::MemoryAuthor;
use meerkat_meta::{
    AgentMemoryConfig, AgentMemoryPerTurnInjection, AgentMemoryProvider, AgentMemorySelection,
    AuthPolicy, BigQueryNaming, ConsolePolicy, MemoryKind, MemoryScope, NewMemoryRecord,
    RuntimeDecisionInputs, RuntimeOpsPolicy, SqliteAgentMemoryStore, TrustedOidcRuntimeConfig,
    UnifiedRuntime, build_runtime_decision_state,
};
use meerkat_mob::{MobDefinition, SpawnMemberSpec};
use serde_json::{Value, json};
use tower::ServiceExt;

/// Per-test mob id counter: 0.8.23's fail-closed in-proc registration
/// means concurrently running tests must not share a supervisor route.
static NEXT_TEST_MOB_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Per-call mob id: 0.8.23's fail-closed in-proc registration means
/// concurrently running tests must not share a supervisor route.
fn classic_mob_toml() -> String {
    format!(
        r#"
[mob]
id = "classic-memory-mob-{}"

[profiles.worker]
model = "gpt-5.5"
runtime_mode = "autonomous_host"
external_addressable = true

[profiles.worker.tools]
comms = true
"#,
        NEXT_TEST_MOB_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}

fn test_definition() -> MobDefinition {
    MobDefinition::from_toml(&classic_mob_toml()).expect("parse test mob definition")
}

// A commander-like profile that ALSO enables meerkat's built-in tool
// categories (builtins/mob/mob_tasks) — the incident-pack commander shape.
// Guards that the recorder `memory` external tool still registers when those
// categories are on (they compose, they do not suppress it). Diagnoses the
// live finding that the commander reached for `task_create` (a mob_tasks tool)
// instead of `memory`: that is tool *competition*, not a missing recorder.
/// Per-call mob id: 0.8.23's fail-closed in-proc registration means
/// concurrently running tests must not share a supervisor route.
fn commander_like_mob_toml() -> String {
    format!(
        r#"
[mob]
id = "classic-memory-mob-{}"

[profiles.worker]
model = "gpt-5.5"
runtime_mode = "autonomous_host"
external_addressable = true

[profiles.worker.tools]
builtins = true
comms = true
mob = true
mob_tasks = true
"#,
        NEXT_TEST_MOB_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}

fn commander_like_definition() -> MobDefinition {
    MobDefinition::from_toml(&commander_like_mob_toml()).expect("parse commander-like definition")
}

/// LLM stub that records every request (tool names + full JSON) so tests can
/// assert on the member's build surface: registered tools and the system
/// instructions the memory customizer injected.
#[derive(Clone, Default)]
struct CaptureClient {
    requests: Arc<std::sync::Mutex<Vec<CapturedRequest>>>,
    notify: Arc<tokio::sync::Notify>,
}

#[derive(Clone)]
struct CapturedRequest {
    tool_names: Vec<String>,
    request_json: String,
}

impl CaptureClient {
    fn captured(&self) -> Vec<CapturedRequest> {
        self.requests.lock().unwrap().clone()
    }

    async fn wait_for_request(&self, minimum: usize) -> Vec<CapturedRequest> {
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                let captured = self.captured();
                if captured.len() >= minimum {
                    return captured;
                }
                self.notify.notified().await;
            }
        })
        .await
        .expect("member turn should reach the LLM client")
    }
}

impl LlmClient for CaptureClient {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        let captured = CapturedRequest {
            tool_names: request
                .tools
                .iter()
                .map(|tool| tool.name.to_string())
                .collect(),
            request_json: serde_json::to_string(request).unwrap_or_default(),
        };
        self.requests.lock().unwrap().push(captured);
        self.notify.notify_waiters();
        Box::pin(async_stream::stream! {
            yield Ok(LlmEvent::TextDelta {
                delta: "ok".to_string(),
                meta: None,
            });
            yield Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::success(StopReason::EndTurn),
            });
        })
    }

    fn provider(&self) -> meerkat::Provider {
        meerkat::Provider::OpenAI
    }

    fn health_check<'life0, 'async_trait>(
        &'life0 self,
    ) -> Pin<Box<dyn Future<Output = Result<(), LlmError>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async { Ok(()) })
    }
}

async fn seed_identity_record(
    store: &SqliteAgentMemoryStore,
    identity: &str,
    title: &str,
    body: &str,
) {
    store
        .remember_authored(
            &MemoryScope::Identity {
                realm: "default".to_string(),
                identity: identity.to_string(),
            },
            NewMemoryRecord {
                kind: MemoryKind::Fact,
                title: title.to_string(),
                description: title.to_string(),
                body: body.to_string(),
                tags: vec![],
                evidence: vec![],
                verification: None,
            },
            MemoryAuthor::Operator,
        )
        .await
        .expect("seed memory record");
}

async fn build_classic_runtime(
    store: SqliteAgentMemoryStore,
    config: AgentMemoryConfig,
    client: CaptureClient,
) -> UnifiedRuntime {
    Box::pin(
        UnifiedRuntime::builder()
            .definition(test_definition())
            .agent_memory(Arc::new(store), config)
            .default_llm_client(Arc::new(client))
            .build(),
    )
    .await
    .expect("classic runtime with agent memory and NO roster must build")
}

// ---------------------------------------------------------------------------
// Build succeeds without a roster; no IdentityRuntime is constructed; the
// panel store is auto-wired.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn classic_agent_memory_builds_without_roster_and_without_identity_runtime() {
    let memory_dir = tempfile::tempdir().expect("memory dir");
    let store = SqliteAgentMemoryStore::open(memory_dir.path()).expect("sqlite store");

    let runtime = build_classic_runtime(
        store,
        AgentMemoryConfig::default(),
        CaptureClient::default(),
    )
    .await;

    assert!(
        runtime.identity_runtime().is_none(),
        "classic memory must not construct an IdentityRuntime"
    );
    assert!(
        runtime.identity_first_context().is_none(),
        "classic memory must not construct an identity-first context"
    );
    assert!(
        runtime.memory_panel_store().is_some(),
        "bundled sqlite provider must auto-wire the Memory panel store"
    );
    runtime.mob_handle().stop().await.expect("stop");
}

// ---------------------------------------------------------------------------
// Member builds get the recorder tool + the build-time memory injection.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn classic_member_build_registers_recorder_and_injects_memory() {
    let memory_dir = tempfile::tempdir().expect("memory dir");
    let store = SqliteAgentMemoryStore::open(memory_dir.path()).expect("sqlite store");
    seed_identity_record(
        &store,
        "helper",
        "Deploy window",
        "Deploys are frozen on Fridays.",
    )
    .await;

    let client = CaptureClient::default();
    let runtime = build_classic_runtime(
        store,
        AgentMemoryConfig {
            selection: AgentMemorySelection::Always,
            ..AgentMemoryConfig::default()
        },
        client.clone(),
    )
    .await;

    runtime
        .spawn_many(vec![SpawnMemberSpec::from_wire(
            "worker".to_string(),
            "helper".to_string(),
            None,
            None,
            None,
        )])
        .await
        .expect("spawn member on the classic path");
    meerkat_meta::send_message_on_mob(&runtime.mob_handle(), "helper", "hello".to_string())
        .await
        .expect("send message");

    let captured = client.wait_for_request(1).await;
    let first = &captured[0];
    assert!(
        first.tool_names.iter().any(|name| name == "memory"),
        "recorder tool must be registered on the member build: {:?}",
        first.tool_names
    );
    assert!(
        first
            .request_json
            .contains("Deploys are frozen on Fridays."),
        "build-time injection must carry the seeded record body"
    );
    assert!(
        first.request_json.contains("Memory recorder protocol"),
        "recorder protocol instructions must be injected"
    );
    runtime.mob_handle().stop().await.expect("stop");
}

// ---------------------------------------------------------------------------
// Reconcile-spawned members ALSO get the recorder tool. The incident-command
// pack (and every roster-less console example) populates its roster via
// `reconcile`, not `spawn_many`; this guards that the customizer fires on the
// reconcile spawn path too (regression: the panel stayed empty because the
// memory tool never reached reconcile-spawned members).
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn classic_reconcile_spawned_member_registers_recorder() {
    use meerkat_mob::runtime::reconcile::ReconcileOptions;

    let memory_dir = tempfile::tempdir().expect("memory dir");
    let store = SqliteAgentMemoryStore::open(memory_dir.path()).expect("sqlite store");
    seed_identity_record(
        &store,
        "helper",
        "Deploy window",
        "Deploys are frozen on Fridays.",
    )
    .await;

    let client = CaptureClient::default();
    let runtime = build_classic_runtime(
        store,
        AgentMemoryConfig {
            selection: AgentMemorySelection::Always,
            ..AgentMemoryConfig::default()
        },
        client.clone(),
    )
    .await;

    runtime
        .mob_handle()
        .reconcile(
            vec![SpawnMemberSpec::from_wire(
                "worker".to_string(),
                "helper".to_string(),
                None,
                None,
                None,
            )],
            ReconcileOptions { retire_stale: true },
        )
        .await
        .expect("reconcile roster on the classic path");
    meerkat_meta::send_message_on_mob(&runtime.mob_handle(), "helper", "hello".to_string())
        .await
        .expect("send message");

    let captured = client.wait_for_request(1).await;
    let first = &captured[0];
    assert!(
        first.tool_names.iter().any(|name| name == "memory"),
        "recorder tool must be registered on the reconcile-spawned member: {:?}",
        first.tool_names
    );
    assert!(
        first
            .request_json
            .contains("Deploys are frozen on Fridays."),
        "build-time injection must carry the seeded record body on reconcile"
    );
    runtime.mob_handle().stop().await.expect("stop");
}

// ---------------------------------------------------------------------------
// The recorder `memory` tool survives on a member whose profile also enables
// meerkat's built-in tool categories (builtins/mob/mob_tasks) — the incident
// commander shape. Diagnoses the live finding that the commander called
// `task_create` (a mob_tasks tool) when asked to "remember": the recorder is
// present, so that is tool *competition* in the model's selection, not a
// missing/suppressed recorder.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn classic_recorder_survives_alongside_mob_tool_categories() {
    use meerkat_mob::runtime::reconcile::ReconcileOptions;

    let memory_dir = tempfile::tempdir().expect("memory dir");
    let store = SqliteAgentMemoryStore::open(memory_dir.path()).expect("sqlite store");

    let client = CaptureClient::default();
    let runtime = Box::pin(
        UnifiedRuntime::builder()
            .definition(commander_like_definition())
            .agent_memory(Arc::new(store), AgentMemoryConfig::default())
            .default_llm_client(Arc::new(client.clone()))
            .build(),
    )
    .await
    .expect("commander-like classic runtime with agent memory must build");

    runtime
        .mob_handle()
        .reconcile(
            vec![SpawnMemberSpec::from_wire(
                "worker".to_string(),
                "helper".to_string(),
                None,
                None,
                None,
            )],
            ReconcileOptions { retire_stale: true },
        )
        .await
        .expect("reconcile commander-like member");
    meerkat_meta::send_message_on_mob(&runtime.mob_handle(), "helper", "hello".to_string())
        .await
        .expect("send message");

    let captured = client.wait_for_request(1).await;
    let first = &captured[0];
    // Both tools are present in the same (large) surface: the recorder is NOT
    // suppressed by the mob categories — the live "commander used task_create
    // to remember" behavior is model mis-selection among the competing tools,
    // not a missing recorder. Guarding both documents that root cause.
    assert!(
        first.tool_names.iter().any(|name| name == "memory"),
        "recorder `memory` tool must register even when builtins/mob/mob_tasks are enabled: {:?}",
        first.tool_names
    );
    assert!(
        first.tool_names.iter().any(|name| name == "task_create"),
        "the mob_tasks `task_create` tool must also be present (it is what the recorder competes \
         with for \"remember\" instructions): {:?}",
        first.tool_names
    );
    runtime.mob_handle().stop().await.expect("stop");
}

// ---------------------------------------------------------------------------
// A spawn customizer the host set on its MobBootstrapSpec keeps running when
// the builder turns classic agent memory on: the host's customizer runs
// first, then the memory customizer.
// ---------------------------------------------------------------------------

/// Labels every member spawn the way a host spawn customizer may.
struct HostLabelCustomizer;

impl meerkat_mob::SpawnMemberCustomizer for HostLabelCustomizer {
    fn customize_spawn(
        &self,
        _ctx: &meerkat_mob::SpawnCustomizationContext,
        spec: &mut SpawnMemberSpec,
    ) -> Result<(), meerkat_mob::MobError> {
        spec.labels
            .get_or_insert_with(std::collections::BTreeMap::new)
            .insert("host_customizer".to_string(), "ran".to_string());
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn classic_memory_keeps_the_spawn_customizer_a_bootstrap_spec_carries() {
    let memory_dir = tempfile::tempdir().expect("memory dir");
    let store = SqliteAgentMemoryStore::open(memory_dir.path()).expect("sqlite store");
    let session_dir = tempfile::tempdir().expect("session dir");
    let client = CaptureClient::default();
    let spec = meerkat_meta::MobBootstrapSpec::ephemeral(
        test_definition(),
        meerkat_mob::MobStorage::in_memory(),
        session_dir.path().to_path_buf(),
        4,
        None,
    )
    .expect("ephemeral bootstrap spec")
    .with_options(meerkat_meta::MobBootstrapOptions {
        allow_ephemeral_sessions: true,
        notify_orchestrator_on_resume: true,
        default_llm_client: Some(Arc::new(client.clone())),
    })
    .with_spawn_member_customizer(Arc::new(HostLabelCustomizer));
    let runtime = Box::pin(
        UnifiedRuntime::builder()
            .mob_spec(spec)
            .module_config(meerkat_meta::MetaConfig {
                modules: Vec::new(),
                discovery: meerkat_meta::DiscoverySpec {
                    namespace: "classic-memory-host-customizer".to_string(),
                    modules: Vec::new(),
                },
                pre_spawn: Vec::new(),
            })
            .timeout(Duration::from_secs(5))
            .agent_memory(Arc::new(store), AgentMemoryConfig::default())
            .build(),
    )
    .await
    .expect("classic runtime with a host spawn customizer and agent memory must build");

    runtime
        .spawn_many(vec![SpawnMemberSpec::from_wire(
            "worker".to_string(),
            "helper".to_string(),
            None,
            None,
            None,
        )])
        .await
        .expect("spawn member on the classic path");
    let labels = runtime
        .mob_handle()
        .list_members()
        .await
        .into_iter()
        .find(|member| member.agent_identity.as_str() == "helper")
        .expect("the spawned member is listed")
        .labels;
    meerkat_meta::send_message_on_mob(&runtime.mob_handle(), "helper", "hello".to_string())
        .await
        .expect("send message");
    let captured = client.wait_for_request(1).await;
    runtime.mob_handle().stop().await.expect("stop");

    assert_eq!(
        labels.get("host_customizer").map(String::as_str),
        Some("ran"),
        "the host's spawn customizer ran on the member: {labels:?}"
    );
    assert!(
        captured[0].tool_names.iter().any(|name| name == "memory"),
        "the memory recorder tool is registered on the member: {:?}",
        captured[0].tool_names
    );
}

// ---------------------------------------------------------------------------
// Per-turn ambient injection is Off/no-op on the classic path: even the
// explicit `budgeted` opt-in must not prepend memory into delivered turns
// (there is no classic send-path hook yet — documented follow-up).
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn classic_per_turn_injection_is_a_noop_even_when_budgeted() {
    let memory_dir = tempfile::tempdir().expect("memory dir");
    let store = SqliteAgentMemoryStore::open(memory_dir.path()).expect("sqlite store");
    seed_identity_record(
        &store,
        "helper",
        "Deploy window",
        "Deploys are frozen on Fridays.",
    )
    .await;

    let client = CaptureClient::default();
    let runtime = build_classic_runtime(
        store,
        AgentMemoryConfig {
            per_turn_injection: AgentMemoryPerTurnInjection::Budgeted,
            ..AgentMemoryConfig::default()
        },
        client.clone(),
    )
    .await;

    runtime
        .spawn_many(vec![SpawnMemberSpec::from_wire(
            "worker".to_string(),
            "helper".to_string(),
            None,
            None,
            None,
        )])
        .await
        .expect("spawn member");
    meerkat_meta::send_message_on_mob(&runtime.mob_handle(), "helper", "what changed?".to_string())
        .await
        .expect("send message");

    let captured = client.wait_for_request(1).await;
    let latest = captured.last().expect("delivered turn");
    assert!(
        !latest.request_json.contains("<meta_memory_observation"),
        "per-turn memory envelope must not appear in classic-path turns"
    );
    runtime.mob_handle().stop().await.expect("stop");
}

// ---------------------------------------------------------------------------
// The console Memory panel RPC is serviceable on the classic path (the
// builder auto-wires the panel store; experience.memory follows from it).
// ---------------------------------------------------------------------------

fn decision_state() -> meerkat_meta::RuntimeDecisionState {
    build_runtime_decision_state(RuntimeDecisionInputs {
        bigquery: BigQueryNaming {
            dataset: "classic_memory_dataset".to_string(),
            table: "classic_memory_table".to_string(),
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
            email_allowlist: vec!["root@example.test".to_string()],
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
    .expect("decision state builds")
}

async fn rpc(app: &axum::Router, method: &str, params: Value) -> Value {
    let payload = json!({
        "jsonrpc": "2.0",
        "id": "test",
        "method": method,
        "params": params,
    });
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

#[tokio::test(flavor = "multi_thread")]
async fn classic_memory_panel_records_rpc_is_serviceable() {
    let memory_dir = tempfile::tempdir().expect("memory dir");
    let store = SqliteAgentMemoryStore::open(memory_dir.path()).expect("sqlite store");
    seed_identity_record(&store, "helper", "Panel fact", "Visible from the panel.").await;

    let runtime = build_classic_runtime(
        store,
        AgentMemoryConfig::default(),
        CaptureClient::default(),
    )
    .await;
    let app = runtime.build_reference_app_router(decision_state());

    let records = rpc(&app, "meta/memory/panel/records", json!({})).await;
    assert_eq!(records["error"], Value::Null, "{records:#?}");
    let rows = records["result"]["records"].as_array().expect("records");
    assert!(
        rows.iter().any(|row| row["title"] == json!("Panel fact")),
        "seeded record must be readable through the panel RPC: {rows:#?}"
    );

    // experience.memory affordance follows the wired panel store.
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/console/experience")
                .body(Body::empty())
                .expect("experience request"),
        )
        .await
        .expect("experience response");
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("experience body");
    let experience: Value = serde_json::from_slice(&body).expect("experience json");
    assert_eq!(
        experience["memory"]["available"],
        json!(true),
        "experience.memory must be available on the classic path: {experience:#?}"
    );
    runtime.mob_handle().stop().await.expect("stop");
}

// Control experiment: turn reaches LLM without memory configured.
#[tokio::test(flavor = "multi_thread")]
async fn control_member_turn_reaches_llm_without_memory() {
    let client = CaptureClient::default();
    let runtime = Box::pin(
        UnifiedRuntime::builder()
            .definition(test_definition())
            .default_llm_client(Arc::new(client.clone()))
            .build(),
    )
    .await
    .expect("classic runtime without memory");
    runtime
        .spawn_many(vec![SpawnMemberSpec::from_wire(
            "worker".to_string(),
            "helper".to_string(),
            None,
            None,
            None,
        )])
        .await
        .expect("spawn member");
    meerkat_meta::send_message_on_mob(&runtime.mob_handle(), "helper", "hello".to_string())
        .await
        .expect("send message");
    client.wait_for_request(1).await;
    runtime.mob_handle().stop().await.expect("stop");
}

// ---------------------------------------------------------------------------
// Gate: genuinely-invalid combinations still error.
// ---------------------------------------------------------------------------

async fn build_err(builder: meerkat_meta::UnifiedRuntimeBuilder) -> String {
    match Box::pin(builder.build()).await {
        Ok(_) => panic!("build should have failed"),
        Err(err) => err.to_string(),
    }
}

/// The full-stack builder path (production deployment shape): store + firewall +
/// judgment engines assembled from `UnifiedRuntimeBuilder` alone — no
/// gateway. Asserts the panel store registers (proving the SQLite stack, not
/// the markdown fallback, is live) and that the recorder-facing provider
/// writes into it.
#[tokio::test(flavor = "multi_thread")]
async fn builder_full_memory_stack_installs_engines_and_panel() {
    let dir = tempfile::tempdir().expect("state dir");
    let engines = meerkat_meta::memory_wiring::MemoryEnginesConfig {
        steward: meerkat_meta::memory::steward::StewardConfig {
            enabled: true,
            ..Default::default()
        },
        distiller: meerkat_meta::memory::distiller::DistillerConfig {
            enabled: true,
            ..Default::default()
        },
    };
    let runtime = UnifiedRuntime::builder()
        .definition(test_definition())
        .persistent_state(dir.path().join("state"))
        .persistent_agent_memory_stack(AgentMemoryConfig::default(), engines)
        .build()
        .await
        .expect("build full memory stack");

    let store = runtime
        .memory_panel_store()
        .expect("panel store registered by the builder stack path");
    // The provider is the same bundled store: a write through it is visible
    // via the panel handle (recorder-path smoke).
    store
        .remember_authored(
            &meerkat_meta::memory::records::MemoryScope::Identity {
                realm: "default".to_string(),
                identity: "worker:one".to_string(),
            },
            meerkat_meta::memory::records::NewMemoryRecord {
                kind: meerkat_meta::memory::records::MemoryKind::Fact,
                title: "builder stack fact".to_string(),
                description: String::new(),
                body: "written through the builder-assembled stack".to_string(),
                tags: Vec::new(),
                evidence: Vec::new(),
                verification: None,
            },
            meerkat_meta::memory::records::MemoryAuthor::Operator,
        )
        .await
        .expect("write through the stack store");
    runtime.shutdown().await;

    // Requires persistent_state, loudly.
    let err = build_err(
        UnifiedRuntime::builder()
            .definition(test_definition())
            .persistent_agent_memory_stack(
                AgentMemoryConfig::default(),
                meerkat_meta::memory_wiring::MemoryEnginesConfig::default(),
            ),
    )
    .await;
    assert!(err.contains("requires persistent_state"), "{err}");
}

#[tokio::test]
async fn gate_still_rejects_invalid_memory_combinations() {
    // Identity-first inputs (a custom AgentCustomizer) still require the
    // roster even when memory is configured.
    struct NoopCustomizer;
    #[async_trait::async_trait]
    impl meerkat_meta::identity_first::contracts::AgentCustomizer for NoopCustomizer {
        async fn customize_build(
            &self,
            _context: &meerkat_meta::identity_first::AgentBuildContext,
            _spec: &meerkat_meta::identity_first::DurableAgentSpec,
            _draft: &mut meerkat_meta::identity_first::AgentBuildDraft,
        ) -> Result<(), meerkat_meta::identity_first::CustomizerError> {
            Ok(())
        }
    }
    let memory_dir2 = tempfile::tempdir().expect("memory dir");
    let store2 = SqliteAgentMemoryStore::open(memory_dir2.path()).expect("sqlite store");
    let err = build_err(
        UnifiedRuntime::builder()
            .definition(test_definition())
            .agent_customizer(Arc::new(NoopCustomizer))
            .agent_memory(Arc::new(store2), AgentMemoryConfig::default()),
    )
    .await;
    assert!(err.contains("roster_provider"), "{err}");
}
