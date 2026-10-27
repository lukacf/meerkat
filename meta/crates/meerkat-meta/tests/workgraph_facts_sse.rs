#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use std::time::Duration;

use axum::body::{Body, BodyDataStream};
use axum::http::{Request, StatusCode};
use chrono::{DateTime, Utc};
use futures::StreamExt;
use meerkat::{
    AgentFactory, AttentionListRequest, Config, CreateWorkItemRequest, WorkAttentionBinding,
    WorkEdge, WorkGraphError, WorkGraphEvent, WorkGraphEventFilter, WorkGraphFact,
    WorkGraphService, WorkGraphStore, WorkGraphStoreKind, WorkItem, WorkItemFilter, WorkItemId,
    WorkNamespace, build_ephemeral_service,
};
use meerkat_client::TestClient;
use meerkat_meta::access::{
    ACTION_WORKGRAPH_VIEW, AccessControlConfig, AccessController, AccessRule,
};
use meerkat_meta::http_sse::{WORKGRAPH_FACTS_STREAM_PATH, workgraph_facts_sse_router};
use meerkat_meta::workgraph_events::{
    WORKGRAPH_FACT_EVENT_TYPE, WORKGRAPH_RESYNC_REQUIRED_EVENT_TYPE, WorkGraphFactEnvelope,
    WorkGraphFactHub, WorkGraphFactTailOptions, spawn_workgraph_fact_tail,
};
use meerkat_meta::{
    AuthPolicy, AuthProvider, BigQueryNaming, ConsolePolicy, DiscoverySpec, MetaConfig,
    MobBootstrapOptions, MobBootstrapSpec, RuntimeDecisionInputs, RuntimeDecisionState,
    RuntimeOpsPolicy, TrustedOidcRuntimeConfig, UnifiedRuntime, build_runtime_decision_state,
};
use meerkat_mob::{MobDefinition, MobStorage};
use serde_json::{Value, json};
use tower::ServiceExt;

#[derive(Default)]
struct CountingReadStore {
    frontier_reads: AtomicUsize,
    page_reads: AtomicUsize,
    frontier_seq: AtomicI64,
    last_page_after_seq: AtomicI64,
    block_frontier: AtomicBool,
    frontier_entered: tokio::sync::Notify,
    frontier_release: tokio::sync::Notify,
}

fn unused_store_operation<T>() -> Result<T, WorkGraphError> {
    Err(WorkGraphError::UnsupportedBackend(
        "counting-read-test-store".to_string(),
    ))
}

#[async_trait::async_trait]
impl WorkGraphStore for CountingReadStore {
    fn kind(&self) -> WorkGraphStoreKind {
        WorkGraphStoreKind::Memory
    }

    async fn get_store_time_utc(&self) -> Result<DateTime<Utc>, WorkGraphError> {
        unused_store_operation()
    }

    async fn insert_item(
        &self,
        _item: WorkItem,
        _event: WorkGraphEvent,
    ) -> Result<WorkItem, WorkGraphError> {
        unused_store_operation()
    }

    async fn update_item_cas(
        &self,
        _item: WorkItem,
        _expected_previous_revision: u64,
        _event: WorkGraphEvent,
    ) -> Result<WorkItem, WorkGraphError> {
        unused_store_operation()
    }

    async fn update_item_and_attention_cas(
        &self,
        _item: WorkItem,
        _expected_previous_revision: u64,
        _item_event: WorkGraphEvent,
        _attention_updates: Vec<(WorkAttentionBinding, u64, WorkGraphEvent)>,
    ) -> Result<WorkItem, WorkGraphError> {
        unused_store_operation()
    }

    async fn get_item(
        &self,
        _realm_id: &str,
        _namespace: &WorkNamespace,
        _id: &WorkItemId,
    ) -> Result<Option<WorkItem>, WorkGraphError> {
        unused_store_operation()
    }

    async fn list_items(&self, _filter: WorkItemFilter) -> Result<Vec<WorkItem>, WorkGraphError> {
        unused_store_operation()
    }

    async fn list_attention_matching_bounded(
        &self,
        _filter: AttentionListRequest,
        _observed_at: DateTime<Utc>,
        _limit: usize,
    ) -> Result<Vec<WorkAttentionBinding>, WorkGraphError> {
        unused_store_operation()
    }

    async fn insert_edge(
        &self,
        _edge: WorkEdge,
        _event: WorkGraphEvent,
    ) -> Result<WorkEdge, WorkGraphError> {
        unused_store_operation()
    }

    async fn list_edges(
        &self,
        _realm_id: &str,
        _namespace: &WorkNamespace,
    ) -> Result<Vec<WorkEdge>, WorkGraphError> {
        unused_store_operation()
    }

    async fn list_events(
        &self,
        filter: WorkGraphEventFilter,
    ) -> Result<Vec<WorkGraphEvent>, WorkGraphError> {
        self.page_reads.fetch_add(1, Ordering::SeqCst);
        self.last_page_after_seq
            .store(filter.after_seq.unwrap_or(0), Ordering::SeqCst);
        Ok(Vec::new())
    }

    async fn latest_event_seq(
        &self,
        _filter: WorkGraphEventFilter,
    ) -> Result<Option<i64>, WorkGraphError> {
        self.frontier_reads.fetch_add(1, Ordering::SeqCst);
        if self.block_frontier.load(Ordering::SeqCst) {
            self.frontier_entered.notify_one();
            self.frontier_release.notified().await;
        }
        let frontier = self.frontier_seq.load(Ordering::SeqCst);
        Ok((frontier > 0).then_some(frontier))
    }
}

fn require_auth_decisions() -> RuntimeDecisionState {
    build_runtime_decision_state(RuntimeDecisionInputs {
        bigquery: BigQueryNaming {
            dataset: "workgraph_facts_sse".to_string(),
            table: "events".to_string(),
        },
        trusted_meta_toml: "modules = []\n".to_string(),
        auth: AuthPolicy {
            default_provider: AuthProvider::GoogleOAuth,
            email_allowlist: vec![],
        },
        trusted_oidc: TrustedOidcRuntimeConfig {
            discovery_json:
                r#"{"issuer":"https://trusted.meta.local","jwks_uri":"https://trusted.meta.local/.well-known/jwks.json"}"#
                    .to_string(),
            jwks_json:
                r#"{"keys":[{"kid":"kid-current","kty":"oct","alg":"HS256","k":"cGhhc2U3LXRydXN0ZWQtY3VycmVudC1zZWNyZXQ"}]}"#
                    .to_string(),
            audience: "meerkat-console".to_string(),
            require_verified_email: false,
        },
        console: ConsolePolicy {
            require_app_auth: true,
            ..ConsolePolicy::default()
        },
        ops: RuntimeOpsPolicy::default(),
        release_metadata_json: include_str!("../assets/release-targets.json").to_string(),
    })
    .expect("decision state")
}

fn workgraph_access(allow: bool) -> AccessController {
    AccessController::new(AccessControlConfig {
        enabled: true,
        admins: vec!["admin@example.test".to_string()],
        rules: if allow {
            vec![AccessRule {
                id: "workgraph-view".to_string(),
                actions: vec![ACTION_WORKGRAPH_VIEW.to_string()],
                ..AccessRule::default()
            }]
        } else {
            Vec::new()
        },
        ..AccessControlConfig::default()
    })
    .expect("access controller")
}

fn fact(seq: i64, item: &str) -> WorkGraphFactEnvelope {
    let item_id = WorkItemId::new(item).expect("item id");
    WorkGraphFactEnvelope {
        seq,
        realm_id: "mob.workgraph-facts-sse".to_string(),
        namespace: WorkNamespace::default(),
        item_id: Some(item_id.clone()),
        fact: WorkGraphFact::ItemReady {
            item_id,
            item_revision: 1,
        },
    }
}

fn mob_spec(
    temp_dir: &tempfile::TempDir,
    workgraph: Option<meerkat::WorkGraphService>,
) -> MobBootstrapSpec {
    let session_path = temp_dir.path().join("sessions");
    std::fs::create_dir_all(&session_path).expect("session path");
    let session_service = Arc::new(build_ephemeral_service(
        AgentFactory::new(&session_path).comms(true),
        Config::default(),
        8,
    ));
    let definition = MobDefinition::from_toml(
        r#"
[mob]
id = "workgraph-facts-sse-runtime"

[profiles.lead]
model = "gpt-5.5"

[profiles.lead.tools]
comms = true
"#,
    )
    .expect("mob definition");
    MobBootstrapSpec::new(definition, MobStorage::in_memory(), session_service)
        .with_options(MobBootstrapOptions {
            allow_ephemeral_sessions: true,
            notify_orchestrator_on_resume: true,
            default_llm_client: Some(Arc::new(TestClient::default())),
        })
        .with_workgraph_service(workgraph)
}

fn module_config() -> MetaConfig {
    MetaConfig {
        modules: Vec::new(),
        discovery: DiscoverySpec {
            namespace: "workgraph-facts-sse".to_string(),
            modules: Vec::new(),
        },
        pre_spawn: Vec::new(),
    }
}

async fn next_sse_frame(body: &mut BodyDataStream) -> String {
    let bytes = body
        .next()
        .await
        .expect("SSE frame")
        .expect("SSE body frame");
    String::from_utf8(bytes.to_vec()).expect("UTF-8 SSE")
}

fn data_json(frame: &str) -> Value {
    let data = frame
        .lines()
        .find_map(|line| line.strip_prefix("data:").map(str::trim))
        .expect("SSE data line");
    serde_json::from_str(data).expect("SSE JSON")
}

#[tokio::test]
async fn initial_sync_precedes_facts_and_sse_never_claims_replay_authority() {
    let hub = WorkGraphFactHub::with_capacity(8);
    let app = workgraph_facts_sse_router(hub.clone(), None, None);
    let response = app
        .oneshot(
            Request::builder()
                .uri(WORKGRAPH_FACTS_STREAM_PATH)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body().into_data_stream();

    let initial = next_sse_frame(&mut body).await;
    assert!(initial.lines().any(|line| {
        line.strip_prefix("event:").map(str::trim) == Some("workgraph.resync_required")
    }));
    assert!(!initial.lines().any(|line| line.starts_with("id:")));
    let initial = data_json(&initial);
    assert_eq!(initial["kind"], "module");
    assert_eq!(initial["module"], "meta.workgraph");
    assert_eq!(initial["event_type"], WORKGRAPH_RESYNC_REQUIRED_EVENT_TYPE);
    assert_eq!(initial["payload"]["reason"], "initial_sync");
    assert_eq!(initial["payload"]["authority"], "durable_workgraph_pull");

    assert!(hub.publish_fact(&fact(41, "ready-after-sync")));
    let wake = next_sse_frame(&mut body).await;
    assert!(
        wake.lines()
            .any(|line| line.strip_prefix("event:").map(str::trim) == Some("workgraph.fact"))
    );
    assert!(!wake.lines().any(|line| line.starts_with("id:")));
    let wake = data_json(&wake);
    assert_eq!(wake["kind"], "module");
    assert_eq!(wake["module"], "meta.workgraph");
    assert_eq!(wake["event_type"], WORKGRAPH_FACT_EVENT_TYPE);
    assert_eq!(wake["payload"]["seq"], 41);
    for forbidden in ["status", "owner", "claim", "evidence", "title"] {
        assert!(wake["payload"].get(forbidden).is_none());
    }
}

#[tokio::test]
async fn activation_window_transition_is_absorbed_by_frontier_after_initial_sync() {
    let store = Arc::new(CountingReadStore::default());
    store.block_frontier.store(true, Ordering::SeqCst);
    let hub = WorkGraphFactHub::new();
    let task = spawn_workgraph_fact_tail(
        WorkGraphService::new(store.clone()),
        hub.clone(),
        WorkGraphFactTailOptions {
            poll_interval: Duration::from_secs(1),
            page_limit: 32,
        },
    );
    let response = workgraph_facts_sse_router(hub.clone(), None, None)
        .oneshot(
            Request::builder()
                .uri(WORKGRAPH_FACTS_STREAM_PATH)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    let mut body = response.into_body().into_data_stream();

    store.frontier_entered.notified().await;
    let initial = next_sse_frame(&mut body).await;
    assert_eq!(data_json(&initial)["payload"]["reason"], "initial_sync");

    // Model a durable transition after initial_sync delivery but before the
    // tail's frontier read completes. The tail deliberately absorbs this
    // sequence into its starting cursor instead of claiming lossless replay.
    store.frontier_seq.store(7, Ordering::SeqCst);
    store.frontier_release.notify_one();
    hub.wait_for_tail_ready().await;
    assert_eq!(store.last_page_after_seq.load(Ordering::SeqCst), 7);

    task.abort();
    let _ = task.await;
}

#[tokio::test]
async fn broadcast_lag_becomes_an_explicit_module_resync_event() {
    let hub = WorkGraphFactHub::with_capacity(1);
    let app = workgraph_facts_sse_router(hub.clone(), None, None);
    let response = app
        .oneshot(
            Request::builder()
                .uri(WORKGRAPH_FACTS_STREAM_PATH)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    assert!(hub.publish_fact(&fact(1, "first")));
    assert!(hub.publish_fact(&fact(2, "second")));

    let mut body = response.into_body().into_data_stream();
    let initial = next_sse_frame(&mut body).await;
    assert_eq!(data_json(&initial)["payload"]["reason"], "initial_sync");
    let lagged = next_sse_frame(&mut body).await;
    let lagged = data_json(&lagged);
    assert_eq!(lagged["kind"], "module");
    assert_eq!(lagged["module"], "meta.workgraph");
    assert_eq!(lagged["event_type"], WORKGRAPH_RESYNC_REQUIRED_EVENT_TYPE);
    assert_eq!(lagged["payload"]["reason"], "lagged");
    assert_eq!(lagged["payload"]["skipped"], 1);
}

#[tokio::test]
async fn route_uses_shared_app_auth_before_subscribing() {
    let hub = WorkGraphFactHub::new();
    let app = workgraph_facts_sse_router(hub.clone(), Some(require_auth_decisions()), None);
    let response = app
        .oneshot(
            Request::builder()
                .uri(WORKGRAPH_FACTS_STREAM_PATH)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(hub.receiver_count(), 0, "auth refusal precedes subscribe");
}

#[tokio::test]
async fn route_requires_workgraph_view_abac_before_subscribing() {
    let denied_hub = WorkGraphFactHub::new();
    let denied =
        workgraph_facts_sse_router(denied_hub.clone(), None, Some(workgraph_access(false)))
            .oneshot(
                Request::builder()
                    .uri(WORKGRAPH_FACTS_STREAM_PATH)
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("denied response");
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    assert_eq!(denied_hub.receiver_count(), 0);

    let allowed =
        workgraph_facts_sse_router(WorkGraphFactHub::new(), None, Some(workgraph_access(true)))
            .oneshot(
                Request::builder()
                    .uri(WORKGRAPH_FACTS_STREAM_PATH)
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("allowed response");
    assert_eq!(allowed.status(), StatusCode::OK);
}

#[tokio::test]
async fn open_stream_ends_when_the_committed_policy_drops_workgraph_view() {
    let access = workgraph_access(true);
    let hub = WorkGraphFactHub::with_capacity(8);
    let app = workgraph_facts_sse_router(hub.clone(), None, Some(access.clone()));
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(WORKGRAPH_FACTS_STREAM_PATH)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body().into_data_stream();
    let initial = next_sse_frame(&mut body).await;
    assert_eq!(data_json(&initial)["payload"]["reason"], "initial_sync");
    assert!(hub.publish_fact(&fact(1, "before-change")));
    assert_eq!(
        data_json(&next_sse_frame(&mut body).await)["payload"]["seq"],
        1
    );

    access
        .delete_rule("workgraph-view")
        .expect("commit the change");
    // The stream still holds its subscription, so this fact is queued on it.
    assert!(hub.publish_fact(&fact(2, "after-change")));
    let next = tokio::time::timeout(Duration::from_secs(20), body.next())
        .await
        .expect("the stream ends");
    assert!(
        next.is_none(),
        "a frame sent after the committed change: {:?}",
        next.map(|frame| frame.map(|bytes| String::from_utf8_lossy(&bytes).into_owned()))
    );
    // A reconnect is decided under the committed policy as well.
    let reconnect = app
        .oneshot(
            Request::builder()
                .uri(WORKGRAPH_FACTS_STREAM_PATH)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("reconnect response");
    assert_eq!(reconnect.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn zero_subscribers_perform_zero_frontier_and_page_reads() {
    let store = Arc::new(CountingReadStore::default());
    let service = WorkGraphService::new(store.clone());
    let hub = WorkGraphFactHub::new();
    let task = spawn_workgraph_fact_tail(
        service,
        hub.clone(),
        WorkGraphFactTailOptions {
            poll_interval: Duration::from_secs(1),
            page_limit: 32,
        },
    );

    tokio::task::yield_now().await;
    assert_eq!(store.frontier_reads.load(Ordering::SeqCst), 0);
    assert_eq!(store.page_reads.load(Ordering::SeqCst), 0);

    let _receiver = hub.subscribe();
    hub.wait_for_tail_ready().await;
    assert_eq!(store.frontier_reads.load(Ordering::SeqCst), 1);
    assert_eq!(store.page_reads.load(Ordering::SeqCst), 1);

    task.abort();
    let _ = task.await;
}

#[tokio::test(start_paused = true)]
async fn tail_is_idle_until_notified_then_discovers_cursor_before_live_facts() {
    let service =
        meerkat_meta::workgraph_wiring::ephemeral_workgraph_service("workgraph-facts-sse");
    service
        .create(CreateWorkItemRequest {
            title: "historical before any subscriber".to_string(),
            ..Default::default()
        })
        .await
        .expect("historical item");

    let hub = WorkGraphFactHub::with_capacity(8);
    let task = spawn_workgraph_fact_tail(
        service.clone(),
        hub.clone(),
        WorkGraphFactTailOptions {
            poll_interval: Duration::from_secs(1),
            page_limit: 32,
        },
    );
    tokio::task::yield_now().await;
    assert_eq!(hub.receiver_count(), 0);

    let mut receiver = hub.subscribe();
    hub.wait_for_tail_ready().await;
    assert!(
        receiver.try_recv().is_err(),
        "cursor discovery must discard historical wake facts"
    );

    let live = service
        .create(CreateWorkItemRequest {
            title: "live after cursor discovery".to_string(),
            ..Default::default()
        })
        .await
        .expect("live item");
    tokio::time::advance(Duration::from_secs(1)).await;
    tokio::task::yield_now().await;
    let event = receiver.recv().await.expect("live fact event");
    let meerkat_meta::UnifiedEvent::Module(module) = event else {
        panic!("WorkGraph tail emitted a non-module event");
    };
    assert_eq!(module.module, "meta.workgraph");
    assert_eq!(module.event_type, WORKGRAPH_FACT_EVENT_TYPE);
    assert_eq!(module.payload["item_id"], json!(live.id));

    task.abort();
    let _ = task.await;
}

#[tokio::test(start_paused = true)]
async fn unified_runtime_owns_tail_only_with_workgraph_and_shutdown_stops_it() {
    let workgraph =
        meerkat_meta::workgraph_wiring::ephemeral_workgraph_service("workgraph-facts-sse-runtime");
    let configured_dir = tempfile::tempdir().expect("configured temp dir");
    let configured = UnifiedRuntime::bootstrap(
        mob_spec(&configured_dir, Some(workgraph.clone())),
        module_config(),
        Duration::from_secs(2),
    )
    .await
    .expect("configured runtime");
    let hub = configured
        .workgraph_fact_hub()
        .expect("WorkGraph runtime owns one fact hub");
    let mut receiver = hub.subscribe();
    hub.wait_for_tail_ready().await;
    configured.shutdown().await;

    workgraph
        .create(CreateWorkItemRequest {
            title: "after runtime shutdown".to_string(),
            ..Default::default()
        })
        .await
        .expect("post-shutdown WorkGraph mutation");
    tokio::time::advance(Duration::from_secs(10)).await;
    tokio::task::yield_now().await;
    assert!(
        receiver.try_recv().is_err(),
        "joined runtime tail must not project facts after shutdown",
    );

    let absent_dir = tempfile::tempdir().expect("absent temp dir");
    let absent = UnifiedRuntime::bootstrap(
        mob_spec(&absent_dir, None),
        module_config(),
        Duration::from_secs(2),
    )
    .await
    .expect("runtime without WorkGraph");
    assert!(
        absent.workgraph_fact_hub().is_none(),
        "no WorkGraph means no hub and no tail",
    );
    absent.shutdown().await;
}
