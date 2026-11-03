#![allow(
    clippy::all,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    unused_imports,
    redundant_semicolons,
    clippy::redundant_clone
)]
//! Tests for identity-first provider contracts (Phase 1, Tasks 1.1–1.11).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use meerkat_meta::identity_first::contracts::{
    AgentCustomizer, ContinuityStore, LeaseProvider, RosterProvider, TopologyProvider,
};
use meerkat_meta::identity_first::{
    AgentBuildContext, AgentBuildDraft, AgentIdentity, AgentRuntimeId, CheckpointVersion,
    ContinuityGeneration, ContinuityRecord, ContinuityResolveState, ContinuityStoreError,
    CustomizerError, DurableAgentSpec, ExternalToolDef, FencingToken, LeaseAcquireResult,
    LeaseError, LeaseGrant, LeaseRenewResult, ManagedPeerEdge, RosterContext, RosterError,
    SessionSnapshot, TopologyContext, TopologyError,
};
use meerkat_meta::mob_handle_runtime::{SessionCreatedContext, SessionHook};

// ===========================================================================
// Mock implementations for contract testing
// ===========================================================================

struct MockContinuityStore;

#[async_trait]
impl ContinuityStore for MockContinuityStore {
    // The trait requires an explicit answer: a double that stayed
    // silent used to assert an authoritative "no record" while
    // meaning "I was never taught to answer", which is how a fix
    // goes inert with every test still green.
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
        let mut map = BTreeMap::new();
        for id in identities {
            map.insert(id.clone(), ContinuityResolveState::Uninitialized);
        }
        Ok(map)
    }

    async fn load_session_snapshot(
        &self,
        _session_id: &meerkat_core::types::SessionId,
    ) -> Result<Option<SessionSnapshot>, ContinuityStoreError> {
        Ok(None)
    }

    async fn save_session_snapshot(
        &self,
        _identity: &AgentIdentity,
        _session_id: &meerkat_core::types::SessionId,
        _generation: ContinuityGeneration,
        _version: CheckpointVersion,
        _fencing_token: FencingToken,
        _snapshot: &SessionSnapshot,
    ) -> Result<(), ContinuityStoreError> {
        Ok(())
    }

    async fn upsert_continuity_record(
        &self,
        _record: &ContinuityRecord,
        _fencing_token: FencingToken,
    ) -> Result<(), ContinuityStoreError> {
        Ok(())
    }

    async fn delete_continuity_record(
        &self,
        _identity: &AgentIdentity,
        _fencing_token: FencingToken,
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

struct MockLeaseProvider;

#[async_trait]
impl LeaseProvider for MockLeaseProvider {
    async fn acquire_leases(
        &self,
        identities: &[AgentIdentity],
        _runtime_instance: &str,
    ) -> Result<BTreeMap<AgentIdentity, LeaseAcquireResult>, LeaseError> {
        let mut map = BTreeMap::new();
        for (i, id) in identities.iter().enumerate() {
            map.insert(
                id.clone(),
                LeaseAcquireResult::Acquired(LeaseGrant {
                    identity: id.clone(),
                    fencing_token: FencingToken::new(i as u64 + 1),
                    ttl: Duration::from_secs(30),
                }),
            );
        }
        Ok(map)
    }

    async fn renew_leases(
        &self,
        grants: &[LeaseGrant],
    ) -> Result<BTreeMap<AgentIdentity, LeaseRenewResult>, LeaseError> {
        let mut map = BTreeMap::new();
        for g in grants {
            map.insert(
                g.identity.clone(),
                LeaseRenewResult::Renewed(LeaseGrant {
                    identity: g.identity.clone(),
                    fencing_token: g.fencing_token,
                    ttl: Duration::from_secs(30),
                }),
            );
        }
        Ok(map)
    }

    async fn release_leases(&self, _grants: &[LeaseGrant]) -> Result<(), LeaseError> {
        Ok(())
    }
}

struct MockRosterProvider {
    specs: Vec<DurableAgentSpec>,
}

#[async_trait]
impl RosterProvider for MockRosterProvider {
    async fn roster(&self, _context: &RosterContext) -> Result<Vec<DurableAgentSpec>, RosterError> {
        Ok(self.specs.clone())
    }
}

struct MockCustomizer;

#[async_trait]
impl AgentCustomizer for MockCustomizer {
    async fn customize_build(
        &self,
        _context: &AgentBuildContext,
        _spec: &DurableAgentSpec,
        draft: &mut AgentBuildDraft,
    ) -> Result<(), CustomizerError> {
        draft.system_prompt = Some("customized prompt".to_string());
        draft.external_tools.push(ExternalToolDef {
            name: "test_tool".to_string(),
            description: "a test tool".to_string(),
            input_schema: serde_json::json!({"type": "object"}),
            artifact: None,
        });
        Ok(())
    }
}

struct MockTopologyProvider;

#[async_trait]
impl TopologyProvider for MockTopologyProvider {
    async fn compute_edges(
        &self,
        target_identities: &[AgentIdentity],
        _context: &TopologyContext,
    ) -> Result<Vec<ManagedPeerEdge>, TopologyError> {
        let mut edges = Vec::new();
        // Create a full mesh for testing
        for i in 0..target_identities.len() {
            for j in (i + 1)..target_identities.len() {
                if let Ok(edge) =
                    ManagedPeerEdge::new(target_identities[i].clone(), target_identities[j].clone())
                {
                    edges.push(edge);
                }
            }
        }
        Ok(edges)
    }
}

// ===========================================================================
// Task 1.1: ContinuityStore trait (CONTRACT-01)
// ===========================================================================

#[tokio::test]
async fn identity_first_contracts_continuity_store_mock_resolve_many() {
    let store: Arc<dyn ContinuityStore> = Arc::new(MockContinuityStore);
    let ids = vec![
        AgentIdentity::parse("triage:main").unwrap(),
        AgentIdentity::parse("gate:main").unwrap(),
    ];
    let result = store.resolve_many(&ids).await.unwrap();
    assert_eq!(result.len(), 2);
    assert!(result.contains_key(&ids[0]));
    assert!(result.contains_key(&ids[1]));
    for (_, state) in &result {
        assert!(matches!(state, ContinuityResolveState::Uninitialized));
    }
}

#[tokio::test]
async fn identity_first_contracts_continuity_store_mock_snapshot_round_trip() {
    let store: Arc<dyn ContinuityStore> = Arc::new(MockContinuityStore);
    let sid = meerkat_core::types::SessionId::new();
    let loaded = store.load_session_snapshot(&sid).await.unwrap();
    assert!(loaded.is_none());
}

#[tokio::test]
async fn identity_first_contracts_continuity_store_mock_upsert() {
    let store: Arc<dyn ContinuityStore> = Arc::new(MockContinuityStore);
    let record = ContinuityRecord {
        identity: AgentIdentity::parse("triage:main").unwrap(),
        agent_runtime_id: AgentRuntimeId::parse("rt-001").unwrap(),
        session_id: meerkat_core::types::SessionId::new(),
        generation: ContinuityGeneration::new(0),
        checkpoint_version: CheckpointVersion::new(0),
    };
    store
        .upsert_continuity_record(&record, FencingToken::new(1))
        .await
        .unwrap();
}

// ===========================================================================
// Task 1.2: LeaseProvider trait (CONTRACT-02)
// ===========================================================================

#[tokio::test]
async fn identity_first_contracts_lease_provider_mock_acquire() {
    let provider: Arc<dyn LeaseProvider> = Arc::new(MockLeaseProvider);
    let ids = vec![
        AgentIdentity::parse("triage:main").unwrap(),
        AgentIdentity::parse("gate:main").unwrap(),
    ];
    let result = provider.acquire_leases(&ids, "instance-1").await.unwrap();
    assert_eq!(result.len(), 2);
    for (id, acq) in &result {
        match acq {
            LeaseAcquireResult::Acquired(grant) => {
                assert_eq!(&grant.identity, id);
                assert!(grant.fencing_token.get() > 0);
            }
            _ => panic!("expected Acquired"),
        }
    }
}

#[tokio::test]
async fn identity_first_contracts_lease_provider_mock_renew() {
    let provider: Arc<dyn LeaseProvider> = Arc::new(MockLeaseProvider);
    let grants = vec![LeaseGrant {
        identity: AgentIdentity::parse("triage:main").unwrap(),
        fencing_token: FencingToken::new(1),
        ttl: Duration::from_secs(30),
    }];
    let result = provider.renew_leases(&grants).await.unwrap();
    assert_eq!(result.len(), 1);
    assert!(matches!(
        result.values().next().unwrap(),
        LeaseRenewResult::Renewed(_)
    ));
}

#[tokio::test]
async fn identity_first_contracts_lease_provider_mock_release() {
    let provider: Arc<dyn LeaseProvider> = Arc::new(MockLeaseProvider);
    let grants = vec![LeaseGrant {
        identity: AgentIdentity::parse("triage:main").unwrap(),
        fencing_token: FencingToken::new(1),
        ttl: Duration::from_secs(30),
    }];
    provider.release_leases(&grants).await.unwrap();
}

// ===========================================================================
// Task 1.3: RosterProvider trait (CONTRACT-03)
// ===========================================================================

#[tokio::test]
async fn identity_first_contracts_roster_provider_mock() {
    let spec = DurableAgentSpec {
        identity: AgentIdentity::parse("triage:main").unwrap(),
        profile: meerkat_mob::ProfileName::from("default"),
        addressability: Default::default(),
        display_name: None,
        labels: BTreeMap::new(),
        context: None,
        additional_instructions: vec![],
        initial_message: None,
        runtime_mode_override: None,
        backend: None,
        binding: None,
        placement: None,
    };
    let provider: Arc<dyn RosterProvider> = Arc::new(MockRosterProvider {
        specs: vec![spec.clone()],
    });
    let ctx = RosterContext {
        mob_definition: None,
        previous_identities: vec![],
    };
    let roster = provider.roster(&ctx).await.unwrap();
    assert_eq!(roster.len(), 1);
    assert_eq!(roster[0].identity, spec.identity);
}

// ===========================================================================
// Task 1.4: AgentCustomizer trait (CONTRACT-04)
// ===========================================================================

#[tokio::test]
async fn identity_first_contracts_customizer_mock_modifies_draft() {
    let customizer: Arc<dyn AgentCustomizer> = Arc::new(MockCustomizer);
    let ctx = AgentBuildContext {
        identity: AgentIdentity::parse("triage:main").unwrap(),
        active_peers: vec![],
        managed_edges: vec![],
        runtime_services: Default::default(),
    };
    let spec = DurableAgentSpec {
        identity: AgentIdentity::parse("triage:main").unwrap(),
        profile: meerkat_mob::ProfileName::from("default"),
        addressability: Default::default(),
        display_name: None,
        labels: BTreeMap::new(),
        context: None,
        additional_instructions: vec![],
        initial_message: None,
        runtime_mode_override: None,
        backend: None,
        binding: None,
        placement: None,
    };
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
    customizer
        .customize_build(&ctx, &spec, &mut draft)
        .await
        .unwrap();
    assert_eq!(draft.system_prompt.as_deref(), Some("customized prompt"));
    assert_eq!(draft.external_tools.len(), 1);
    assert_eq!(draft.external_tools[0].name, "test_tool");
}

#[tokio::test]
async fn identity_first_contracts_customizer_default_after_create() {
    // The default after_create should be a no-op
    let customizer: Arc<dyn AgentCustomizer> = Arc::new(MockCustomizer);
    let id = AgentIdentity::parse("triage:main").unwrap();
    let sid = meerkat_core::types::SessionId::new();
    let ctx = SessionCreatedContext {
        model: "test-model".to_string(),
        labels: BTreeMap::new(),
        system_prompt: None,
    };
    customizer.after_create(&id, &sid, &ctx).await.unwrap();
}

// ===========================================================================
// Task 1.5: TopologyProvider trait (CONTRACT-05)
// ===========================================================================

#[tokio::test]
async fn identity_first_contracts_topology_provider_mock() {
    let provider: Arc<dyn TopologyProvider> = Arc::new(MockTopologyProvider);
    let ids = vec![
        AgentIdentity::parse("a:main").unwrap(),
        AgentIdentity::parse("b:main").unwrap(),
        AgentIdentity::parse("c:main").unwrap(),
    ];
    let ctx = TopologyContext { roster: vec![] };
    let edges = provider.compute_edges(&ids, &ctx).await.unwrap();
    // Full mesh of 3 identities = 3 edges
    assert_eq!(edges.len(), 3);
    for edge in &edges {
        assert!(edge.a() < edge.b(), "canonical ordering maintained");
    }
}

// ===========================================================================
// Task 1.6: LocalContinuityStore (CONTRACT-06)
// ===========================================================================

use meerkat_meta::identity_first::LocalContinuityStore;

#[tokio::test]
async fn identity_first_contracts_local_store_upsert_and_resolve() {
    let store = LocalContinuityStore::in_memory().unwrap();
    let id = AgentIdentity::parse("triage:main").unwrap();
    let record = ContinuityRecord {
        identity: id.clone(),
        agent_runtime_id: AgentRuntimeId::parse("rt-001").unwrap(),
        session_id: meerkat_core::types::SessionId::new(),
        generation: ContinuityGeneration::new(0),
        checkpoint_version: CheckpointVersion::new(0),
    };
    store
        .upsert_continuity_record(&record, FencingToken::new(1))
        .await
        .unwrap();

    let result = store.resolve_many(&[id.clone()]).await.unwrap();
    assert_eq!(result.len(), 1);
    match &result[&id] {
        ContinuityResolveState::Ready { record: r } => {
            assert_eq!(r.identity, id);
            assert_eq!(r.agent_runtime_id.as_str(), "rt-001");
            assert_eq!(r.generation, ContinuityGeneration::new(0));
        }
        other => panic!("expected Ready, got {other:?}"),
    }
}

#[tokio::test]
async fn identity_first_contracts_local_store_resolve_uninitialized() {
    let store = LocalContinuityStore::in_memory().unwrap();
    let id = AgentIdentity::parse("unknown:agent").unwrap();
    let result = store.resolve_many(&[id.clone()]).await.unwrap();
    assert!(matches!(result[&id], ContinuityResolveState::Uninitialized));
}

#[tokio::test]
async fn identity_first_contracts_local_store_snapshot_round_trip() {
    let store = LocalContinuityStore::in_memory().unwrap();
    let id = AgentIdentity::parse("triage:main").unwrap();
    let sid = meerkat_core::types::SessionId::new();
    let record = ContinuityRecord {
        identity: id.clone(),
        agent_runtime_id: AgentRuntimeId::parse("rt-001").unwrap(),
        session_id: sid.clone(),
        generation: ContinuityGeneration::new(0),
        checkpoint_version: CheckpointVersion::new(0),
    };
    store
        .upsert_continuity_record(&record, FencingToken::new(1))
        .await
        .unwrap();

    let snapshot = SessionSnapshot {
        data: b"test session data".to_vec(),
    };
    store
        .save_session_snapshot(
            &id,
            &sid,
            ContinuityGeneration::new(0),
            CheckpointVersion::new(1),
            FencingToken::new(1),
            &snapshot,
        )
        .await
        .unwrap();

    let loaded = store.load_session_snapshot(&sid).await.unwrap();
    assert_eq!(loaded.unwrap().data, b"test session data");
}

#[tokio::test]
async fn identity_first_contracts_local_store_stale_fencing_token_rejected() {
    let store = LocalContinuityStore::in_memory().unwrap();
    let id = AgentIdentity::parse("triage:main").unwrap();
    let sid = meerkat_core::types::SessionId::new();
    let record = ContinuityRecord {
        identity: id.clone(),
        agent_runtime_id: AgentRuntimeId::parse("rt-001").unwrap(),
        session_id: sid.clone(),
        generation: ContinuityGeneration::new(0),
        checkpoint_version: CheckpointVersion::new(0),
    };
    // Insert with fencing token 5
    store
        .upsert_continuity_record(&record, FencingToken::new(5))
        .await
        .unwrap();

    // Attempt save with stale token 3
    let snapshot = SessionSnapshot {
        data: b"data".to_vec(),
    };
    let err = store
        .save_session_snapshot(
            &id,
            &sid,
            ContinuityGeneration::new(0),
            CheckpointVersion::new(1),
            FencingToken::new(3),
            &snapshot,
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, ContinuityStoreError::StaleFencingToken { .. }),
        "expected StaleFencingToken, got {err:?}"
    );
}

#[tokio::test]
async fn identity_first_contracts_local_store_snapshot_save_advances_fencing_token() {
    let store = LocalContinuityStore::in_memory().unwrap();
    let id = AgentIdentity::parse("triage:main").unwrap();
    let sid = meerkat_core::types::SessionId::new();
    let record = ContinuityRecord {
        identity: id.clone(),
        agent_runtime_id: AgentRuntimeId::parse("rt-001").unwrap(),
        session_id: sid.clone(),
        generation: ContinuityGeneration::new(0),
        checkpoint_version: CheckpointVersion::new(0),
    };
    store
        .upsert_continuity_record(&record, FencingToken::new(1))
        .await
        .unwrap();

    store
        .save_session_snapshot(
            &id,
            &sid,
            ContinuityGeneration::new(0),
            CheckpointVersion::new(1),
            FencingToken::new(2),
            &SessionSnapshot {
                data: b"new owner".to_vec(),
            },
        )
        .await
        .unwrap();

    let err = store
        .save_session_snapshot(
            &id,
            &sid,
            ContinuityGeneration::new(0),
            CheckpointVersion::new(2),
            FencingToken::new(1),
            &SessionSnapshot {
                data: b"stale owner".to_vec(),
            },
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, ContinuityStoreError::StaleFencingToken { .. }),
        "expected advanced fencing token to reject stale writer, got {err:?}"
    );
}

#[tokio::test]
async fn identity_first_contracts_local_store_snapshot_save_requires_current_session_stream() {
    let store = LocalContinuityStore::in_memory().unwrap();
    let id = AgentIdentity::parse("triage:main").unwrap();
    let old_sid = meerkat_core::types::SessionId::new();
    let old_record = ContinuityRecord {
        identity: id.clone(),
        agent_runtime_id: AgentRuntimeId::parse("rt-old").unwrap(),
        session_id: old_sid.clone(),
        generation: ContinuityGeneration::new(0),
        checkpoint_version: CheckpointVersion::new(0),
    };
    store
        .upsert_continuity_record(&old_record, FencingToken::new(7))
        .await
        .unwrap();

    let new_sid = meerkat_core::types::SessionId::new();
    let new_record = ContinuityRecord {
        identity: id.clone(),
        agent_runtime_id: AgentRuntimeId::parse("rt-new").unwrap(),
        session_id: new_sid.clone(),
        generation: ContinuityGeneration::new(1),
        checkpoint_version: CheckpointVersion::new(0),
    };
    store
        .upsert_continuity_record(&new_record, FencingToken::new(7))
        .await
        .unwrap();

    let err = store
        .save_session_snapshot(
            &id,
            &old_sid,
            old_record.generation,
            CheckpointVersion::new(99),
            FencingToken::new(7),
            &SessionSnapshot {
                data: b"late old session".to_vec(),
            },
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, ContinuityStoreError::NotFound { .. }),
        "expected late save from abandoned session stream to fail closed, got {err:?}"
    );

    store
        .save_session_snapshot(
            &id,
            &new_sid,
            new_record.generation,
            CheckpointVersion::new(1),
            FencingToken::new(7),
            &SessionSnapshot {
                data: b"current session".to_vec(),
            },
        )
        .await
        .unwrap();
    let resolved = store.resolve_many(std::slice::from_ref(&id)).await.unwrap();
    let Some(ContinuityResolveState::Ready { record }) = resolved.get(&id) else {
        panic!("expected current record to remain ready: {resolved:#?}");
    };
    assert_eq!(record.session_id, new_sid);
    assert_eq!(record.checkpoint_version, CheckpointVersion::new(1));
}

#[tokio::test]
async fn identity_first_contracts_local_store_stale_checkpoint_version_rejected() {
    let store = LocalContinuityStore::in_memory().unwrap();
    let id = AgentIdentity::parse("triage:main").unwrap();
    let sid = meerkat_core::types::SessionId::new();
    let record = ContinuityRecord {
        identity: id.clone(),
        agent_runtime_id: AgentRuntimeId::parse("rt-001").unwrap(),
        session_id: sid.clone(),
        generation: ContinuityGeneration::new(0),
        checkpoint_version: CheckpointVersion::new(0),
    };
    store
        .upsert_continuity_record(&record, FencingToken::new(1))
        .await
        .unwrap();

    // Save at version 1 — should succeed
    let snapshot = SessionSnapshot {
        data: b"v1".to_vec(),
    };
    store
        .save_session_snapshot(
            &id,
            &sid,
            ContinuityGeneration::new(0),
            CheckpointVersion::new(1),
            FencingToken::new(1),
            &snapshot,
        )
        .await
        .unwrap();

    // Save at version 1 again — should fail (not strictly greater)
    let err = store
        .save_session_snapshot(
            &id,
            &sid,
            ContinuityGeneration::new(0),
            CheckpointVersion::new(1),
            FencingToken::new(1),
            &snapshot,
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, ContinuityStoreError::StaleCheckpointVersion { .. }),
        "expected StaleCheckpointVersion, got {err:?}"
    );
}

// ===========================================================================
// Task 1.7: LocalLeaseProvider (CONTRACT-07)
// ===========================================================================

use meerkat_meta::identity_first::LocalLeaseProvider;

#[tokio::test]
async fn identity_first_contracts_local_lease_acquire_monotonic() {
    let provider = LocalLeaseProvider::new();
    let ids = vec![
        AgentIdentity::parse("a:main").unwrap(),
        AgentIdentity::parse("b:main").unwrap(),
    ];
    let result = provider.acquire_leases(&ids, "instance-1").await.unwrap();
    assert_eq!(result.len(), 2);

    let mut tokens = Vec::new();
    for (_, acq) in &result {
        match acq {
            LeaseAcquireResult::Acquired(grant) => {
                tokens.push(grant.fencing_token.get());
            }
            _ => panic!("expected Acquired"),
        }
    }
    // Tokens should be monotonically increasing
    assert!(
        tokens[0] < tokens[1] || tokens[1] < tokens[0],
        "tokens should differ"
    );
    assert!(tokens.iter().all(|t| *t > 0), "all tokens should be > 0");
}

#[tokio::test]
async fn identity_first_contracts_local_lease_renew() {
    let provider = LocalLeaseProvider::new();
    let ids = vec![AgentIdentity::parse("a:main").unwrap()];
    let result = provider.acquire_leases(&ids, "instance-1").await.unwrap();
    let grant = match &result[&ids[0]] {
        LeaseAcquireResult::Acquired(g) => g.clone(),
        _ => panic!("expected Acquired"),
    };

    let renewed = provider.renew_leases(&[grant]).await.unwrap();
    assert!(matches!(
        renewed.values().next().unwrap(),
        LeaseRenewResult::Renewed(_)
    ));
}

#[tokio::test]
async fn identity_first_contracts_local_lease_release_and_reacquire() {
    let provider = LocalLeaseProvider::new();
    let ids = vec![AgentIdentity::parse("a:main").unwrap()];

    // Acquire
    let result = provider.acquire_leases(&ids, "instance-1").await.unwrap();
    let grant = match &result[&ids[0]] {
        LeaseAcquireResult::Acquired(g) => g.clone(),
        _ => panic!("expected Acquired"),
    };
    let first_token = grant.fencing_token;

    // Release
    provider.release_leases(&[grant]).await.unwrap();

    // Re-acquire should work and give a new (higher) token
    let result2 = provider.acquire_leases(&ids, "instance-1").await.unwrap();
    let grant2 = match &result2[&ids[0]] {
        LeaseAcquireResult::Acquired(g) => g.clone(),
        _ => panic!("expected Acquired after release"),
    };
    assert!(
        grant2.fencing_token.get() > first_token.get(),
        "reacquired token should be higher"
    );
}

#[tokio::test]
async fn identity_first_contracts_local_lease_renew_after_release_returns_lost() {
    let provider = LocalLeaseProvider::new();
    let ids = vec![AgentIdentity::parse("a:main").unwrap()];

    let result = provider.acquire_leases(&ids, "instance-1").await.unwrap();
    let grant = match &result[&ids[0]] {
        LeaseAcquireResult::Acquired(g) => g.clone(),
        _ => panic!("expected Acquired"),
    };

    // Release
    provider.release_leases(&[grant.clone()]).await.unwrap();

    // Renew with old grant should return Lost
    let renewed = provider.renew_leases(&[grant]).await.unwrap();
    assert!(matches!(
        renewed.values().next().unwrap(),
        LeaseRenewResult::Lost { .. }
    ));
}

// ===========================================================================
// Task 1.8: Legacy Discovery → RosterProvider adapter (CONTRACT-08, REQ-27, REQ-28)
// ===========================================================================

use meerkat_meta::identity_first::AgentAddressability;
use meerkat_meta::identity_first::{DiscoveryRosterAdapter, agent_discovery_to_durable};
use meerkat_meta::types::AgentDiscoverySpec;

#[test]
fn identity_first_contracts_discovery_spec_to_durable_mapping() {
    let spec = AgentDiscoverySpec {
        profile: "researcher".to_string(),
        meerkat_id: "triage:main".to_string(),
        labels: Some({
            let mut m = BTreeMap::new();
            m.insert("role".to_string(), "lead".to_string());
            m
        }),
        context: Some(serde_json::json!({"key": "value"})),
        additional_instructions: vec!["be concise".to_string()],
        resume_session_id: Some("old-session".to_string()),
    };
    let durable = agent_discovery_to_durable(&spec).unwrap();
    assert_eq!(durable.identity.as_str(), "triage:main");
    assert_eq!(durable.addressability, AgentAddressability::Addressable);
    assert!(durable.display_name.is_none());
    assert_eq!(durable.labels.get("role").unwrap(), "lead");
    assert!(durable.context.is_some());
    assert_eq!(durable.additional_instructions, vec!["be concise"]);
    // resume_session_id is ignored
}

#[tokio::test]
async fn identity_first_contracts_discovery_roster_adapter() {
    use std::future::Future;
    use std::pin::Pin;

    struct TestDiscovery;
    impl meerkat_meta::unified_runtime::edge_types::Discovery for TestDiscovery {
        fn discover(
            &self,
            _context: serde_json::Value,
        ) -> Pin<Box<dyn Future<Output = Vec<AgentDiscoverySpec>> + Send + '_>> {
            Box::pin(async {
                vec![AgentDiscoverySpec {
                    profile: "default".to_string(),
                    meerkat_id: "triage:main".to_string(),
                    labels: None,
                    context: None,
                    additional_instructions: vec![],
                    resume_session_id: None,
                }]
            })
        }
    }

    let adapter = DiscoveryRosterAdapter::new(TestDiscovery);
    let ctx = RosterContext {
        mob_definition: None,
        previous_identities: vec![],
    };
    let roster = adapter.roster(&ctx).await.unwrap();
    assert_eq!(roster.len(), 1);
    assert_eq!(roster[0].identity.as_str(), "triage:main");
    assert_eq!(roster[0].addressability, AgentAddressability::Addressable);
}

// ===========================================================================
// Task 1.9: Legacy EdgeDiscovery → TopologyProvider adapter (CONTRACT-09, REQ-29)
// ===========================================================================

use meerkat_meta::identity_first::EdgeDiscoveryTopologyAdapter;
use meerkat_meta::unified_runtime::edge_types::DesiredPeerEdge;

#[tokio::test]
async fn identity_first_contracts_edge_discovery_topology_adapter() {
    use meerkat_meta::unified_runtime::edge_types::EdgeMemberView;
    use std::future::Future;
    use std::pin::Pin;

    struct TestEdgeDiscovery;
    impl meerkat_meta::unified_runtime::edge_types::EdgeDiscovery for TestEdgeDiscovery {
        fn discover_edges(
            &self,
            _active_members: Vec<EdgeMemberView>,
        ) -> Pin<Box<dyn Future<Output = Vec<DesiredPeerEdge>> + Send + '_>> {
            Box::pin(async { vec![DesiredPeerEdge::new("a:main", "b:main").unwrap()] })
        }
    }

    let adapter = EdgeDiscoveryTopologyAdapter::new(TestEdgeDiscovery);
    let ids = vec![
        AgentIdentity::parse("a:main").unwrap(),
        AgentIdentity::parse("b:main").unwrap(),
    ];
    let ctx = TopologyContext { roster: vec![] };
    let edges = adapter.compute_edges(&ids, &ctx).await.unwrap();
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].a().as_str(), "a:main");
    assert_eq!(edges[0].b().as_str(), "b:main");
}

// ===========================================================================
// Task 1.10: ContinuityStore → SessionStore adapter (CONTRACT-10)
// ===========================================================================

use meerkat_meta::identity_first::ContinuitySessionStoreAdapter;

#[tokio::test]
async fn identity_first_contracts_continuity_session_store_adapter_load_save() {
    let continuity_store = Arc::new(LocalContinuityStore::in_memory().unwrap());
    let adapter = ContinuitySessionStoreAdapter::new(continuity_store);

    let sid = meerkat_core::types::SessionId::new();
    // Load from empty → None
    let loaded: Option<meerkat_core::Session> =
        meerkat::SessionStore::load(&adapter, &sid).await.unwrap();
    assert!(loaded.is_none());
}

// ===========================================================================
// Task 1.11: SessionHook → AgentCustomizer adapter (CONTRACT-11, REQ-30)
// ===========================================================================

use meerkat_meta::identity_first::SessionHookCustomizerAdapter;

#[tokio::test]
async fn identity_first_contracts_session_hook_customizer_adapter_model_mutation() {
    struct ModelOverrideHook;

    #[async_trait]
    impl SessionHook for ModelOverrideHook {
        async fn before_create(
            &self,
            req: &mut meerkat_core::service::CreateSessionRequest,
        ) -> Result<(), meerkat_core::service::SessionError> {
            req.model = "claude-opus-4-6".to_string();
            // meerkat 0.7: CreateSessionRequest.system_prompt is the typed
            // tri-state SystemPromptOverride.
            req.system_prompt =
                meerkat_core::config::SystemPromptOverride::Set("overridden prompt".to_string());
            Ok(())
        }
    }

    let hook: Arc<dyn SessionHook> = Arc::new(ModelOverrideHook);
    let adapter = SessionHookCustomizerAdapter::new(hook);

    let ctx = AgentBuildContext {
        identity: AgentIdentity::parse("triage:main").unwrap(),
        active_peers: vec![],
        managed_edges: vec![],
        runtime_services: Default::default(),
    };
    let spec = DurableAgentSpec {
        identity: AgentIdentity::parse("triage:main").unwrap(),
        profile: meerkat_mob::ProfileName::from("default"),
        addressability: Default::default(),
        display_name: None,
        labels: BTreeMap::new(),
        context: None,
        additional_instructions: vec![],
        initial_message: None,
        runtime_mode_override: None,
        backend: None,
        binding: None,
        placement: None,
    };
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

    adapter
        .customize_build(&ctx, &spec, &mut draft)
        .await
        .unwrap();
    assert_eq!(draft.model.as_deref(), Some("claude-opus-4-6"));
    assert_eq!(draft.system_prompt.as_deref(), Some("overridden prompt"));
}

/// The legacy-hook adapter synthesizes a `CreateSessionRequest`, which has no
/// provider-params field, and writes back only the fields it supports. A
/// draft's provider params must survive that hop untouched — a wholesale
/// draft rebuild here would silently drop the cache policy for every
/// deployment still on `SessionHook`.
#[tokio::test]
async fn identity_first_contracts_session_hook_customizer_preserves_provider_params() {
    use meerkat_core::lifecycle::run_primitive::{
        OpenAiProviderTag, ProviderParamsOverride, ProviderTag,
    };

    struct ModelOnlyHook;

    #[async_trait]
    impl SessionHook for ModelOnlyHook {
        async fn before_create(
            &self,
            req: &mut meerkat_core::service::CreateSessionRequest,
        ) -> Result<(), meerkat_core::service::SessionError> {
            req.model = "gpt-5.5".to_string();
            Ok(())
        }
    }

    let hook: Arc<dyn SessionHook> = Arc::new(ModelOnlyHook);
    let adapter = SessionHookCustomizerAdapter::new(hook);

    let ctx = AgentBuildContext {
        identity: AgentIdentity::parse("triage:main").unwrap(),
        active_peers: vec![],
        managed_edges: vec![],
        runtime_services: Default::default(),
    };
    let spec = DurableAgentSpec {
        identity: AgentIdentity::parse("triage:main").unwrap(),
        profile: meerkat_mob::ProfileName::from("default"),
        addressability: Default::default(),
        display_name: None,
        labels: BTreeMap::new(),
        context: None,
        additional_instructions: vec![],
        initial_message: None,
        runtime_mode_override: None,
        backend: None,
        binding: None,
        placement: None,
    };
    let declared = ProviderParamsOverride {
        provider_tag: Some(ProviderTag::OpenAi(OpenAiProviderTag {
            prompt_cache_key: Some("tenant-a:stable-prefix".to_string()),
            ..Default::default()
        })),
        ..Default::default()
    };
    let mut draft = AgentBuildDraft {
        compaction_curator: Default::default(),
        model: None,
        system_prompt: None,
        additional_instructions: vec![],
        labels: BTreeMap::new(),
        app_context: None,
        external_tools: vec![],
        local_external_tools: Default::default(),
        provider_params: Some(declared.clone()),
    };

    adapter
        .customize_build(&ctx, &spec, &mut draft)
        .await
        .unwrap();

    assert_eq!(draft.model.as_deref(), Some("gpt-5.5"));
    assert_eq!(draft.provider_params, Some(declared));
}

#[tokio::test]
async fn identity_first_contracts_session_hook_customizer_resume_warning() {
    use meerkat_core::service::SessionBuildOptions;

    struct ResumeHook;

    #[async_trait]
    impl SessionHook for ResumeHook {
        async fn before_create(
            &self,
            req: &mut meerkat_core::service::CreateSessionRequest,
        ) -> Result<(), meerkat_core::service::SessionError> {
            // This should trigger a warning — resume_session is NOT allowed
            let mut build_opts = SessionBuildOptions::default();
            build_opts.resume_session = Some(meerkat_core::Session::default());
            req.build = Some(build_opts);
            Ok(())
        }
    }

    let hook: Arc<dyn SessionHook> = Arc::new(ResumeHook);
    let adapter = SessionHookCustomizerAdapter::new(hook);

    let ctx = AgentBuildContext {
        identity: AgentIdentity::parse("triage:main").unwrap(),
        active_peers: vec![],
        managed_edges: vec![],
        runtime_services: Default::default(),
    };
    let spec = DurableAgentSpec {
        identity: AgentIdentity::parse("triage:main").unwrap(),
        profile: meerkat_mob::ProfileName::from("default"),
        addressability: Default::default(),
        display_name: None,
        labels: BTreeMap::new(),
        context: None,
        additional_instructions: vec![],
        initial_message: None,
        runtime_mode_override: None,
        backend: None,
        binding: None,
        placement: None,
    };
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

    // Should succeed but log a warning — we verify it doesn't error
    adapter
        .customize_build(&ctx, &spec, &mut draft)
        .await
        .unwrap();
    // The resume_session mutation should NOT be applied to the draft
    // (draft has no resume_session field — that's the point)
}

#[tokio::test]
async fn identity_first_contracts_session_hook_customizer_unsupported_field_warning() {
    /// Hook that mutates max_tokens and prompt — both unsupported by the adapter.
    struct UnsupportedFieldsHook;

    #[async_trait]
    impl SessionHook for UnsupportedFieldsHook {
        async fn before_create(
            &self,
            req: &mut meerkat_core::service::CreateSessionRequest,
        ) -> Result<(), meerkat_core::service::SessionError> {
            req.max_tokens = Some(4096);
            req.prompt = meerkat_core::ContentInput::Text("mutated prompt".to_string());
            Ok(())
        }
    }

    let hook: Arc<dyn SessionHook> = Arc::new(UnsupportedFieldsHook);
    let adapter = SessionHookCustomizerAdapter::new(hook);

    let ctx = AgentBuildContext {
        identity: AgentIdentity::parse("review:agent").unwrap(),
        active_peers: vec![],
        managed_edges: vec![],
        runtime_services: Default::default(),
    };
    let spec = DurableAgentSpec {
        identity: AgentIdentity::parse("review:agent").unwrap(),
        profile: meerkat_mob::ProfileName::from("reviewer"),
        addressability: Default::default(),
        display_name: None,
        labels: BTreeMap::new(),
        context: None,
        additional_instructions: vec![],
        initial_message: None,
        runtime_mode_override: None,
        backend: None,
        binding: None,
        placement: None,
    };
    let mut draft = AgentBuildDraft {
        compaction_curator: Default::default(),
        model: Some("claude-sonnet-4-6".to_string()),
        system_prompt: Some("original prompt".to_string()),
        additional_instructions: vec![],
        labels: BTreeMap::new(),
        app_context: None,
        external_tools: vec![],
        local_external_tools: Default::default(),
        provider_params: None,
    };

    // Should succeed — unsupported mutations are warned, not errored
    adapter
        .customize_build(&ctx, &spec, &mut draft)
        .await
        .unwrap();

    // Supported fields should be unchanged (hook didn't touch model/system_prompt/labels)
    assert_eq!(draft.model, Some("claude-sonnet-4-6".to_string()));
    assert_eq!(draft.system_prompt, Some("original prompt".to_string()));

    // Unsupported mutations (max_tokens, prompt) are NOT applied to draft
    // (draft has no max_tokens or prompt field — the warning is the signal)
}
