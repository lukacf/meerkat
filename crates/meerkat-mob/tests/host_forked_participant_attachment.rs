//! Durable host-side forked-participant attachment association lifecycle
//! (issue #159 phase 3 slice B).
//!
//! These drive the REAL source-owner [`ForkedParticipantService`] over a REAL
//! durable capability store, and the REAL host binding persistence over a REAL
//! sqlite runtime store. Nothing here mocks the lifecycle machine: attach and
//! release verdicts come from the canonical
//! `ForkedParticipantLifecycleMachine` exactly as they do in production.
//!
//! What is pinned:
//!   * a materialized row carries its association through serialization and
//!     process restart, and recovery re-validates it;
//!   * an EXACT materialize replay compares and never re-attaches — the
//!     capability store observes exactly one attach-driven exact load;
//!   * any replay difference, in either direction, is refused;
//!   * teardown releases exactly once, converges on retry, and never releases
//!     twice;
//!   * a release that cannot be proven retains the row and the debt;
//!   * a revoke-shaped sweep that hits one failure releases nothing further
//!     and reports the failure;
//!   * supersession releases the old association;
//!   * a definitive build failure releases, an ambiguous one retains a durable
//!     obligation, and a malformed recovered association fails closed.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use meerkat_contracts::wire::supervisor_bridge::{
    BridgeCapabilities, BridgeForkedParticipantAttachment, BridgePeerIdentity, BridgePeerSpec,
    BridgeRejectionCause, MaterializeLaunchOutcome,
};
use meerkat_contracts::wire::{PortableMemberSpec, portable_member_spec_digest};
use meerkat_core::SessionId;
use meerkat_core::service::SessionError;
use meerkat_mob::forked_participant::{
    ForkedParticipantAttachmentAssociation, ForkedParticipantAttachmentId, ForkedParticipantError,
    ForkedParticipantOperationScope, ForkedParticipantOwnerRoute,
    ForkedParticipantPendingAttachment, ForkedParticipantPendingTerminal, ForkedParticipantRef,
    ForkedParticipantRequest, ForkedParticipantRequestId, ForkedParticipantReusePolicy,
    ForkedParticipantService, ForkedParticipantSourceRuntime, PlannedChildEvidence,
    PlannedForkOutcome, PlannedForkRequest, SessionExecutionEvidence,
};
use meerkat_mob::ids::AgentIdentity;
use meerkat_mob::machines::mob_host_binding_authority::{
    AgentIdentity as AuthorityAgentIdentity, MemberKey,
    MemberSessionDisposal as MachineMemberSessionDisposal, MobHostBindingAuthorityAuthority,
    MobId as AuthorityMobId,
};
use meerkat_mob::runtime::host_actor::{
    ForkedParticipantAttachmentObligation, ForkedParticipantObligationAuthority,
    ForkedParticipantObligationCause, HostBindObservations, HostBindServeOutcome,
    HostBootstrapTokenSlot, MaterializedMemberRow, MobHostBindingPersistence, MobHostBindingRecord,
    ReleaseAdmission, RuntimeStoreHostBindingPersistence, admit_resume_against_fork_protection,
    commit_member_release_after_disposal, correlate_pending_forked_participant_attachments,
    record_materialized_member, recover_or_create_binding_authority,
    release_forked_participant_association, replayed_forked_participant_attachment_matches,
    resolve_release_admission, run_final_forked_participant_sweep, serve_host_bind,
    validate_recovered_forked_participant_routes,
};
use meerkat_mob::store::{
    ForkedParticipantRecord, ForkedParticipantStore, InMemoryForkedParticipantStore, MobStoreError,
};

const PREFIX_DIGEST: &str = "sha256:selected-prefix";
const PREFIX_COUNT: usize = 3;
const SOURCE_MEMBER: &str = "researcher";
const HOST_ID: &str = "host-a";
const MOB_ID: &str = "mob-capability";
const MEMBER_IDENTITY: &str = "branch-1";

// ---------------------------------------------------------------------------
// Real service over a fake source runtime (the runtime is the only fake: it
// stands in for durable session bodies, not for lifecycle authority).
// ---------------------------------------------------------------------------

#[derive(Default)]
struct FakeRuntimeState {
    sources: HashMap<String, SessionExecutionEvidence>,
    children: HashMap<String, PlannedChildEvidence>,
    archived: Vec<SessionId>,
}

#[derive(Default, Clone)]
struct FakeSourceRuntime {
    state: Arc<Mutex<FakeRuntimeState>>,
}

impl FakeSourceRuntime {
    fn lock(&self) -> std::sync::MutexGuard<'_, FakeRuntimeState> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }

    fn register_source(&self, session_id: &SessionId) {
        self.lock()
            .sources
            .insert(session_id.to_string(), source_evidence());
    }

    fn archived(&self) -> Vec<SessionId> {
        self.lock().archived.clone()
    }
}

#[async_trait]
impl ForkedParticipantSourceRuntime for FakeSourceRuntime {
    async fn session_execution_evidence(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<SessionExecutionEvidence>, SessionError> {
        Ok(self.lock().sources.get(&session_id.to_string()).cloned())
    }

    async fn fork_planned_child(
        &self,
        request: PlannedForkRequest,
    ) -> Result<PlannedForkOutcome, SessionError> {
        let mut state = self.lock();
        let source = state
            .sources
            .get(&request.source_session_id.to_string())
            .cloned()
            .unwrap_or(SessionExecutionEvidence {
                agent_identity: None,
                realm_id: None,
                tool_access_policy: None,
                auth_binding: None,
            });
        let evidence = PlannedChildEvidence {
            prefix_digest: PREFIX_DIGEST.to_string(),
            prefix_message_count: request.prefix_message_count.unwrap_or(PREFIX_COUNT),
            execution: source,
        };
        state.children.insert(
            request.planned_child_session_id.to_string(),
            evidence.clone(),
        );
        Ok(PlannedForkOutcome {
            child_session_id: request.planned_child_session_id,
            prefix_message_count: evidence.prefix_message_count,
            prefix_digest: evidence.prefix_digest,
        })
    }

    async fn planned_child_evidence(
        &self,
        child_session_id: &SessionId,
    ) -> Result<Option<PlannedChildEvidence>, SessionError> {
        Ok(self
            .lock()
            .children
            .get(&child_session_id.to_string())
            .cloned())
    }

    async fn archive_fork_session(&self, child_session_id: &SessionId) -> Result<(), SessionError> {
        let mut state = self.lock();
        state.archived.push(child_session_id.clone());
        state.children.remove(&child_session_id.to_string());
        Ok(())
    }
}

/// Store decorator that counts full-reference loads and can be made to fail.
///
/// `load_exact` is the exact primitive every holder-driven transition performs
/// once, so counting it is a faithful proxy for "how many attach/release calls
/// actually reached the owner".
struct ObservableStore {
    inner: Arc<dyn ForkedParticipantStore>,
    exact_loads: AtomicUsize,
    fail_exact_loads: Mutex<usize>,
}

impl ObservableStore {
    fn new(inner: Arc<dyn ForkedParticipantStore>) -> Arc<Self> {
        Arc::new(Self {
            inner,
            exact_loads: AtomicUsize::new(0),
            fail_exact_loads: Mutex::new(0),
        })
    }

    fn exact_loads(&self) -> usize {
        self.exact_loads.load(Ordering::SeqCst)
    }

    fn fail_next_exact_loads(&self, count: usize) {
        *self
            .fail_exact_loads
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = count;
    }
}

#[async_trait]
impl ForkedParticipantStore for ObservableStore {
    async fn insert_reserved(
        &self,
        record: &ForkedParticipantRecord,
    ) -> Result<ForkedParticipantRecord, MobStoreError> {
        self.inner.insert_reserved(record).await
    }

    async fn load_by_capability_id(
        &self,
        capability_id: &meerkat_mob::forked_participant::ForkedParticipantCapabilityId,
    ) -> Result<Option<ForkedParticipantRecord>, MobStoreError> {
        self.inner.load_by_capability_id(capability_id).await
    }

    async fn load_by_request_id(
        &self,
        request_id: &ForkedParticipantRequestId,
    ) -> Result<Option<ForkedParticipantRecord>, MobStoreError> {
        self.inner.load_by_request_id(request_id).await
    }

    async fn load_by_fork_session_id(
        &self,
        fork_session_id: &SessionId,
    ) -> Result<Option<ForkedParticipantRecord>, MobStoreError> {
        self.inner.load_by_fork_session_id(fork_session_id).await
    }

    async fn load_exact(
        &self,
        capability: &ForkedParticipantRef,
    ) -> Result<ForkedParticipantRecord, MobStoreError> {
        self.exact_loads.fetch_add(1, Ordering::SeqCst);
        {
            let mut remaining = self
                .fail_exact_loads
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if *remaining > 0 {
                *remaining -= 1;
                return Err(MobStoreError::ReadFailed(
                    "injected capability store read fault".to_string(),
                ));
            }
        }
        self.inner.load_exact(capability).await
    }

    async fn commit(
        &self,
        record: &ForkedParticipantRecord,
    ) -> Result<ForkedParticipantRecord, MobStoreError> {
        self.inner.commit(record).await
    }

    async fn list_all(&self) -> Result<Vec<ForkedParticipantRecord>, MobStoreError> {
        self.inner.list_all().await
    }
}

fn realm() -> meerkat_core::RealmId {
    meerkat_core::RealmId::parse("global").expect("realm")
}

fn host_route() -> ForkedParticipantOwnerRoute {
    ForkedParticipantOwnerRoute::Host {
        realm_id: realm(),
        host_id: meerkat_mob::machines::mob_machine::HostId::from(HOST_ID.to_string()),
    }
}

fn source_evidence() -> SessionExecutionEvidence {
    SessionExecutionEvidence {
        agent_identity: Some(AgentIdentity::from(SOURCE_MEMBER)),
        realm_id: Some(realm()),
        tool_access_policy: None,
        auth_binding: None,
    }
}

fn now() -> DateTime<Utc> {
    Utc::now()
}

struct CapabilityHarness {
    service: ForkedParticipantService,
    store: Arc<ObservableStore>,
    runtime: FakeSourceRuntime,
}

impl CapabilityHarness {
    fn new() -> Self {
        let runtime = FakeSourceRuntime::default();
        let store = ObservableStore::new(Arc::new(InMemoryForkedParticipantStore::new()));
        let service = ForkedParticipantService::new(
            host_route(),
            Arc::clone(&store) as Arc<dyn ForkedParticipantStore>,
            Arc::new(runtime.clone()),
        )
        .expect("service composes");
        Self {
            service,
            store,
            runtime,
        }
    }

    /// Mint one live capability through the REAL create path.
    async fn mint(
        &self,
        request_id: &str,
        reuse: ForkedParticipantReusePolicy,
    ) -> CapabilityFixture {
        self.mint_with_ttl(request_id, reuse, Duration::from_secs(600))
            .await
    }

    /// Mint with an explicit time-to-live, so a test can keep one capability
    /// clear of another's expiry observation.
    async fn mint_with_ttl(
        &self,
        request_id: &str,
        reuse: ForkedParticipantReusePolicy,
        ttl: Duration,
    ) -> CapabilityFixture {
        let source_session_id = SessionId::new();
        self.runtime.register_source(&source_session_id);
        let request = ForkedParticipantRequest {
            request_id: ForkedParticipantRequestId::new(request_id).expect("request id"),
            source_identity: AgentIdentity::from(SOURCE_MEMBER),
            source_session_id,
            owner_route: host_route(),
            prefix_message_count: Some(PREFIX_COUNT),
            scope: ForkedParticipantOperationScope::InvokeAndObserve,
            reuse,
            ttl,
        };
        let capability = self.service.create(&request, now()).await.expect("create");
        CapabilityFixture { capability }
    }

    /// Rebuild the owner service over the SAME durable capability store, as a
    /// restarted process would.
    fn reopen(&self) -> ForkedParticipantService {
        ForkedParticipantService::new(
            host_route(),
            Arc::clone(&self.store) as Arc<dyn ForkedParticipantStore>,
            Arc::new(self.runtime.clone()),
        )
        .expect("service composes")
    }
}

struct CapabilityFixture {
    capability: ForkedParticipantRef,
}

impl CapabilityFixture {
    fn association(&self, attachment_id: &str) -> ForkedParticipantAttachmentAssociation {
        ForkedParticipantAttachmentAssociation::new(
            self.capability.clone(),
            ForkedParticipantAttachmentId::new(attachment_id).expect("attachment id"),
        )
    }

    fn wire(&self, attachment_id: &str) -> BridgeForkedParticipantAttachment {
        BridgeForkedParticipantAttachment {
            attachment_id: attachment_id.to_string(),
            capability: meerkat_mob::forked_participant::bridge_ref(&self.capability),
        }
    }
}

// ---------------------------------------------------------------------------
// Host binding persistence fixture (real sqlite runtime store)
// ---------------------------------------------------------------------------

struct HostFixture {
    _dir: tempfile::TempDir,
    store: Arc<meerkat_runtime::store::SqliteRuntimeStore>,
    persistence: RuntimeStoreHostBindingPersistence,
    authority: MobHostBindingAuthorityAuthority,
}

impl HostFixture {
    async fn bound() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Arc::new(
            meerkat_runtime::store::SqliteRuntimeStore::new(dir.path().join("runtime.sqlite3"))
                .expect("sqlite runtime store"),
        );
        let persistence = RuntimeStoreHostBindingPersistence::new(
            Arc::clone(&store) as Arc<dyn meerkat_runtime::store::RuntimeStore>
        );
        let mut authority = MobHostBindingAuthorityAuthority::new();
        let mut token = HostBootstrapTokenSlot::mint();
        let keypair = meerkat_comms::Keypair::generate();
        let pubkey = keypair.public_key();
        let supervisor = BridgePeerIdentity::try_from(&BridgePeerSpec {
            name: "supervisor-a".to_string(),
            peer_id: pubkey.to_peer_id().as_str(),
            address: "tcp://127.0.0.1:1".to_string(),
            pubkey: *pubkey.as_bytes(),
        })
        .expect("valid supervisor spec");
        let presented = token.current().to_string();
        let token_valid = meerkat_comms::constant_time_str_eq(token.current(), &presented);
        let outcome = serve_host_bind(
            &mut authority,
            &persistence,
            &mut token,
            HostBindObservations {
                mob_id: MOB_ID.to_string(),
                supervisor,
                epoch: 1,
                binding_generation: 1,
                sender_matches_supervisor: true,
                address_matches: true,
                token_valid,
                accepted_capabilities: BridgeCapabilities::default(),
            },
        )
        .await
        .expect("host binds");
        assert!(
            matches!(outcome, HostBindServeOutcome::Accepted { .. }),
            "the fixture host must bind"
        );
        Self {
            _dir: dir,
            store,
            persistence,
            authority,
        }
    }

    /// Rebuild the host binding authority from durable rows alone, exactly as
    /// a restarted daemon does. The temp dir (and therefore the sqlite file)
    /// is moved into the successor, so the durable state is the same bytes.
    async fn restart(self) -> Self {
        let Self {
            _dir,
            store,
            persistence,
            authority: _,
        } = self;
        let authority = recover_or_create_binding_authority(&persistence)
            .await
            .expect("recovery accepts the durable rows");
        Self {
            _dir,
            store,
            persistence,
            authority,
        }
    }

    fn member_key(&self) -> MemberKey {
        MemberKey::new(
            AuthorityMobId::from(MOB_ID),
            AuthorityAgentIdentity::from(MEMBER_IDENTITY),
        )
    }

    async fn record(&self) -> MobHostBindingRecord {
        self.persistence
            .load(MOB_ID)
            .await
            .expect("load")
            .expect("bound record")
    }
}

fn materialized_row(session_id: &str, generation: u64, fence_token: u64) -> MaterializedMemberRow {
    materialized_row_for(MEMBER_IDENTITY, session_id, generation, fence_token)
}

fn materialized_row_for(
    identity: &str,
    session_id: &str,
    generation: u64,
    fence_token: u64,
) -> MaterializedMemberRow {
    let spec: PortableMemberSpec = serde_json::from_value(serde_json::json!({
        "mob_id": MOB_ID,
        "profile_name": "worker",
        "agent_identity": identity,
        "profile": {
            "model": "claude-opus-4-8",
            "provider": "anthropic",
            "tools": { "comms": true },
            "runtime_mode": "turn_driven"
        },
        "definition_extract": {},
        "overlay": {
            "system_prompt": { "prompt": "disable" },
            "runtime_mode": "turn_driven"
        }
    }))
    .expect("portable member spec decodes");
    let spec_digest = portable_member_spec_digest(&spec).expect("portable spec digests");
    let keypair = meerkat_comms::Keypair::generate();
    let pubkey = keypair.public_key();
    MaterializedMemberRow {
        generation,
        generation_start_seq: 1,
        fence_token,
        session_id: session_id.to_string(),
        spec_digest,
        spec,
        engine_version_at_build: "0.0.0-test".to_string(),
        member_pubkey: pubkey.to_pubkey_string(),
        member_peer_id: pubkey.to_peer_id().to_string(),
        launch_outcome: MaterializeLaunchOutcome::ResumedFromSnapshot,
        resolved_auth_binding: None,
        supervisor_name: "supervisor-a".to_string(),
        supervisor_address: "tcp://127.0.0.1:1".to_string(),
        forked_participant_attachment: None,
    }
}

// ===========================================================================
// Materialize: association is persisted in the SAME durable transaction and
// survives serialization + restart, and recovery re-validates it.
// ===========================================================================

#[tokio::test]
async fn association_commits_with_the_row_and_survives_restart() {
    let capabilities = CapabilityHarness::new();
    let fixture = capabilities
        .mint("req-persist", ForkedParticipantReusePolicy::OneShot)
        .await;
    let association = fixture.association("attach-1");
    let fork_session_id = fixture.capability.fork_session_id().to_string();

    let mut host = HostFixture::bound().await;
    let key = host.member_key();
    record_materialized_member(
        &mut host.authority,
        &host.persistence,
        &key,
        materialized_row(&fork_session_id, 1, 1),
        Some(association.clone()),
    )
    .await
    .expect("materialized row records with its association");

    // The durable blob carries the association verbatim.
    let recorded = host.record().await;
    assert_eq!(
        recorded
            .materialized
            .get(MEMBER_IDENTITY)
            .and_then(|row| row.forked_participant_attachment.clone()),
        Some(association.clone()),
        "the association must ride the same durable row"
    );

    // Restart: a fresh persistence over the SAME store recovers it, and the
    // route validator accepts it against the composed service.
    let reopened = RuntimeStoreHostBindingPersistence::new(
        Arc::clone(&host.store) as Arc<dyn meerkat_runtime::store::RuntimeStore>
    );
    recover_or_create_binding_authority(&reopened)
        .await
        .expect("recovery accepts the association-carrying row");
    let records = reopened.list_records().await.expect("list records");
    validate_recovered_forked_participant_routes(&records, Some(&capabilities.service))
        .expect("the recovered association routes to this host");
    assert_eq!(
        records[0]
            .1
            .materialized
            .get(MEMBER_IDENTITY)
            .and_then(|row| row.forked_participant_attachment.clone()),
        Some(association),
        "the association must survive serialization unchanged"
    );
}

#[tokio::test]
async fn a_recovered_association_for_another_route_fails_closed() {
    let capabilities = CapabilityHarness::new();
    let fixture = capabilities
        .mint("req-foreign", ForkedParticipantReusePolicy::OneShot)
        .await;
    let association = fixture.association("attach-1");
    let fork_session_id = fixture.capability.fork_session_id().to_string();

    let mut host = HostFixture::bound().await;
    let key = host.member_key();
    record_materialized_member(
        &mut host.authority,
        &host.persistence,
        &key,
        materialized_row(&fork_session_id, 1, 1),
        Some(association),
    )
    .await
    .expect("row records");
    let records = host.persistence.list_records().await.expect("list");

    // No composed service at all: the residency exists but nothing could ever
    // release its attachment.
    let error = validate_recovered_forked_participant_routes(&records, None)
        .expect_err("an unroutable association must fail closed");
    assert!(
        error.to_string().contains("source-owner service"),
        "expected a typed unroutable-association failure, got {error}"
    );

    // A service for a DIFFERENT host: the association names another owner.
    let other = ForkedParticipantService::new(
        ForkedParticipantOwnerRoute::Host {
            realm_id: realm(),
            host_id: meerkat_mob::machines::mob_machine::HostId::from("host-b".to_string()),
        },
        Arc::new(InMemoryForkedParticipantStore::new()),
        Arc::new(FakeSourceRuntime::default()),
    )
    .expect("service composes");
    let error = validate_recovered_forked_participant_routes(&records, Some(&other))
        .expect_err("a foreign-route association must fail closed");
    assert!(
        error.to_string().contains("another host route"),
        "expected a typed route mismatch, got {error}"
    );
}

#[tokio::test]
async fn a_malformed_recovered_association_fails_closed() {
    let capabilities = CapabilityHarness::new();
    let fixture = capabilities
        .mint("req-malformed", ForkedParticipantReusePolicy::OneShot)
        .await;
    let association = fixture.association("attach-1");

    let mut host = HostFixture::bound().await;
    let key = host.member_key();
    // The row names a DIFFERENT session than the association's fork session.
    record_materialized_member(
        &mut host.authority,
        &host.persistence,
        &key,
        materialized_row(&SessionId::new().to_string(), 1, 1),
        Some(association),
    )
    .await
    .expect("the row itself records");

    let reopened = RuntimeStoreHostBindingPersistence::new(
        Arc::clone(&host.store) as Arc<dyn meerkat_runtime::store::RuntimeStore>
    );
    let error = match recover_or_create_binding_authority(&reopened).await {
        Ok(_) => panic!("recovery must reject a self-contradicting association"),
        Err(error) => error,
    };
    assert!(
        error.to_string().contains("fork session"),
        "expected a fork-session mismatch, got {error}"
    );
    let records = reopened.list_records().await.expect("list");
    validate_recovered_forked_participant_routes(&records, Some(&capabilities.service))
        .expect_err("the route validator must reject it too");
}

// ===========================================================================
// Materialize replay: compare, never re-attach.
// ===========================================================================

#[tokio::test]
async fn exact_replay_compares_the_association_without_a_second_attach() {
    let capabilities = CapabilityHarness::new();
    let fixture = capabilities
        .mint("req-replay", ForkedParticipantReusePolicy::OneShot)
        .await;
    let association = fixture.association("attach-1");

    let attach_baseline = capabilities.store.exact_loads();
    capabilities
        .service
        .attach(
            &association.capability,
            &association.attachment_id,
            true,
            now(),
        )
        .await
        .expect("first attach is granted");
    let after_attach = capabilities.store.exact_loads();
    assert_eq!(
        after_attach - attach_baseline,
        1,
        "one attach performs exactly one exact capability load"
    );

    // The replay arm's decision: identical attachment ⇒ answer only.
    assert!(
        replayed_forked_participant_attachment_matches(
            Some(&fixture.wire("attach-1")),
            Some(&association),
        ),
        "an identical attachment must replay"
    );
    assert_eq!(
        capabilities.store.exact_loads(),
        after_attach,
        "materialize replay must not touch capability lifecycle at all"
    );
}

#[tokio::test]
async fn replay_refuses_every_association_difference() {
    let capabilities = CapabilityHarness::new();
    let one = capabilities
        .mint("req-diff-a", ForkedParticipantReusePolicy::OneShot)
        .await;
    let two = capabilities
        .mint("req-diff-b", ForkedParticipantReusePolicy::OneShot)
        .await;
    let association = one.association("attach-1");

    assert!(
        !replayed_forked_participant_attachment_matches(None, Some(&association)),
        "an absent incoming attachment must not replay a recorded one"
    );
    assert!(
        !replayed_forked_participant_attachment_matches(Some(&one.wire("attach-1")), None),
        "a present incoming attachment must not replay an unassociated row"
    );
    assert!(
        !replayed_forked_participant_attachment_matches(
            Some(&one.wire("attach-2")),
            Some(&association)
        ),
        "a different attachment id must not replay"
    );
    assert!(
        !replayed_forked_participant_attachment_matches(
            Some(&two.wire("attach-1")),
            Some(&association)
        ),
        "a different capability must not replay"
    );
    assert!(
        replayed_forked_participant_attachment_matches(None, None),
        "an unassociated replay is still a replay"
    );
}

// ===========================================================================
// Teardown: release exactly once, converge on retry, never twice.
// ===========================================================================

#[tokio::test]
async fn teardown_releases_once_and_a_second_release_converges() {
    let capabilities = CapabilityHarness::new();
    let fixture = capabilities
        .mint("req-release", ForkedParticipantReusePolicy::OneShot)
        .await;
    let association = fixture.association("attach-1");
    capabilities
        .service
        .attach(
            &association.capability,
            &association.attachment_id,
            true,
            now(),
        )
        .await
        .expect("attach");

    release_forked_participant_association(Some(&capabilities.service), &association)
        .await
        .expect("the first release succeeds");

    // A one-shot capability terminalizes on release. The exact retry that a
    // lost reply produces must still converge rather than strand the row.
    release_forked_participant_association(Some(&capabilities.service), &association)
        .await
        .expect("an exact retry converges instead of failing");

    // Direct service-level proof that no second attachment survived.
    let error = capabilities
        .service
        .attach(
            &association.capability,
            &ForkedParticipantAttachmentId::new("attach-2").expect("attachment id"),
            true,
            now(),
        )
        .await
        .expect_err("a one-shot capability must not attach again after release");
    assert!(
        matches!(error, ForkedParticipantError::AttachDenied { .. }),
        "expected a typed attach denial, got {error:?}"
    );
}

#[tokio::test]
async fn an_unprovable_release_retains_the_row_and_the_retry_converges() {
    let capabilities = CapabilityHarness::new();
    let fixture = capabilities
        .mint("req-debt", ForkedParticipantReusePolicy::OneShot)
        .await;
    let association = fixture.association("attach-1");
    capabilities
        .service
        .attach(
            &association.capability,
            &association.attachment_id,
            true,
            now(),
        )
        .await
        .expect("attach");

    // The durable materialized row exists and carries the association.
    let mut host = HostFixture::bound().await;
    let key = host.member_key();
    record_materialized_member(
        &mut host.authority,
        &host.persistence,
        &key,
        materialized_row(&fixture.capability.fork_session_id().to_string(), 1, 1),
        Some(association.clone()),
    )
    .await
    .expect("row records");

    capabilities.store.fail_next_exact_loads(1);
    let failure = release_forked_participant_association(Some(&capabilities.service), &association)
        .await
        .expect_err("an unprovable release must be typed, not swallowed");
    assert_eq!(
        failure.cause,
        BridgeRejectionCause::Unavailable,
        "a store fault degrades to Unavailable so the caller retries"
    );

    // The release was withheld, so the row and its association are retained.
    let retained = host.record().await;
    assert_eq!(
        retained
            .materialized
            .get(MEMBER_IDENTITY)
            .and_then(|row| row.forked_participant_attachment.clone()),
        Some(association.clone()),
        "a withheld release must keep the row and the association"
    );

    // The exact retry converges once the fault clears.
    release_forked_participant_association(Some(&capabilities.service), &association)
        .await
        .expect("the retry converges");
}

#[tokio::test]
async fn a_revoke_shaped_sweep_stops_at_the_first_unprovable_release() {
    let capabilities = CapabilityHarness::new();
    let first = capabilities
        .mint("req-revoke-a", ForkedParticipantReusePolicy::OneShot)
        .await;
    let second = capabilities
        .mint("req-revoke-b", ForkedParticipantReusePolicy::OneShot)
        .await;
    let first_association = first.association("attach-a");
    let second_association = second.association("attach-b");
    for association in [&first_association, &second_association] {
        capabilities
            .service
            .attach(
                &association.capability,
                &association.attachment_id,
                true,
                now(),
            )
            .await
            .expect("attach");
    }

    // The revoke arm releases every association before it may write a
    // receipt. Injecting a fault on the SECOND release proves the sweep
    // reports failure rather than claiming a clean revocation.
    release_forked_participant_association(Some(&capabilities.service), &first_association)
        .await
        .expect("the first association releases");
    capabilities.store.fail_next_exact_loads(1);
    release_forked_participant_association(Some(&capabilities.service), &second_association)
        .await
        .expect_err("the failing association must block the revoke receipt");

    // Retry converges, which is what makes the withheld receipt safe.
    release_forked_participant_association(Some(&capabilities.service), &second_association)
        .await
        .expect("the retry converges");
    release_forked_participant_association(Some(&capabilities.service), &first_association)
        .await
        .expect("an already-released association stays converged on replay");
}

#[tokio::test]
async fn supersession_releases_the_old_association_before_the_new_row_commits() {
    let capabilities = CapabilityHarness::new();
    let old = capabilities
        .mint("req-super-old", ForkedParticipantReusePolicy::OneShot)
        .await;
    let new = capabilities
        .mint("req-super-new", ForkedParticipantReusePolicy::OneShot)
        .await;
    let old_association = old.association("attach-old");
    let new_association = new.association("attach-new");
    capabilities
        .service
        .attach(
            &old_association.capability,
            &old_association.attachment_id,
            true,
            now(),
        )
        .await
        .expect("old attach");

    let mut host = HostFixture::bound().await;
    let key = host.member_key();
    record_materialized_member(
        &mut host.authority,
        &host.persistence,
        &key,
        materialized_row(&old.capability.fork_session_id().to_string(), 1, 1),
        Some(old_association.clone()),
    )
    .await
    .expect("old row records");

    // Replacement: attach the new capability, release the old association,
    // then commit the replacement row — exactly the serving order.
    capabilities
        .service
        .attach(
            &new_association.capability,
            &new_association.attachment_id,
            true,
            now(),
        )
        .await
        .expect("new attach");
    release_forked_participant_association(Some(&capabilities.service), &old_association)
        .await
        .expect("the superseded association releases before the replacement commits");
    record_materialized_member(
        &mut host.authority,
        &host.persistence,
        &key,
        materialized_row(&new.capability.fork_session_id().to_string(), 2, 2),
        Some(new_association.clone()),
    )
    .await
    .expect("replacement row records");

    let record = host.record().await;
    assert_eq!(
        record
            .materialized
            .get(MEMBER_IDENTITY)
            .and_then(|row| row.forked_participant_attachment.clone()),
        Some(new_association.clone()),
        "the replacement row owns the new association"
    );
    // The old capability is terminal; the new one is still attached.
    let old_reattach = capabilities
        .service
        .attach(
            &old_association.capability,
            &ForkedParticipantAttachmentId::new("attach-old-2").expect("attachment id"),
            true,
            now(),
        )
        .await
        .expect_err("the superseded capability must be released");
    assert!(
        matches!(old_reattach, ForkedParticipantError::AttachDenied { .. }),
        "expected a typed denial, got {old_reattach:?}"
    );
    release_forked_participant_association(Some(&capabilities.service), &new_association)
        .await
        .expect("the surviving association is still releasable");
}

// ===========================================================================
// Build-failure compensation: definitive releases, ambiguous retains.
// ===========================================================================

#[tokio::test]
async fn a_definitive_build_failure_releases_the_new_attachment() {
    let capabilities = CapabilityHarness::new();
    let fixture = capabilities
        .mint("req-build-fail", ForkedParticipantReusePolicy::OneShot)
        .await;
    let association = fixture.association("attach-1");
    capabilities
        .service
        .attach(
            &association.capability,
            &association.attachment_id,
            true,
            now(),
        )
        .await
        .expect("attach");

    // The definitive-failure arm compensates immediately.
    release_forked_participant_association(Some(&capabilities.service), &association)
        .await
        .expect("a definitive build failure releases the admitted attachment");

    let host = HostFixture::bound().await;
    let record = host.record().await;
    assert!(
        record.forked_participant_obligations.is_empty(),
        "a proven release accrues no obligation"
    );
}

#[tokio::test]
async fn an_ambiguous_build_failure_retains_a_durable_obligation() {
    let capabilities = CapabilityHarness::new();
    let fixture = capabilities
        .mint("req-ambiguous", ForkedParticipantReusePolicy::OneShot)
        .await;
    let association = fixture.association("attach-1");
    capabilities
        .service
        .attach(
            &association.capability,
            &association.attachment_id,
            true,
            now(),
        )
        .await
        .expect("attach");

    let host = HostFixture::bound().await;
    let expected = host.record().await;
    let token = ForkedParticipantObligationAuthority::retain(
        MOB_ID,
        MEMBER_IDENTITY,
        &association,
        ForkedParticipantObligationCause::AmbiguousBuild,
        "member build failed in an ambiguous class",
    );
    let next = token.apply(&expected);
    assert!(
        host.persistence
            .compare_and_put_forked_participant_obligations(MOB_ID, &expected, &next, &token)
            .await
            .expect("obligation write"),
        "the obligation must persist"
    );

    let retained = host.record().await;
    assert_eq!(
        retained.forked_participant_obligations.len(),
        1,
        "the ambiguous failure retains exactly one obligation"
    );
    let obligation: &ForkedParticipantAttachmentObligation = retained
        .forked_participant_obligations
        .get(&association.association_key())
        .expect("obligation keyed by association");
    assert_eq!(obligation.agent_identity, MEMBER_IDENTITY);
    assert_eq!(
        obligation.cause,
        ForkedParticipantObligationCause::AmbiguousBuild
    );
    assert_eq!(obligation.association, association);

    // The attachment was deliberately NOT released: the capability is still
    // busy, which is exactly what makes blind release wrong.
    let still_attached = capabilities
        .service
        .attach(
            &association.capability,
            &ForkedParticipantAttachmentId::new("attach-2").expect("attachment id"),
            true,
            now(),
        )
        .await
        .expect_err("the ambiguous attachment is still held");
    assert!(
        matches!(still_attached, ForkedParticipantError::AttachDenied { .. }),
        "expected a typed denial, got {still_attached:?}"
    );

    // A later materialized row adopting the same association discharges the
    // obligation in the SAME durable transaction.
    let mut host = host;
    let key = host.member_key();
    record_materialized_member(
        &mut host.authority,
        &host.persistence,
        &key,
        materialized_row(&fixture.capability.fork_session_id().to_string(), 1, 1),
        Some(association.clone()),
    )
    .await
    .expect("row records");
    let after = host.record().await;
    assert!(
        after.forked_participant_obligations.is_empty(),
        "adopting the association discharges its obligation atomically"
    );

    // The obligation region survives serialization and re-validates on boot.
    let reopened = RuntimeStoreHostBindingPersistence::new(
        Arc::clone(&host.store) as Arc<dyn meerkat_runtime::store::RuntimeStore>
    );
    let records = reopened.list_records().await.expect("list");
    validate_recovered_forked_participant_routes(&records, Some(&capabilities.service))
        .expect("the recovered state is routable");
}

#[tokio::test]
async fn an_obligation_write_that_rekeys_the_association_is_refused() {
    let capabilities = CapabilityHarness::new();
    let fixture = capabilities
        .mint("req-rekey", ForkedParticipantReusePolicy::OneShot)
        .await;
    let association = fixture.association("attach-1");

    let host = HostFixture::bound().await;
    let expected = host.record().await;
    let token = ForkedParticipantObligationAuthority::retain(
        MOB_ID,
        MEMBER_IDENTITY,
        &association,
        ForkedParticipantObligationCause::ReleaseUnproven,
        "release could not be proven",
    );
    let mut forged = token.apply(&expected);
    // Rekey the entry: the token no longer describes the write.
    let entry = forged
        .forked_participant_obligations
        .remove(&association.association_key())
        .expect("entry");
    forged
        .forked_participant_obligations
        .insert("forged-key".to_string(), entry);
    let error = host
        .persistence
        .compare_and_put_forked_participant_obligations(MOB_ID, &expected, &forged, &token)
        .await
        .expect_err("a rekeyed obligation write must be refused");
    assert!(
        error.to_string().contains("lacks the retained entry"),
        "expected a typed witness refusal, got {error}"
    );

    // Discharge is likewise exact.
    let discharge = ForkedParticipantObligationAuthority::discharge(MOB_ID, &association);
    let mut sibling_mutation = discharge.apply(&expected);
    sibling_mutation.epoch += 1;
    let error = host
        .persistence
        .compare_and_put_forked_participant_obligations(
            MOB_ID,
            &expected,
            &sibling_mutation,
            &discharge,
        )
        .await
        .expect_err("an obligation write may not alter a sibling region");
    assert!(
        error.to_string().contains("altered a sibling region"),
        "expected a typed sibling-region refusal, got {error}"
    );
}

#[tokio::test]
async fn a_mismatched_release_is_cleanup_debt_not_a_silent_success() {
    let capabilities = CapabilityHarness::new();
    let fixture = capabilities
        .mint("req-mismatch", ForkedParticipantReusePolicy::OneShot)
        .await;
    let held = fixture.association("attach-held");
    capabilities
        .service
        .attach(&held.capability, &held.attachment_id, true, now())
        .await
        .expect("attach");

    // A stale association naming a DIFFERENT attachment must never be folded
    // into success: a different attachment is genuinely still active.
    let stale = fixture.association("attach-stale");
    let failure = release_forked_participant_association(Some(&capabilities.service), &stale)
        .await
        .expect_err("a mismatched release must be typed");
    assert_eq!(
        failure.cause,
        BridgeRejectionCause::ForkedParticipantCleanupDebt,
        "a surviving release rejection is unreconciled teardown work"
    );

    // The truly held association still releases, and the capability is then
    // terminal — one attachment, one release, no double release.
    release_forked_participant_association(Some(&capabilities.service), &held)
        .await
        .expect("the held association releases");
}

#[tokio::test]
async fn an_association_without_a_composed_service_is_never_silently_dropped() {
    let capabilities = CapabilityHarness::new();
    let fixture = capabilities
        .mint("req-no-service", ForkedParticipantReusePolicy::OneShot)
        .await;
    let association = fixture.association("attach-1");
    let failure = release_forked_participant_association(None, &association)
        .await
        .expect_err("a host with no service must not claim a release");
    assert_eq!(
        failure.cause,
        BridgeRejectionCause::ForkedParticipantProtocolUnsupported,
        "the absence of a source-owner service is a typed refusal, not a no-op"
    );
}

// ===========================================================================
// Host-autonomous convergence after coordinator loss.
//
// These drive the REAL enumeration (`list_pending_attached`), the REAL
// correlation the sweep performs, the REAL generated release admission, and
// the REAL durable half of teardown (`commit_member_release_after_disposal`).
// The only thing supplied rather than performed is the disposal VERDICT: the
// live-runtime disposal effect belongs to `HostMemberMaterializer` and is
// covered by the bridge-arm suites, which now run this exact same function.
// ===========================================================================

/// Attach one capability and drive it to a terminal parked behind that
/// attachment, exactly as a coordinator that then disappears would leave it.
async fn park_attached_terminal(
    capabilities: &CapabilityHarness,
    fixture: &CapabilityFixture,
    association: &ForkedParticipantAttachmentAssociation,
    terminal: ForkedParticipantPendingTerminal,
) {
    capabilities
        .service
        .attach(
            &association.capability,
            &association.attachment_id,
            true,
            now(),
        )
        .await
        .expect("attach");
    match terminal {
        ForkedParticipantPendingTerminal::Expiry => {
            // Observe expiry from beyond the TTL: the machine records the
            // terminal and parks it because an attachment is held.
            let report = capabilities
                .service
                .sweep_expiry(fixture.capability.expires_at() + ChronoDuration::seconds(1))
                .await
                .expect("expiry sweep");
            assert_eq!(
                report.expiry_pending_attached.len(),
                1,
                "an attached capability parks its expiry"
            );
        }
        ForkedParticipantPendingTerminal::Revocation => {
            let outcome = capabilities
                .service
                .revoke(fixture.capability.capability_id(), true)
                .await
                .expect("revoke");
            assert_eq!(
                outcome,
                meerkat_mob::forked_participant::ForkedParticipantRevocationOutcome::PendingAttachedRelease,
                "revoking an attached capability parks behind the release"
            );
        }
    }
}

/// Record one materialized residency carrying `association`.
async fn materialize_with_association(
    host: &mut HostFixture,
    identity: &str,
    session_id: &str,
    generation: u64,
    fence_token: u64,
    association: &ForkedParticipantAttachmentAssociation,
) {
    let key = MemberKey::new(
        AuthorityMobId::from(MOB_ID),
        AuthorityAgentIdentity::from(identity),
    );
    record_materialized_member(
        &mut host.authority,
        &host.persistence,
        &key,
        materialized_row_for(identity, session_id, generation, fence_token),
        Some(association.clone()),
    )
    .await
    .expect("materialized row records");
}

/// The convergence step the host sweep performs, with the disposal verdict
/// supplied instead of performed.
async fn converge_once(
    host: &mut HostFixture,
    capabilities: &CapabilityHarness,
    disposal: Option<MachineMemberSessionDisposal>,
) -> (usize, usize, usize) {
    let report = capabilities
        .service
        .list_pending_attached()
        .await
        .expect("pending-attached enumeration");
    let records = host.persistence.list_records().await.expect("list records");
    let correlations = correlate_pending_forked_participant_attachments(&report.pending, &records);
    let unheld = report.pending.len() - correlations.len();
    let mut converged = 0;
    let mut retained = 0;
    for correlation in correlations {
        let key = MemberKey::new(
            AuthorityMobId::from(correlation.mob_id.as_str()),
            AuthorityAgentIdentity::from(correlation.agent_identity.as_str()),
        );
        match resolve_release_admission(
            &mut host.authority,
            &key,
            correlation.generation,
            correlation.fence_token,
        )
        .expect("release admission resolves")
        {
            ReleaseAdmission::Admitted => {}
            // Already released; nothing left to converge.
            ReleaseAdmission::Replay { .. } => continue,
            ReleaseAdmission::Rejected { .. } => {
                retained += 1;
                continue;
            }
        }
        // A host whose disposal has not been proven never reaches the durable
        // half: the row, its association, and the attachment all survive.
        let Some(disposal) = disposal else {
            retained += 1;
            continue;
        };
        match commit_member_release_after_disposal(
            &mut host.authority,
            &host.persistence,
            Some(&capabilities.service),
            &key,
            correlation.generation,
            correlation.fence_token,
            disposal,
        )
        .await
        {
            Ok(_) => converged += 1,
            Err(_) => retained += 1,
        }
    }
    (converged, retained, unheld)
}

#[tokio::test]
async fn expiry_parked_behind_a_lost_coordinator_converges_on_a_host_tick() {
    let capabilities = CapabilityHarness::new();
    let fixture = capabilities
        .mint("req-expiry-converge", ForkedParticipantReusePolicy::OneShot)
        .await;
    let association = fixture.association("attach-1");
    let fork_session_id = fixture.capability.fork_session_id().to_string();

    let mut host = HostFixture::bound().await;
    materialize_with_association(
        &mut host,
        MEMBER_IDENTITY,
        &fork_session_id,
        1,
        1,
        &association,
    )
    .await;
    park_attached_terminal(
        &capabilities,
        &fixture,
        &association,
        ForkedParticipantPendingTerminal::Expiry,
    )
    .await;

    // The coordinator sends nothing. The host's own tick converges.
    let (converged, retained, unheld) = converge_once(
        &mut host,
        &capabilities,
        Some(MachineMemberSessionDisposal::Archived),
    )
    .await;
    assert_eq!(
        (converged, retained, unheld),
        (1, 0, 0),
        "the host must converge the residency it holds the attachment for"
    );

    // Member released durably; the association went with the row.
    let record = host.record().await;
    assert!(
        !record.materialized.contains_key(MEMBER_IDENTITY),
        "the residency is released"
    );
    assert!(
        record.released.contains_key(MEMBER_IDENTITY),
        "the release receipt is recorded"
    );

    // Capability terminalized and is no longer parked.
    let pending = capabilities
        .service
        .list_pending_attached()
        .await
        .expect("enumeration");
    assert!(
        pending.pending.is_empty(),
        "the terminal is no longer parked behind an attachment"
    );

    // The existing cleanup sweep — not the convergence — archives the fork.
    let cleanup = capabilities
        .service
        .sweep_cleanup(now())
        .await
        .expect("cleanup sweep");
    assert_eq!(
        cleanup.completed.len(),
        1,
        "the terminalized capability's fork is archived by the ordinary cleanup sweep"
    );
    assert!(
        capabilities
            .runtime
            .archived()
            .contains(&fixture.capability.fork_session_id().clone()),
        "the fork session is the one archived"
    );
}

#[tokio::test]
async fn revocation_parked_behind_a_lost_coordinator_converges_on_a_host_tick() {
    let capabilities = CapabilityHarness::new();
    let fixture = capabilities
        .mint("req-revoke-converge", ForkedParticipantReusePolicy::OneShot)
        .await;
    let association = fixture.association("attach-1");
    let fork_session_id = fixture.capability.fork_session_id().to_string();

    let mut host = HostFixture::bound().await;
    materialize_with_association(
        &mut host,
        MEMBER_IDENTITY,
        &fork_session_id,
        1,
        1,
        &association,
    )
    .await;
    park_attached_terminal(
        &capabilities,
        &fixture,
        &association,
        ForkedParticipantPendingTerminal::Revocation,
    )
    .await;

    // Revocation-pending must receive the same autonomous convergence as
    // expiry-pending, and the enumeration must say so typed.
    let report = capabilities
        .service
        .list_pending_attached()
        .await
        .expect("enumeration");
    assert_eq!(report.pending.len(), 1);
    assert_eq!(
        report.pending[0].terminal,
        ForkedParticipantPendingTerminal::Revocation,
        "the parked terminal is read from the machine phase, not inferred"
    );

    let (converged, retained, unheld) = converge_once(
        &mut host,
        &capabilities,
        Some(MachineMemberSessionDisposal::Archived),
    )
    .await;
    assert_eq!((converged, retained, unheld), (1, 0, 0));
    let record = host.record().await;
    assert!(record.released.contains_key(MEMBER_IDENTITY));
    let cleanup = capabilities
        .service
        .sweep_cleanup(now())
        .await
        .expect("cleanup sweep");
    assert_eq!(cleanup.completed.len(), 1, "the revoked fork is archived");
}

#[tokio::test]
async fn a_blocked_disposal_retains_the_row_and_a_later_tick_converges() {
    let capabilities = CapabilityHarness::new();
    let fixture = capabilities
        .mint("req-blocked", ForkedParticipantReusePolicy::OneShot)
        .await;
    let association = fixture.association("attach-1");
    let fork_session_id = fixture.capability.fork_session_id().to_string();

    let mut host = HostFixture::bound().await;
    materialize_with_association(
        &mut host,
        MEMBER_IDENTITY,
        &fork_session_id,
        1,
        1,
        &association,
    )
    .await;
    park_attached_terminal(
        &capabilities,
        &fixture,
        &association,
        ForkedParticipantPendingTerminal::Expiry,
    )
    .await;

    // Tick 1: disposal cannot be proven. Nothing is recorded and — critically
    // — the capability attachment is NOT released while the runtime may live.
    let (converged, retained, _) = converge_once(&mut host, &capabilities, None).await;
    assert_eq!((converged, retained), (0, 1), "the tick retains the work");
    let record = host.record().await;
    assert_eq!(
        record
            .materialized
            .get(MEMBER_IDENTITY)
            .and_then(|row| row.forked_participant_attachment.clone()),
        Some(association.clone()),
        "a blocked disposal keeps the row and its association"
    );
    let still_parked = capabilities
        .service
        .list_pending_attached()
        .await
        .expect("enumeration");
    assert_eq!(
        still_parked.pending.len(),
        1,
        "the attachment is still held, so the terminal is still parked"
    );

    // Tick 2: disposal succeeds and the same tuple converges.
    let (converged, retained, _) = converge_once(
        &mut host,
        &capabilities,
        Some(MachineMemberSessionDisposal::Archived),
    )
    .await;
    assert_eq!((converged, retained), (1, 0), "the retry converges");
    assert!(host.record().await.released.contains_key(MEMBER_IDENTITY));
}

#[tokio::test]
async fn a_capability_release_failure_retains_only_its_own_association() {
    let capabilities = CapabilityHarness::new();
    let stuck = capabilities
        .mint_with_ttl(
            "req-multi-stuck",
            ForkedParticipantReusePolicy::OneShot,
            Duration::from_secs(60),
        )
        .await;
    // A much longer TTL keeps this capability clear of the expiry observation
    // the stuck one's parking performs: expiry is swept store-wide.
    let healthy = capabilities
        .mint_with_ttl(
            "req-multi-healthy",
            ForkedParticipantReusePolicy::OneShot,
            Duration::from_secs(12 * 60 * 60),
        )
        .await;
    let stuck_association = stuck.association("attach-stuck");
    let healthy_association = healthy.association("attach-healthy");

    let mut host = HostFixture::bound().await;
    materialize_with_association(
        &mut host,
        "branch-stuck",
        &stuck.capability.fork_session_id().to_string(),
        1,
        1,
        &stuck_association,
    )
    .await;
    materialize_with_association(
        &mut host,
        "branch-healthy",
        &healthy.capability.fork_session_id().to_string(),
        1,
        1,
        &healthy_association,
    )
    .await;
    park_attached_terminal(
        &capabilities,
        &stuck,
        &stuck_association,
        ForkedParticipantPendingTerminal::Expiry,
    )
    .await;
    park_attached_terminal(
        &capabilities,
        &healthy,
        &healthy_association,
        ForkedParticipantPendingTerminal::Revocation,
    )
    .await;

    // The enumeration is ordered by durable record order; fail the FIRST
    // capability release the pass attempts so the stuck entry is not the last.
    capabilities.store.fail_next_exact_loads(1);
    let (converged, retained, unheld) = converge_once(
        &mut host,
        &capabilities,
        Some(MachineMemberSessionDisposal::Archived),
    )
    .await;
    assert_eq!(unheld, 0, "both associations are held by this host");
    assert_eq!(
        (converged, retained),
        (1, 1),
        "one stuck association must not stop the pass from converging the other"
    );

    let record = host.record().await;
    assert_eq!(
        record.materialized.len(),
        1,
        "exactly the stuck residency is retained"
    );
    assert_eq!(
        record.released.len(),
        1,
        "exactly the healthy residency is released"
    );

    // The stuck one converges on the next tick.
    let (converged, retained, _) = converge_once(
        &mut host,
        &capabilities,
        Some(MachineMemberSessionDisposal::Archived),
    )
    .await;
    assert_eq!((converged, retained), (1, 0), "the stuck entry converges");
    assert!(host.record().await.materialized.is_empty());
}

#[tokio::test]
async fn convergence_never_touches_an_attachment_this_host_does_not_hold() {
    let capabilities = CapabilityHarness::new();
    let fixture = capabilities
        .mint("req-unheld", ForkedParticipantReusePolicy::OneShot)
        .await;
    let association = fixture.association("attach-1");

    // Parked terminal, but NO durable residency on this host carries the
    // association: another holder owns the attachment.
    let mut host = HostFixture::bound().await;
    park_attached_terminal(
        &capabilities,
        &fixture,
        &association,
        ForkedParticipantPendingTerminal::Expiry,
    )
    .await;

    let (converged, retained, unheld) = converge_once(
        &mut host,
        &capabilities,
        Some(MachineMemberSessionDisposal::Archived),
    )
    .await;
    assert_eq!(
        (converged, retained, unheld),
        (0, 0, 1),
        "an unheld parked terminal is counted, never converged"
    );
    let still_parked = capabilities
        .service
        .list_pending_attached()
        .await
        .expect("enumeration");
    assert_eq!(
        still_parked.pending.len(),
        1,
        "the other holder's attachment is untouched"
    );
}

#[tokio::test]
async fn convergence_works_after_restart_from_serialized_associations() {
    let capabilities = CapabilityHarness::new();
    let fixture = capabilities
        .mint(
            "req-restart-converge",
            ForkedParticipantReusePolicy::OneShot,
        )
        .await;
    let association = fixture.association("attach-1");
    let fork_session_id = fixture.capability.fork_session_id().to_string();

    let mut host = HostFixture::bound().await;
    materialize_with_association(
        &mut host,
        MEMBER_IDENTITY,
        &fork_session_id,
        1,
        1,
        &association,
    )
    .await;
    park_attached_terminal(
        &capabilities,
        &fixture,
        &association,
        ForkedParticipantPendingTerminal::Expiry,
    )
    .await;

    // Restart: authority and persistence are rebuilt from the durable rows
    // alone, and the capability service from the durable capability store.
    let mut restarted = host.restart().await;
    let reopened_service = capabilities.reopen();
    let restarted_capabilities = CapabilityHarness {
        service: reopened_service,
        store: Arc::clone(&capabilities.store),
        runtime: capabilities.runtime.clone(),
    };

    let (converged, retained, unheld) = converge_once(
        &mut restarted,
        &restarted_capabilities,
        Some(MachineMemberSessionDisposal::Archived),
    )
    .await;
    assert_eq!(
        (converged, retained, unheld),
        (1, 0, 0),
        "a restarted host converges from serialized associations alone"
    );
    assert!(
        restarted
            .record()
            .await
            .released
            .contains_key(MEMBER_IDENTITY)
    );
}

#[tokio::test]
async fn correlation_matches_only_the_exact_full_association() {
    let capabilities = CapabilityHarness::new();
    let one = capabilities
        .mint("req-corr-a", ForkedParticipantReusePolicy::OneShot)
        .await;
    let two = capabilities
        .mint("req-corr-b", ForkedParticipantReusePolicy::OneShot)
        .await;
    let held = one.association("attach-held");

    let mut host = HostFixture::bound().await;
    materialize_with_association(
        &mut host,
        MEMBER_IDENTITY,
        &one.capability.fork_session_id().to_string(),
        1,
        1,
        &held,
    )
    .await;
    let records = host.persistence.list_records().await.expect("list");

    let matching = ForkedParticipantPendingAttachment {
        capability: one.capability.clone(),
        attachment_id: held.attachment_id.clone(),
        terminal: ForkedParticipantPendingTerminal::Expiry,
    };
    assert_eq!(
        correlate_pending_forked_participant_attachments(std::slice::from_ref(&matching), &records)
            .len(),
        1,
        "the exact association correlates"
    );

    // Same capability, different attachment id: NOT this residency's.
    let other_attachment = ForkedParticipantPendingAttachment {
        capability: one.capability.clone(),
        attachment_id: ForkedParticipantAttachmentId::new("attach-other").expect("id"),
        terminal: ForkedParticipantPendingTerminal::Expiry,
    };
    assert!(
        correlate_pending_forked_participant_attachments(&[other_attachment], &records).is_empty(),
        "a different attachment id must not correlate"
    );

    // Different capability that happens to reuse the attachment id.
    let other_capability = ForkedParticipantPendingAttachment {
        capability: two.capability.clone(),
        attachment_id: held.attachment_id.clone(),
        terminal: ForkedParticipantPendingTerminal::Expiry,
    };
    assert!(
        correlate_pending_forked_participant_attachments(&[other_capability], &records).is_empty(),
        "a different capability must not correlate on a shared attachment id"
    );

    // A row with no association never correlates to anything.
    let mut unassociated_records = records;
    for (_, record) in &mut unassociated_records {
        for row in record.materialized.values_mut() {
            row.forked_participant_attachment = None;
        }
    }
    assert!(
        correlate_pending_forked_participant_attachments(&[matching], &unassociated_records)
            .is_empty(),
        "an unassociated row is never correlated by session id or member name"
    );
}

#[tokio::test]
async fn an_unreadable_parked_record_is_reported_and_never_blocks_the_rest() {
    let capabilities = CapabilityHarness::new();
    let healthy = capabilities
        .mint("req-readable", ForkedParticipantReusePolicy::OneShot)
        .await;
    let association = healthy.association("attach-1");
    park_attached_terminal(
        &capabilities,
        &healthy,
        &association,
        ForkedParticipantPendingTerminal::Expiry,
    )
    .await;

    // Corrupt one record's active attachment id in place, exactly as a
    // damaged durable row would deserialize.
    let broken = capabilities
        .mint("req-unreadable", ForkedParticipantReusePolicy::OneShot)
        .await;
    let broken_association = broken.association("attach-2");
    park_attached_terminal(
        &capabilities,
        &broken,
        &broken_association,
        ForkedParticipantPendingTerminal::Revocation,
    )
    .await;
    let mut record = capabilities
        .store
        .load_by_capability_id(broken.capability.capability_id())
        .await
        .expect("load")
        .expect("record");
    record.machine_state.active_attachment_id = Some("   ".to_string());
    capabilities.store.commit(&record).await.expect("commit");

    let report = capabilities
        .service
        .list_pending_attached()
        .await
        .expect("enumeration");
    assert_eq!(
        report.pending.len(),
        1,
        "the readable parked terminal is still enumerated"
    );
    assert_eq!(
        report.unreadable.len(),
        1,
        "the unreadable record is reported, never silently dropped"
    );
}

// ===========================================================================
// Shutdown-path final sweep budget.
// ===========================================================================

#[tokio::test(start_paused = true)]
async fn a_blocked_final_sweep_does_not_hold_shutdown_open() {
    // The shutdown sweep is bounded; a sweep that cannot make progress must
    // report incompletion rather than pin the actor's exit.
    assert!(
        !run_final_forked_participant_sweep(std::future::pending::<()>()).await,
        "a blocked final sweep must time out"
    );
    // A sweep that completes inside the budget still reports completion.
    assert!(
        run_final_forked_participant_sweep(async {
            tokio::time::sleep(Duration::from_millis(50)).await;
        })
        .await,
        "a prompt final sweep completes"
    );
}

// ===========================================================================
// Containment: a fork child session id must never substitute for its bearer.
//
// These drive the REAL owner lookup (`protected_fork_session`, backed by the
// durable `load_by_fork_session_id`) and the REAL host adjudication the
// materialize arm runs before dedup replay, preflight, or any build.
// ===========================================================================

fn wire_attachment(
    capability: &ForkedParticipantRef,
    attachment_id: &str,
) -> BridgeForkedParticipantAttachment {
    BridgeForkedParticipantAttachment {
        attachment_id: attachment_id.to_string(),
        capability: meerkat_mob::forked_participant::bridge_ref(capability),
    }
}

#[tokio::test]
async fn a_protected_resume_without_an_attachment_is_refused_and_consumes_no_use() {
    let capabilities = CapabilityHarness::new();
    let fixture = capabilities
        .mint("req-protected", ForkedParticipantReusePolicy::OneShot)
        .await;
    let fork_session_id = fixture.capability.fork_session_id().clone();

    // Owner truth says the session is protected.
    let protection = capabilities
        .service
        .protected_fork_session(&fork_session_id)
        .await
        .expect("containment lookup")
        .expect("the fork child session is capability-protected");
    assert_eq!(
        protection.capability.as_ref(),
        Some(&fixture.capability),
        "protection carries the owner's exact recorded reference"
    );

    // An ordinary Resume that merely knows the visible id is refused.
    let (cause, reason) = admit_resume_against_fork_protection(
        &fork_session_id.to_string(),
        Some(&protection),
        None,
        capabilities.service.owner_route(),
    )
    .expect_err("a bare resume of a protected session must fail closed");
    assert_eq!(cause, BridgeRejectionCause::ForkedParticipantTampered);
    assert!(
        reason.contains("authority is required"),
        "the refusal must name the missing authentication, got {reason}"
    );

    // The refusal happened before any attach, so the one-shot budget is
    // untouched: the legitimate holder can still attach.
    let association = fixture.association("attach-1");
    capabilities
        .service
        .attach(
            &association.capability,
            &association.attachment_id,
            true,
            now(),
        )
        .await
        .expect("the refused resume consumed no use of the capability");
}

#[tokio::test]
async fn a_protected_resume_with_the_wrong_reference_is_refused() {
    let capabilities = CapabilityHarness::new();
    let target = capabilities
        .mint(
            "req-protected-target",
            ForkedParticipantReusePolicy::OneShot,
        )
        .await;
    let other = capabilities
        .mint("req-protected-other", ForkedParticipantReusePolicy::OneShot)
        .await;
    let fork_session_id = target.capability.fork_session_id().clone();
    let protection = capabilities
        .service
        .protected_fork_session(&fork_session_id)
        .await
        .expect("lookup")
        .expect("protected");

    // A different capability's reference does not authenticate this session.
    let (cause, reason) = admit_resume_against_fork_protection(
        &fork_session_id.to_string(),
        Some(&protection),
        Some(&wire_attachment(&other.capability, "attach-1")),
        capabilities.service.owner_route(),
    )
    .expect_err("a foreign reference must not unlock a protected session");
    assert_eq!(cause, BridgeRejectionCause::ForkedParticipantTampered);
    assert!(
        reason.contains("not the one its owner recorded"),
        "got {reason}"
    );

    // A tampered projection of the RIGHT capability is likewise refused: the
    // comparison is field-for-field against owner truth.
    let mut widened = wire_attachment(&target.capability, "attach-1");
    widened.capability.scope =
        meerkat_contracts::wire::supervisor_bridge::BridgeForkedParticipantScope::InvokeAndObserve;
    widened.capability.prefix_message_count += 1;
    let (cause, _) = admit_resume_against_fork_protection(
        &fork_session_id.to_string(),
        Some(&protection),
        Some(&widened),
        capabilities.service.owner_route(),
    )
    .expect_err("a rewritten reference must be refused");
    assert_eq!(cause, BridgeRejectionCause::ForkedParticipantTampered);

    // A malformed attachment id on an otherwise exact reference is refused.
    let (cause, _) = admit_resume_against_fork_protection(
        &fork_session_id.to_string(),
        Some(&protection),
        Some(&wire_attachment(&target.capability, "   ")),
        capabilities.service.owner_route(),
    )
    .expect_err("a malformed attachment id must be refused");
    assert_eq!(cause, BridgeRejectionCause::ForkedParticipantTampered);

    // The exact recorded reference is admitted.
    admit_resume_against_fork_protection(
        &fork_session_id.to_string(),
        Some(&protection),
        Some(&wire_attachment(&target.capability, "attach-1")),
        capabilities.service.owner_route(),
    )
    .expect("the owner's exact reference authenticates the resume");
}

#[tokio::test]
async fn an_ordinary_resume_of_an_unprotected_session_is_unchanged() {
    let capabilities = CapabilityHarness::new();
    let fixture = capabilities
        .mint("req-unprotected", ForkedParticipantReusePolicy::OneShot)
        .await;

    // A session no capability record claims is not protected.
    let unrelated = SessionId::new();
    assert!(
        capabilities
            .service
            .protected_fork_session(&unrelated)
            .await
            .expect("lookup")
            .is_none(),
        "an unrelated session must not resolve to a capability record"
    );
    admit_resume_against_fork_protection(
        &unrelated.to_string(),
        None,
        None,
        capabilities.service.owner_route(),
    )
    .expect("an unprotected resume is completely unchanged");

    // The capability's own source session is likewise unprotected: only the
    // FORK CHILD is capability custody.
    let source = fixture.capability.provenance().source_session_id.clone();
    assert!(
        capabilities
            .service
            .protected_fork_session(&source)
            .await
            .expect("lookup")
            .is_none(),
        "the source session is not the fork child and is not protected"
    );
}

#[tokio::test]
async fn a_reserved_but_unactivated_fork_session_admits_nothing() {
    // The crash window between "planned child is durable" and "activation is
    // recorded" is protected too: the record owns the session but has no
    // reference, so nothing can authenticate against it.
    let capabilities = CapabilityHarness::new();
    let source_session_id = SessionId::new();
    capabilities.runtime.register_source(&source_session_id);
    let request = ForkedParticipantRequest {
        request_id: ForkedParticipantRequestId::new("req-reserved-only").expect("request id"),
        source_identity: AgentIdentity::from(SOURCE_MEMBER),
        source_session_id,
        owner_route: host_route(),
        prefix_message_count: Some(PREFIX_COUNT),
        scope: ForkedParticipantOperationScope::InvokeAndObserve,
        reuse: ForkedParticipantReusePolicy::OneShot,
        ttl: Duration::from_secs(600),
    };
    let reservation = capabilities
        .service
        .reserve(&request, now())
        .await
        .expect("reserve");

    let protection = capabilities
        .service
        .protected_fork_session(&reservation.planned_child_session_id)
        .await
        .expect("lookup")
        .expect("a reserved planned child is already protected");
    assert!(
        protection.capability.is_none(),
        "a reserved record carries no activated reference yet"
    );

    let (cause, reason) = admit_resume_against_fork_protection(
        &reservation.planned_child_session_id.to_string(),
        Some(&protection),
        None,
        capabilities.service.owner_route(),
    )
    .expect_err("a bare resume of a reserved fork child is refused");
    assert_eq!(cause, BridgeRejectionCause::ForkedParticipantTampered);
    assert!(reason.contains("capability-protected"), "got {reason}");
}

#[tokio::test]
async fn a_protected_capability_owned_by_another_route_is_refused() {
    let capabilities = CapabilityHarness::new();
    let fixture = capabilities
        .mint("req-route-gate", ForkedParticipantReusePolicy::OneShot)
        .await;
    let fork_session_id = fixture.capability.fork_session_id().clone();
    let protection = capabilities
        .service
        .protected_fork_session(&fork_session_id)
        .await
        .expect("lookup")
        .expect("protected");

    let foreign_route = ForkedParticipantOwnerRoute::Host {
        realm_id: realm(),
        host_id: meerkat_mob::machines::mob_machine::HostId::from("host-b".to_string()),
    };
    let (cause, _) = admit_resume_against_fork_protection(
        &fork_session_id.to_string(),
        Some(&protection),
        Some(&wire_attachment(&fixture.capability, "attach-1")),
        &foreign_route,
    )
    .expect_err("a host that does not own the route must not serve the resume");
    assert_eq!(cause, BridgeRejectionCause::ForkedParticipantRouteMismatch);
}

#[tokio::test]
async fn the_containment_gate_survives_a_capability_store_restart() {
    let capabilities = CapabilityHarness::new();
    let fixture = capabilities
        .mint("req-gate-restart", ForkedParticipantReusePolicy::OneShot)
        .await;
    let fork_session_id = fixture.capability.fork_session_id().clone();

    // Rebuild the owner service over the SAME durable store, as a restarted
    // process would: protection is durable, not process-local.
    let reopened = capabilities.reopen();
    let protection = reopened
        .protected_fork_session(&fork_session_id)
        .await
        .expect("lookup")
        .expect("protection survives restart");
    assert_eq!(protection.capability.as_ref(), Some(&fixture.capability));

    let (cause, _) = admit_resume_against_fork_protection(
        &fork_session_id.to_string(),
        Some(&protection),
        None,
        reopened.owner_route(),
    )
    .expect_err("the gate still refuses a bare resume after restart");
    assert_eq!(cause, BridgeRejectionCause::ForkedParticipantTampered);

    admit_resume_against_fork_protection(
        &fork_session_id.to_string(),
        Some(&protection),
        Some(&wire_attachment(&fixture.capability, "attach-1")),
        reopened.owner_route(),
    )
    .expect("and still admits the exact recorded reference");
}
