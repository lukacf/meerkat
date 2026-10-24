//! A mob member whose runtime executor stopped while its registration stayed
//! is rebuilt by the next turn on that same registration (the
//! MissingLiveMaterialization redrive). The member declares a real MCP
//! server, so the rebuilt construction composes a successor router for the
//! registration its predecessor served.
//!
//! The predecessor router's shutdown must have completed before the
//! successor's construction starts, the successor must serve the server, the
//! registration's tool surface must stay Operating and the member must stay
//! Active.
//!
//! Evidence limits: "shutdown completed" is the router's observable
//! retirement and cache state (taken out, and its tool cache cleared once the
//! shutdown wait ended), not a receipt of successful physical cleanup: the
//! cache is also cleared when the shutdown task ends without reporting, and
//! nothing here sees the server process exit. "Callable" is the router's
//! route table for the operation, not a tools/call round trip.

#![cfg(any(target_os = "linux", target_os = "macos"))]
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use meerkat_core::AgentToolDispatcher as _;
use meerkat_core::McpOperationObservation as Observation;
use meerkat_core::service::{CreateSessionRequest, SessionError};
use meerkat_mcp::McpRouterAdapter;
use meerkat_mob::{
    AgentIdentity, MobBuilder, MobDefinition, MobId, MobMemberStatus, MobStorage, ProfileBinding,
    ProfileName,
};
use meerkat_runtime::composition::{ConsumerSurface as _, OwnedFieldValue};
use meerkat_runtime::generated::meerkat_mob_seam as seam_facts;
use meerkat_session::SessionAgentBuilder;

const SERVER: &str = "fixture";
const OPERATION: &str = "echo";
const MEMBER: &str = "w-mcp-redrive";

/// One earlier router as a construction found it when it started.
#[derive(Debug, Clone)]
struct EarlierRouter {
    /// What the router reported without waiting; `None` when its state could
    /// not be read at once (a shutdown holds it while cleaning up), which
    /// counts as not completed.
    observed: Option<Observation>,
    /// The router had been taken out and its shutdown wait had ended
    /// (retirement and cache state, not proof of physical cleanup).
    shutdown_finished: bool,
}

/// What one construction found, when it started, about the routers that
/// earlier constructions composed (in composition order).
#[derive(Debug, Clone)]
struct ConstructionEntry {
    earlier_routers: Vec<EarlierRouter>,
}

/// Records, at the start of every construction, whether the routers that
/// earlier constructions composed have finished their shutdown, then builds
/// the real factory agent. The record never waits: instrumentation that
/// waited through a router's cleanup would itself supply the ordering under
/// test.
struct OrderingProbeBuilder {
    inner: meerkat::FactoryAgentBuilder,
    routers: Arc<Mutex<Vec<Arc<McpRouterAdapter>>>>,
    entries: Arc<Mutex<Vec<ConstructionEntry>>>,
}

impl OrderingProbeBuilder {
    fn record_entry(&self) {
        use futures::FutureExt as _;
        let routers = self.routers.lock().unwrap().clone();
        let mut earlier_routers = Vec::with_capacity(routers.len());
        for router in &routers {
            // One poll, outside the cooperative budget so an available state
            // is always read: a shutdown in progress holds the router's state
            // until its cleanup ends, and that reads as not completed. A
            // shutdown takes the router out (Retired) and clears its tool
            // cache only when its shutdown wait ends (finished, or the
            // shutdown task ended without reporting).
            let observed = tokio::task::unconstrained(router.observe_operation(SERVER, OPERATION))
                .now_or_never();
            let shutdown_finished =
                observed == Some(Observation::Retired) && router.tools().is_empty();
            earlier_routers.push(EarlierRouter {
                observed,
                shutdown_finished,
            });
        }
        self.entries
            .lock()
            .unwrap()
            .push(ConstructionEntry { earlier_routers });
    }
}

#[async_trait::async_trait]
impl SessionAgentBuilder for OrderingProbeBuilder {
    type Agent = meerkat::FactoryAgent;

    async fn model_supports_inline_video(
        &self,
        identity: &meerkat_core::SessionLlmIdentity,
    ) -> Option<bool> {
        self.inner.model_supports_inline_video(identity).await
    }

    async fn build_agent(
        &self,
        req: &CreateSessionRequest,
        event_tx: tokio::sync::mpsc::Sender<meerkat_core::AgentEvent>,
    ) -> Result<Self::Agent, SessionError> {
        self.record_entry();
        self.inner.build_agent(req, event_tx).await
    }

    async fn build_agent_taking_tools(
        &self,
        req: &mut CreateSessionRequest,
        event_tx: tokio::sync::mpsc::Sender<meerkat_core::AgentEvent>,
    ) -> Result<Self::Agent, SessionError> {
        self.record_entry();
        self.inner.build_agent_taking_tools(req, event_tx).await
    }

    async fn abort_absent_session_compaction_stages(
        &self,
        session_id: &meerkat_core::SessionId,
    ) -> Result<(), SessionError> {
        self.inner
            .abort_absent_session_compaction_stages(session_id)
            .await
    }
}

fn mcp_member_definition() -> MobDefinition {
    let profile = meerkat_mob::Profile {
        model_fallback: None,
        model: "claude-haiku-4-5-20251001".to_string(),
        provider: None,
        self_hosted_server_id: None,
        image_generation_provider: None,
        auto_compact_threshold: None,
        resume_overrides: Vec::new(),
        skills: Vec::new(),
        tools: meerkat_mob::ToolConfig {
            comms: true,
            mcp_servers: vec![meerkat_core::McpServerConfig::stdio(
                SERVER,
                mcp_test_server::fixture_binary()
                    .to_string_lossy()
                    .to_string(),
                Vec::new(),
                HashMap::new(),
            )],
            ..meerkat_mob::ToolConfig::default()
        },
        peer_description: "a worker with a declarative MCP server".to_string(),
        external_addressable: false,
        backend: Some(meerkat_mob::MobBackendKind::Session),
        runtime_mode: meerkat_mob::MobRuntimeMode::TurnDriven,
        max_inline_peer_notifications: None,
        output_schema: None,
        provider_params: None,
        durable: false,
    };
    let mut profiles = BTreeMap::new();
    profiles.insert(
        ProfileName::from("worker"),
        ProfileBinding::Inline(Box::new(profile)),
    );
    let mut definition = MobDefinition::explicit(MobId::from(format!(
        "mcp-redrive-{}",
        uuid::Uuid::new_v4().simple()
    )));
    definition.profiles = profiles;
    definition
}

async fn wait_callable(router: &McpRouterAdapter, what: &str) {
    let mut last = Observation::Absent;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while tokio::time::Instant::now() < deadline {
        // Take the router's finished connection results, as the agent does at
        // its boundaries; observing alone only reads the route table.
        let _ = router.poll_external_updates().await;
        last = router.observe_operation(SERVER, OPERATION).await;
        if matches!(last, Observation::Callable { .. }) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("{what}: the fixture server never became callable, last observed {last:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_live_redrive_retires_the_predecessor_router_before_the_successor_prepares() {
    let temp = tempfile::TempDir::new().expect("factory roots");
    let root = temp.path();
    for directory in ["store", "user", "runtime", "project", "context"] {
        std::fs::create_dir_all(root.join(directory)).expect("create isolated factory root");
    }
    let routers: Arc<Mutex<Vec<Arc<McpRouterAdapter>>>> = Arc::default();
    let observer = Arc::clone(&routers);
    let factory = meerkat::AgentFactory::new(root.join("store"))
        .user_config_root(root.join("user"))
        .runtime_root(root.join("runtime"))
        .project_root(root.join("project"))
        .context_root(root.join("context"))
        .builtins(false)
        .comms(true)
        .with_mcp_router_observer_for_test(move |router| {
            observer.lock().unwrap().push(router);
        });
    let mut inner = meerkat::FactoryAgentBuilder::new(factory, meerkat::Config::default());
    inner.default_llm_client = Some(Arc::new(meerkat_llm_core::TestClient::default()));
    let session_store = Arc::new(meerkat_store::MemoryStore::new());
    inner.default_session_store = Some(Arc::new(meerkat_store::StoreAdapter::new(Arc::clone(
        &session_store,
    ))));
    let entries: Arc<Mutex<Vec<ConstructionEntry>>> = Arc::default();
    // A store-backed service keeps the member's session snapshot, so a
    // missing live materialization can be rebuilt from it.
    let service = Arc::new(meerkat_session::PersistentSessionService::new(
        OrderingProbeBuilder {
            inner,
            routers: Arc::clone(&routers),
            entries: Arc::clone(&entries),
        },
        16,
        session_store,
        Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
        Arc::new(meerkat_store::MemoryBlobStore::default()),
    ));
    let handle = MobBuilder::new(mcp_member_definition(), MobStorage::in_memory())
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create the mob");
    let adapter = service
        .acquire_canonical_runtime_adapter(None)
        .expect("the service's runtime adapter");

    let identity = AgentIdentity::from(MEMBER);
    handle
        .spawn_spec(meerkat_mob::SpawnMemberSpec::new("worker", MEMBER))
        .await
        .expect("spawn the MCP member");
    let session_id = handle
        .resolve_bridge_session_id(&identity)
        .await
        .expect("session-backed member");
    let initial = handle
        .member_status(&identity)
        .await
        .expect("initial member status");
    let (agent_runtime_id, fence_token) = initial
        .runtime_identity_fields()
        .map(|(id, fence)| (id.clone(), fence))
        .expect("active member has a runtime identity");
    let predecessor = Arc::clone(
        routers
            .lock()
            .unwrap()
            .last()
            .expect("the spawn composed the member's router"),
    );
    // A first turn lets the member's agent take its finished connection.
    handle
        .member(&identity)
        .await
        .expect("member handle")
        .internal_turn(meerkat_core::ContentInput::from("first turn".to_string()))
        .await
        .expect("first turn");
    wait_callable(&predecessor, "spawned member").await;

    // A completed stop leaves the registration without an executor.
    adapter
        .stop_runtime_executor(&session_id, "completed limbo shutdown")
        .await
        .expect("stop the member's runtime executor");
    assert!(
        adapter.contains_session(&session_id).await,
        "a completed stop keeps the registration"
    );
    let bindings = adapter
        .prepare_local_session_bindings(session_id.clone())
        .await
        .expect("delivery-time preparation readmits the stopped runtime");
    assert!(
        !adapter
            .session_has_executor(&session_id)
            .await
            .expect("inspect the readmitted runtime"),
        "binding preparation fabricates no executor"
    );
    meerkat_runtime::MeerkatConsumerSurface::pinned_unobserved_for_test(
        Arc::clone(&adapter),
        session_id.clone(),
    )
    .apply_routed_input(
        seam_facts::inputs::prepare_bindings(),
        vec![
            (
                seam_facts::fields::agent_runtime_id(),
                OwnedFieldValue::Str(agent_runtime_id.to_string()),
            ),
            (
                seam_facts::fields::fence_token(),
                OwnedFieldValue::U64(fence_token.get()),
            ),
            (
                seam_facts::fields::generation(),
                OwnedFieldValue::U64(agent_runtime_id.generation.get()),
            ),
            (
                seam_facts::fields::session_id(),
                OwnedFieldValue::Str(session_id.to_string()),
            ),
        ],
    )
    .await
    .expect("the composition route binds the stale tuple");
    let constructions_before_turn = entries.lock().unwrap().len();

    // The next turn redrives the missing live materialization on the same
    // registration.
    tokio::time::timeout(
        Duration::from_secs(60),
        handle
            .member(&identity)
            .await
            .expect("member handle")
            .internal_turn(meerkat_core::ContentInput::from(
                "execute after the missing-live redrive".to_string(),
            )),
    )
    .await
    .expect("the redrive does not wedge")
    .expect("the redrive and its turn succeed");

    let entries = entries.lock().unwrap().clone();
    let successor_entry = entries
        .get(constructions_before_turn)
        .cloned()
        .unwrap_or_else(|| panic!("the redrive built the member again: {entries:?}"));
    let routers = routers.lock().unwrap().clone();
    let successor = Arc::clone(routers.last().expect("the redrive composed a router"));
    assert!(
        !Arc::ptr_eq(&predecessor, &successor),
        "the redrive composes its own router"
    );

    // (a) The recovered member's server is callable through its own router
    // and through the registration's installed observer.
    wait_callable(&successor, "recovered member").await;
    let installed = bindings
        .mcp_observation_slot()
        .and_then(|slot| slot.current())
        .and_then(|(_, observer)| observer)
        .expect("the successor is installed on the registration");
    assert!(
        matches!(
            installed.observe_operation(SERVER, OPERATION).await,
            Observation::Callable { .. }
        ),
        "the registration observes the recovered server as callable"
    );

    // (c) The registration's surface stays Operating, with the server Active
    // under the recovered router.
    assert_eq!(
        bindings
            .external_tool_surface()
            .surface_snapshot(SERVER)
            .and_then(|entry| entry.base_state),
        Some(meerkat_core::ExternalToolSurfaceBaseState::Active),
        "the recovered router keeps the server Active on the registration"
    );
    assert_eq!(
        bindings
            .external_tool_surface()
            .diagnostic_snapshot()
            .surface_phase,
        meerkat_core::ExternalToolSurfaceGlobalPhase::Operating,
        "the redrive leaves the registration's surface Operating"
    );

    // (d) The member stays Active with the same runtime identity.
    let status = handle
        .member_status(&identity)
        .await
        .expect("final member status");
    assert_eq!(status.status, MobMemberStatus::Active, "{status:?}");
    assert!(!status.is_final);
    let (final_runtime_id, final_fence) = status
        .runtime_identity_fields()
        .expect("the recovered member keeps its runtime identity");
    assert_eq!(final_runtime_id, &agent_runtime_id);
    assert_eq!(final_fence, fence_token);

    // (b) The predecessor was retired and its shutdown wait had ended before
    // the successor's construction started (observable retirement and cache
    // state, not physical cleanup).
    let predecessor_index = routers
        .iter()
        .position(|router| Arc::ptr_eq(router, &predecessor))
        .expect("predecessor recorded");
    let at_successor_entry = successor_entry
        .earlier_routers
        .get(predecessor_index)
        .cloned()
        .unwrap_or_else(|| {
            panic!("the successor's construction saw the predecessor: {successor_entry:?}")
        });
    let predecessor_now = predecessor.observe_operation(SERVER, OPERATION).await;
    handle.shutdown().await.expect("release the test mob");
    assert!(
        at_successor_entry.shutdown_finished,
        "the predecessor router's shutdown must finish before the successor prepares: \
         at the successor's construction entry the predecessor read {:?} (None: busy), \
         finished shutting down: {}; after the redrive it is {predecessor_now:?}",
        at_successor_entry.observed, at_successor_entry.shutdown_finished,
    );
}
