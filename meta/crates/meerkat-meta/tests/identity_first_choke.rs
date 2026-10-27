#![allow(
    clippy::all,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    unused_imports,
    redundant_semicolons,
    clippy::redundant_clone
)]
//! Integration chokepoint tests for identity-first continuity (Phase 4).
//!
//! CHOKE tests verify data boundaries between components. Each test traces
//! data flow across a boundary and asserts on both sides.
//!
//! Test naming convention: `identity_first_choke_NN_<description>`
//! to match the `test(identity_first)` filter.
//!
//! Note on gateway-dependent CHOKEs (CHOKE-08 through CHOKE-10, CHOKE-13
//! through CHOKE-15): These trace data across the gateway wire format and are
//! partially covered by the gateway bridge unit tests in `gateway_bridges.rs`.
//! The Rust-side chokepoint (serialization + deserialization of typed newtypes)
//! is verified here; the full SDK round-trip is in the Python/TypeScript test suites.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use meerkat_meta::identity_first::contracts::{
    AgentCustomizer, ContinuityStore, LeaseProvider, TopologyProvider,
};
use meerkat_meta::identity_first::gateway_bridges::{CallbackBridge, GatewayContinuityStore};
use meerkat_meta::identity_first::orchestrator::{RestoreOutcome, restore_flow};
use meerkat_meta::identity_first::{
    AgentAddressability, AgentBuildContext, AgentBuildDraft, AgentIdentity, AgentRuntimeId,
    CheckpointVersion, ContinuityFailure, ContinuityFailureKind, ContinuityGeneration,
    ContinuityRecord, ContinuityResolveState, ContinuityStoreError, CustomizerError, DispatchInput,
    DispatchOrigin, DurabilityPolicy, DurableAgentSpec, ExternalToolDef, FencingToken,
    IdentityLifecycleState, IdentityRuntime, IdentityRuntimeConfig, IdentityRuntimeError,
    LeaseAcquireResult, LeaseGrant, LeaseRenewResult, ManagedPeerEdge, SessionSnapshot,
    TopologyContext, TopologyError,
};
use meerkat_meta::identity_first::{
    ContinuitySessionStoreAdapter, LocalContinuityStore, LocalLeaseProvider,
    SessionHookCustomizerAdapter,
};
use serde_json::{Value, json};

// ===========================================================================
// Helpers
// ===========================================================================

fn make_identity(name: &str) -> AgentIdentity {
    AgentIdentity::parse(name).unwrap()
}

fn make_spec(name: &str) -> DurableAgentSpec {
    DurableAgentSpec {
        identity: make_identity(name),
        profile: meerkat_mob::ProfileName::from("default"),
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

fn make_internal_spec(name: &str) -> DurableAgentSpec {
    DurableAgentSpec {
        addressability: AgentAddressability::InternalOnly,
        ..make_spec(name)
    }
}

fn make_runtime(store: Arc<dyn ContinuityStore>, lease: Arc<dyn LeaseProvider>) -> IdentityRuntime {
    IdentityRuntime::new(IdentityRuntimeConfig {
        continuity_store: store,
        lease_provider: lease,
        runtime_instance_id: "choke-runtime".to_string(),
        has_runtime_store: false,
        durability_policy: DurabilityPolicy::SyncWriteThrough,
        bridge: None,
        default_timeout: None,
    })
}

#[path = "support/core_bridge.rs"]
mod core_bridge;
#[path = "support/core_members.rs"]
mod core_members;

/// [`make_runtime`] with a session bridge whose Core is the members fixture,
/// for scenarios that need Core to build members.
fn make_runtime_with_core_bridge(
    store: Arc<dyn ContinuityStore>,
    lease: Arc<dyn LeaseProvider>,
) -> IdentityRuntime {
    IdentityRuntime::new(IdentityRuntimeConfig {
        continuity_store: store,
        lease_provider: lease,
        runtime_instance_id: "choke-runtime".to_string(),
        has_runtime_store: false,
        durability_policy: DurabilityPolicy::SyncWriteThrough,
        bridge: Some(Arc::new(core_bridge::CoreBridge::default())),
        default_timeout: None,
    })
}

fn make_content() -> meerkat_core::ContentInput {
    meerkat_core::ContentInput::Text("hello".to_string())
}

fn make_dispatch_input() -> DispatchInput {
    DispatchInput {
        content: make_content(),
        origin: DispatchOrigin::Connector,
        correlation_id: None,
        idempotency_key: None,
    }
}

// ===========================================================================
// Mock callback bridge for gateway tests
// ===========================================================================

struct MockBridgeInner {
    calls: tokio::sync::Mutex<Vec<(String, Value)>>,
    responses: tokio::sync::Mutex<std::collections::HashMap<String, Result<Value, String>>>,
}

/// Newtype wrapper to satisfy orphan rules for `CallbackBridge` impl.
struct MockBridge(Arc<MockBridgeInner>);

impl MockBridge {
    #[allow(dead_code)]
    async fn last_call(&self) -> (String, Value) {
        self.0
            .calls
            .lock()
            .await
            .last()
            .cloned()
            .unwrap_or_else(|| (String::new(), Value::Null))
    }
}

#[async_trait]
impl CallbackBridge for MockBridge {
    async fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        self.0.calls.lock().await.push((method.to_string(), params));
        let responses = self.0.responses.lock().await;
        match responses.get(method) {
            Some(Ok(v)) => Ok(v.clone()),
            Some(Err(e)) => Err(e.clone()),
            None => Ok(Value::Null),
        }
    }
}

// ===========================================================================
// CHOKE-01: RosterProvider → DurableAgentSpec → ContinuityStore.resolve_many
// ===========================================================================

// ===========================================================================
// CHOKE-02: LeaseProvider.acquire → FencingToken → ContinuityStore.save
// ===========================================================================

// ===========================================================================
// CHOKE-03: ContinuityStore.load_session_snapshot → Meerkat session restore
// ===========================================================================

// ===========================================================================
// CHOKE-04: TopologyProvider edges → comms wiring
// ===========================================================================

#[tokio::test]
async fn identity_first_choke_04_topology_edges_to_wiring() {
    struct ExactTopology;

    #[async_trait]
    impl TopologyProvider for ExactTopology {
        async fn compute_edges(
            &self,
            _target: &[AgentIdentity],
            _ctx: &TopologyContext,
        ) -> Result<Vec<ManagedPeerEdge>, TopologyError> {
            Ok(vec![
                ManagedPeerEdge::new(
                    AgentIdentity::parse("a:main").unwrap(),
                    AgentIdentity::parse("b:main").unwrap(),
                )
                .unwrap(),
            ])
        }
    }

    let store = Arc::new(LocalContinuityStore::in_memory().unwrap());
    let lease_prov = Arc::new(LocalLeaseProvider::new());
    let runtime = make_runtime(store.clone(), lease_prov.clone());

    let roster = vec![make_spec("a:main"), make_spec("b:main")];
    let result = restore_flow(&runtime, &roster, Some(&ExactTopology), None)
        .await
        .unwrap();

    // Verify the topology provider result flows through to restore_flow output
    assert_eq!(result.managed_edges.len(), 1);
    let edge = &result.managed_edges[0];
    assert_eq!(edge.a(), &make_identity("a:main"));
    assert_eq!(edge.b(), &make_identity("b:main"));

    // Verify both identities are registered (wiring would use their AgentRuntimeIds)
    assert!(runtime.contains(&make_identity("a:main")).await);
    assert!(runtime.contains(&make_identity("b:main")).await);
}

// ===========================================================================
// CHOKE-05: Builder persistent_state → bundled LocalContinuityStore + LocalLeaseProvider
// ===========================================================================

// ===========================================================================
// CHOKE-06: DurableAgentSpec.addressability → send/dispatch enforcement
// ===========================================================================

#[tokio::test]
async fn identity_first_choke_06_addressability_to_delivery_enforcement() {
    let store = Arc::new(LocalContinuityStore::in_memory().unwrap());
    let lease = Arc::new(LocalLeaseProvider::new());
    let runtime = make_runtime(store, lease);

    // Register Addressable
    runtime.register(make_spec("public:main")).await;

    // Register InternalOnly
    runtime.register(make_internal_spec("internal:main")).await;

    // Trace: addressability from roster → runtime → delivery enforcement
    let status_public = runtime.status(&make_identity("public:main")).await.unwrap();
    assert_eq!(
        status_public.addressability,
        AgentAddressability::Addressable
    );

    let status_internal = runtime
        .status(&make_identity("internal:main"))
        .await
        .unwrap();
    assert_eq!(
        status_internal.addressability,
        AgentAddressability::InternalOnly
    );

    // send() enforcement matches addressability. This runtime has no session
    // bridge, so no member exists to deliver to (#306: Core holds members);
    // addressability is judged before delivery either way.
    assert!(!matches!(
        runtime
            .send(&make_identity("public:main"), &make_content())
            .await,
        Err(IdentityRuntimeError::NotAddressable(_))
    ));
    assert!(matches!(
        runtime
            .send(&make_identity("internal:main"), &make_content())
            .await,
        Err(IdentityRuntimeError::NotAddressable(_))
    ));

    // dispatch() is not refused as not addressable for either
    assert!(!matches!(
        runtime
            .dispatch(&make_identity("public:main"), &make_dispatch_input())
            .await,
        Err(IdentityRuntimeError::NotAddressable(_))
    ));
    assert!(!matches!(
        runtime
            .dispatch(&make_identity("internal:main"), &make_dispatch_input())
            .await,
        Err(IdentityRuntimeError::NotAddressable(_))
    ));
}

// ===========================================================================
// CHOKE-07: AgentIdentity → AgentRuntimeId → MeerkatId mapping
// ===========================================================================

// ===========================================================================
// CHOKE-08: Gateway wire format — newtypes as primitives
// ===========================================================================

#[tokio::test]
async fn identity_first_choke_08_gateway_newtype_serialization() {
    // Verify Rust newtypes serialize to JSON primitives correctly
    let id = AgentIdentity::parse("triage:main").unwrap();
    let id_json = serde_json::to_value(&id).unwrap();
    assert!(
        id_json.is_string(),
        "AgentIdentity must serialize as string"
    );
    assert_eq!(id_json.as_str().unwrap(), "triage:main");

    let ft = FencingToken::new(42);
    let ft_json = serde_json::to_value(&ft).unwrap();
    assert!(
        ft_json.is_number(),
        "FencingToken must serialize as integer"
    );
    assert_eq!(ft_json.as_u64().unwrap(), 42);

    let cv = CheckpointVersion::new(7);
    let cv_json = serde_json::to_value(&cv).unwrap();
    assert!(
        cv_json.is_number(),
        "CheckpointVersion must serialize as integer"
    );
    assert_eq!(cv_json.as_u64().unwrap(), 7);

    let cg = ContinuityGeneration::new(3);
    let cg_json = serde_json::to_value(&cg).unwrap();
    assert!(
        cg_json.is_number(),
        "ContinuityGeneration must serialize as integer"
    );
    assert_eq!(cg_json.as_u64().unwrap(), 3);

    // LeaseGrant.ttl serialized as integer ms via custom serde
    let grant = LeaseGrant {
        identity: id.clone(),
        fencing_token: ft,
        ttl: Duration::from_secs(5),
    };
    let grant_json = serde_json::to_value(&grant).unwrap();
    let ttl_val = &grant_json["ttl"];
    assert!(
        ttl_val.is_number(),
        "ttl should serialize as ms integer, got: {grant_json}"
    );
    assert_eq!(ttl_val.as_u64().unwrap(), 5000);

    // Round-trip: deserialize back
    let id_back: AgentIdentity = serde_json::from_value(id_json).unwrap();
    assert_eq!(id_back, id);

    let ft_back: FencingToken = serde_json::from_value(ft_json).unwrap();
    assert_eq!(ft_back, ft);

    let cv_back: CheckpointVersion = serde_json::from_value(cv_json).unwrap();
    assert_eq!(cv_back, cv);
}

// ===========================================================================
// CHOKE-09: Python/TS DurableAgentSpec → gateway → Rust DurableAgentSpec
// ===========================================================================

#[tokio::test]
async fn identity_first_choke_09_durable_agent_spec_gateway_round_trip() {
    let spec = DurableAgentSpec {
        identity: make_identity("triage:main"),
        profile: meerkat_mob::ProfileName::from("expert"),
        addressability: AgentAddressability::Addressable,
        display_name: Some(
            meerkat_meta::identity_first::DisplayName::parse("Triage Lead").unwrap(),
        ),
        labels: {
            let mut m = BTreeMap::new();
            m.insert("env".to_string(), "prod".to_string());
            m.insert("team".to_string(), "alpha".to_string());
            m
        },
        context: Some(serde_json::json!({"app_key": "value123"})),
        additional_instructions: vec!["be thorough".to_string()],
        initial_message: None,
        runtime_mode_override: None,
        backend: None,
        binding: None,
        placement: None,
    };

    // Serialize to JSON (simulates gateway wire)
    let json = serde_json::to_value(&spec).unwrap();

    // Verify key fields survive
    assert_eq!(json["identity"].as_str().unwrap(), "triage:main");
    assert_eq!(json["labels"]["env"].as_str().unwrap(), "prod");

    // Deserialize back (simulates Rust receiving from gateway)
    let recovered: DurableAgentSpec = serde_json::from_value(json).unwrap();
    assert_eq!(recovered.identity, spec.identity);
    assert_eq!(recovered.addressability, spec.addressability);
    assert_eq!(recovered.labels, spec.labels);
    assert_eq!(recovered.context, spec.context);
    assert_eq!(
        recovered.additional_instructions,
        spec.additional_instructions
    );
}

// ===========================================================================
// CHOKE-10: Provider → gateway callback → Rust trait impl
// ===========================================================================

// ===========================================================================
// CHOKE-11: dispatch → runtime substrate → ack semantics
// ===========================================================================

// ===========================================================================
// CHOKE-12: Active owner → fence/retire → destructive continuity mutation
// ===========================================================================

// ===========================================================================
// CHOKE-13: Lease renewal → behavioral transition on loss
// ===========================================================================

// ===========================================================================
// CHOKE-14: IdentityStatus wire format across languages
// ===========================================================================

#[tokio::test]
async fn identity_first_choke_14_identity_status_wire_format() {
    let store = Arc::new(LocalContinuityStore::in_memory().unwrap());
    let lease = Arc::new(LocalLeaseProvider::new());
    let runtime = make_runtime(store, lease);

    let mut spec = make_spec("triage:main");
    spec.labels
        .insert("env".to_string(), "production".to_string());
    spec.labels.insert("tier".to_string(), "1".to_string());

    runtime.register(spec).await;

    let status = runtime.status(&make_identity("triage:main")).await.unwrap();

    // Serialize to JSON (simulates gateway wire)
    let json = serde_json::to_value(&status).unwrap();

    // Verify all fields survive serialization
    assert_eq!(json["identity"].as_str().unwrap(), "triage:main");
    // No session bridge, so Core holds no member: dormant (#306).
    assert_eq!(json["state"].as_str().unwrap(), "dormant");

    // Labels as string map
    let labels = json["labels"].as_object().unwrap();
    assert_eq!(labels["env"].as_str().unwrap(), "production");
    assert_eq!(labels["tier"].as_str().unwrap(), "1");

    // The generation is Core's member generation: none without a member.
    assert!(json["generation"].is_null());

    // Meta's continuity facts left the wire with the continuity store (#306).
    for removed in [
        "checkpoint_version",
        "lease",
        "continuity_health",
        "continuity_unrecoverable",
    ] {
        assert!(
            json.get(removed).is_none(),
            "{removed} must be gone: {json}"
        );
    }
}

// ===========================================================================
// CHOKE-15: Destructive lifecycle ops over gateway bridge
// ===========================================================================

// ===========================================================================
// CHOKE-16: ContinuityStore → SessionStore adapter → Meerkat SessionService
// ===========================================================================

#[tokio::test]
async fn identity_first_choke_16_continuity_session_store_adapter() {
    let store = Arc::new(LocalContinuityStore::in_memory().unwrap());
    let adapter = ContinuitySessionStoreAdapter::new(store.clone());

    let sid = meerkat_core::types::SessionId::new();

    // Meerkat session restore reads flow through to ContinuityStore
    let loaded: Option<meerkat_core::Session> =
        meerkat::SessionStore::load(&adapter, &sid).await.unwrap();
    assert!(loaded.is_none(), "load from empty store should return None");

    // The adapter delegates to ContinuityStore, not local SQLite.
    // No dual-write: there's one authoritative truth (ContinuityStore).
}

// ===========================================================================
// CHOKE-17: TopologyProvider → AgentBuildContext → AgentCustomizer
// ===========================================================================

#[tokio::test]
async fn identity_first_choke_17_topology_to_customizer_context() {
    let store = Arc::new(LocalContinuityStore::in_memory().unwrap());
    let lease_prov = Arc::new(LocalLeaseProvider::new());
    // The customizer runs when Core builds a member, so the restore needs a bridge.
    let runtime = make_runtime_with_core_bridge(store.clone(), lease_prov.clone());

    let seen_contexts = Arc::new(tokio::sync::Mutex::new(Vec::new()));

    struct Tracker {
        seen: Arc<tokio::sync::Mutex<Vec<AgentBuildContext>>>,
    }

    #[async_trait]
    impl AgentCustomizer for Tracker {
        async fn customize_build(
            &self,
            context: &AgentBuildContext,
            _spec: &DurableAgentSpec,
            _draft: &mut AgentBuildDraft,
        ) -> Result<(), CustomizerError> {
            self.seen.lock().await.push(context.clone());
            Ok(())
        }
    }

    struct FullMeshTopology;

    #[async_trait]
    impl TopologyProvider for FullMeshTopology {
        async fn compute_edges(
            &self,
            target: &[AgentIdentity],
            _ctx: &TopologyContext,
        ) -> Result<Vec<ManagedPeerEdge>, TopologyError> {
            let mut edges = Vec::new();
            for i in 0..target.len() {
                for j in (i + 1)..target.len() {
                    if let Ok(e) = ManagedPeerEdge::new(target[i].clone(), target[j].clone()) {
                        edges.push(e);
                    }
                }
            }
            Ok(edges)
        }
    }

    let customizer = Tracker {
        seen: seen_contexts.clone(),
    };
    let roster = vec![
        make_spec("a:main"),
        make_spec("b:main"),
        make_spec("c:main"),
    ];
    restore_flow(
        &runtime,
        &roster,
        Some(&FullMeshTopology),
        Some(&customizer),
    )
    .await
    .unwrap();

    let contexts = seen_contexts.lock().await;
    assert_eq!(contexts.len(), 3, "customizer called once per identity");

    for ctx in contexts.iter() {
        // On cold boot, active_peers includes ALL identities being activated
        assert_eq!(ctx.active_peers.len(), 3);
        // managed_edges from topology: full mesh of 3 → 3 edges
        assert_eq!(ctx.managed_edges.len(), 3);
        // Context populated BEFORE customize_build (sequencing verified by the fact
        // that customize_build sees the data)
    }
}

// ===========================================================================
// CHOKE-18: Reconciliation hot-update → runtime registry → behavioral change
// ===========================================================================

#[tokio::test]
async fn identity_first_choke_18_hot_update_registry_behavioral_change() {
    let store = Arc::new(LocalContinuityStore::in_memory().unwrap());
    let lease = Arc::new(LocalLeaseProvider::new());
    let runtime = make_runtime(store, lease);

    // Register as InternalOnly
    runtime.register(make_internal_spec("agent:main")).await;

    let id = make_identity("agent:main");

    // send() rejected (InternalOnly)
    assert!(matches!(
        runtime.send(&id, &make_content()).await,
        Err(IdentityRuntimeError::NotAddressable(_))
    ));

    // Hot-update: flip addressability to Addressable
    let new_spec = make_spec("agent:main"); // Addressable
    runtime.update_spec(new_spec).await.unwrap();

    // send() is no longer refused as not addressable, no restart needed
    // (this runtime has no session bridge, so no member exists to deliver to)
    assert!(!matches!(
        runtime.send(&id, &make_content()).await,
        Err(IdentityRuntimeError::NotAddressable(_))
    ));

    // Hot-update: labels change visible in status()
    let mut labeled_spec = make_spec("agent:main");
    labeled_spec
        .labels
        .insert("team".to_string(), "alpha".to_string());
    runtime.update_spec(labeled_spec).await.unwrap();

    let status = runtime.status(&id).await.unwrap();
    assert_eq!(status.labels.get("team"), Some(&"alpha".to_string()));
}

// ===========================================================================
// CHOKE-19: Roster identity uniqueness validation
// ===========================================================================

#[tokio::test]
async fn identity_first_choke_19_roster_uniqueness_validation() {
    let store = Arc::new(LocalContinuityStore::in_memory().unwrap());
    let lease_prov = Arc::new(LocalLeaseProvider::new());
    let runtime = make_runtime(store.clone(), lease_prov.clone());

    // Duplicate identities
    let roster = vec![make_spec("dup:main"), make_spec("dup:main")];
    let result = restore_flow(&runtime, &roster, None, None).await;

    // Must fail with structured error
    match result {
        Err(IdentityRuntimeError::DuplicateIdentity(id)) => {
            assert_eq!(
                id,
                make_identity("dup:main"),
                "error must name the duplicated identity"
            );
        }
        Err(other) => panic!("expected DuplicateIdentity, got: {other}"),
        Ok(_) => panic!("expected error for duplicate identities"),
    }

    // No partial activation
    assert!(!runtime.contains(&make_identity("dup:main")).await);

    // No leases acquired, no sessions created, no continuity records written
    let resolved = store
        .resolve_many(&[make_identity("dup:main")])
        .await
        .unwrap();
    assert!(matches!(
        resolved.get(&make_identity("dup:main")).unwrap(),
        ContinuityResolveState::Uninitialized
    ));
}

// ===========================================================================
// CHOKE-20: SessionHook → AgentCustomizer adapter — unsupported mutation detection
// ===========================================================================

#[tokio::test]
async fn identity_first_choke_20_session_hook_adapter_unsupported_mutation() {
    use meerkat_meta::mob_handle_runtime::SessionHook;

    struct MutatesUnsupported;

    #[async_trait]
    impl SessionHook for MutatesUnsupported {
        async fn before_create(
            &self,
            req: &mut meerkat_core::service::CreateSessionRequest,
        ) -> Result<(), meerkat_core::service::SessionError> {
            // Supported mutations
            req.model = "claude-opus-4-6".to_string();
            // meerkat 0.7: CreateSessionRequest.system_prompt is the typed
            // tri-state SystemPromptOverride.
            req.system_prompt =
                meerkat_core::config::SystemPromptOverride::Set("new prompt".to_string());
            // Unsupported mutation
            req.max_tokens = Some(8192);
            Ok(())
        }
    }

    let hook: Arc<dyn SessionHook> = Arc::new(MutatesUnsupported);
    let adapter = SessionHookCustomizerAdapter::new(hook);

    let ctx = AgentBuildContext {
        identity: make_identity("triage:main"),
        active_peers: vec![],
        managed_edges: vec![],
        runtime_services: Default::default(),
    };
    let spec = make_spec("triage:main");
    let mut draft = AgentBuildDraft {
        compaction_curator: Default::default(),
        model: None,
        system_prompt: None,
        additional_instructions: vec![],
        labels: BTreeMap::new(),
        app_context: None,
        external_tools: vec![],
        local_external_tools: Default::default(),
        provider_params: None,
    };

    // Should succeed (unsupported mutations are warned, not errored)
    adapter
        .customize_build(&ctx, &spec, &mut draft)
        .await
        .unwrap();

    // Supported mutations ARE applied
    assert_eq!(draft.model.as_deref(), Some("claude-opus-4-6"));
    assert_eq!(draft.system_prompt.as_deref(), Some("new prompt"));

    // Unsupported mutations are NOT applied to draft
    // (draft has no max_tokens field — the adapter discards it after warning)
}
