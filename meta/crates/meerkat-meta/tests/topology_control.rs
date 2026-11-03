#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::uninlined_format_args
)]

use std::collections::BTreeMap;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use futures::stream;
use meerkat::{AgentFactory, Config, build_ephemeral_service};
use meerkat_client::{LlmClient, LlmError, LlmEvent, LlmRequest, TestClient};
use meerkat_comms::{InprocRegistry, PeerMeta};
use meerkat_core::types::HandlingMode;
use meerkat_core::{Message, Provider, StopReason};
use meerkat_meta::{
    AccessControlConfig, AccessController, AccessRule, DesiredPeerEdge, DiscoverySpec,
    EdgeDiscovery, MetaConfig, MobBootstrapOptions, MobBootstrapSpec,
    SameProcessTopologyCoordinator, TopologyAction, TopologyApplyRequest,
    TopologyBilateralApplyRequest, TopologyBilateralPlanRequest, TopologyBootstrapConfig,
    TopologyControlMode, TopologyControlPolicy, TopologyEdge, TopologyEndpoint, TopologyMutation,
    TopologyOperationStatus, UnifiedRuntime,
};
use meerkat_mob::{
    MobDefinition, MobRuntimeMode, MobStorage, SpawnMemberSpec, ids::AgentIdentity as MeerkatId,
};

/// The one definition of the normalized-provider-accounting contract every
/// Meta LLM double must satisfy under meerkat 0.8.22. See the module docs.
#[path = "support/llm_usage.rs"]
mod llm_usage;

static TOPOLOGY_TEST_LOCK: LazyLock<tokio::sync::Mutex<()>> =
    LazyLock::new(|| tokio::sync::Mutex::new(()));

fn editable_policy() -> TopologyControlPolicy {
    TopologyControlPolicy {
        mode: TopologyControlMode::Editable,
        allow_bulk: true,
        max_batch_size: 8,
        allow_cross_authority: true,
        receipt_limit: 8,
        idempotency_history_limit: 32,
    }
}

fn topology_access(actions: &[&str]) -> AccessController {
    AccessController::new(AccessControlConfig {
        enabled: true,
        admins: vec!["root".to_string()],
        rules: vec![AccessRule {
            id: "desktop-topology".to_string(),
            subjects: vec!["desktop-host".to_string()],
            actions: actions.iter().map(|action| (*action).to_string()).collect(),
            agents: vec!["*".to_string()],
            ..AccessRule::default()
        }],
        ..AccessControlConfig::default()
    })
    .expect("topology access controller")
}

/// `TestClient::default()` DOES synthesize accounting under 0.8.22, but under
/// `Provider::Other` - and every profile here is `gpt-5.5`, whose canonical
/// owner is `Provider::OpenAI`, so the turn would fail closed with
/// `normalized_provider_accounting_identity_mismatch`. See the rule in
/// `tests/support/llm_usage.rs`.
async fn build_runtime(root: &Path, authority: &str, member: &str) -> UnifiedRuntime {
    build_runtime_with_client(
        root,
        authority,
        member,
        Arc::new(TestClient::for_provider(Provider::OpenAI)),
    )
    .await
}

async fn build_runtime_with_client(
    root: &Path,
    authority: &str,
    member: &str,
    client: Arc<dyn LlmClient>,
) -> UnifiedRuntime {
    build_runtime_with_client_and_policy(root, authority, member, client, editable_policy()).await
}

async fn build_runtime_with_client_and_policy(
    root: &Path,
    authority: &str,
    member: &str,
    client: Arc<dyn LlmClient>,
    policy: TopologyControlPolicy,
) -> UnifiedRuntime {
    let session_path = root.join(format!("{authority}-sessions"));
    std::fs::create_dir_all(&session_path).expect("session path");
    let factory = AgentFactory::new(&session_path).comms(true);
    let session_service = Arc::new(build_ephemeral_service(factory, Config::default(), 8));
    let definition = MobDefinition::from_toml(&format!(
        r#"
[mob]
id = "{authority}"

[profiles.worker]
model = "gpt-5.5"
external_addressable = true

[profiles.worker.tools]
comms = true
"#
    ))
    .expect("definition");
    let mob_spec = MobBootstrapSpec::new(definition, MobStorage::in_memory(), session_service)
        .with_options(MobBootstrapOptions {
            allow_ephemeral_sessions: true,
            notify_orchestrator_on_resume: true,
            default_llm_client: Some(client),
        });
    let module_config = MetaConfig {
        modules: vec![],
        discovery: DiscoverySpec {
            namespace: format!("topology-{authority}"),
            modules: vec![],
        },
        pre_spawn: vec![],
    };
    let runtime = UnifiedRuntime::bootstrap_with_topology(
        mob_spec,
        module_config,
        Duration::from_secs(2),
        TopologyBootstrapConfig {
            policy,
            state_path: Some(root.join(format!("{authority}-topology.json"))),
        },
    )
    .await
    .expect("bootstrap topology runtime");
    runtime
        .spawn(
            SpawnMemberSpec::from_wire(
                "worker".to_string(),
                MeerkatId::from(member).to_string(),
                None,
                Some(MobRuntimeMode::TurnDriven),
                None,
            )
            .with_additional_instructions(vec![format!("You are {member}.")]),
        )
        .await
        .expect("spawn topology member");
    runtime
}

#[derive(Clone)]
struct BlockingEdgeDiscovery {
    state: Arc<BlockingEdgeDiscoveryState>,
}

struct BlockingEdgeDiscoveryState {
    armed: AtomicBool,
    entered: tokio::sync::Semaphore,
    release: tokio::sync::Semaphore,
}

impl BlockingEdgeDiscovery {
    fn new() -> Self {
        Self {
            state: Arc::new(BlockingEdgeDiscoveryState {
                armed: AtomicBool::new(false),
                entered: tokio::sync::Semaphore::new(0),
                release: tokio::sync::Semaphore::new(0),
            }),
        }
    }

    fn arm(&self) {
        self.state.armed.store(true, Ordering::SeqCst);
    }

    async fn wait_until_entered(&self) {
        tokio::time::timeout(Duration::from_secs(3), self.state.entered.acquire())
            .await
            .expect("blocking discovery was not entered")
            .expect("blocking discovery semaphore closed")
            .forget();
    }
}

impl BlockingEdgeDiscovery {
    fn release(&self) {
        self.state.release.add_permits(1);
    }
}

/// Counts every discovery and, while gated, parks each one until released, so
/// a test can hold an edge discovery in flight and count how many ran.
#[derive(Clone)]
struct CountingGatedEdgeDiscovery {
    state: Arc<CountingGatedEdgeDiscoveryState>,
}

impl Default for CountingGatedEdgeDiscovery {
    fn default() -> Self {
        Self {
            state: Arc::new(CountingGatedEdgeDiscoveryState {
                gated: AtomicBool::new(false),
                calls: AtomicUsize::new(0),
                entered: tokio::sync::Semaphore::new(0),
                release: tokio::sync::Semaphore::new(0),
            }),
        }
    }
}

struct CountingGatedEdgeDiscoveryState {
    gated: AtomicBool,
    calls: AtomicUsize,
    entered: tokio::sync::Semaphore,
    release: tokio::sync::Semaphore,
}

impl CountingGatedEdgeDiscovery {
    fn gate(&self) {
        self.state.gated.store(true, Ordering::SeqCst);
    }

    fn open(&self) {
        self.state.gated.store(false, Ordering::SeqCst);
        self.state.release.add_permits(64);
    }

    fn calls(&self) -> usize {
        self.state.calls.load(Ordering::SeqCst)
    }

    async fn wait_until_entered(&self) {
        tokio::time::timeout(Duration::from_secs(3), self.state.entered.acquire())
            .await
            .expect("gated discovery was not entered")
            .expect("gated discovery semaphore closed")
            .forget();
    }
}

impl EdgeDiscovery for CountingGatedEdgeDiscovery {
    fn discover_edges(
        &self,
        _active_members: Vec<meerkat_meta::unified_runtime::edge_types::EdgeMemberView>,
    ) -> Pin<Box<dyn Future<Output = Vec<DesiredPeerEdge>> + Send + '_>> {
        Box::pin(async move {
            self.state.calls.fetch_add(1, Ordering::SeqCst);
            if self.state.gated.load(Ordering::SeqCst) {
                self.state.entered.add_permits(1);
                self.state
                    .release
                    .acquire()
                    .await
                    .expect("gated discovery release semaphore closed")
                    .forget();
            }
            Vec::new()
        })
    }
}

fn connect_alice_bob(expected_revision: u64, idempotency_key: &str) -> TopologyApplyRequest {
    TopologyApplyRequest {
        expected_revision,
        idempotency_key: idempotency_key.to_string(),
        operations: vec![TopologyMutation {
            action: TopologyAction::Connect,
            edge: TopologyEdge::new(
                TopologyEndpoint::local("alice"),
                TopologyEndpoint::local("bob"),
            )
            .expect("local edge"),
        }],
        reason: Some("topology query concurrency acceptance".to_string()),
        risk_tier: None,
    }
}

fn alice_bob_operator_added(snapshot: &meerkat_meta::TopologySnapshot) -> bool {
    // Snapshot edges carry the runtime's authority on both endpoints.
    let endpoint = |identity: &str| TopologyEndpoint {
        authority: Some(snapshot.authority.clone()),
        identity: identity.to_string(),
    };
    let edge = TopologyEdge::new(endpoint("alice"), endpoint("bob")).expect("local edge");
    snapshot
        .edges
        .iter()
        .any(|candidate| candidate.edge == edge && candidate.operator_added)
}

async fn spawn_bob(runtime: &UnifiedRuntime) {
    runtime
        .spawn(
            SpawnMemberSpec::from_wire(
                "worker".to_string(),
                MeerkatId::from("bob").to_string(),
                None,
                Some(MobRuntimeMode::TurnDriven),
                None,
            )
            .with_additional_instructions(vec!["You are bob.".to_string()]),
        )
        .await
        .expect("spawn bob");
}

impl EdgeDiscovery for BlockingEdgeDiscovery {
    fn discover_edges(
        &self,
        _active_members: Vec<meerkat_meta::unified_runtime::edge_types::EdgeMemberView>,
    ) -> Pin<Box<dyn Future<Output = Vec<DesiredPeerEdge>> + Send + '_>> {
        Box::pin(async move {
            if self.state.armed.swap(false, Ordering::SeqCst) {
                self.state.entered.add_permits(1);
                self.state
                    .release
                    .acquire()
                    .await
                    .expect("blocking discovery release semaphore closed")
                    .forget();
            }
            Vec::new()
        })
    }
}

async fn build_runtime_with_edge_discovery(
    root: &Path,
    authority: &str,
    member: &str,
    edge_discovery: impl EdgeDiscovery + 'static,
) -> UnifiedRuntime {
    let session_path = root.join(format!("{authority}-blocking-sessions"));
    std::fs::create_dir_all(&session_path).expect("session path");
    let factory = AgentFactory::new(&session_path).comms(true);
    let session_service = Arc::new(build_ephemeral_service(factory, Config::default(), 8));
    let definition = MobDefinition::from_toml(&format!(
        r#"
[mob]
id = "{authority}"

[profiles.worker]
model = "gpt-5.5"
external_addressable = true

[profiles.worker.tools]
comms = true
"#
    ))
    .expect("definition");
    let mob_spec = MobBootstrapSpec::new(definition, MobStorage::in_memory(), session_service)
        .with_options(MobBootstrapOptions {
            allow_ephemeral_sessions: true,
            notify_orchestrator_on_resume: true,
            default_llm_client: Some(Arc::new(TestClient::for_provider(Provider::OpenAI))),
        });
    let module_config = MetaConfig {
        modules: vec![],
        discovery: DiscoverySpec {
            namespace: format!("topology-{authority}"),
            modules: vec![],
        },
        pre_spawn: vec![],
    };
    let runtime = UnifiedRuntime::builder()
        .mob_spec(mob_spec)
        .module_config(module_config)
        .timeout(Duration::from_secs(2))
        .persistent_state(root.join(format!("{authority}-blocking-state")))
        .edge_discovery(edge_discovery)
        .topology_control(editable_policy())
        .expect("editable topology policy")
        .build()
        .await
        .expect("build blocking topology runtime");
    runtime
        .spawn(
            SpawnMemberSpec::from_wire(
                "worker".to_string(),
                MeerkatId::from(member).to_string(),
                None,
                Some(MobRuntimeMode::TurnDriven),
                None,
            )
            .with_additional_instructions(vec![format!("You are {member}.")]),
        )
        .await
        .expect("spawn topology member");
    runtime
}

struct ToolSendClient {
    peer_id: String,
    bodies: Vec<String>,
    calls: AtomicUsize,
    observations: tokio::sync::mpsc::UnboundedSender<String>,
}

impl ToolSendClient {
    fn new(
        peer_id: String,
        bodies: impl IntoIterator<Item = impl Into<String>>,
        observations: tokio::sync::mpsc::UnboundedSender<String>,
    ) -> Self {
        Self {
            peer_id,
            bodies: bodies.into_iter().map(Into::into).collect(),
            calls: AtomicUsize::new(0),
            observations,
        }
    }
}

impl LlmClient for ToolSendClient {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        let replay = serde_json::to_string(&request.messages).unwrap_or_default();
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
        let _ = self.observations.send(replay);
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        if call == 0 {
            let [usage, done] = llm_usage::usage_then_done(request, provider, StopReason::EndTurn);
            return Box::pin(stream::iter(vec![
                Ok(LlmEvent::TextDelta {
                    delta: "local turn observed".to_string(),
                    meta: None,
                }),
                Ok(usage),
                Ok(done),
            ]));
        }
        let script_call = call - 1;
        if script_call.is_multiple_of(3) {
            let [usage, done] = llm_usage::usage_then_done(request, provider, StopReason::ToolUse);
            Box::pin(stream::iter(vec![
                Ok(LlmEvent::ToolCallComplete {
                    id: format!("peers-{script_call}"),
                    name: "peers".to_string(),
                    args: serde_json::json!({}),
                    meta: None,
                }),
                Ok(usage),
                Ok(done),
            ]))
        } else if script_call % 3 == 1 {
            let body = self
                .bodies
                .get(script_call / 3)
                .cloned()
                .unwrap_or_else(|| format!("unexpected scripted send {script_call}"));
            let [usage, done] = llm_usage::usage_then_done(request, provider, StopReason::ToolUse);
            Box::pin(stream::iter(vec![
                Ok(LlmEvent::ToolCallComplete {
                    id: format!("send-{script_call}"),
                    name: "send_message".to_string(),
                    args: serde_json::json!({
                        "peer_id": self.peer_id,
                        "body": body,
                        "handling_mode": "queue",
                    }),
                    meta: None,
                }),
                Ok(usage),
                Ok(done),
            ]))
        } else {
            let [usage, done] = llm_usage::usage_then_done(request, provider, StopReason::EndTurn);
            Box::pin(stream::iter(vec![
                Ok(LlmEvent::TextDelta {
                    delta: "tool observed".to_string(),
                    meta: None,
                }),
                Ok(usage),
                Ok(done),
            ]))
        }
    }

    fn provider(&self) -> Provider {
        Provider::Other
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

struct RecordingClient {
    observations: tokio::sync::mpsc::UnboundedSender<String>,
}

impl LlmClient for RecordingClient {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        let replay = serde_json::to_string(&request.messages).unwrap_or_default();
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
        let _ = self.observations.send(replay);
        let [usage, done] = llm_usage::usage_then_done(request, provider, StopReason::EndTurn);
        Box::pin(stream::iter(vec![
            Ok(LlmEvent::TextDelta {
                delta: "peer input observed".to_string(),
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
    ) -> Pin<Box<dyn Future<Output = Result<(), LlmError>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async { Ok(()) })
    }
}

async fn next_observation(
    receiver: &mut tokio::sync::mpsc::UnboundedReceiver<String>,
    label: &str,
) -> String {
    tokio::time::timeout(Duration::from_secs(3), receiver.recv())
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {label}"))
        .unwrap_or_else(|| panic!("{label} observation channel closed"))
}

async fn trigger_agent_tool_send(
    runtime: &UnifiedRuntime,
    sender_observations: &mut tokio::sync::mpsc::UnboundedReceiver<String>,
) -> String {
    runtime
        .mob_handle()
        .member(&MeerkatId::from("alice"))
        .await
        .expect("alice member handle")
        .send("send the scripted peer update", HandlingMode::Queue)
        .await
        .expect("trigger alice agent turn");
    let _request_before_tool = next_observation(sender_observations, "sender peers request").await;
    let peers_result = next_observation(sender_observations, "sender peers-result request").await;
    let send_result = next_observation(sender_observations, "sender send-result request").await;
    format!("peers={peers_result}\nsend={send_result}")
}

async fn prove_normal_agent_turn(
    runtime: &UnifiedRuntime,
    sender_observations: &mut tokio::sync::mpsc::UnboundedReceiver<String>,
) {
    runtime
        .mob_handle()
        .member(&MeerkatId::from("alice"))
        .await
        .expect("alice member handle")
        .send("local harness control", HandlingMode::Queue)
        .await
        .expect("trigger local control turn");
    let request = next_observation(sender_observations, "local control request").await;
    assert!(
        request.contains("local harness control"),
        "the injected client must observe a normal local member turn before topology testing: {request}"
    );
}

fn edge(
    left_authority: &str,
    left_member: &str,
    right_authority: &str,
    right_member: &str,
) -> TopologyEdge {
    TopologyEdge::new(
        TopologyEndpoint {
            authority: Some(left_authority.to_string()),
            identity: left_member.to_string(),
        },
        TopologyEndpoint {
            authority: Some(right_authority.to_string()),
            identity: right_member.to_string(),
        },
    )
    .expect("edge")
}

fn mutation(action: TopologyAction, edge: &TopologyEdge) -> TopologyMutation {
    TopologyMutation {
        action,
        edge: edge.clone(),
    }
}

async fn shutdown(runtime: &UnifiedRuntime) {
    let report = runtime.shutdown().await;
    assert!(
        report.mob_stop.is_ok(),
        "mob stop failed: {:?}",
        report.mob_stop
    );
}

async fn spawn_test_member(runtime: &UnifiedRuntime, member: &str) {
    runtime
        .spawn(SpawnMemberSpec::from_wire(
            "worker".to_string(),
            MeerkatId::from(member).to_string(),
            None,
            Some(MobRuntimeMode::TurnDriven),
            None,
        ))
        .await
        .unwrap_or_else(|error| panic!("spawn {member}: {error}"));
}

/// A member whose runtime lives in this process presents its durable
/// endpoint with owner `Local`: cross-mob wiring keeps describing it by its
/// live runtime (and `inproc` between same-process mobs). Only a placed
/// member (owner `Host`) is described by its durable host endpoint.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_member_durable_endpoint_is_owned_locally() {
    let _serial = TOPOLOGY_TEST_LOCK.lock().await;
    let root = tempfile::tempdir().expect("root");
    let runtime = build_runtime(root.path(), "endpoint-owner", "alice").await;
    let (peer_id, _, _) = runtime
        .local_member_peer_info("alice")
        .await
        .expect("alice peer info");
    // Resolve the roster identity the alias maps to by its published peer id.
    let identity = runtime
        .mob_handle()
        .list_all_members()
        .await
        .into_iter()
        .find(|entry| entry.peer_id().is_some_and(|id| id.to_string() == peer_id))
        .map(|entry| entry.agent_identity)
        .expect("alice is in the roster");
    let endpoint = runtime
        .mob_handle()
        .member_peer_endpoint(&identity)
        .await
        .expect("query alice endpoint")
        .expect("a spawned member has a durable endpoint");
    assert_eq!(endpoint.owner, meerkat_mob::MobMemberEndpointOwner::Local);
    assert_eq!(endpoint.descriptor.peer_id.to_string(), peer_id);
    assert!(
        runtime
            .mob_handle()
            .member_peer_endpoint(&meerkat_mob::AgentIdentity::from("nobody"))
            .await
            .expect("query absent endpoint")
            .is_none(),
        "an absent member publishes no endpoint"
    );
    shutdown(&runtime).await;
}

/// Public-host acceptance: query/plan/apply/operation all run through the
/// coordinator, the resulting edge carries a real sender-attributed peer
/// message, and a clean process reconstruction restores process-local aliases
/// and both physical trust halves from durable intent before disconnect.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bilateral_public_api_delivers_recovers_and_disconnects() {
    let _serial = TOPOLOGY_TEST_LOCK.lock().await;
    let root = tempfile::tempdir().expect("root");
    let journal = root.path().join("pair.json");
    let edge = edge("topology-a", "alice", "topology-b", "bob");

    let (bob_observations_tx, mut bob_observations) = tokio::sync::mpsc::unbounded_channel();
    let right = build_runtime_with_client(
        root.path(),
        "topology-b",
        "bob",
        Arc::new(RecordingClient {
            observations: bob_observations_tx,
        }),
    )
    .await;
    let (bob_peer_id, _, _) = right
        .local_member_peer_info("bob")
        .await
        .expect("bob peer info");
    let (alice_observations_tx, mut alice_observations) = tokio::sync::mpsc::unbounded_channel();
    let left = build_runtime_with_client(
        root.path(),
        "topology-a",
        "alice",
        Arc::new(ToolSendClient::new(
            bob_peer_id,
            ["cross-runtime delivery before restart"],
            alice_observations_tx,
        )),
    )
    .await;
    prove_normal_agent_turn(&left, &mut alice_observations).await;
    let coordinator = SameProcessTopologyCoordinator::open(&journal).expect("coordinator");

    let initial = coordinator
        .query(&left, &right, None)
        .await
        .expect("initial query");
    assert_eq!(initial.authorities, ["topology-a", "topology-b"]);
    let operations = vec![mutation(TopologyAction::Connect, &edge)];
    let plan = coordinator
        .plan(
            &left,
            &right,
            TopologyBilateralPlanRequest {
                expected_revisions: initial.authority_revisions.clone(),
                operations: operations.clone(),
            },
            None,
        )
        .await
        .expect("connect plan");
    assert_eq!(plan.operations.len(), 1);
    assert!(plan.operations[0].requires_physical_change);

    let applied = coordinator
        .apply(
            &left,
            &right,
            TopologyBilateralApplyRequest {
                expected_revisions: initial.authority_revisions,
                idempotency_key: "connect-a-b".to_string(),
                operations,
                reason: Some("integration acceptance".to_string()),
                risk_tier: None,
            },
            None,
            "integration-test",
        )
        .await
        .expect("connect apply");
    assert_eq!(applied.status, TopologyOperationStatus::Applied);
    assert_eq!(
        coordinator
            .operation(&left, &right, &applied.operation_id, None)
            .await
            .expect("operation lookup"),
        applied
    );
    let connected = coordinator
        .query(&left, &right, None)
        .await
        .expect("connected query");
    let projected = connected
        .edges
        .iter()
        .find(|candidate| candidate.edge == edge)
        .expect("cross edge projected");
    assert!(projected.desired && projected.actual);

    let sender_result = trigger_agent_tool_send(&left, &mut alice_observations).await;
    assert!(
        sender_result.contains(&right.local_member_peer_info("bob").await.unwrap().0),
        "agent-facing peers tool must expose Bob's canonical peer id: {sender_result}"
    );
    assert!(
        !sender_result.contains("peer_not_found_or_not_trusted"),
        "agent-facing send_message tool unexpectedly failed: {sender_result}"
    );
    let recipient_request = next_observation(&mut bob_observations, "bob peer delivery").await;
    assert!(recipient_request.contains("cross-runtime delivery before restart"));
    assert!(
        recipient_request.contains("topology-a/worker/alice"),
        "recipient transcript must preserve authenticated sender attribution: {recipient_request}"
    );

    shutdown(&left).await;
    shutdown(&right).await;
    drop(coordinator);
    drop(left);
    drop(right);

    let (bob_observations_tx, mut bob_observations) = tokio::sync::mpsc::unbounded_channel();
    let right = build_runtime_with_client(
        root.path(),
        "topology-b",
        "bob",
        Arc::new(RecordingClient {
            observations: bob_observations_tx,
        }),
    )
    .await;
    let (bob_peer_id, _, _) = right
        .local_member_peer_info("bob")
        .await
        .expect("bob peer info after restart");
    let (alice_observations_tx, mut alice_observations) = tokio::sync::mpsc::unbounded_channel();
    let left = build_runtime_with_client(
        root.path(),
        "topology-a",
        "alice",
        Arc::new(ToolSendClient::new(
            bob_peer_id,
            [
                "cross-runtime delivery after restart",
                "delivery must fail after disconnect",
            ],
            alice_observations_tx,
        )),
    )
    .await;
    prove_normal_agent_turn(&left, &mut alice_observations).await;
    let coordinator = SameProcessTopologyCoordinator::open(&journal).expect("reopen coordinator");
    coordinator
        .recover(&left, &right)
        .await
        .expect("clean restart recovery");
    let recovered = coordinator
        .query(&left, &right, None)
        .await
        .expect("recovered query");
    let projected = recovered
        .edges
        .iter()
        .find(|candidate| candidate.edge == edge)
        .expect("recovered edge projected");
    assert!(projected.desired && projected.actual);
    let sender_result = trigger_agent_tool_send(&left, &mut alice_observations).await;
    assert!(!sender_result.contains("peer_not_found_or_not_trusted"));
    let recipient_request =
        next_observation(&mut bob_observations, "bob delivery after restart").await;
    assert!(recipient_request.contains("cross-runtime delivery after restart"));
    assert!(recipient_request.contains("topology-a/worker/alice"));

    let disconnected = coordinator
        .apply(
            &left,
            &right,
            TopologyBilateralApplyRequest {
                expected_revisions: recovered.authority_revisions,
                idempotency_key: "disconnect-a-b".to_string(),
                operations: vec![mutation(TopologyAction::Disconnect, &edge)],
                reason: None,
                risk_tier: None,
            },
            None,
            "integration-test",
        )
        .await
        .expect("disconnect apply");
    assert_eq!(disconnected.status, TopologyOperationStatus::Applied);
    let final_snapshot = coordinator
        .query(&left, &right, None)
        .await
        .expect("disconnected query");
    let projected = final_snapshot
        .edges
        .iter()
        .find(|candidate| candidate.edge == edge)
        .expect("suppression projected");
    assert!(projected.suppressed && !projected.desired && !projected.actual);

    let denied_result = trigger_agent_tool_send(&left, &mut alice_observations).await;
    assert!(
        denied_result.contains("peer_not_found_or_not_trusted")
            || denied_result.contains("not trusted"),
        "disconnected peer tool call must fail closed: {denied_result}"
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(400), bob_observations.recv())
            .await
            .is_err(),
        "bob must not receive a message after disconnect"
    );

    let audit = coordinator
        .audit(&left, &right, None, None, 20)
        .await
        .expect("bilateral audit");
    assert_eq!(audit.records.len(), 2, "one durable record per operation");
    assert!(audit.records[0].seq < audit.records[1].seq);

    shutdown(&left).await;
    shutdown(&right).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bilateral_reconnect_requires_both_authorities_to_grant_it() {
    let _serial = TOPOLOGY_TEST_LOCK.lock().await;
    let root = tempfile::tempdir().expect("root");
    let journal = root.path().join("abac-pair.json");
    let edge = edge("abac-a", "alice", "abac-b", "bob");
    let mut left = build_runtime(root.path(), "abac-a", "alice").await;
    let mut right = build_runtime(root.path(), "abac-b", "bob").await;
    left.set_access_controller(topology_access(&[
        "agent.view",
        "topology.view",
        "topology.connect",
        "topology.disconnect",
        "topology.reconnect",
        "topology.cross_authority",
    ]));
    right.set_access_controller(topology_access(&[
        "agent.view",
        "topology.view",
        "topology.connect",
        "topology.disconnect",
        "topology.cross_authority",
    ]));
    let coordinator = SameProcessTopologyCoordinator::open(&journal).expect("coordinator");

    let initial = coordinator
        .query(&left, &right, Some("desktop-host"))
        .await
        .expect("principal-shaped initial query");
    let connected = coordinator
        .apply(
            &left,
            &right,
            TopologyBilateralApplyRequest {
                expected_revisions: initial.authority_revisions,
                idempotency_key: "abac-connect".to_string(),
                operations: vec![mutation(TopologyAction::Connect, &edge)],
                reason: None,
                risk_tier: None,
            },
            Some("desktop-host"),
            "desktop-host",
        )
        .await
        .expect("connect granted by both authorities");
    assert!(
        connected.actor.is_empty(),
        "operation attribution must be redacted without topology.audit on both authorities"
    );
    assert!(
        serde_json::to_value(&connected)
            .expect("serialized receipt")
            .get("actor")
            .is_none(),
        "redacted attribution must be omitted from the wire receipt"
    );
    let connected_operation_id = connected.operation_id.clone();
    let connected_revisions = connected
        .authority_revisions
        .iter()
        .map(|(authority, transition)| (authority.clone(), transition.revision))
        .collect();
    let disconnected = coordinator
        .apply(
            &left,
            &right,
            TopologyBilateralApplyRequest {
                expected_revisions: connected_revisions,
                idempotency_key: "abac-disconnect".to_string(),
                operations: vec![mutation(TopologyAction::Disconnect, &edge)],
                reason: None,
                risk_tier: None,
            },
            Some("desktop-host"),
            "desktop-host",
        )
        .await
        .expect("disconnect granted by both authorities");

    left.set_access_controller(topology_access(&[
        "agent.view",
        "topology.view",
        "topology.connect",
        "topology.disconnect",
        "topology.reconnect",
        "topology.cross_authority",
        "topology.audit",
    ]));
    right.set_access_controller(topology_access(&[
        "agent.view",
        "topology.view",
        "topology.connect",
        "topology.disconnect",
        "topology.cross_authority",
        "topology.audit",
    ]));
    let attributed = coordinator
        .operation(&left, &right, &connected_operation_id, Some("desktop-host"))
        .await
        .expect("auditor can resolve durable attribution");
    assert_eq!(attributed.actor, "desktop-host");

    let shaped = coordinator
        .query(&left, &right, Some("desktop-host"))
        .await
        .expect("principal-shaped query");
    assert_eq!(
        shaped.nodes.len(),
        2,
        "both visible endpoints remain projected"
    );
    assert!(shaped.nodes.iter().all(|node| {
        node.affordances
            .as_ref()
            .is_some_and(|affordances| !affordances.can_reconnect)
    }));
    let denied = coordinator
        .apply(
            &left,
            &right,
            TopologyBilateralApplyRequest {
                expected_revisions: disconnected
                    .authority_revisions
                    .iter()
                    .map(|(authority, transition)| (authority.clone(), transition.revision))
                    .collect(),
                idempotency_key: "abac-reconnect".to_string(),
                operations: vec![mutation(TopologyAction::Reconnect, &edge)],
                reason: None,
                risk_tier: None,
            },
            Some("desktop-host"),
            "desktop-host",
        )
        .await
        .expect_err("one missing reconnect grant must deny the bilateral mutation");
    assert_eq!(denied.kind(), "topology_access_denied");

    shutdown(&left).await;
    shutdown(&right).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn aborted_requested_attempt_is_interrupted_and_retry_can_proceed_without_reopen() {
    let _serial = TOPOLOGY_TEST_LOCK.lock().await;
    let root = tempfile::tempdir().expect("root");
    let journal = root.path().join("cancelled-pair.json");
    let gate = BlockingEdgeDiscovery::new();
    let left = Arc::new(
        build_runtime_with_edge_discovery(root.path(), "cancel-a", "alice", gate.clone()).await,
    );
    let right = Arc::new(build_runtime(root.path(), "cancel-b", "bob").await);
    let coordinator = SameProcessTopologyCoordinator::open(&journal).expect("coordinator");
    let initial = coordinator
        .query(&left, &right, None)
        .await
        .expect("initial query");
    let request = TopologyBilateralApplyRequest {
        expected_revisions: initial.authority_revisions.clone(),
        idempotency_key: "cancelled-request".to_string(),
        operations: vec![mutation(
            TopologyAction::Connect,
            &edge("cancel-a", "alice", "cancel-b", "bob"),
        )],
        reason: Some("cancellation acceptance".to_string()),
        risk_tier: None,
    };

    gate.arm();
    let task = tokio::spawn({
        let coordinator = coordinator.clone();
        let left = Arc::clone(&left);
        let right = Arc::clone(&right);
        let request = request.clone();
        async move {
            coordinator
                .apply(&left, &right, request, None, "cancellation-test")
                .await
        }
    });
    gate.wait_until_entered().await;
    task.abort();
    assert!(
        task.await
            .expect_err("apply task must be cancelled")
            .is_cancelled()
    );

    let audit = coordinator
        .audit(&left, &right, None, None, 20)
        .await
        .expect("same-process audit reconciles the orphan");
    assert_eq!(audit.records.len(), 1);
    assert_eq!(
        audit.records[0].status,
        meerkat_meta::TopologyOperationRecordStatus::Interrupted
    );
    let persisted = std::fs::read_to_string(&journal).expect("read coordinator journal");
    assert!(
        persisted.contains("\"status\": \"interrupted\""),
        "Interrupted must be durable without reopening the coordinator: {persisted}"
    );
    let after_cancel = coordinator
        .query(&left, &right, None)
        .await
        .expect("query remains available after reconciliation");
    assert_eq!(
        after_cancel.authority_revisions,
        initial.authority_revisions
    );

    let retried = coordinator
        .apply(&left, &right, request, None, "cancellation-test")
        .await
        .expect("same idempotency key may proceed because the cancelled attempt had no effects");
    assert_eq!(retried.status, TopologyOperationStatus::Applied);
    let audit = coordinator
        .audit(&left, &right, None, Some(audit.records[0].seq), 20)
        .await
        .expect("audit after retry");
    assert_eq!(audit.records.len(), 1);
    assert_eq!(
        audit.records[0].status,
        meerkat_meta::TopologyOperationRecordStatus::Applied
    );

    shutdown(&left).await;
    shutdown(&right).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn aborted_local_requested_attempt_is_durable_and_reconciled_without_reopen() {
    let _serial = TOPOLOGY_TEST_LOCK.lock().await;
    let root = tempfile::tempdir().expect("root");
    let gate = BlockingEdgeDiscovery::new();
    let runtime = Arc::new(
        build_runtime_with_edge_discovery(root.path(), "local-cancel", "alice", gate.clone()).await,
    );
    runtime
        .spawn(
            SpawnMemberSpec::from_wire(
                "worker".to_string(),
                MeerkatId::from("bob").to_string(),
                None,
                Some(MobRuntimeMode::TurnDriven),
                None,
            )
            .with_additional_instructions(vec!["You are bob.".to_string()]),
        )
        .await
        .expect("spawn bob");
    let topology = runtime.topology_runtime_handle();
    let initial = topology.query().await.expect("initial local query");
    let request = TopologyApplyRequest {
        expected_revision: initial.revision,
        idempotency_key: "cancelled-local-request".to_string(),
        operations: vec![TopologyMutation {
            action: TopologyAction::Connect,
            edge: TopologyEdge::new(
                TopologyEndpoint::local("alice"),
                TopologyEndpoint::local("bob"),
            )
            .expect("local edge"),
        }],
        reason: Some("local cancellation acceptance".to_string()),
        risk_tier: None,
    };

    gate.arm();
    let task = tokio::spawn({
        let topology = topology.clone();
        let request = request.clone();
        async move { topology.apply(request, "cancellation-test").await }
    });
    gate.wait_until_entered().await;
    task.abort();
    assert!(
        task.await
            .expect_err("local apply must be cancelled")
            .is_cancelled()
    );

    let audit = runtime
        .topology_controller()
        .operation_records(None, 20)
        .await
        .expect("same-process local audit reconciles orphan");
    assert_eq!(audit.records.len(), 1);
    assert_eq!(
        audit.records[0].status,
        meerkat_meta::TopologyOperationRecordStatus::Interrupted
    );
    let retried = topology
        .apply(request, "cancellation-test")
        .await
        .expect("local retry after interruption");
    assert_eq!(retried.status, TopologyOperationStatus::Applied);

    shutdown(&runtime).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn downstream_alias_failure_removes_attempt_state_and_allows_clean_retry() {
    let _serial = TOPOLOGY_TEST_LOCK.lock().await;
    let root = tempfile::tempdir().expect("root");
    let journal = root.path().join("rollback-pair.json");
    let left = build_runtime(root.path(), "rollback-a", "alice").await;
    let right = build_runtime(root.path(), "rollback-b", "bob").await;
    let edge = edge("rollback-a", "alice", "rollback-b", "bob");
    let (alice_peer_id, alice_name, alice_address) = left
        .local_member_peer_info("alice")
        .await
        .expect("alice peer info");
    let (_bob_peer_id, bob_name, _) = right
        .local_member_peer_info("bob")
        .await
        .expect("bob peer info");
    let left_namespace = meerkat_core::mob_realm_id("rollback-a")
        .expect("left namespace")
        .as_str()
        .to_string();
    let right_namespace = meerkat_core::mob_realm_id("rollback-b")
        .expect("right namespace")
        .as_str()
        .to_string();
    let registry = InprocRegistry::global();
    let alice_registration = registry
        .peers_in_namespace(&left_namespace)
        .into_iter()
        .find(|peer| peer.name == alice_name)
        .expect("alice canonical registration");
    let alice_sender = registry
        .get_by_pubkey_in_namespace(&left_namespace, &alice_registration.pubkey)
        .expect("alice canonical sender");
    let bob_registration = registry
        .peers_in_namespace(&right_namespace)
        .into_iter()
        .find(|peer| peer.name == bob_name)
        .expect("bob canonical registration");
    let bob_sender = registry
        .get_by_pubkey_in_namespace(&right_namespace, &bob_registration.pubkey)
        .expect("bob canonical sender");

    // Start with one healthy pre-existing half. The coordinator treats this
    // as a degraded orphan and will canonicalize it to disconnected if the
    // opposite-half repair fails.
    right
        .wire_local(
            "bob",
            &alice_name,
            &alice_peer_id,
            &alice_address,
            Some(*alice_registration.pubkey.as_bytes()),
        )
        .await
        .expect("pre-wire Bob to Alice");
    let alias = registry.register_with_meta_in_namespace(
        &right_namespace,
        alice_name.clone(),
        alice_registration.pubkey,
        alice_sender,
        PeerMeta::default(),
    );
    assert!(
        !alias.is_rejected()
            && !matches!(
                alias,
                meerkat_comms::RegistrationOutcome::ReplacedPubkey { .. }
            )
    );

    // Remove Bob's canonical route only long enough to force alias
    // installation to fail after Alice's trust repair has succeeded.
    registry.unregister_in_namespace(&right_namespace, &bob_registration.pubkey);
    let coordinator = SameProcessTopologyCoordinator::open(&journal).expect("coordinator");
    let initial = coordinator.query(&left, &right, None).await.expect("query");
    let error = coordinator
        .apply(
            &left,
            &right,
            TopologyBilateralApplyRequest {
                expected_revisions: initial.authority_revisions,
                idempotency_key: "forced-alias-failure".to_string(),
                operations: vec![mutation(TopologyAction::Connect, &edge)],
                reason: None,
                risk_tier: None,
            },
            None,
            "rollback-test",
        )
        .await
        .expect_err("missing canonical route must fail alias installation");
    assert_eq!(error.kind(), "topology_apply_failed");
    assert_eq!(
        error.receipt().expect("rollback receipt").status,
        TopologyOperationStatus::RolledBack
    );

    // Restore the unrelated canonical route and verify this attempt left no
    // one-sided trust or newly-created opposite alias behind.
    let restored = registry.register_with_meta_in_namespace(
        &right_namespace,
        bob_name.clone(),
        bob_registration.pubkey,
        bob_sender,
        bob_registration.meta,
    );
    assert!(
        !restored.is_rejected()
            && !matches!(
                restored,
                meerkat_comms::RegistrationOutcome::ReplacedPubkey { .. }
            )
    );
    assert!(
        !registry
            .peers_in_namespace(&left_namespace)
            .iter()
            .any(|peer| peer.name == bob_name && peer.pubkey == bob_registration.pubkey),
        "attempt-created Bob alias must be absent from Alice's namespace"
    );
    let alice_member = left
        .mob_handle()
        .get_member(&MeerkatId::from("alice"))
        .await
        .expect("alice lookup")
        .expect("alice member");
    let bob_member = right
        .mob_handle()
        .get_member(&MeerkatId::from("bob"))
        .await
        .expect("bob lookup")
        .expect("bob member");
    assert!(
        !alice_member
            .wired_to
            .iter()
            .any(|peer| peer.as_str() == bob_name)
    );
    assert!(
        !bob_member
            .wired_to
            .iter()
            .any(|peer| peer.as_str() == alice_name)
    );

    let retry_snapshot = coordinator
        .query(&left, &right, None)
        .await
        .expect("retry snapshot");
    let retried = coordinator
        .apply(
            &left,
            &right,
            TopologyBilateralApplyRequest {
                expected_revisions: retry_snapshot.authority_revisions,
                idempotency_key: "forced-alias-failure-retry".to_string(),
                operations: vec![mutation(TopologyAction::Connect, &edge)],
                reason: None,
                risk_tier: None,
            },
            None,
            "rollback-test",
        )
        .await
        .expect("fresh idempotency key cleanly retries durable intent");
    assert_eq!(retried.status, TopologyOperationStatus::Applied);

    shutdown(&left).await;
    shutdown(&right).await;
}

/// Coordinator ownership is pinned to one canonical journal path. Copies are
/// not implicit migrations: both concurrent clones and stale copies reopened
/// after the original advances must fail closed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn copied_journal_cannot_become_second_live_pair_owner() {
    let _serial = TOPOLOGY_TEST_LOCK.lock().await;
    let root = tempfile::tempdir().expect("root");
    let left = build_runtime(root.path(), "owner-a", "alice").await;
    let right = build_runtime(root.path(), "owner-b", "bob").await;
    let original_path = root.path().join("original.json");
    let copied_path = root.path().join("copied.json");
    let original = SameProcessTopologyCoordinator::open(&original_path).expect("original");
    original
        .query(&left, &right, None)
        .await
        .expect("bind original");
    std::fs::copy(&original_path, &copied_path).expect("copy journal");
    let snapshot = original
        .query(&left, &right, None)
        .await
        .expect("snapshot before advance");
    original
        .apply(
            &left,
            &right,
            TopologyBilateralApplyRequest {
                expected_revisions: snapshot.authority_revisions,
                idempotency_key: "advance-original".to_string(),
                operations: vec![mutation(
                    TopologyAction::Connect,
                    &edge("owner-a", "alice", "owner-b", "bob"),
                )],
                reason: None,
                risk_tier: None,
            },
            None,
            "lease-test",
        )
        .await
        .expect("advance original journal");
    let copied = SameProcessTopologyCoordinator::open(&copied_path).expect("open copied file");
    let rejected = copied
        .query(&left, &right, None)
        .await
        .expect_err("copied live owner must fail closed");
    assert_eq!(rejected.kind(), "topology_authority_mismatch");
    drop(copied);
    drop(original);
    let stale = SameProcessTopologyCoordinator::open(&copied_path).expect("reopen stale copy");
    let rejected = stale
        .query(&left, &right, None)
        .await
        .expect_err("stale copied path must remain rejected after original drops");
    assert_eq!(rejected.kind(), "topology_authority_mismatch");
    shutdown(&left).await;
    shutdown(&right).await;
}

#[test]
fn same_canonical_path_has_one_owner_until_last_clone_drops() {
    let root = tempfile::tempdir().expect("root");
    let path = root.path().join("pair.json");
    let owner = SameProcessTopologyCoordinator::open(&path).expect("owner");
    let clone = owner.clone();
    assert!(SameProcessTopologyCoordinator::open(&path).is_err());
    drop(owner);
    assert!(
        SameProcessTopologyCoordinator::open(&path).is_err(),
        "an Arc clone must retain the file lease"
    );
    drop(clone);
    SameProcessTopologyCoordinator::open(&path).expect("lease released after final clone");
}

#[test]
fn path_aliases_share_one_lease_and_journal_symlinks_are_rejected() {
    use std::os::unix::fs::symlink;

    let root = tempfile::tempdir().expect("root");
    let real_parent = root.path().join("real");
    std::fs::create_dir_all(&real_parent).expect("real parent");
    let symlink_parent = root.path().join("alias");
    symlink(&real_parent, &symlink_parent).expect("symlink parent");
    let owner = SameProcessTopologyCoordinator::open(symlink_parent.join("pair.json"))
        .expect("open through symlinked parent");
    assert!(
        SameProcessTopologyCoordinator::open(real_parent.join("pair.json")).is_err(),
        "canonical parent aliases must contend on the same lock"
    );
    drop(owner);
    SameProcessTopologyCoordinator::open(real_parent.join("pair.json"))
        .expect("canonical path reopens after owner drop");

    let target = root.path().join("target.json");
    std::fs::write(&target, b"{}").expect("target journal");
    let linked = root.path().join("linked.json");
    symlink(&target, &linked).expect("journal symlink");
    assert!(
        SameProcessTopologyCoordinator::open(&linked).is_err(),
        "journal files that are symlinks must fail closed"
    );
}

#[test]
fn policy_default_keeps_bulk_cross_and_mutation_disabled() {
    let policy = TopologyControlPolicy::default();
    assert_eq!(policy.mode, TopologyControlMode::Disabled);
    assert!(!policy.allow_bulk);
    assert!(!policy.allow_cross_authority);
    assert_eq!(policy.max_batch_size, 1);
    assert!(policy.idempotency_history_limit >= policy.receipt_limit);
    let _empty = BTreeMap::<String, u64>::new();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disabled_and_read_only_modes_fail_closed_at_the_runtime_surface() {
    let _serial = TOPOLOGY_TEST_LOCK.lock().await;
    let root = tempfile::tempdir().expect("root");
    let local_request = |revision: u64, key: &str| TopologyApplyRequest {
        expected_revision: revision,
        idempotency_key: key.to_string(),
        operations: vec![TopologyMutation {
            action: TopologyAction::Connect,
            edge: TopologyEdge::new(
                TopologyEndpoint::local("alice"),
                TopologyEndpoint::local("bob"),
            )
            .expect("edge"),
        }],
        reason: None,
        risk_tier: None,
    };
    let disabled = build_runtime_with_client_and_policy(
        root.path(),
        "disabled-mode",
        "alice",
        Arc::new(TestClient::for_provider(Provider::OpenAI)),
        TopologyControlPolicy::default(),
    )
    .await;
    spawn_test_member(&disabled, "bob").await;
    let disabled_topology = disabled.topology_runtime_handle();
    let disabled_snapshot = disabled_topology.query().await.expect("disabled query");
    assert_eq!(disabled_snapshot.policy.mode, TopologyControlMode::Disabled);
    let disabled_plan = disabled_topology
        .plan(meerkat_meta::TopologyPlanRequest {
            expected_revision: disabled_snapshot.revision,
            operations: local_request(disabled_snapshot.revision, "unused").operations,
        })
        .await
        .expect_err("disabled mode must not expose planning");
    assert_eq!(disabled_plan.kind(), "topology_control_disabled");
    let disabled_apply = disabled_topology
        .apply(
            local_request(disabled_snapshot.revision, "disabled-apply"),
            "mode-test",
        )
        .await
        .expect_err("disabled mode must deny apply");
    assert_eq!(disabled_apply.kind(), "topology_control_disabled");

    let read_only_policy = TopologyControlPolicy {
        mode: TopologyControlMode::ReadOnly,
        ..TopologyControlPolicy::default()
    };
    let read_only = build_runtime_with_client_and_policy(
        root.path(),
        "read-only-mode",
        "alice",
        Arc::new(TestClient::for_provider(Provider::OpenAI)),
        read_only_policy,
    )
    .await;
    spawn_test_member(&read_only, "bob").await;
    let read_only_topology = read_only.topology_runtime_handle();
    let read_only_snapshot = read_only_topology.query().await.expect("read-only query");
    assert_eq!(
        read_only_snapshot.policy.mode,
        TopologyControlMode::ReadOnly
    );
    read_only_topology
        .plan(meerkat_meta::TopologyPlanRequest {
            expected_revision: read_only_snapshot.revision,
            operations: local_request(read_only_snapshot.revision, "unused").operations,
        })
        .await
        .expect("read-only mode advertises planning");
    let read_only_apply = read_only_topology
        .apply(
            local_request(read_only_snapshot.revision, "read-only-apply"),
            "mode-test",
        )
        .await
        .expect_err("read-only mode must deny apply");
    assert_eq!(read_only_apply.kind(), "topology_control_read_only");

    shutdown(&disabled).await;
    shutdown(&read_only).await;
}

/// A runtime (mob id `authority`, unique per test: mob ids name process-wide
/// comms participants) whose identity-first context wants one managed edge
/// from an active identity to a dormant one, and the edge reconcile expects
/// to skip it: the dormant endpoint is not a member.
async fn runtime_with_a_dormant_edge_endpoint(
    root: &Path,
    authority: &str,
) -> (UnifiedRuntime, DesiredPeerEdge) {
    use meerkat_meta::identity_first::{
        AgentAddressability, AgentIdentity, DurabilityPolicy, DurableAgentSpec,
        IdentityFirstRuntimeContext, IdentityRuntime, IdentityRuntimeConfig, LocalContinuityStore,
        LocalLeaseProvider, ManagedPeerEdge, MobSessionBridge, RosterContext, RosterError,
        RosterProvider,
    };

    struct FixedRoster(Vec<DurableAgentSpec>);

    #[async_trait::async_trait]
    impl RosterProvider for FixedRoster {
        async fn roster(
            &self,
            _context: &RosterContext,
        ) -> Result<Vec<DurableAgentSpec>, RosterError> {
            Ok(self.0.clone())
        }
    }

    fn identity_spec(identity: &AgentIdentity) -> DurableAgentSpec {
        DurableAgentSpec {
            identity: identity.clone(),
            profile: meerkat_mob::ProfileName::from("worker"),
            addressability: AgentAddressability::Addressable,
            display_name: None,
            labels: BTreeMap::new(),
            context: None,
            additional_instructions: Vec::new(),
            initial_message: None,
            runtime_mode_override: None,
            backend: None,
            binding: None,
            placement: None,
        }
    }

    let mut unified = build_runtime(root, authority, "classic-member").await;
    let identity_runtime = Arc::new(IdentityRuntime::new(IdentityRuntimeConfig {
        continuity_store: Arc::new(LocalContinuityStore::in_memory().expect("continuity store")),
        lease_provider: Arc::new(LocalLeaseProvider::new()),
        runtime_instance_id: authority.to_string(),
        has_runtime_store: true,
        durability_policy: DurabilityPolicy::SyncWriteThrough,
        bridge: Some(Arc::new(MobSessionBridge::new(unified.mob_handle()))),
        default_timeout: None,
    }));
    let active = AgentIdentity::parse("domain:active").expect("active identity");
    let dormant = AgentIdentity::parse("domain:dormant").expect("dormant identity");
    let active_spec = identity_spec(&active);
    let dormant_spec = identity_spec(&dormant);
    identity_runtime.register(active_spec.clone()).await;
    identity_runtime.register(dormant_spec.clone()).await;
    let managed = ManagedPeerEdge::new(active.clone(), dormant.clone()).expect("managed edge");
    identity_runtime.set_desired_peer_edges(vec![managed]).await;
    unified.attach_identity_first_context(Arc::new(IdentityFirstRuntimeContext::new(
        identity_runtime,
        Arc::new(FixedRoster(vec![active_spec, dormant_spec])),
        None,
        None,
        Some(unified.mob_handle().definition().clone()),
    )));
    let expected =
        DesiredPeerEdge::new(active.to_string(), dormant.to_string()).expect("desired peer edge");
    (unified, expected)
}

#[tokio::test]
async fn identity_reconcile_reports_dormant_endpoint_as_missing_and_incomplete() {
    let root = tempfile::tempdir().expect("tempdir");
    let (unified, expected) =
        runtime_with_a_dormant_edge_endpoint(root.path(), "identity-dormant").await;

    let report = unified.reconcile_edges().await;
    assert_eq!(report.desired_edges, vec![expected.clone()]);
    assert_eq!(report.skipped_missing_members, vec![expected]);
    assert!(report.wired_edges.is_empty());
    assert!(report.failures.is_empty());
    assert!(
        !report.is_complete(),
        "a desired edge with a dormant endpoint must remain incomplete"
    );

    shutdown(&unified).await;
}

/// An edge reconcile that only skipped edges failed nothing, so it does not
/// page: lazy identity bootstrap leaves an edge to a dormant endpoint
/// pending by design, on every boot. The report still lists the edge.
#[tokio::test]
async fn an_edge_reconcile_that_only_skipped_edges_does_not_page() {
    let root = tempfile::tempdir().expect("tempdir");
    let (mut unified, expected) =
        runtime_with_a_dormant_edge_endpoint(root.path(), "identity-dormant-alert").await;
    let captured: Arc<tokio::sync::Mutex<Vec<meerkat_meta::ErrorEvent>>> =
        Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let sink = captured.clone();
    let hook: meerkat_meta::ErrorHook = Arc::new(move |event| {
        let sink = sink.clone();
        Box::pin(async move {
            sink.lock().await.push(event);
        })
    });
    unified.set_error_hook(hook);

    let report = unified.reconcile_edges().await;
    assert_eq!(report.skipped_missing_members, vec![expected]);
    assert!(report.failures.is_empty());
    // The hook runs on a detached task; this test's runtime has one thread,
    // so yielding runs every task fired so far.
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
    let alerts = captured.lock().await.clone();
    assert!(
        !alerts
            .iter()
            .any(|alert| matches!(alert, meerkat_meta::ErrorEvent::ReconcileIncomplete { .. })),
        "a reconcile that failed nothing must not page: {alerts:?}"
    );

    shutdown(&unified).await;
}

/// Observed on MobKit 0.8.43: queries arriving about 1/s with a 0.8 s
/// edge discovery queued for minutes, because every read took the exclusive
/// mutation lock. Concurrent reads now share one computation: with one
/// discovery held in flight, eight more callers cost at most one further
/// discovery, not eight serialized ones.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_topology_queries_share_one_discovery_instead_of_serializing() {
    let _serial = TOPOLOGY_TEST_LOCK.lock().await;
    let root = tempfile::tempdir().expect("root");
    let discovery = CountingGatedEdgeDiscovery::default();
    let runtime = Arc::new(
        build_runtime_with_edge_discovery(root.path(), "query-flights", "alice", discovery.clone())
            .await,
    );
    let topology = runtime.topology_runtime_handle();
    topology.query().await.expect("warm query");
    let baseline = discovery.calls();

    discovery.gate();
    let first = tokio::spawn({
        let topology = topology.clone();
        async move { topology.query().await }
    });
    discovery.wait_until_entered().await;
    // Poll each joiner once: registration in the shared flight slots happens
    // synchronously on that first poll, so all eight are queued behind the
    // flight in progress before any discovery is released.
    let handles = (0..8).map(|_| topology.clone()).collect::<Vec<_>>();
    let mut joined = handles
        .iter()
        .map(|handle| Box::pin(handle.query()))
        .collect::<Vec<_>>();
    for joiner in &mut joined {
        assert!(
            futures::poll!(joiner.as_mut()).is_pending(),
            "joiner waits on the shared flight"
        );
    }
    assert_eq!(discovery.calls(), baseline + 1, "one discovery in flight");
    discovery.open();

    first.await.expect("first query task").expect("first query");
    for result in futures::future::join_all(joined).await {
        result.expect("joined query");
    }
    let used = discovery.calls() - baseline;
    assert!(
        used <= 2,
        "nine concurrent queries ran {used} discoveries; at most the running one plus one shared fresh one"
    );

    shutdown(&runtime).await;
}

/// A read while an apply is mid-flight (journal written, wiring parked) does
/// not wait for it and reports the committed state before the apply; after
/// the apply commits a read reports the new state. Never the half-applied
/// target the journal holds.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn topology_query_during_apply_sees_before_then_after_never_half() {
    let _serial = TOPOLOGY_TEST_LOCK.lock().await;
    let root = tempfile::tempdir().expect("root");
    let gate = BlockingEdgeDiscovery::new();
    let runtime = Arc::new(
        build_runtime_with_edge_discovery(root.path(), "query-during-apply", "alice", gate.clone())
            .await,
    );
    spawn_bob(&runtime).await;
    let topology = runtime.topology_runtime_handle();
    let before = topology.query().await.expect("query before apply");
    assert!(!alice_bob_operator_added(&before));

    gate.arm();
    let apply = tokio::spawn({
        let topology = topology.clone();
        let request = connect_alice_bob(before.revision, "query-during-apply");
        async move { topology.apply(request, "query-concurrency").await }
    });
    gate.wait_until_entered().await;
    let during = tokio::time::timeout(Duration::from_secs(5), topology.query())
        .await
        .expect("a read must not wait for an in-flight apply")
        .expect("query during apply");
    assert_eq!(
        during.revision, before.revision,
        "the uncommitted target revision is never reported"
    );
    assert!(
        !alice_bob_operator_added(&during),
        "the uncommitted target edge is never reported"
    );

    gate.release();
    let receipt = apply.await.expect("apply task").expect("apply");
    assert_eq!(receipt.status, TopologyOperationStatus::Applied);
    let after = topology.query().await.expect("query after apply");
    assert_eq!(after.revision, before.revision + 1);
    assert!(alice_bob_operator_added(&after));

    shutdown(&runtime).await;
}

/// An interrupted journal (its apply was cancelled mid-flight) is still
/// recovered on the next read, under the mutation lock, because no mutation
/// holds the lock: the read reports the rolled-back committed state and a
/// later apply against that revision succeeds.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn topology_query_recovers_an_interrupted_journal() {
    let _serial = TOPOLOGY_TEST_LOCK.lock().await;
    let root = tempfile::tempdir().expect("root");
    let gate = BlockingEdgeDiscovery::new();
    let runtime = Arc::new(
        build_runtime_with_edge_discovery(root.path(), "query-recovers", "alice", gate.clone())
            .await,
    );
    spawn_bob(&runtime).await;
    let topology = runtime.topology_runtime_handle();
    let before = topology.query().await.expect("query before apply");

    gate.arm();
    let apply = tokio::spawn({
        let topology = topology.clone();
        let request = connect_alice_bob(before.revision, "interrupted-apply");
        async move { topology.apply(request, "query-concurrency").await }
    });
    gate.wait_until_entered().await;
    apply.abort();
    assert!(apply.await.expect_err("apply cancelled").is_cancelled());

    let recovered = topology
        .query()
        .await
        .expect("the read recovers the interrupted journal");
    assert_eq!(
        recovered.revision, before.revision,
        "the interrupted operation rolled back"
    );
    assert!(!alice_bob_operator_added(&recovered));
    let audit = runtime
        .topology_controller()
        .operation_records(None, 20)
        .await
        .expect("audit after recovery");
    assert!(
        audit.records.iter().all(|record| !matches!(
            record.status,
            meerkat_meta::TopologyOperationRecordStatus::Pending
                | meerkat_meta::TopologyOperationRecordStatus::Requested
        )),
        "recovery closed the interrupted operation: {:?}",
        audit.records
    );
    let retried = topology
        .apply(
            connect_alice_bob(recovered.revision, "after-recovery"),
            "query-concurrency",
        )
        .await
        .expect("apply after the read's recovery");
    assert_eq!(retried.status, TopologyOperationStatus::Applied);

    shutdown(&runtime).await;
}

/// An expensive edge discovery inside a read no longer holds the mutation
/// lock, so an apply proceeds and commits while the read is parked.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn expensive_topology_query_does_not_block_apply() {
    let _serial = TOPOLOGY_TEST_LOCK.lock().await;
    let root = tempfile::tempdir().expect("root");
    let gate = BlockingEdgeDiscovery::new();
    let runtime = Arc::new(
        build_runtime_with_edge_discovery(root.path(), "query-vs-apply", "alice", gate.clone())
            .await,
    );
    spawn_bob(&runtime).await;
    let topology = runtime.topology_runtime_handle();
    let before = topology.query().await.expect("query before");

    gate.arm();
    let parked_query = tokio::spawn({
        let topology = topology.clone();
        async move { topology.query().await }
    });
    gate.wait_until_entered().await;
    let receipt = tokio::time::timeout(
        Duration::from_secs(5),
        topology.apply(
            connect_alice_bob(before.revision, "apply-beside-query"),
            "query-concurrency",
        ),
    )
    .await
    .expect("an apply must not wait for a read's edge discovery")
    .expect("apply beside a parked read");
    assert_eq!(receipt.status, TopologyOperationStatus::Applied);

    gate.release();
    parked_query
        .await
        .expect("parked query task")
        .expect("parked query");
    let after = topology.query().await.expect("query after");
    assert!(alice_bob_operator_added(&after));

    shutdown(&runtime).await;
}

/// The issuer the test Console trusts, and a second development issuer for a
/// second Console over the same runtime.
const TRUSTED_ISSUER: &str = "https://trusted.meta.localhost";
const OTHER_ISSUER: &str = "https://other.meta.localhost";

/// Console decisions that verify HS256 test tokens from `issuer`, with two
/// people and two service identities allowlisted.
fn console_decisions(issuer: &str, require_app_auth: bool) -> meerkat_meta::RuntimeDecisionState {
    meerkat_meta::build_runtime_decision_state(meerkat_meta::RuntimeDecisionInputs {
        bigquery: meerkat_meta::BigQueryNaming {
            dataset: "topology_scope_dataset".to_string(),
            table: "topology_scope_table".to_string(),
        },
        trusted_meta_toml: r#"
[[modules]]
id = "router"
command = "router-bin"
args = ["--mode", "fast"]
restart_policy = "always"
"#
        .to_string(),
        auth: meerkat_meta::AuthPolicy {
            default_provider: meerkat_meta::AuthProvider::GoogleOAuth,
            email_allowlist: vec![
                "alice@example.com".to_string(),
                "bob@example.com".to_string(),
                "svc:robot".to_string(),
                "svc:other".to_string(),
            ],
        },
        trusted_oidc: meerkat_meta::TrustedOidcRuntimeConfig {
            discovery_json: serde_json::json!({
                "issuer": issuer,
                "jwks_uri": format!("{issuer}/.well-known/jwks.json")
            })
            .to_string(),
            jwks_json: r#"{"keys":[{"kid":"kid-current","kty":"oct","alg":"HS256","k":"cGhhc2U4LXRydXN0ZWQtY3VycmVudC1zZWNyZXQ"}]}"#
                .to_string(),
            audience: "meerkat-console".to_string(),
            require_verified_email: false,
        },
        console: meerkat_meta::ConsolePolicy {
            require_app_auth,
            ..meerkat_meta::ConsolePolicy::default()
        },
        ops: meerkat_meta::RuntimeOpsPolicy::default(),
        release_metadata_json: include_str!("../assets/release-targets.json").to_string(),
    })
    .expect("console decisions")
}

/// Console decisions that require sign-in with a token from the trusted
/// issuer.
fn authenticated_console_decisions() -> meerkat_meta::RuntimeDecisionState {
    console_decisions(TRUSTED_ISSUER, true)
}

/// A test token from `issuer` with `claims`, valid for five minutes.
fn signed_console_token(issuer: &str, mut claims: serde_json::Value) -> String {
    claims["iss"] = issuer.into();
    claims["aud"] = "meerkat-console".into();
    claims["exp"] = (chrono::Utc::now().timestamp() + 300).into();
    let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256);
    header.kid = Some("kid-current".to_string());
    jsonwebtoken::encode(
        &header,
        &claims,
        &jsonwebtoken::EncodingKey::from_secret(b"phase8-trusted-current-secret"),
    )
    .expect("console token")
}

/// A token from the trusted issuer for `subject`: a verified human, or a
/// service identity for an `svc:` name.
fn console_token(subject: &str) -> String {
    signed_console_token(
        TRUSTED_ISSUER,
        serde_json::json!({
            "sub": subject,
            "email": subject,
            "provider": "google_oauth",
        }),
    )
}

/// `meta/topology/apply` through the Console JSON-RPC route, signed in as
/// `subject`.
async fn console_apply(
    app: &axum::Router,
    subject: &str,
    request: &TopologyApplyRequest,
) -> serde_json::Value {
    console_apply_bearing(app, Some(&console_token(subject)), request).await
}

/// `meta/topology/apply` through the Console JSON-RPC route, bearing `token`
/// when there is one.
async fn console_apply_bearing(
    app: &axum::Router,
    token: Option<&str>,
    request: &TopologyApplyRequest,
) -> serde_json::Value {
    use tower::ServiceExt;

    let mut builder = axum::http::Request::builder()
        .method("POST")
        .uri("/console/rpc")
        .header("content-type", "application/json");
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    let response = app
        .clone()
        .oneshot(
            builder
                .body(axum::body::Body::from(
                    serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": 1,
                        "method": "meta/topology/apply",
                        "params": request,
                    })
                    .to_string(),
                ))
                .expect("console request"),
        )
        .await
        .expect("console response");
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("console body");
    serde_json::from_slice(&body).expect("console json")
}

/// `meta/topology/apply` through the gateway's JSON-RPC surface.
async fn gateway_apply(
    runtime: &UnifiedRuntime,
    request: &TopologyApplyRequest,
) -> serde_json::Value {
    let response = meerkat_meta::handle_unified_rpc_json(
        runtime,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "meta/topology/apply",
            "params": request,
        })
        .to_string(),
        Duration::from_secs(5),
        None,
        None,
    )
    .await;
    serde_json::from_str(&response).expect("gateway json")
}

/// A disconnect of alice and bob under `idempotency_key`.
fn disconnect_alice_bob(expected_revision: u64, idempotency_key: &str) -> TopologyApplyRequest {
    let mut request = connect_alice_bob(expected_revision, idempotency_key);
    request.operations[0].action = TopologyAction::Disconnect;
    request
}

/// The same key from two Console callers names two operations: the second
/// caller's identical request is evaluated as its own apply (here a revision
/// conflict, since the first apply advanced the revision), and the first
/// caller's retry still replays its own apply.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_same_key_from_two_console_callers_names_two_operations() {
    let _serial = TOPOLOGY_TEST_LOCK.lock().await;
    let temp = tempfile::tempdir().expect("temp dir");
    let runtime = build_runtime(temp.path(), "topology-scope-replay", "alice").await;
    spawn_bob(&runtime).await;
    let app = runtime.build_console_json_router(authenticated_console_decisions());
    let revision = runtime
        .topology_runtime_handle()
        .controller()
        .revision()
        .await;
    let request = connect_alice_bob(revision, "shared-key");

    let first = console_apply(&app, "alice@example.com", &request).await;
    let receipt = first["result"].clone();
    assert_eq!(receipt["status"], "applied", "{first}");

    let other = console_apply(&app, "bob@example.com", &request).await;
    assert!(
        other.get("result").is_none(),
        "the second caller's apply is its own: {other}"
    );
    assert_eq!(
        other["error"]["data"]["kind"], "topology_revision_conflict",
        "{other}"
    );

    let replay = console_apply(&app, "alice@example.com", &request).await;
    assert_eq!(replay["result"], receipt, "{replay}");
    shutdown(&runtime).await;
}

/// A second Console caller's different request under a key the first caller
/// used is evaluated as its own apply.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_second_console_caller_reusing_a_key_applies_its_own_request() {
    let _serial = TOPOLOGY_TEST_LOCK.lock().await;
    let temp = tempfile::tempdir().expect("temp dir");
    let runtime = build_runtime(temp.path(), "topology-scope-reuse", "alice").await;
    spawn_bob(&runtime).await;
    let app = runtime.build_console_json_router(authenticated_console_decisions());
    let revision = runtime
        .topology_runtime_handle()
        .controller()
        .revision()
        .await;

    let first = console_apply(
        &app,
        "alice@example.com",
        &connect_alice_bob(revision, "taken-key"),
    )
    .await;
    assert_eq!(first["result"]["status"], "applied", "{first}");

    let mut different = connect_alice_bob(revision + 1, "taken-key");
    different.reason = Some("a different caller's own request".to_string());
    let other = console_apply(&app, "bob@example.com", &different).await;
    assert!(
        other.get("error").is_none(),
        "the second caller's request is applied as its own: {other}"
    );
    assert_eq!(other["result"]["status"], "noop", "{other}");
    shutdown(&runtime).await;
}

/// The gateway's JSON-RPC surface and a Console caller do not share apply
/// keys: the Console caller's identical request under the key the gateway
/// applied is evaluated as its own apply.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_gateway_and_a_console_caller_do_not_share_apply_keys() {
    let _serial = TOPOLOGY_TEST_LOCK.lock().await;
    let temp = tempfile::tempdir().expect("temp dir");
    let runtime = build_runtime(temp.path(), "topology-scope-surfaces", "alice").await;
    spawn_bob(&runtime).await;
    let app = runtime.build_console_json_router(authenticated_console_decisions());
    let revision = runtime
        .topology_runtime_handle()
        .controller()
        .revision()
        .await;
    let request = connect_alice_bob(revision, "gateway-key");

    let gateway = gateway_apply(&runtime, &request).await;
    assert_eq!(gateway["result"]["status"], "applied", "{gateway}");

    let console = console_apply(&app, "alice@example.com", &request).await;
    assert!(
        console.get("result").is_none(),
        "the Console caller's apply is its own: {console}"
    );
    assert_eq!(
        console["error"]["data"]["kind"], "topology_revision_conflict",
        "{console}"
    );
    shutdown(&runtime).await;
}

/// The bilateral coordinator's apply keys are the naming principal's own: a
/// second principal's identical request under the same key is evaluated as
/// its own apply, while the first principal's retry still replays its apply.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bilateral_apply_keys_are_the_naming_principals_own() {
    let _serial = TOPOLOGY_TEST_LOCK.lock().await;
    let root = tempfile::tempdir().expect("root");
    let right = build_runtime(root.path(), "topology-scope-pair-b", "bob").await;
    let left = build_runtime(root.path(), "topology-scope-pair-a", "alice").await;
    let coordinator =
        SameProcessTopologyCoordinator::open(root.path().join("pair.json")).expect("coordinator");
    let initial = coordinator
        .query(&left, &right, None)
        .await
        .expect("initial query");
    let request = TopologyBilateralApplyRequest {
        expected_revisions: initial.authority_revisions,
        idempotency_key: "pair-key".to_string(),
        operations: vec![mutation(
            TopologyAction::Connect,
            &edge(
                "topology-scope-pair-a",
                "alice",
                "topology-scope-pair-b",
                "bob",
            ),
        )],
        reason: Some("scoped bilateral apply".to_string()),
        risk_tier: None,
    };

    let applied = coordinator
        .apply(
            &left,
            &right,
            request.clone(),
            Some("alice@example.com"),
            "alice@example.com",
        )
        .await
        .expect("the first principal applies");
    assert_eq!(applied.status, TopologyOperationStatus::Applied);

    let other = coordinator
        .apply(
            &left,
            &right,
            request.clone(),
            Some("bob@example.com"),
            "bob@example.com",
        )
        .await;
    assert!(
        matches!(
            other,
            Err(meerkat_meta::TopologyControlError::RevisionConflict { .. })
        ),
        "the second principal's apply is its own: {other:?}"
    );

    let replay = coordinator
        .apply(
            &left,
            &right,
            request,
            Some("alice@example.com"),
            "alice@example.com",
        )
        .await
        .expect("the first principal replays");
    assert_eq!(replay, applied);
    shutdown(&left).await;
    shutdown(&right).await;
}

/// A Console caller's own apply key keeps its semantics: its retry replays
/// its apply across a restart, and its reuse of the key for a different
/// request is still refused, while another caller's identical request after
/// the restart is still evaluated as its own.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_console_callers_own_apply_key_replays_after_restart_and_refuses_reuse() {
    let _serial = TOPOLOGY_TEST_LOCK.lock().await;
    let temp = tempfile::tempdir().expect("temp dir");
    let authority = "topology-scope-restart";
    let runtime = build_runtime(temp.path(), authority, "alice").await;
    spawn_bob(&runtime).await;
    let app = runtime.build_console_json_router(authenticated_console_decisions());
    let revision = runtime
        .topology_runtime_handle()
        .controller()
        .revision()
        .await;
    let request = connect_alice_bob(revision, "restart-key");
    let first = console_apply(&app, "alice@example.com", &request).await;
    let receipt = first["result"].clone();
    assert_eq!(receipt["status"], "applied", "{first}");
    drop(app);
    shutdown(&runtime).await;
    drop(runtime);

    let runtime = build_runtime(temp.path(), authority, "alice").await;
    spawn_bob(&runtime).await;
    let app = runtime.build_console_json_router(authenticated_console_decisions());
    let replay = console_apply(&app, "alice@example.com", &request).await;
    assert_eq!(replay["result"], receipt, "{replay}");

    let mut reused = request.clone();
    reused.reason = Some("the same caller's different request".to_string());
    let conflict = console_apply(&app, "alice@example.com", &reused).await;
    assert_eq!(
        conflict["error"]["data"]["kind"], "topology_idempotency_conflict",
        "{conflict}"
    );

    let other = console_apply(&app, "bob@example.com", &request).await;
    assert!(other.get("result").is_none(), "{other}");
    assert_eq!(
        other["error"]["data"]["kind"], "topology_revision_conflict",
        "{other}"
    );
    shutdown(&runtime).await;
}

/// The host's Rust API without a principal keeps its key shape: a host apply
/// replays to the host, and a Console caller never matches the host's key.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_hosts_unscoped_apply_key_is_not_matched_by_a_console_caller() {
    let _serial = TOPOLOGY_TEST_LOCK.lock().await;
    let temp = tempfile::tempdir().expect("temp dir");
    let runtime = build_runtime(temp.path(), "topology-scope-host", "alice").await;
    spawn_bob(&runtime).await;
    let app = runtime.build_console_json_router(authenticated_console_decisions());
    let topology = runtime.topology_runtime_handle();
    let revision = topology.controller().revision().await;
    let request = connect_alice_bob(revision, "host-key");
    let applied = topology
        .apply(request.clone(), "host")
        .await
        .expect("the host applies");
    assert_eq!(applied.status, TopologyOperationStatus::Applied);
    assert_eq!(
        topology
            .apply(request.clone(), "host")
            .await
            .expect("the host replays"),
        applied
    );

    let console = console_apply(&app, "alice@example.com", &request).await;
    assert!(console.get("result").is_none(), "{console}");
    assert_eq!(
        console["error"]["data"]["kind"], "topology_revision_conflict",
        "{console}"
    );
    shutdown(&runtime).await;
}

/// A Console principal without a verified human (here a service identity)
/// has its own apply keys, scoped by the trust source that verified it: its
/// retry replays its apply, while another service identity, a verified
/// human, and the same service identity verified by another issuer each
/// apply their own request under the same key.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_console_principal_without_a_verified_human_has_its_own_apply_keys() {
    let _serial = TOPOLOGY_TEST_LOCK.lock().await;
    let temp = tempfile::tempdir().expect("temp dir");
    let runtime = build_runtime(temp.path(), "topology-scope-principal", "alice").await;
    spawn_bob(&runtime).await;
    let trusted = runtime.build_console_json_router(console_decisions(TRUSTED_ISSUER, true));
    let other_issuer = runtime.build_console_json_router(console_decisions(OTHER_ISSUER, true));
    let revision = runtime
        .topology_runtime_handle()
        .controller()
        .revision()
        .await;
    let request = connect_alice_bob(revision, "principal-key");
    let robot = console_token("svc:robot");

    let first = console_apply_bearing(&trusted, Some(&robot), &request).await;
    let receipt = first["result"].clone();
    assert_eq!(receipt["status"], "applied", "{first}");
    let replay = console_apply_bearing(&trusted, Some(&robot), &request).await;
    assert_eq!(replay["result"], receipt, "{replay}");

    let robot_from_other_issuer = signed_console_token(
        OTHER_ISSUER,
        serde_json::json!({
            "sub": "svc:robot",
            "email": "svc:robot",
            "provider": "google_oauth",
        }),
    );
    for (caller, app, token) in [
        (
            "another service identity",
            &trusted,
            console_token("svc:other"),
        ),
        (
            "a verified human",
            &trusted,
            console_token("alice@example.com"),
        ),
        (
            "the same service identity verified by another issuer",
            &other_issuer,
            robot_from_other_issuer,
        ),
    ] {
        let own = console_apply_bearing(app, Some(&token), &request).await;
        assert!(own.get("result").is_none(), "{caller}: {own}");
        assert_eq!(
            own["error"]["data"]["kind"], "topology_revision_conflict",
            "{caller}: {own}"
        );
    }
    shutdown(&runtime).await;
}

/// A principal the host names on its Rust API has its own apply keys, apart
/// from a Console principal of the same name: the Console principal's
/// identical request under the key the host applied is evaluated as its own
/// apply.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_principal_the_host_names_is_not_a_console_principal_of_that_name() {
    let _serial = TOPOLOGY_TEST_LOCK.lock().await;
    let temp = tempfile::tempdir().expect("temp dir");
    let runtime = build_runtime(temp.path(), "topology-scope-named", "alice").await;
    spawn_bob(&runtime).await;
    let app = runtime.build_console_json_router(authenticated_console_decisions());
    let topology = runtime.topology_runtime_handle();
    let revision = topology.controller().revision().await;
    let request = connect_alice_bob(revision, "named-key");
    let applied = topology
        .apply_as(request.clone(), Some("bob@example.com"), "bob@example.com")
        .await
        .expect("the host applies for bob");
    assert_eq!(applied.status, TopologyOperationStatus::Applied);

    // A token without a subject: an authenticated principal, no verified
    // human.
    let principal = signed_console_token(
        TRUSTED_ISSUER,
        serde_json::json!({
            "email": "bob@example.com",
            "provider": "google_oauth",
        }),
    );
    let console = console_apply_bearing(&app, Some(&principal), &request).await;
    assert!(console.get("result").is_none(), "{console}");
    assert_eq!(
        console["error"]["data"]["kind"], "topology_revision_conflict",
        "{console}"
    );
    shutdown(&runtime).await;
}

/// An open Console without an access controller never resolves tokens: every
/// caller there, with a valid token or none, is the shared anonymous account
/// and shares its apply keys, while the gateway's surface keeps its own.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_open_consoles_callers_share_the_anonymous_apply_keys() {
    let _serial = TOPOLOGY_TEST_LOCK.lock().await;
    let temp = tempfile::tempdir().expect("temp dir");
    let runtime = build_runtime(temp.path(), "topology-scope-open", "alice").await;
    spawn_bob(&runtime).await;
    let app = runtime.build_console_json_router(console_decisions(TRUSTED_ISSUER, false));
    let revision = runtime
        .topology_runtime_handle()
        .controller()
        .revision()
        .await;
    let request = connect_alice_bob(revision, "open-key");

    let first =
        console_apply_bearing(&app, Some(&console_token("alice@example.com")), &request).await;
    let receipt = first["result"].clone();
    assert_eq!(receipt["status"], "applied", "{first}");
    for token in [Some(console_token("bob@example.com")), None] {
        let shared = console_apply_bearing(&app, token.as_deref(), &request).await;
        assert_eq!(shared["result"], receipt, "{shared}");
    }

    let gateway = gateway_apply(&runtime, &request).await;
    assert!(gateway.get("result").is_none(), "{gateway}");
    assert_eq!(
        gateway["error"]["data"]["kind"], "topology_revision_conflict",
        "{gateway}"
    );
    shutdown(&runtime).await;
}

/// Compacted history is per caller: once a Console caller's apply has aged
/// out of the retained receipts, its retry with that key is refused as
/// compacted history, while another caller's and the host's identical
/// requests under the same key are evaluated as their own applies.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_console_callers_compacted_apply_key_is_its_own() {
    let _serial = TOPOLOGY_TEST_LOCK.lock().await;
    let temp = tempfile::tempdir().expect("temp dir");
    let runtime = build_runtime_with_client_and_policy(
        temp.path(),
        "topology-scope-compacted",
        "alice",
        Arc::new(TestClient::for_provider(Provider::OpenAI)),
        TopologyControlPolicy {
            receipt_limit: 1,
            ..editable_policy()
        },
    )
    .await;
    spawn_bob(&runtime).await;
    let app = runtime.build_console_json_router(authenticated_console_decisions());
    let topology = runtime.topology_runtime_handle();
    let revision = topology.controller().revision().await;
    let request = connect_alice_bob(revision, "aged-key");

    let first = console_apply(&app, "alice@example.com", &request).await;
    assert_eq!(first["result"]["status"], "applied", "{first}");
    // The one retained receipt is now the second apply's.
    let second = console_apply(
        &app,
        "alice@example.com",
        &disconnect_alice_bob(revision + 1, "newer-key"),
    )
    .await;
    assert_eq!(second["result"]["status"], "applied", "{second}");

    let retry = console_apply(&app, "alice@example.com", &request).await;
    assert_eq!(
        retry["error"]["data"]["kind"], "topology_idempotency_history_compacted",
        "{retry}"
    );
    let other = console_apply(&app, "bob@example.com", &request).await;
    assert_eq!(
        other["error"]["data"]["kind"], "topology_revision_conflict",
        "{other}"
    );
    let host = topology
        .apply(request, "host")
        .await
        .expect_err("the host's identical request is its own apply");
    assert_eq!(host.kind(), "topology_revision_conflict");
    shutdown(&runtime).await;
}

/// A bilateral journal entry filed before keys were scoped, under a key that
/// begins with the reserved prefix, names no caller's apply: opening the
/// journal drops it and keeps every other entry.
#[test]
fn opening_a_journal_drops_reserved_prefix_entries_from_before_keys_were_scoped() {
    let root = tempfile::tempdir().expect("root");
    let journal = root.path().join("earlier-pair.json");
    std::fs::write(
        &journal,
        serde_json::to_vec_pretty(&serde_json::json!({
            "idempotency": {
                "apply-scoped:earlier": {"fingerprint": "f1", "operation_id": "o1"},
                "host-key": {"fingerprint": "f2", "operation_id": "o2"},
                "apply-scoped:filed": {"fingerprint": "f3", "operation_id": "o3", "scoped": true},
            }
        }))
        .expect("journal json"),
    )
    .expect("write journal");
    drop(SameProcessTopologyCoordinator::open(&journal).expect("open the journal"));

    let persisted: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&journal).expect("read journal"))
            .expect("journal json");
    let keys = persisted["idempotency"]
        .as_object()
        .expect("idempotency entries")
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(keys, vec!["apply-scoped:filed", "host-key"], "{persisted}");
}
