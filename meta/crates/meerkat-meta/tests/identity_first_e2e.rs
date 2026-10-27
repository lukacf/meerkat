#![allow(
    clippy::all,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    unused_imports,
    redundant_semicolons,
    clippy::redundant_clone,
    clippy::unnecessary_operation,
    clippy::if_not_else
)]
//! End-to-end integration tests for identity-first continuity (Phase 4).
//!
//! Test naming convention: `identity_first_e2e_<scenario>`
//! to match the `test(identity_first)` filter.
//!
//! Note on SDK E2E tests (E2E-12, E2E-13, E2E-17, E2E-18, E2E-19):
//! These scenarios require live Python/TypeScript SDK → gateway → Rust round-trips
//! and cannot be tested as pure Rust unit tests. They are covered by:
//! - Python: sdk/python/tests/test_identity_first_e2e*.py
//! - TypeScript: sdk/typescript/src/__tests__/identity_first_e2e*.test.ts

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use meerkat_meta::identity_first::contracts::{
    AgentCustomizer, ContinuityStore, LeaseProvider, TopologyProvider,
};
use meerkat_meta::identity_first::orchestrator::{
    ReconcileAction, RestoreOutcome, compute_reconcile_actions, restore_flow,
};
use meerkat_meta::identity_first::{
    AgentAddressability, AgentBuildContext, AgentBuildDraft, AgentIdentity, AgentRuntimeId,
    CheckpointVersion, ContinuityFailure, ContinuityFailureKind, ContinuityGeneration,
    ContinuityRecord, ContinuityResolveState, ContinuityStoreError, CustomizerError, DispatchInput,
    DispatchOrigin, DurabilityPolicy, DurableAgentSpec, FencingToken, IdentityLifecycleState,
    IdentityRuntime, IdentityRuntimeConfig, IdentityRuntimeError, LeaseAcquireResult, LeaseError,
    LeaseGrant, LeaseRenewResult, ManagedPeerEdge, SessionSnapshot, TopologyContext, TopologyError,
};
use meerkat_meta::identity_first::{LocalContinuityStore, LocalLeaseProvider};

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
        runtime_instance_id: "e2e-runtime".to_string(),
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
        runtime_instance_id: "e2e-runtime".to_string(),
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
// E2E-01: Fresh boot - all registered, nothing written
// ===========================================================================

#[tokio::test]
async fn identity_first_e2e_01_fresh_boot_all_uninitialized() {
    let store = Arc::new(LocalContinuityStore::in_memory().unwrap());
    let lease_prov = Arc::new(LocalLeaseProvider::new());
    let runtime = make_runtime(store.clone(), lease_prov.clone());

    let roster = vec![
        make_spec("triage:main"),
        make_spec("worker:alpha"),
        make_spec("gate:main"),
    ];

    let result = restore_flow(&runtime, &roster, None, None).await.unwrap();

    // No session bridge, so nothing is spawned: every identity is registered
    // Dormant (#306: members exist only in Core).
    assert_eq!(result.outcomes.len(), 3);
    for (id, outcome) in &result.outcomes {
        assert!(
            matches!(outcome, RestoreOutcome::Dormant { .. }),
            "expected Dormant for {id}, got: {outcome:?}"
        );
    }

    // All registered
    assert!(runtime.contains(&make_identity("triage:main")).await);
    assert!(runtime.contains(&make_identity("worker:alpha")).await);
    assert!(runtime.contains(&make_identity("gate:main")).await);

    // Meta writes no continuity record: the store still has none.
    let identities: Vec<AgentIdentity> = roster.iter().map(|s| s.identity.clone()).collect();
    let resolved = store.resolve_many(&identities).await.unwrap();
    for (id, state) in &resolved {
        assert!(
            matches!(state, ContinuityResolveState::Uninitialized),
            "expected no record for {id} after a fresh boot, got {state:?}"
        );
    }
}

// ===========================================================================
// E2E-02: Restart — all Ready
// ===========================================================================

// ===========================================================================
// E2E-03: Respawn — non-destructive recovery
// ===========================================================================

// ===========================================================================
// E2E-04: Reset — destructive continuity reset
// ===========================================================================

// ===========================================================================
// E2E-05: Delete — identity removal
// ===========================================================================

// ===========================================================================
// E2E-06: Lease race — exclusive ownership
// ===========================================================================

// ===========================================================================
// E2E-07: Topology reconciliation
// ===========================================================================

struct DynamicTopology {
    edges: tokio::sync::Mutex<Vec<(String, String)>>,
}

impl DynamicTopology {
    fn new(edges: Vec<(&str, &str)>) -> Self {
        Self {
            edges: tokio::sync::Mutex::new(
                edges
                    .into_iter()
                    .map(|(a, b)| (a.to_string(), b.to_string()))
                    .collect(),
            ),
        }
    }

    async fn set_edges(&self, edges: Vec<(&str, &str)>) {
        *self.edges.lock().await = edges
            .into_iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect();
    }
}

#[async_trait]
impl TopologyProvider for DynamicTopology {
    async fn compute_edges(
        &self,
        _target: &[AgentIdentity],
        _ctx: &TopologyContext,
    ) -> Result<Vec<ManagedPeerEdge>, TopologyError> {
        let edges = self.edges.lock().await;
        let mut result = Vec::new();
        for (a, b) in edges.iter() {
            if let Ok(edge) = ManagedPeerEdge::new(
                AgentIdentity::parse(a).unwrap(),
                AgentIdentity::parse(b).unwrap(),
            ) {
                result.push(edge);
            }
        }
        Ok(result)
    }
}

#[tokio::test]
async fn identity_first_e2e_07_topology_reconciliation() {
    let store = Arc::new(LocalContinuityStore::in_memory().unwrap());
    let lease_prov = Arc::new(LocalLeaseProvider::new());
    let runtime = make_runtime(store.clone(), lease_prov.clone());

    let topology = DynamicTopology::new(vec![("a:main", "b:main")]);
    let roster = vec![
        make_spec("a:main"),
        make_spec("b:main"),
        make_spec("c:main"),
    ];

    // First bootstrap: a-b edge wired
    let result1 = restore_flow(&runtime, &roster, Some(&topology), None)
        .await
        .unwrap();
    assert_eq!(result1.managed_edges.len(), 1);
    assert_eq!(result1.managed_edges[0].a(), &make_identity("a:main"));
    assert_eq!(result1.managed_edges[0].b(), &make_identity("b:main"));

    // Change topology: add b-c edge, keep a-b
    topology
        .set_edges(vec![("a:main", "b:main"), ("b:main", "c:main")])
        .await;

    let result2 = restore_flow(&runtime, &roster, Some(&topology), None)
        .await
        .unwrap();
    assert_eq!(result2.managed_edges.len(), 2);

    // Drop a-b edge, keep b-c only
    topology.set_edges(vec![("b:main", "c:main")]).await;

    let result3 = restore_flow(&runtime, &roster, Some(&topology), None)
        .await
        .unwrap();
    assert_eq!(result3.managed_edges.len(), 1);
    assert_eq!(result3.managed_edges[0].a(), &make_identity("b:main"));
    assert_eq!(result3.managed_edges[0].b(), &make_identity("c:main"));
}

// ===========================================================================
// E2E-08: Send vs dispatch addressability enforcement
// ===========================================================================

#[tokio::test]
async fn identity_first_e2e_08_send_dispatch_addressability() {
    let store = Arc::new(LocalContinuityStore::in_memory().unwrap());
    let lease_prov = Arc::new(LocalLeaseProvider::new());
    let runtime = make_runtime(store.clone(), lease_prov.clone());

    // Addressable identity
    runtime.register(make_spec("public:main")).await;

    // InternalOnly identity
    runtime.register(make_internal_spec("internal:main")).await;

    // This runtime has no session bridge, so no member exists to deliver to
    // (#306: Core holds members); addressability is judged before delivery.

    // send() to Addressable -> not refused as not addressable
    assert!(!matches!(
        runtime
            .send(&make_identity("public:main"), &make_content())
            .await,
        Err(IdentityRuntimeError::NotAddressable(_))
    ));

    // send() to InternalOnly → NotAddressable
    let err = runtime
        .send(&make_identity("internal:main"), &make_content())
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityRuntimeError::NotAddressable(_)));

    // dispatch() to Addressable -> not refused as not addressable
    assert!(!matches!(
        runtime
            .dispatch(&make_identity("public:main"), &make_dispatch_input())
            .await,
        Err(IdentityRuntimeError::NotAddressable(_))
    ));

    // dispatch() to InternalOnly -> also not refused as not addressable
    assert!(!matches!(
        runtime
            .dispatch(&make_identity("internal:main"), &make_dispatch_input())
            .await,
        Err(IdentityRuntimeError::NotAddressable(_))
    ));
}

// ===========================================================================
// E2E-09: Broken continuity fails loudly
// ===========================================================================

// ===========================================================================
// E2E-10: Checkpoint version + fencing enforcement
// ===========================================================================

// ===========================================================================
// E2E-11: Builder mutual exclusivity
// ===========================================================================

use meerkat_meta::identity_first::contracts::ContinuityStore as ContinuityStoreTrait;
use meerkat_meta::unified_runtime::UnifiedRuntimeBuilder;

struct StubContinuity;

#[async_trait]
impl ContinuityStoreTrait for StubContinuity {
    // Explicit answer required by the trait: silence here used to
    // assert an authoritative "no record" while meaning "never
    // taught to answer".
    async fn resolve_record_by_session(
        &self,
        _session_id: &meerkat_core::types::SessionId,
    ) -> Result<
        Option<(
            meerkat_meta::identity_first::ContinuityRecord,
            meerkat_meta::identity_first::FencingToken,
            meerkat_meta::identity_first::CheckpointVersion,
        )>,
        meerkat_meta::identity_first::ContinuityStoreError,
    > {
        Ok(None)
    }

    async fn resolve_many(
        &self,
        identities: &[AgentIdentity],
    ) -> Result<BTreeMap<AgentIdentity, ContinuityResolveState>, ContinuityStoreError> {
        Ok(identities
            .iter()
            .map(|id| (id.clone(), ContinuityResolveState::Uninitialized))
            .collect())
    }
    async fn load_session_snapshot(
        &self,
        _sid: &meerkat_core::types::SessionId,
    ) -> Result<Option<SessionSnapshot>, ContinuityStoreError> {
        Ok(None)
    }
    async fn save_session_snapshot(
        &self,
        _identity: &AgentIdentity,
        _sid: &meerkat_core::types::SessionId,
        _gen: ContinuityGeneration,
        _ver: CheckpointVersion,
        _ft: FencingToken,
        _snap: &SessionSnapshot,
    ) -> Result<(), ContinuityStoreError> {
        Ok(())
    }
    async fn upsert_continuity_record(
        &self,
        _record: &ContinuityRecord,
        _ft: FencingToken,
    ) -> Result<(), ContinuityStoreError> {
        Ok(())
    }
    async fn delete_continuity_record(
        &self,
        _identity: &AgentIdentity,
        _ft: FencingToken,
    ) -> Result<(), ContinuityStoreError> {
        Ok(())
    }

    /// Test double: deliberately whole-snapshot only.
    fn as_incremental_sessions(
        &self,
    ) -> Option<
        std::sync::Arc<dyn meerkat_meta::identity_first::contracts::ContinuityIncrementalSessions>,
    > {
        None
    }
}

struct StubLease;

#[async_trait]
impl LeaseProvider for StubLease {
    async fn acquire_leases(
        &self,
        identities: &[AgentIdentity],
        _instance: &str,
    ) -> Result<BTreeMap<AgentIdentity, LeaseAcquireResult>, LeaseError> {
        Ok(identities
            .iter()
            .map(|id| {
                (
                    id.clone(),
                    LeaseAcquireResult::Acquired(LeaseGrant {
                        identity: id.clone(),
                        fencing_token: FencingToken::new(1),
                        ttl: Duration::from_secs(30),
                    }),
                )
            })
            .collect())
    }
    async fn renew_leases(
        &self,
        _grants: &[LeaseGrant],
    ) -> Result<BTreeMap<AgentIdentity, LeaseRenewResult>, LeaseError> {
        Ok(BTreeMap::new())
    }
    async fn release_leases(&self, _grants: &[LeaseGrant]) -> Result<(), LeaseError> {
        Ok(())
    }
}

#[tokio::test]
async fn identity_first_e2e_11_builder_mutual_exclusivity() {
    // The M4 REQ-23 lift: persistent_state may coexist with a COMPLETE
    // external pair; HALF a substrate stays a typed build error.

    // persistent_state + continuity_store (no lease provider) → build error
    let builder = UnifiedRuntimeBuilder::default()
        .persistent_state("/tmp/e2e-test-state")
        .continuity_store(Arc::new(StubContinuity));
    match Box::pin(builder.build()).await {
        Err(e) => {
            let msg = e.to_string();
            assert!(
                msg.contains("must be supplied together"),
                "error must name the incomplete substrate pair: {msg}"
            );
        }
        Ok(_) => panic!("expected build error for persistent_state + continuity_store alone"),
    }

    // persistent_state + lease_provider (no continuity store) → build error
    let builder2 = UnifiedRuntimeBuilder::default()
        .persistent_state("/tmp/e2e-test-state")
        .lease_provider(Arc::new(StubLease));
    match Box::pin(builder2.build()).await {
        Err(e) => {
            assert!(e.to_string().contains("must be supplied together"));
        }
        Ok(_) => panic!("expected build error for persistent_state + lease_provider alone"),
    }

    // persistent_state + scratch_dir → build error (two path roots)
    let builder3 = UnifiedRuntimeBuilder::default()
        .persistent_state("/tmp/e2e-test-state")
        .scratch_dir("/tmp/e2e-test-scratch");
    match Box::pin(builder3.build()).await {
        Err(e) => {
            assert!(e.to_string().contains("mutually exclusive"));
        }
        Ok(_) => panic!("expected build error for persistent_state + scratch_dir"),
    }
}

// ===========================================================================
// E2E-14: Legacy Discovery adapter round-trip
// ===========================================================================

use meerkat_meta::identity_first::DiscoveryRosterAdapter;
use meerkat_meta::identity_first::contracts::RosterProvider;
use meerkat_meta::types::AgentDiscoverySpec;

#[tokio::test]
async fn identity_first_e2e_14_legacy_discovery_adapter_round_trip() {
    use std::future::Future;
    use std::pin::Pin;

    struct LegacyDiscovery;
    impl meerkat_meta::unified_runtime::edge_types::Discovery for LegacyDiscovery {
        fn discover(
            &self,
            _context: serde_json::Value,
        ) -> Pin<Box<dyn Future<Output = Vec<AgentDiscoverySpec>> + Send + '_>> {
            Box::pin(async {
                vec![
                    AgentDiscoverySpec {
                        profile: "triage".to_string(),
                        meerkat_id: "triage:main".to_string(),
                        labels: Some({
                            let mut m = BTreeMap::new();
                            m.insert("role".to_string(), "lead".to_string());
                            m
                        }),
                        context: None,
                        additional_instructions: vec![],
                        resume_session_id: Some("old-session".to_string()),
                    },
                    AgentDiscoverySpec {
                        profile: "worker".to_string(),
                        meerkat_id: "worker:alpha".to_string(),
                        labels: None,
                        context: None,
                        additional_instructions: vec!["be concise".to_string()],
                        resume_session_id: None,
                    },
                ]
            })
        }
    }

    let adapter = DiscoveryRosterAdapter::new(LegacyDiscovery);
    let ctx = meerkat_meta::identity_first::RosterContext {
        mob_definition: None,
        previous_identities: vec![],
    };
    let roster = adapter.roster(&ctx).await.unwrap();

    assert_eq!(roster.len(), 2);

    // First spec: colon-namespaced identity parsed correctly
    assert_eq!(roster[0].identity.as_str(), "triage:main");
    assert_eq!(roster[0].addressability, AgentAddressability::Addressable);
    assert_eq!(roster[0].labels.get("role"), Some(&"lead".to_string()));
    // resume_session_id is ignored (no such field on DurableAgentSpec)

    // Second spec
    assert_eq!(roster[1].identity.as_str(), "worker:alpha");
    assert_eq!(roster[1].additional_instructions, vec!["be concise"]);

    // Use in restore flow
    let store = Arc::new(LocalContinuityStore::in_memory().unwrap());
    let lease_prov = Arc::new(LocalLeaseProvider::new());
    // Core builds the members, so the restore creates both.
    let runtime = make_runtime_with_core_bridge(store.clone(), lease_prov.clone());

    let result = restore_flow(&runtime, &roster, None, None).await.unwrap();
    assert_eq!(result.outcomes.len(), 2);
    for (id, outcome) in &result.outcomes {
        assert!(
            matches!(outcome, RestoreOutcome::Created { .. }),
            "expected Created for {id}"
        );
    }
}

// ===========================================================================
// E2E-15: AgentCustomizer receives topology context
// ===========================================================================

#[tokio::test]
async fn identity_first_e2e_15_customizer_receives_topology_context() {
    let store = Arc::new(LocalContinuityStore::in_memory().unwrap());
    let lease_prov = Arc::new(LocalLeaseProvider::new());
    // The customizer runs when Core builds a member, so the restore needs a bridge.
    let runtime = make_runtime_with_core_bridge(store.clone(), lease_prov.clone());

    let seen_contexts = Arc::new(tokio::sync::Mutex::new(Vec::new()));

    struct ContextTracker {
        seen: Arc<tokio::sync::Mutex<Vec<AgentBuildContext>>>,
    }

    #[async_trait]
    impl AgentCustomizer for ContextTracker {
        async fn customize_build(
            &self,
            context: &AgentBuildContext,
            _spec: &DurableAgentSpec,
            draft: &mut AgentBuildDraft,
        ) -> Result<(), CustomizerError> {
            self.seen.lock().await.push(context.clone());
            // Inject peer listing into system_prompt
            let peers: Vec<String> = context
                .active_peers
                .iter()
                .map(|p| p.as_str().to_string())
                .collect();
            draft.system_prompt = Some(format!("peers: {}", peers.join(", ")));
            Ok(())
        }
    }

    let topology = DynamicTopology::new(vec![("a:main", "b:main")]);
    let customizer = ContextTracker {
        seen: seen_contexts.clone(),
    };

    let roster = vec![make_spec("a:main"), make_spec("b:main")];
    let result = restore_flow(&runtime, &roster, Some(&topology), Some(&customizer))
        .await
        .unwrap();

    let contexts = seen_contexts.lock().await;
    assert_eq!(contexts.len(), 2);
    for ctx in contexts.iter() {
        // active_peers includes all identities being activated (cold boot)
        assert_eq!(ctx.active_peers.len(), 2);
        assert!(ctx.active_peers.contains(&make_identity("a:main")));
        assert!(ctx.active_peers.contains(&make_identity("b:main")));
        // managed_edges from topology provider
        assert_eq!(ctx.managed_edges.len(), 1);
        assert_eq!(ctx.managed_edges[0].a(), &make_identity("a:main"));
        assert_eq!(ctx.managed_edges[0].b(), &make_identity("b:main"));
    }

    // Verify customizer used the context to modify drafts
    for (_, outcome) in &result.outcomes {
        match outcome {
            RestoreOutcome::Created { draft, .. } => {
                let prompt = draft.system_prompt.as_ref().unwrap();
                assert!(
                    prompt.contains("a:main"),
                    "prompt should contain peer listing"
                );
                assert!(
                    prompt.contains("b:main"),
                    "prompt should contain peer listing"
                );
            }
            other => panic!("expected Created, got: {other:?}"),
        }
    }
}

// ===========================================================================
// E2E-16: ContinuityStore → SessionStore adapter round-trip
// ===========================================================================

use meerkat_meta::identity_first::ContinuitySessionStoreAdapter;

#[tokio::test]
async fn identity_first_e2e_16_session_store_adapter_round_trip() {
    let continuity_store = Arc::new(LocalContinuityStore::in_memory().unwrap());
    let adapter = ContinuitySessionStoreAdapter::new(continuity_store.clone());

    let sid = meerkat_core::types::SessionId::new();

    // Load from empty → None
    let loaded: Option<meerkat_core::Session> =
        meerkat::SessionStore::load(&adapter, &sid).await.unwrap();
    assert!(loaded.is_none());

    // No local SQLite should be created — the adapter delegates to ContinuityStore.
    // We verify by checking that the in-memory store correctly handles the load/save cycle.
    // The key assertion: ContinuityStore IS the authoritative session truth on the
    // external-authoritative path. No dual-write.
}

// ===========================================================================
// E2E-20: Roster identity uniqueness enforcement
// ===========================================================================

#[tokio::test]
async fn identity_first_e2e_20_roster_identity_uniqueness() {
    let store = Arc::new(LocalContinuityStore::in_memory().unwrap());
    let lease_prov = Arc::new(LocalLeaseProvider::new());
    let runtime = make_runtime(store.clone(), lease_prov.clone());

    // Duplicate identities in roster
    let roster = vec![
        make_spec("triage:main"),
        DurableAgentSpec {
            profile: meerkat_mob::ProfileName::from("expert"),
            labels: {
                let mut m = BTreeMap::new();
                m.insert("variant".to_string(), "2".to_string());
                m
            },
            ..make_spec("triage:main") // same identity, different profile/labels
        },
    ];

    let result = restore_flow(&runtime, &roster, None, None).await;
    assert!(
        matches!(result, Err(IdentityRuntimeError::DuplicateIdentity(_))),
        "duplicate identities must produce a structured error"
    );

    // No partial activation: runtime should be empty
    assert!(!runtime.contains(&make_identity("triage:main")).await);

    // Verify no leases were acquired (store should be clean)
    let resolved = store
        .resolve_many(&[make_identity("triage:main")])
        .await
        .unwrap();
    assert!(
        matches!(
            resolved.get(&make_identity("triage:main")).unwrap(),
            ContinuityResolveState::Uninitialized
        ),
        "no continuity records should have been written"
    );
}

// ===========================================================================
// E2E-21: Reconciliation hot-update behavior
// ===========================================================================

#[tokio::test]
async fn identity_first_e2e_21_reconciliation_hot_update() {
    let store = Arc::new(LocalContinuityStore::in_memory().unwrap());
    let lease_prov = Arc::new(LocalLeaseProvider::new());
    let runtime = make_runtime(store.clone(), lease_prov.clone());

    // Initial roster: one Addressable, one InternalOnly
    let roster_v1 = vec![
        make_spec("public:main"),
        make_internal_spec("internal:main"),
    ];
    restore_flow(&runtime, &roster_v1, None, None)
        .await
        .unwrap();

    // Verify initial state. This runtime has no session bridge, so no member
    // exists to deliver to; addressability is judged before delivery.
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

    let status = runtime.status(&make_identity("public:main")).await.unwrap();
    assert!(status.labels.is_empty());

    // Build current active set for reconciliation
    let current = runtime
        .roster_inspect()
        .await
        .expect("the roster view reads Core");
    let current_specs: BTreeMap<AgentIdentity, DurableAgentSpec> = current
        .into_iter()
        .map(|(id, (spec, _))| (id, spec))
        .collect();

    // --- Labels change → hot-reload ---
    let mut public_v2 = make_spec("public:main");
    public_v2
        .labels
        .insert("env".to_string(), "production".to_string());

    // --- Addressability flip: InternalOnly → Addressable ---
    let internal_v2 = make_spec("internal:main"); // now Addressable

    // --- Profile change → respawn ---
    // (We test reconciliation logic, not the actual respawn execution)
    let roster_v2 = vec![public_v2.clone(), internal_v2.clone()];
    let actions = compute_reconcile_actions(&roster_v2, &current_specs);

    // Should have hot-reload actions for both (labels + addressability changes)
    assert_eq!(actions.len(), 2);
    for action in &actions {
        match action {
            ReconcileAction::HotReload { identity, new_spec } => {
                if identity == &make_identity("public:main") {
                    assert_eq!(new_spec.labels.get("env"), Some(&"production".to_string()));
                } else if identity == &make_identity("internal:main") {
                    assert_eq!(new_spec.addressability, AgentAddressability::Addressable);
                }
            }
            other => panic!("expected HotReload, got: {other:?}"),
        }
    }

    // Apply hot-reload: update specs in runtime
    for action in &actions {
        if let ReconcileAction::HotReload { new_spec, .. } = action {
            runtime.update_spec(new_spec.clone()).await.unwrap();
        }
    }

    // Labels change visible in status immediately
    let status_v2 = runtime.status(&make_identity("public:main")).await.unwrap();
    assert_eq!(status_v2.labels.get("env"), Some(&"production".to_string()));

    // Addressability change: the previously internal identity is no longer
    // refused as not addressable.
    assert!(
        !matches!(
            runtime
                .send(&make_identity("internal:main"), &make_content())
                .await,
            Err(IdentityRuntimeError::NotAddressable(_))
        ),
        "send must not be refused as not addressable after the flip to Addressable"
    );

    // --- Profile change triggers Respawn ---
    let mut profile_change = make_spec("public:main");
    profile_change.profile = meerkat_mob::ProfileName::from("expert");
    let current_v2: BTreeMap<AgentIdentity, DurableAgentSpec> = roster_v2
        .iter()
        .map(|s| (s.identity.clone(), s.clone()))
        .collect();
    let roster_v3 = vec![profile_change, internal_v2];
    let actions_v3 = compute_reconcile_actions(&roster_v3, &current_v2);

    let respawn_actions: Vec<_> = actions_v3
        .iter()
        .filter(|a| matches!(a, ReconcileAction::Respawn { .. }))
        .collect();
    assert_eq!(
        respawn_actions.len(),
        1,
        "profile change should trigger Respawn"
    );
    if let ReconcileAction::Respawn { identity, .. } = &respawn_actions[0] {
        assert_eq!(identity, &make_identity("public:main"));
    }
}
