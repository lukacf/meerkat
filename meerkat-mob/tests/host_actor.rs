//! Mob host actor serving-core tests (multi-host mobs §7.2 step 3, §14 R8,
//! DEC-P2-4/6/10).
//!
//! These drive the comms-free serving seam (`serve_host_bind` /
//! `serve_host_rebind` / `revoke_host_binding`) against the REAL generated
//! authority and a REAL sqlite runtime store, pinning:
//!   * persist-before-commit: durable rows never lag (or lead) the
//!     committed authority state;
//!   * bootstrap-token single-use + re-mint (DEC-P2-4);
//!   * idempotent bind replay and rebind replay acks;
//!   * strictly-monotonic rebind with durable CAS;
//!   * revoke deleting the row under the deletion witness;
//!   * boot recovery folding rows through the generated
//!     `recover_from_state` seam, and corrupt rows aborting typed.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use meerkat_contracts::wire::supervisor_bridge::{
    BridgeCapabilities, BridgePeerIdentity, BridgePeerSpec, BridgeRejectionCause,
    BridgeTurnOutcomeAck, MaterializeLaunchOutcome, WireFlowTurnOutcome,
};
use meerkat_contracts::wire::{PortableMemberSpec, portable_member_spec_digest};
use meerkat_mob::machines::mob_host_binding_authority::{
    AgentIdentity as AuthorityAgentIdentity, MemberKey,
    MemberSessionDisposal as MachineMemberSessionDisposal, MobHostBindingAuthorityAuthority,
    MobHostBindingAuthorityInput, MobHostBindingAuthorityMutator, MobId as AuthorityMobId,
    PeerId as AuthorityPeerId, PeerSigningKey as AuthorityPeerSigningKey,
};
use meerkat_mob::runtime::host_actor::{
    HostBindObservations, HostBindServeOutcome, HostBindingPersistenceAuthority,
    HostBootstrapTokenSlot, HostRebindObservations, HostRebindServeOutcome, MaterializedMemberRow,
    MemberRowPersistenceAuthority, MobHostActorError, MobHostBindingPersistence,
    MobHostBindingRecord, RuntimeStoreHostBindingPersistence, TrackedInputCancelDisposition,
    TrackedInputCancelPersistenceAuthority, TurnOutcomeAckDisposition,
    TurnOutcomeAckPersistenceAuthority, TurnOutcomePendingCancelDisposition,
    TurnOutcomePendingPersistenceAuthority, TurnOutcomePendingReservationDisposition,
    TurnOutcomePendingRow, TurnOutcomePersistenceAuthority, TurnOutcomeRecordDisposition,
    TurnOutcomeRow, acknowledge_turn_outcome_journal, cancel_tracked_input_journal,
    cancel_turn_outcome_pending, complete_tracked_input_cancel_journal, record_materialized_member,
    record_member_release, record_turn_outcome_journal, recover_or_create_binding_authority,
    reserve_turn_outcome_pending, reserve_turn_outcome_pending_replay_only, revoke_host_binding,
    serve_host_bind, serve_host_rebind,
};
use meerkat_runtime::store::RuntimeStore;

struct Fixture {
    _dir: tempfile::TempDir,
    store: Arc<meerkat_runtime::store::SqliteRuntimeStore>,
    persistence: RuntimeStoreHostBindingPersistence,
    authority: MobHostBindingAuthorityAuthority,
    token: HostBootstrapTokenSlot,
}

/// Fault seam for the write-ACK-loss row: delegate the insert so the exact
/// durable record commits, then return an error once. `serve_host_bind` must
/// reread and converge without requiring a daemon restart.
struct CommittedThenErrorPersistence {
    inner: RuntimeStoreHostBindingPersistence,
    fail_after_next_insert: AtomicBool,
    fail_after_next_compare_and_put: AtomicBool,
    fail_after_next_revoke: AtomicBool,
    member_region_fault: CommittedThenErrorRegion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommittedThenErrorRegion {
    None,
    Materialize,
    Release,
    Pending,
    Outcome,
    Ack,
    TrackedCancel,
}

impl CommittedThenErrorPersistence {
    fn new(inner: RuntimeStoreHostBindingPersistence) -> Self {
        Self {
            inner,
            fail_after_next_insert: AtomicBool::new(true),
            fail_after_next_compare_and_put: AtomicBool::new(false),
            fail_after_next_revoke: AtomicBool::new(false),
            member_region_fault: CommittedThenErrorRegion::None,
        }
    }

    fn for_rebind(inner: RuntimeStoreHostBindingPersistence) -> Self {
        Self {
            inner,
            fail_after_next_insert: AtomicBool::new(false),
            fail_after_next_compare_and_put: AtomicBool::new(true),
            fail_after_next_revoke: AtomicBool::new(false),
            member_region_fault: CommittedThenErrorRegion::None,
        }
    }

    fn for_revoke(inner: RuntimeStoreHostBindingPersistence) -> Self {
        Self {
            inner,
            fail_after_next_insert: AtomicBool::new(false),
            fail_after_next_compare_and_put: AtomicBool::new(false),
            fail_after_next_revoke: AtomicBool::new(true),
            member_region_fault: CommittedThenErrorRegion::None,
        }
    }

    fn for_member_region(
        inner: RuntimeStoreHostBindingPersistence,
        member_region_fault: CommittedThenErrorRegion,
    ) -> Self {
        Self {
            inner,
            fail_after_next_insert: AtomicBool::new(false),
            fail_after_next_compare_and_put: AtomicBool::new(false),
            fail_after_next_revoke: AtomicBool::new(false),
            member_region_fault,
        }
    }
}

#[async_trait]
impl MobHostBindingPersistence for CommittedThenErrorPersistence {
    async fn list_records(&self) -> Result<Vec<(String, MobHostBindingRecord)>, MobHostActorError> {
        self.inner.list_records().await
    }

    async fn load(&self, mob_id: &str) -> Result<Option<MobHostBindingRecord>, MobHostActorError> {
        self.inner.load(mob_id).await
    }

    async fn list_revocations(
        &self,
    ) -> Result<
        Vec<(
            String,
            meerkat_mob::runtime::host_actor::MobHostRevocationReceipt,
        )>,
        MobHostActorError,
    > {
        self.inner.list_revocations().await
    }

    async fn load_revocation(
        &self,
        mob_id: &str,
    ) -> Result<Option<meerkat_mob::runtime::host_actor::MobHostRevocationReceipt>, MobHostActorError>
    {
        self.inner.load_revocation(mob_id).await
    }

    async fn put_if_absent(
        &self,
        mob_id: &str,
        record: &MobHostBindingRecord,
        authority: &HostBindingPersistenceAuthority,
    ) -> Result<bool, MobHostActorError> {
        let inserted = self.inner.put_if_absent(mob_id, record, authority).await?;
        if inserted && self.fail_after_next_insert.swap(false, Ordering::SeqCst) {
            return Err(MobHostActorError::Internal {
                detail: "injected bind write completion loss after durable commit".to_string(),
            });
        }
        Ok(inserted)
    }

    async fn compare_and_put(
        &self,
        mob_id: &str,
        expected: &MobHostBindingRecord,
        next: &MobHostBindingRecord,
        authority: &HostBindingPersistenceAuthority,
    ) -> Result<bool, MobHostActorError> {
        let swapped = self
            .inner
            .compare_and_put(mob_id, expected, next, authority)
            .await?;
        if swapped
            && self
                .fail_after_next_compare_and_put
                .swap(false, Ordering::SeqCst)
        {
            return Err(MobHostActorError::Internal {
                detail: "injected rebind write completion loss after durable commit".to_string(),
            });
        }
        Ok(swapped)
    }

    async fn compare_and_put_member_rows(
        &self,
        mob_id: &str,
        expected: &MobHostBindingRecord,
        next: &MobHostBindingRecord,
        authority: &MemberRowPersistenceAuthority,
    ) -> Result<bool, MobHostActorError> {
        let swapped = self
            .inner
            .compare_and_put_member_rows(mob_id, expected, next, authority)
            .await?;
        let selected = matches!(
            (self.member_region_fault, authority),
            (
                CommittedThenErrorRegion::Materialize,
                MemberRowPersistenceAuthority::Materialized { .. }
            ) | (
                CommittedThenErrorRegion::Release,
                MemberRowPersistenceAuthority::Released { .. }
            )
        );
        if swapped && selected {
            return Err(MobHostActorError::Internal {
                detail: format!(
                    "injected {:?} completion loss after durable commit",
                    self.member_region_fault
                ),
            });
        }
        Ok(swapped)
    }

    async fn compare_and_put_turn_outcome_pending(
        &self,
        mob_id: &str,
        expected: &MobHostBindingRecord,
        next: &MobHostBindingRecord,
        authority: &TurnOutcomePendingPersistenceAuthority,
    ) -> Result<bool, MobHostActorError> {
        let swapped = self
            .inner
            .compare_and_put_turn_outcome_pending(mob_id, expected, next, authority)
            .await?;
        if swapped && self.member_region_fault == CommittedThenErrorRegion::Pending {
            return Err(MobHostActorError::Internal {
                detail: "injected Pending completion loss after durable commit".to_string(),
            });
        }
        Ok(swapped)
    }

    async fn compare_and_put_turn_outcomes(
        &self,
        mob_id: &str,
        expected: &MobHostBindingRecord,
        next: &MobHostBindingRecord,
        authority: &TurnOutcomePersistenceAuthority,
    ) -> Result<bool, MobHostActorError> {
        let swapped = self
            .inner
            .compare_and_put_turn_outcomes(mob_id, expected, next, authority)
            .await?;
        if swapped && self.member_region_fault == CommittedThenErrorRegion::Outcome {
            return Err(MobHostActorError::Internal {
                detail: "injected outcome completion loss after durable commit".to_string(),
            });
        }
        Ok(swapped)
    }

    async fn compare_and_put_turn_outcome_ack(
        &self,
        mob_id: &str,
        expected: &MobHostBindingRecord,
        next: &MobHostBindingRecord,
        authority: &TurnOutcomeAckPersistenceAuthority,
    ) -> Result<bool, MobHostActorError> {
        let swapped = self
            .inner
            .compare_and_put_turn_outcome_ack(mob_id, expected, next, authority)
            .await?;
        if swapped && self.member_region_fault == CommittedThenErrorRegion::Ack {
            return Err(MobHostActorError::Internal {
                detail: "injected acknowledgement completion loss after durable commit".to_string(),
            });
        }
        Ok(swapped)
    }

    async fn compare_and_put_tracked_input_cancel(
        &self,
        mob_id: &str,
        expected: &MobHostBindingRecord,
        next: &MobHostBindingRecord,
        authority: &TrackedInputCancelPersistenceAuthority,
    ) -> Result<bool, MobHostActorError> {
        let swapped = self
            .inner
            .compare_and_put_tracked_input_cancel(mob_id, expected, next, authority)
            .await?;
        if swapped && self.member_region_fault == CommittedThenErrorRegion::TrackedCancel {
            return Err(MobHostActorError::Internal {
                detail: "injected tracked cancellation completion loss after durable commit"
                    .to_string(),
            });
        }
        Ok(swapped)
    }

    async fn revoke(
        &self,
        mob_id: &str,
        expected: &MobHostBindingRecord,
        receipt: &meerkat_mob::runtime::host_actor::MobHostRevocationReceipt,
        authority: &meerkat_mob::runtime::host_actor::HostBindingDeletionAuthority,
    ) -> Result<bool, MobHostActorError> {
        let revoked = self
            .inner
            .revoke(mob_id, expected, receipt, authority)
            .await?;
        if revoked && self.fail_after_next_revoke.swap(false, Ordering::SeqCst) {
            return Err(MobHostActorError::Internal {
                detail: "injected revoke write completion loss after durable commit".to_string(),
            });
        }
        Ok(revoked)
    }
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(
        meerkat_runtime::store::SqliteRuntimeStore::new(dir.path().join("sessions.sqlite3"))
            .expect("open sqlite runtime store"),
    );
    let persistence = RuntimeStoreHostBindingPersistence::new(
        Arc::clone(&store) as Arc<dyn meerkat_runtime::store::RuntimeStore>
    );
    Fixture {
        _dir: dir,
        store,
        persistence,
        authority: MobHostBindingAuthorityAuthority::new(),
        token: HostBootstrapTokenSlot::mint(),
    }
}

/// A supervisor identity whose peer id is genuinely derived from its pubkey
/// (the wire boundary validates the pair).
fn supervisor_identity(name: &str) -> BridgePeerIdentity {
    let keypair = meerkat_comms::Keypair::generate();
    let pubkey = keypair.public_key();
    let spec = BridgePeerSpec {
        name: name.to_string(),
        peer_id: pubkey.to_peer_id().as_str(),
        address: "tcp://127.0.0.1:1".to_string(),
        pubkey: *pubkey.as_bytes(),
    };
    BridgePeerIdentity::try_from(&spec).expect("valid supervisor spec")
}

fn token_matches(slot: &HostBootstrapTokenSlot, presented: &str) -> bool {
    meerkat_comms::constant_time_str_eq(slot.current(), presented)
}

fn bind_observations(
    fixture: &Fixture,
    mob_id: &str,
    supervisor: &BridgePeerIdentity,
    epoch: u64,
    presented_token: &str,
) -> HostBindObservations {
    HostBindObservations {
        mob_id: mob_id.to_string(),
        supervisor: supervisor.clone(),
        epoch,
        binding_generation: 1,
        sender_matches_supervisor: true,
        address_matches: true,
        token_valid: token_matches(&fixture.token, presented_token),
        accepted_capabilities: BridgeCapabilities::default(),
    }
}

async fn bind_fixture(fixture: &mut Fixture, mob_id: &str) -> BridgePeerIdentity {
    let supervisor = supervisor_identity("supervisor-a");
    let token = fixture.token.current().to_string();
    let observations = bind_observations(fixture, mob_id, &supervisor, 1, &token);
    let outcome = serve_host_bind(
        &mut fixture.authority,
        &fixture.persistence,
        &mut fixture.token,
        observations,
    )
    .await
    .expect("host binds for member-journal test");
    assert!(matches!(outcome, HostBindServeOutcome::Accepted { .. }));
    supervisor
}

fn member_key(mob_id: &str, identity: &str) -> MemberKey {
    MemberKey::new(
        AuthorityMobId::from(mob_id),
        AuthorityAgentIdentity::from(identity),
    )
}

fn materialized_row(
    mob_id: &str,
    identity: &str,
    generation: u64,
    fence_token: u64,
) -> MaterializedMemberRow {
    let spec: PortableMemberSpec = serde_json::from_value(serde_json::json!({
        "mob_id": mob_id,
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
    .expect("minimal portable member spec decodes");
    let spec_digest = portable_member_spec_digest(&spec).expect("portable spec digests");
    let member_keypair = meerkat_comms::Keypair::generate();
    let member_pubkey = member_keypair.public_key();
    MaterializedMemberRow {
        generation,
        generation_start_seq: 1,
        fence_token,
        session_id: meerkat_core::types::SessionId::new().to_string(),
        spec_digest,
        spec,
        engine_version_at_build: "0.0.0-test".to_string(),
        member_pubkey: member_pubkey.to_pubkey_string(),
        member_peer_id: member_pubkey.to_peer_id().to_string(),
        launch_outcome: MaterializeLaunchOutcome::Fresh,
        resolved_auth_binding: None,
        supervisor_name: "supervisor-a".to_string(),
        supervisor_address: "tcp://127.0.0.1:1".to_string(),
    }
}

fn outcome_row(
    input_id: &str,
    generation: u64,
    fence_token: u64,
    terminal_seq: u64,
) -> TurnOutcomeRow {
    TurnOutcomeRow {
        input_id: input_id.to_string(),
        generation,
        fence_token,
        terminal_seq,
        outcome: WireFlowTurnOutcome::RunCompleted,
    }
}

async fn reserve_pending(
    fixture: &mut Fixture,
    key: &MemberKey,
    input_id: &str,
    generation: u64,
    fence_token: u64,
    window_start: u64,
) -> TurnOutcomePendingReservationDisposition {
    reserve_turn_outcome_pending(
        &mut fixture.authority,
        &fixture.persistence,
        key,
        &TurnOutcomePendingRow {
            input_id: input_id.to_string(),
            generation,
            fence_token,
            window_start,
        },
    )
    .await
    .expect("Pending reservation records")
}

#[tokio::test(flavor = "multi_thread")]
async fn fresh_bind_persists_row_consumes_token_and_commits() {
    let mut fixture = fixture();
    let supervisor = supervisor_identity("supervisor-a");
    let token = fixture.token.current().to_string();

    let observations = bind_observations(&fixture, "mob-1", &supervisor, 4, &token);
    let outcome = serve_host_bind(
        &mut fixture.authority,
        &fixture.persistence,
        &mut fixture.token,
        observations,
    )
    .await
    .expect("bind serves");
    assert!(
        matches!(outcome, HostBindServeOutcome::Accepted { fresh: true, .. }),
        "first bind must be a fresh accept"
    );

    // Durable row matches the machine's recorded tuple.
    let record = fixture
        .persistence
        .load("mob-1")
        .await
        .expect("load row")
        .expect("row present");
    assert_eq!(record.supervisor_peer_id, supervisor.peer_id.as_str());
    assert_eq!(record.epoch, 4);

    // The committed authority state carries the binding.
    let mob = AuthorityMobId::from("mob-1");
    assert!(fixture.authority.state().binding_phases.contains_key(&mob));

    // DEC-P2-4: the token was consumed and re-minted.
    assert!(!token_matches(&fixture.token, &token));
    assert_ne!(fixture.token.current(), token);
}

#[tokio::test(flavor = "multi_thread")]
async fn committed_then_error_bind_converges_and_live_exact_generation_replays() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(
        meerkat_runtime::store::SqliteRuntimeStore::new(dir.path().join("sessions.sqlite3"))
            .expect("open sqlite runtime store"),
    );
    let persistence = CommittedThenErrorPersistence::new(RuntimeStoreHostBindingPersistence::new(
        Arc::clone(&store) as Arc<dyn meerkat_runtime::store::RuntimeStore>,
    ));
    let mut authority = MobHostBindingAuthorityAuthority::new();
    let mut token = HostBootstrapTokenSlot::mint();
    let supervisor = supervisor_identity("supervisor-ack-loss");
    let first_token = token.current().to_string();
    let accepted_capabilities = BridgeCapabilities {
        engine_version: "accepted-before-reply-loss".to_string(),
        ..BridgeCapabilities::default()
    };
    let first = HostBindObservations {
        mob_id: "mob-ack-loss".to_string(),
        supervisor: supervisor.clone(),
        epoch: 7,
        binding_generation: 11,
        sender_matches_supervisor: true,
        address_matches: true,
        token_valid: token_matches(&token, &first_token),
        accepted_capabilities: accepted_capabilities.clone(),
    };

    let outcome = serve_host_bind(&mut authority, &persistence, &mut token, first)
        .await
        .expect("bounded durable reread must converge a committed-then-error insert");
    assert!(matches!(
        outcome,
        HostBindServeOutcome::Accepted { fresh: true, .. }
    ));
    let durable = persistence
        .load("mob-ack-loss")
        .await
        .expect("load committed row")
        .expect("exact row present");
    assert_eq!(durable.binding_generation, 11);
    assert_eq!(
        durable.accepted_capabilities,
        Some(accepted_capabilities.clone())
    );

    // ACK loss leaves the controller holding the original T0 request. Exact
    // bound replay authenticates the recorded supervisor/epoch/G/sender/
    // address tuple and must not require the newly published T1 token.
    let replay_token = token.current().to_string();
    let replay = HostBindObservations {
        mob_id: "mob-ack-loss".to_string(),
        supervisor,
        epoch: 7,
        binding_generation: 11,
        sender_matches_supervisor: true,
        address_matches: true,
        token_valid: token_matches(&token, &first_token),
        accepted_capabilities: BridgeCapabilities {
            engine_version: "downgraded-after-remote-commit".to_string(),
            ..BridgeCapabilities::default()
        },
    };
    let outcome = serve_host_bind(&mut authority, &persistence, &mut token, replay)
        .await
        .expect("live exact-generation bind retry must replay");
    assert!(matches!(
        outcome,
        HostBindServeOutcome::Accepted { fresh: false, .. }
    ));
    assert_eq!(token.current(), replay_token);
    let durable = persistence
        .load("mob-ack-loss")
        .await
        .expect("reload replayed row")
        .expect("replayed row remains");
    assert_eq!(
        durable.accepted_capabilities,
        Some(accepted_capabilities),
        "exact replay must retain the capability snapshot committed with the authority tuple"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn committed_then_error_rebind_converges_live_and_after_restart() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(
        meerkat_runtime::store::SqliteRuntimeStore::new(dir.path().join("sessions.sqlite3"))
            .expect("open sqlite runtime store"),
    );
    let persistence =
        CommittedThenErrorPersistence::for_rebind(RuntimeStoreHostBindingPersistence::new(
            Arc::clone(&store) as Arc<dyn meerkat_runtime::store::RuntimeStore>,
        ));
    let mut authority = MobHostBindingAuthorityAuthority::new();
    let mut token = HostBootstrapTokenSlot::mint();
    let supervisor_a = supervisor_identity("supervisor-a");
    let supervisor_b = supervisor_identity("supervisor-b");
    let bind_token = token.current().to_string();
    let token_valid = token_matches(&token, &bind_token);
    serve_host_bind(
        &mut authority,
        &persistence,
        &mut token,
        HostBindObservations {
            mob_id: "mob-rebind-ack-loss".to_string(),
            supervisor: supervisor_a,
            epoch: 1,
            binding_generation: 5,
            sender_matches_supervisor: true,
            address_matches: true,
            token_valid,
            accepted_capabilities: BridgeCapabilities::default(),
        },
    )
    .await
    .expect("initial bind");

    let rebind = || HostRebindObservations {
        mob_id: "mob-rebind-ack-loss".to_string(),
        supervisor: supervisor_b.clone(),
        epoch: 2,
        binding_generation: 5,
        sender_matches_supervisor: true,
        accepted_capabilities: BridgeCapabilities::default(),
    };
    assert!(matches!(
        serve_host_rebind(&mut authority, &persistence, rebind())
            .await
            .expect("exact durable reread converges committed-then-error rebind"),
        HostRebindServeOutcome::Accepted { .. }
    ));
    assert!(matches!(
        serve_host_rebind(&mut authority, &persistence, rebind())
            .await
            .expect("live exact rebind replay"),
        HostRebindServeOutcome::Accepted { .. }
    ));

    let mut recovered = recover_or_create_binding_authority(&persistence)
        .await
        .expect("restart recovers committed rebind row");
    assert!(matches!(
        serve_host_rebind(&mut recovered, &persistence, rebind())
            .await
            .expect("restart exact rebind replay"),
        HostRebindServeOutcome::Accepted { .. }
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn committed_then_error_revoke_converges_live_and_after_restart() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(
        meerkat_runtime::store::SqliteRuntimeStore::new(dir.path().join("sessions.sqlite3"))
            .expect("open sqlite runtime store"),
    );
    let persistence =
        CommittedThenErrorPersistence::for_revoke(RuntimeStoreHostBindingPersistence::new(
            Arc::clone(&store) as Arc<dyn meerkat_runtime::store::RuntimeStore>,
        ));
    let mut authority = MobHostBindingAuthorityAuthority::new();
    let mut token = HostBootstrapTokenSlot::mint();
    let supervisor = supervisor_identity("supervisor-revoke");
    let bind_token = token.current().to_string();
    let token_valid = token_matches(&token, &bind_token);
    serve_host_bind(
        &mut authority,
        &persistence,
        &mut token,
        HostBindObservations {
            mob_id: "mob-revoke-ack-loss".to_string(),
            supervisor: supervisor.clone(),
            epoch: 4,
            binding_generation: 8,
            sender_matches_supervisor: true,
            address_matches: true,
            token_valid,
            accepted_capabilities: BridgeCapabilities::default(),
        },
    )
    .await
    .expect("initial bind");
    let supervisor_peer_id = supervisor.peer_id.as_str();

    assert!(
        revoke_host_binding(
            &mut authority,
            &persistence,
            "mob-revoke-ack-loss",
            supervisor_peer_id.as_str(),
            supervisor.pubkey.into_bytes(),
            4,
            8,
        )
        .await
        .expect("exact receipt reread converges committed-then-error revoke")
    );
    assert!(
        revoke_host_binding(
            &mut authority,
            &persistence,
            "mob-revoke-ack-loss",
            supervisor_peer_id.as_str(),
            supervisor.pubkey.into_bytes(),
            4,
            8,
        )
        .await
        .expect("live exact revoke replay")
    );

    let mut recovered = recover_or_create_binding_authority(&persistence)
        .await
        .expect("restart recovers revoke receipt");
    assert!(
        revoke_host_binding(
            &mut recovered,
            &persistence,
            "mob-revoke-ack-loss",
            supervisor_peer_id.as_str(),
            supervisor.pubkey.into_bytes(),
            4,
            8,
        )
        .await
        .expect("restart exact revoke replay")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn committed_then_error_materialize_and_release_converge_exactly() {
    let mut fixture = fixture();
    bind_fixture(&mut fixture, "mob-member-ack-loss").await;
    let key = member_key("mob-member-ack-loss", "worker-1");

    let materialize = CommittedThenErrorPersistence::for_member_region(
        RuntimeStoreHostBindingPersistence::new(
            Arc::clone(&fixture.store) as Arc<dyn meerkat_runtime::store::RuntimeStore>
        ),
        CommittedThenErrorRegion::Materialize,
    );
    record_materialized_member(
        &mut fixture.authority,
        &materialize,
        &key,
        materialized_row("mob-member-ack-loss", "worker-1", 3, 7),
    )
    .await
    .expect("exact durable reread converges lost materialize completion");
    assert_eq!(
        materialize
            .load("mob-member-ack-loss")
            .await
            .expect("materialized row loads")
            .expect("binding remains")
            .materialized["worker-1"]
            .fence_token,
        7
    );

    let release = CommittedThenErrorPersistence::for_member_region(
        RuntimeStoreHostBindingPersistence::new(
            Arc::clone(&fixture.store) as Arc<dyn meerkat_runtime::store::RuntimeStore>
        ),
        CommittedThenErrorRegion::Release,
    );
    record_member_release(
        &mut fixture.authority,
        &release,
        &key,
        3,
        7,
        MachineMemberSessionDisposal::Archived,
    )
    .await
    .expect("exact durable reread converges lost release completion");
    let durable = release
        .load("mob-member-ack-loss")
        .await
        .expect("released row loads")
        .expect("binding remains");
    assert!(!durable.materialized.contains_key("worker-1"));
    assert!(durable.released.contains_key("worker-1"));
}

#[tokio::test(flavor = "multi_thread")]
async fn committed_then_error_pending_outcome_and_ack_converge_exactly() {
    let mut fixture = fixture();
    bind_fixture(&mut fixture, "mob-journal-ack-loss").await;
    let key = member_key("mob-journal-ack-loss", "worker-1");
    record_materialized_member(
        &mut fixture.authority,
        &fixture.persistence,
        &key,
        materialized_row("mob-journal-ack-loss", "worker-1", 5, 9),
    )
    .await
    .expect("member materializes");

    let pending = CommittedThenErrorPersistence::for_member_region(
        RuntimeStoreHostBindingPersistence::new(
            Arc::clone(&fixture.store) as Arc<dyn meerkat_runtime::store::RuntimeStore>
        ),
        CommittedThenErrorRegion::Pending,
    );
    let reserved = TurnOutcomePendingRow {
        input_id: "terminal".to_string(),
        generation: 5,
        fence_token: 9,
        window_start: 41,
    };
    assert_eq!(
        reserve_turn_outcome_pending(&mut fixture.authority, &pending, &key, &reserved)
            .await
            .expect("lost Pending reservation completion converges"),
        TurnOutcomePendingReservationDisposition::Reserved { window_start: 41 }
    );

    let canceled = TurnOutcomePendingRow {
        input_id: "canceled".to_string(),
        window_start: 42,
        ..reserved.clone()
    };
    reserve_turn_outcome_pending(&mut fixture.authority, &pending, &key, &canceled)
        .await
        .expect("second lost Pending reservation completion converges");
    assert_eq!(
        cancel_turn_outcome_pending(&mut fixture.authority, &pending, &key, 5, 9, "canceled",)
            .await
            .expect("lost Pending cancellation completion converges"),
        TurnOutcomePendingCancelDisposition::Canceled
    );

    let outcome = CommittedThenErrorPersistence::for_member_region(
        RuntimeStoreHostBindingPersistence::new(
            Arc::clone(&fixture.store) as Arc<dyn meerkat_runtime::store::RuntimeStore>
        ),
        CommittedThenErrorRegion::Outcome,
    );
    assert_eq!(
        record_turn_outcome_journal(
            &mut fixture.authority,
            &outcome,
            &key,
            &outcome_row("terminal", 5, 9, 77),
        )
        .await
        .expect("lost terminal-row completion converges"),
        TurnOutcomeRecordDisposition::Recorded
    );

    let ack = CommittedThenErrorPersistence::for_member_region(
        RuntimeStoreHostBindingPersistence::new(
            Arc::clone(&fixture.store) as Arc<dyn meerkat_runtime::store::RuntimeStore>
        ),
        CommittedThenErrorRegion::Ack,
    );
    assert_eq!(
        acknowledge_turn_outcome_journal(
            &mut fixture.authority,
            &ack,
            &key,
            &BridgeTurnOutcomeAck {
                generation: 5,
                fence_token: 9,
                input_id: "terminal".to_string(),
            },
        )
        .await
        .expect("lost acknowledgement completion converges"),
        TurnOutcomeAckDisposition::Pruned
    );
    let durable = ack
        .load("mob-journal-ack-loss")
        .await
        .expect("binding row loads")
        .expect("binding remains");
    assert!(!durable.turn_outcomes.contains_key("worker-1"));
    assert!(!durable.turn_outcome_pending.contains_key("worker-1"));
    assert!(
        durable
            .turn_outcome_acknowledged
            .get("worker-1")
            .is_some_and(|rows| rows.iter().any(|row| {
                row.input_id == "terminal" && row.generation == 5 && row.fence_token == 9
            })),
        "ACK must replace the payload row with an exact durable tombstone"
    );

    let mut recovered = recover_or_create_binding_authority(&ack)
        .await
        .expect("cold recovery folds ACK tombstones");
    assert_eq!(
        reserve_turn_outcome_pending_replay_only(
            &mut recovered,
            &ack,
            &key,
            &TurnOutcomePendingRow {
                window_start: u64::MAX,
                ..reserved
            },
        )
        .await
        .expect("delayed post-ACK delivery replays after cold recovery"),
        TurnOutcomePendingReservationDisposition::TerminalReplay
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn tracked_cancel_no_effect_blocks_delayed_delivery_across_restart() {
    let mut fixture = fixture();
    bind_fixture(&mut fixture, "mob-cancel-no-effect").await;
    let key = member_key("mob-cancel-no-effect", "worker-1");
    record_materialized_member(
        &mut fixture.authority,
        &fixture.persistence,
        &key,
        materialized_row("mob-cancel-no-effect", "worker-1", 5, 9),
    )
    .await
    .expect("member materializes");

    assert_eq!(
        cancel_tracked_input_journal(
            &mut fixture.authority,
            &fixture.persistence,
            &key,
            "cancel-before-send",
            5,
            9,
            false,
        )
        .await
        .expect("absent exact input records a negative receipt"),
        TrackedInputCancelDisposition::NoEffect
    );
    assert_eq!(
        reserve_turn_outcome_pending_replay_only(
            &mut fixture.authority,
            &fixture.persistence,
            &key,
            &TurnOutcomePendingRow {
                input_id: "cancel-before-send".to_string(),
                generation: 5,
                fence_token: 9,
                window_start: u64::MAX,
            },
        )
        .await
        .expect("delayed delivery is actor-deduplicated"),
        TurnOutcomePendingReservationDisposition::TerminalReplay
    );

    let mut recovered = recover_or_create_binding_authority(&fixture.persistence)
        .await
        .expect("negative receipt survives host restart");
    assert_eq!(
        cancel_tracked_input_journal(
            &mut recovered,
            &fixture.persistence,
            &key,
            "cancel-before-send",
            5,
            9,
            false,
        )
        .await
        .expect("cancel replay is stable"),
        TrackedInputCancelDisposition::NoEffect
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn accepted_pending_cancel_blocks_delivery_before_runtime_quiescence() {
    let mut fixture = fixture();
    bind_fixture(&mut fixture, "mob-cancel-accepted").await;
    let key = member_key("mob-cancel-accepted", "worker-1");
    record_materialized_member(
        &mut fixture.authority,
        &fixture.persistence,
        &key,
        materialized_row("mob-cancel-accepted", "worker-1", 5, 9),
    )
    .await
    .expect("member materializes");
    assert_eq!(
        reserve_pending(&mut fixture, &key, "accepted-input", 5, 9, 41).await,
        TurnOutcomePendingReservationDisposition::Reserved { window_start: 41 }
    );

    let cancel = CommittedThenErrorPersistence::for_member_region(
        RuntimeStoreHostBindingPersistence::new(
            Arc::clone(&fixture.store) as Arc<dyn meerkat_runtime::store::RuntimeStore>
        ),
        CommittedThenErrorRegion::TrackedCancel,
    );
    assert_eq!(
        cancel_tracked_input_journal(
            &mut fixture.authority,
            &cancel,
            &key,
            "accepted-input",
            5,
            9,
            true,
        )
        .await
        .expect("lost Cancelling write completion converges"),
        TrackedInputCancelDisposition::Cancelling
    );
    assert_eq!(
        reserve_turn_outcome_pending_replay_only(
            &mut fixture.authority,
            &cancel,
            &key,
            &TurnOutcomePendingRow {
                input_id: "accepted-input".to_string(),
                generation: 5,
                fence_token: 9,
                window_start: u64::MAX,
            },
        )
        .await
        .expect("Cancelling is already a delivery fence"),
        TurnOutcomePendingReservationDisposition::TerminalReplay
    );

    let mut recovered = recover_or_create_binding_authority(&cancel)
        .await
        .expect("Cancelling receipt survives restart");
    assert_eq!(
        cancel_tracked_input_journal(&mut recovered, &cancel, &key, "accepted-input", 5, 9, true,)
            .await
            .expect("retry resumes level-triggered cancellation"),
        TrackedInputCancelDisposition::Cancelling
    );
    assert_eq!(
        complete_tracked_input_cancel_journal(
            &mut recovered,
            &cancel,
            &key,
            "accepted-input",
            5,
            9,
        )
        .await
        .expect("runtime quiescence advances the receipt"),
        TrackedInputCancelDisposition::Cancelled
    );
    let durable = cancel
        .load("mob-cancel-accepted")
        .await
        .expect("binding loads")
        .expect("binding remains");
    assert!(!durable.turn_outcome_pending.contains_key("worker-1"));
    assert!(
        durable
            .tracked_input_cancellations
            .get("worker-1")
            .is_some_and(|rows| rows.iter().any(|row| {
                row.input_id == "accepted-input"
                    && row.outcome
                        == meerkat_mob::runtime::host_actor::RecordedTrackedInputCancelKind::Cancelled
            }))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn tracked_cancel_returns_existing_terminal_verbatim() {
    let mut fixture = fixture();
    bind_fixture(&mut fixture, "mob-cancel-terminal").await;
    let key = member_key("mob-cancel-terminal", "worker-1");
    record_materialized_member(
        &mut fixture.authority,
        &fixture.persistence,
        &key,
        materialized_row("mob-cancel-terminal", "worker-1", 5, 9),
    )
    .await
    .expect("member materializes");
    reserve_pending(&mut fixture, &key, "terminal-input", 5, 9, 41).await;
    let expected = outcome_row("terminal-input", 5, 9, 77).to_wire();
    record_turn_outcome_journal(
        &mut fixture.authority,
        &fixture.persistence,
        &key,
        &outcome_row("terminal-input", 5, 9, 77),
    )
    .await
    .expect("terminal records");
    assert_eq!(
        cancel_tracked_input_journal(
            &mut fixture.authority,
            &fixture.persistence,
            &key,
            "terminal-input",
            5,
            9,
            true,
        )
        .await
        .expect("terminal wins cancellation race"),
        TrackedInputCancelDisposition::Terminal(expected)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn consumed_token_rejects_next_mob_and_fresh_token_binds_it() {
    let mut fixture = fixture();
    let supervisor = supervisor_identity("supervisor-a");
    let first_token = fixture.token.current().to_string();

    let observations = bind_observations(&fixture, "mob-1", &supervisor, 1, &first_token);
    let outcome = serve_host_bind(
        &mut fixture.authority,
        &fixture.persistence,
        &mut fixture.token,
        observations,
    )
    .await
    .expect("first bind serves");
    assert!(matches!(outcome, HostBindServeOutcome::Accepted { .. }));

    // §7.2 one-time token: replaying the consumed token for a SECOND mob is
    // a typed InvalidBootstrapToken reject — machine-decided from the
    // `token_valid=false` observation.
    let observations = bind_observations(&fixture, "mob-2", &supervisor, 1, &first_token);
    let outcome = serve_host_bind(
        &mut fixture.authority,
        &fixture.persistence,
        &mut fixture.token,
        observations,
    )
    .await
    .expect("second bind serves");
    match outcome {
        HostBindServeOutcome::Rejected { cause, .. } => {
            assert_eq!(cause, BridgeRejectionCause::InvalidBootstrapToken);
        }
        HostBindServeOutcome::Accepted { .. } => panic!("consumed token must not bind"),
    }
    assert!(
        fixture
            .persistence
            .load("mob-2")
            .await
            .expect("load")
            .is_none(),
        "rejected bind must not persist"
    );

    // The re-minted token binds the second mob (multi-tenant daemon, A14).
    let fresh_token = fixture.token.current().to_string();
    let observations = bind_observations(&fixture, "mob-2", &supervisor, 1, &fresh_token);
    let outcome = serve_host_bind(
        &mut fixture.authority,
        &fixture.persistence,
        &mut fixture.token,
        observations,
    )
    .await
    .expect("third bind serves");
    assert!(matches!(
        outcome,
        HostBindServeOutcome::Accepted { fresh: true, .. }
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn second_supervisor_for_bound_mob_stays_already_bound() {
    let mut fixture = fixture();
    let supervisor_a = supervisor_identity("supervisor-a");
    let token = fixture.token.current().to_string();
    let observations = bind_observations(&fixture, "mob-1", &supervisor_a, 1, &token);
    serve_host_bind(
        &mut fixture.authority,
        &fixture.persistence,
        &mut fixture.token,
        observations,
    )
    .await
    .expect("bind serves");

    let supervisor_b = supervisor_identity("supervisor-b");
    let fresh_token = fixture.token.current().to_string();
    let observations = bind_observations(&fixture, "mob-1", &supervisor_b, 9, &fresh_token);
    let outcome = serve_host_bind(
        &mut fixture.authority,
        &fixture.persistence,
        &mut fixture.token,
        observations,
    )
    .await
    .expect("second supervisor bind serves");
    match outcome {
        HostBindServeOutcome::Rejected { cause, .. } => {
            assert_eq!(cause, BridgeRejectionCause::AlreadyBound);
        }
        HostBindServeOutcome::Accepted { .. } => panic!("A14: second supervisor must reject"),
    }
    // The fresh token was NOT consumed by the rejected attempt.
    assert!(token_matches(&fixture.token, &fresh_token));
}

#[tokio::test(flavor = "multi_thread")]
async fn preexisting_durable_row_blocks_fresh_accept_and_drops_prepared_state() {
    let mut fixture = fixture();
    let supervisor = supervisor_identity("supervisor-a");

    // Seed a well-formed but divergent durable row through the raw store
    // (simulating a half-cleaned prior daemon state the machine no longer
    // remembers). Corrupt bytes are a distinct RecordSerde contract covered
    // by `recovery_folds_durable_rows_and_corrupt_rows_abort_typed` below.
    let divergent = MobHostBindingRecord {
        supervisor_peer_id: "peer-divergent".to_string(),
        supervisor_signing_key: [9u8; 32],
        epoch: 99,
        binding_generation: 99,
        accepted_capabilities: None,
        materialized: std::collections::BTreeMap::new(),
        released: std::collections::BTreeMap::new(),
        turn_outcomes: std::collections::BTreeMap::new(),
        turn_outcome_pending: std::collections::BTreeMap::new(),
        turn_outcome_acknowledged: std::collections::BTreeMap::new(),
        tracked_input_cancellations: std::collections::BTreeMap::new(),
    };
    let divergent = serde_json::to_vec(&divergent).expect("serialize divergent durable row");
    fixture
        .store
        .put_mob_host_binding_if_absent("mob-1", &divergent)
        .await
        .expect("seed row");

    let token = fixture.token.current().to_string();
    let observations = bind_observations(&fixture, "mob-1", &supervisor, 1, &token);
    let err = serve_host_bind(
        &mut fixture.authority,
        &fixture.persistence,
        &mut fixture.token,
        observations,
    )
    .await
    .expect_err("divergent durable row must fail the ceremony");
    assert!(
        matches!(&err, MobHostActorError::StoreDiverged { .. }),
        "well-formed conflicting authority must surface StoreDiverged, got {err:?}"
    );

    // In-memory truth never advanced past durable truth: no binding
    // committed, token untouched.
    let mob = AuthorityMobId::from("mob-1");
    assert!(!fixture.authority.state().binding_phases.contains_key(&mob));
    assert!(token_matches(&fixture.token, &token));
}

#[tokio::test(flavor = "multi_thread")]
async fn rebind_advances_epoch_swaps_row_and_reports_rotated_out_supervisor() {
    let mut fixture = fixture();
    let supervisor_a = supervisor_identity("supervisor-a");
    let token = fixture.token.current().to_string();
    let observations = bind_observations(&fixture, "mob-1", &supervisor_a, 1, &token);
    serve_host_bind(
        &mut fixture.authority,
        &fixture.persistence,
        &mut fixture.token,
        observations,
    )
    .await
    .expect("bind serves");

    // A rebind whose envelope sender does not match the offered supervisor
    // is a typed SenderMismatch, machine-decided from the observation.
    let outcome = serve_host_rebind(
        &mut fixture.authority,
        &fixture.persistence,
        HostRebindObservations {
            mob_id: "mob-1".to_string(),
            supervisor: supervisor_a.clone(),
            epoch: 1,
            binding_generation: 1,
            sender_matches_supervisor: false,
            accepted_capabilities: BridgeCapabilities::default(),
        },
    )
    .await
    .expect("rebind serves");
    assert!(matches!(
        outcome,
        HostRebindServeOutcome::Rejected {
            cause: BridgeRejectionCause::SenderMismatch,
            ..
        }
    ));

    // Rotation to a NEW supervisor at epoch+1 swaps the durable row and
    // names the rotated-out peer.
    let supervisor_b = supervisor_identity("supervisor-b");
    let outcome = serve_host_rebind(
        &mut fixture.authority,
        &fixture.persistence,
        HostRebindObservations {
            mob_id: "mob-1".to_string(),
            supervisor: supervisor_b.clone(),
            epoch: 2,
            binding_generation: 1,
            sender_matches_supervisor: true,
            accepted_capabilities: BridgeCapabilities::default(),
        },
    )
    .await
    .expect("rebind serves");
    match outcome {
        HostRebindServeOutcome::Accepted {
            previous_supervisor_peer_id,
            ..
        } => {
            assert_eq!(
                previous_supervisor_peer_id.as_deref(),
                Some(supervisor_a.peer_id.as_str().as_str())
            );
        }
        HostRebindServeOutcome::Rejected { cause, reason } => {
            panic!("rotation rebind must accept, got {cause:?}: {reason}")
        }
    }
    let record = fixture
        .persistence
        .load("mob-1")
        .await
        .expect("load")
        .expect("row present");
    assert_eq!(record.supervisor_peer_id, supervisor_b.peer_id.as_str());
    assert_eq!(record.epoch, 2);

    // Lower epoch from the OLD supervisor after rotation: StaleSupervisor.
    let outcome = serve_host_rebind(
        &mut fixture.authority,
        &fixture.persistence,
        HostRebindObservations {
            mob_id: "mob-1".to_string(),
            supervisor: supervisor_a.clone(),
            epoch: 2,
            binding_generation: 1,
            sender_matches_supervisor: true,
            accepted_capabilities: BridgeCapabilities::default(),
        },
    )
    .await
    .expect("rebind serves");
    assert!(matches!(
        outcome,
        HostRebindServeOutcome::Rejected {
            cause: BridgeRejectionCause::StaleSupervisor,
            ..
        }
    ));

    // FLAG-2(ii): retry at the ACCEPTED epoch from the accepted supervisor
    // converges as an idempotent replay ack (row untouched, no rotation
    // report).
    let outcome = serve_host_rebind(
        &mut fixture.authority,
        &fixture.persistence,
        HostRebindObservations {
            mob_id: "mob-1".to_string(),
            supervisor: supervisor_b.clone(),
            epoch: 2,
            binding_generation: 1,
            sender_matches_supervisor: true,
            accepted_capabilities: BridgeCapabilities::default(),
        },
    )
    .await
    .expect("replay rebind serves");
    match outcome {
        HostRebindServeOutcome::Accepted {
            previous_supervisor_peer_id,
            ..
        } => assert_eq!(previous_supervisor_peer_id, None),
        HostRebindServeOutcome::Rejected { cause, reason } => {
            panic!("rebind replay must re-ack idempotently, got {cause:?}: {reason}")
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn rebind_unbound_mob_is_not_bound() {
    let mut fixture = fixture();
    let supervisor = supervisor_identity("supervisor-a");
    let outcome = serve_host_rebind(
        &mut fixture.authority,
        &fixture.persistence,
        HostRebindObservations {
            mob_id: "mob-x".to_string(),
            supervisor,
            epoch: 5,
            binding_generation: 1,
            sender_matches_supervisor: true,
            accepted_capabilities: BridgeCapabilities::default(),
        },
    )
    .await
    .expect("rebind serves");
    assert!(matches!(
        outcome,
        HostRebindServeOutcome::Rejected {
            cause: BridgeRejectionCause::NotBound,
            ..
        }
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn revoke_atomically_installs_durable_receipt_and_exact_retry_replays() {
    let mut fixture = fixture();
    let supervisor = supervisor_identity("supervisor-a");
    let token = fixture.token.current().to_string();
    let observations = bind_observations(&fixture, "mob-1", &supervisor, 1, &token);
    serve_host_bind(
        &mut fixture.authority,
        &fixture.persistence,
        &mut fixture.token,
        observations,
    )
    .await
    .expect("bind serves");

    let revoked = revoke_host_binding(
        &mut fixture.authority,
        &fixture.persistence,
        "mob-1",
        &supervisor.peer_id.as_str(),
        supervisor.pubkey.into_bytes(),
        1,
        1,
    )
    .await
    .expect("revoke serves");
    assert!(revoked);
    assert!(
        fixture
            .persistence
            .load("mob-1")
            .await
            .expect("load")
            .is_none()
    );
    let mob = AuthorityMobId::from("mob-1");
    assert!(!fixture.authority.state().binding_phases.contains_key(&mob));
    let receipt = fixture
        .persistence
        .load_revocation("mob-1")
        .await
        .expect("load receipt")
        .expect("receipt present");
    assert_eq!(receipt.supervisor_peer_id, supervisor.peer_id.as_str());
    assert_eq!(
        receipt.supervisor_signing_key,
        supervisor.pubkey.into_bytes()
    );
    assert_eq!(receipt.epoch, 1);

    let revoked = revoke_host_binding(
        &mut fixture.authority,
        &fixture.persistence,
        "mob-1",
        &supervisor.peer_id.as_str(),
        supervisor.pubkey.into_bytes(),
        1,
        1,
    )
    .await
    .expect("exact revoke retry serves");
    assert!(revoked, "exact retry must replay the durable receipt");
}

#[tokio::test(flavor = "multi_thread")]
async fn release_before_watcher_drops_terminal_without_durable_zombie_row() {
    let mut fixture = fixture();
    bind_fixture(&mut fixture, "mob-1").await;
    let key = member_key("mob-1", "worker-1");
    record_materialized_member(
        &mut fixture.authority,
        &fixture.persistence,
        &key,
        materialized_row("mob-1", "worker-1", 1, 1),
    )
    .await
    .expect("member materialization records");

    assert_eq!(
        reserve_pending(&mut fixture, &key, "late-after-release", 1, 1, 1).await,
        TurnOutcomePendingReservationDisposition::Reserved { window_start: 1 }
    );

    record_member_release(
        &mut fixture.authority,
        &fixture.persistence,
        &key,
        1,
        1,
        MachineMemberSessionDisposal::Archived,
    )
    .await
    .expect("member release records");

    let disposition = record_turn_outcome_journal(
        &mut fixture.authority,
        &fixture.persistence,
        &key,
        &outcome_row("late-after-release", 1, 1, 41),
    )
    .await
    .expect("late watcher terminal is handled");
    assert_eq!(disposition, TurnOutcomeRecordDisposition::DroppedStale);

    let durable = fixture
        .persistence
        .load("mob-1")
        .await
        .expect("durable row loads")
        .expect("binding remains");
    assert!(
        !durable.turn_outcomes.contains_key("worker-1"),
        "released identity must never regain an invisible durable journal row"
    );
    assert!(
        !durable.turn_outcome_pending.contains_key("worker-1"),
        "released identity must not retain its pre-accept Pending row"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn same_generation_higher_fence_prunes_old_row_and_old_watcher() {
    let mut fixture = fixture();
    bind_fixture(&mut fixture, "mob-1").await;
    let key = member_key("mob-1", "worker-1");
    record_materialized_member(
        &mut fixture.authority,
        &fixture.persistence,
        &key,
        materialized_row("mob-1", "worker-1", 7, 10),
    )
    .await
    .expect("old fence materializes");
    assert_eq!(
        reserve_pending(&mut fixture, &key, "old-current", 7, 10, 1).await,
        TurnOutcomePendingReservationDisposition::Reserved { window_start: 1 }
    );
    assert_eq!(
        record_turn_outcome_journal(
            &mut fixture.authority,
            &fixture.persistence,
            &key,
            &outcome_row("old-current", 7, 10, 41),
        )
        .await
        .expect("old current terminal records"),
        TurnOutcomeRecordDisposition::Recorded
    );
    assert_eq!(
        reserve_pending(&mut fixture, &key, "pending-old-watcher", 7, 10, 2).await,
        TurnOutcomePendingReservationDisposition::Reserved { window_start: 2 }
    );

    record_materialized_member(
        &mut fixture.authority,
        &fixture.persistence,
        &key,
        materialized_row("mob-1", "worker-1", 7, 11),
    )
    .await
    .expect("higher fence rematerializes at the same generation");
    let durable = fixture
        .persistence
        .load("mob-1")
        .await
        .expect("durable row loads")
        .expect("binding remains");
    assert!(
        !durable.turn_outcomes.contains_key("worker-1"),
        "materialization CAS must prune the old fence's row"
    );
    assert!(
        !durable.turn_outcome_pending.contains_key("worker-1"),
        "materialization CAS must prune the old fence's Pending row"
    );

    assert_eq!(
        record_turn_outcome_journal(
            &mut fixture.authority,
            &fixture.persistence,
            &key,
            &outcome_row("pending-old-watcher", 7, 10, 42),
        )
        .await
        .expect("old watcher terminal is handled"),
        TurnOutcomeRecordDisposition::DroppedStale
    );
    let current = outcome_row("current-fence", 7, 11, 43);
    assert_eq!(
        reserve_pending(&mut fixture, &key, "current-fence", 7, 11, 2).await,
        TurnOutcomePendingReservationDisposition::Reserved { window_start: 2 }
    );
    assert_eq!(
        record_turn_outcome_journal(&mut fixture.authority, &fixture.persistence, &key, &current,)
            .await
            .expect("current fence terminal records"),
        TurnOutcomeRecordDisposition::Recorded
    );
    assert_eq!(
        record_turn_outcome_journal(
            &mut fixture.authority,
            &fixture.persistence,
            &key,
            &TurnOutcomeRow {
                terminal_seq: 999,
                outcome: WireFlowTurnOutcome::ChannelClosed,
                ..current
            },
        )
        .await
        .expect("exact replay converges"),
        TurnOutcomeRecordDisposition::Replayed
    );

    let durable = fixture
        .persistence
        .load("mob-1")
        .await
        .expect("durable row loads")
        .expect("binding remains");
    let rows = durable
        .turn_outcomes
        .get("worker-1")
        .expect("current row remains");
    assert_eq!(rows.len(), 1);
    assert_eq!((rows[0].generation, rows[0].fence_token), (7, 11));
    assert_eq!(rows[0].terminal_seq, 43, "recorded row wins replay");
}

#[tokio::test(flavor = "multi_thread")]
async fn repeated_materialization_generations_keep_durable_journal_bounded() {
    let mut fixture = fixture();
    bind_fixture(&mut fixture, "mob-1").await;
    let key = member_key("mob-1", "worker-1");

    for generation in 1..=32 {
        let fence_token = generation * 10;
        record_materialized_member(
            &mut fixture.authority,
            &fixture.persistence,
            &key,
            materialized_row("mob-1", "worker-1", generation, fence_token),
        )
        .await
        .expect("successive generation materializes");

        if generation > 1 {
            assert_eq!(
                record_turn_outcome_journal(
                    &mut fixture.authority,
                    &fixture.persistence,
                    &key,
                    &outcome_row(
                        "late-previous-generation",
                        generation - 1,
                        (generation - 1) * 10,
                        generation,
                    ),
                )
                .await
                .expect("previous watcher terminal is handled"),
                TurnOutcomeRecordDisposition::DroppedStale
            );
        }

        assert_eq!(
            reserve_pending(
                &mut fixture,
                &key,
                "current",
                generation,
                fence_token,
                generation,
            )
            .await,
            TurnOutcomePendingReservationDisposition::Reserved {
                window_start: generation
            }
        );
        assert_eq!(
            record_turn_outcome_journal(
                &mut fixture.authority,
                &fixture.persistence,
                &key,
                &outcome_row("current", generation, fence_token, generation),
            )
            .await
            .expect("current watcher terminal records"),
            TurnOutcomeRecordDisposition::Recorded
        );
        let durable = fixture
            .persistence
            .load("mob-1")
            .await
            .expect("durable row loads")
            .expect("binding remains");
        let rows = durable
            .turn_outcomes
            .get("worker-1")
            .expect("current row remains");
        assert_eq!(
            rows.len(),
            1,
            "generation {generation} must not accumulate zombie rows"
        );
        assert_eq!(
            (rows[0].generation, rows[0].fence_token),
            (generation, fence_token)
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn crash_after_pending_reserve_before_accept_recovers_exact_window() {
    let mut fixture = fixture();
    bind_fixture(&mut fixture, "mob-1").await;
    let key = member_key("mob-1", "worker-1");
    record_materialized_member(
        &mut fixture.authority,
        &fixture.persistence,
        &key,
        materialized_row("mob-1", "worker-1", 7, 11),
    )
    .await
    .expect("member materializes");
    assert_eq!(
        reserve_pending(&mut fixture, &key, "pre-accept-crash", 7, 11, 42).await,
        TurnOutcomePendingReservationDisposition::Reserved { window_start: 42 }
    );

    let recovered = recover_or_create_binding_authority(&fixture.persistence)
        .await
        .expect("host authority recovers after pre-accept crash");
    let turn_key = meerkat_mob::machines::mob_host_binding_authority::TurnKey::new(
        AuthorityMobId::from("mob-1"),
        AuthorityAgentIdentity::from("worker-1"),
        meerkat_mob::machines::mob_host_binding_authority::Generation(7),
        meerkat_mob::machines::mob_host_binding_authority::FenceToken(11),
        meerkat_mob::machines::mob_host_binding_authority::InputId::from("pre-accept-crash"),
    );
    assert_eq!(
        recovered
            .state()
            .turn_outcome_pending_window_starts
            .get(&turn_key),
        Some(&42),
        "restart must retain Pending without treating it as runtime acceptance"
    );
    assert!(
        !recovered
            .state()
            .turn_outcome_terminal_seqs
            .contains_key(&turn_key)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn replay_only_arbitration_allows_exact_pending_or_terminal_and_never_commits_fresh() {
    let mut fixture = fixture();
    bind_fixture(&mut fixture, "mob-sequence-exhaustion").await;
    let key = member_key("mob-sequence-exhaustion", "worker-1");
    record_materialized_member(
        &mut fixture.authority,
        &fixture.persistence,
        &key,
        materialized_row("mob-sequence-exhaustion", "worker-1", 7, 11),
    )
    .await
    .expect("member materializes");

    let probe = |input_id: &str| TurnOutcomePendingRow {
        input_id: input_id.to_string(),
        generation: 7,
        fence_token: 11,
        // Replay-only arbitration must never commit this inert proposal.
        window_start: u64::MAX,
    };

    assert_eq!(
        reserve_turn_outcome_pending_replay_only(
            &mut fixture.authority,
            &fixture.persistence,
            &key,
            &probe("fresh-at-max"),
        )
        .await
        .expect("fresh replay-only arbitration is typed"),
        TurnOutcomePendingReservationDisposition::FreshRequired
    );
    assert!(
        !fixture
            .authority
            .state()
            .turn_outcome_pending_window_starts
            .keys()
            .any(|turn| turn.input_id.0 == "fresh-at-max"),
        "fresh replay probe must not commit prepared authority"
    );
    let durable = fixture
        .persistence
        .load("mob-sequence-exhaustion")
        .await
        .expect("durable row loads")
        .expect("binding remains");
    assert!(
        durable
            .turn_outcome_pending
            .get("worker-1")
            .is_none_or(|rows| rows.iter().all(|row| row.input_id != "fresh-at-max")),
        "fresh replay probe must not persist Pending"
    );

    assert_eq!(
        reserve_pending(&mut fixture, &key, "pending-at-max", 7, 11, 42).await,
        TurnOutcomePendingReservationDisposition::Reserved { window_start: 42 }
    );
    assert_eq!(
        reserve_turn_outcome_pending_replay_only(
            &mut fixture.authority,
            &fixture.persistence,
            &key,
            &probe("pending-at-max"),
        )
        .await
        .expect("existing Pending remains replayable at exhaustion"),
        TurnOutcomePendingReservationDisposition::Replayed { window_start: 42 }
    );

    assert_eq!(
        reserve_pending(&mut fixture, &key, "terminal-at-max", 7, 11, 43).await,
        TurnOutcomePendingReservationDisposition::Reserved { window_start: 43 }
    );
    assert_eq!(
        record_turn_outcome_journal(
            &mut fixture.authority,
            &fixture.persistence,
            &key,
            &outcome_row("terminal-at-max", 7, 11, 77),
        )
        .await
        .expect("terminal records before exhaustion replay"),
        TurnOutcomeRecordDisposition::Recorded
    );
    assert_eq!(
        reserve_turn_outcome_pending_replay_only(
            &mut fixture.authority,
            &fixture.persistence,
            &key,
            &probe("terminal-at-max"),
        )
        .await
        .expect("existing terminal remains replayable at exhaustion"),
        TurnOutcomePendingReservationDisposition::TerminalReplay
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mixed_pending_and_terminal_quota_survives_sqlite_restart() {
    let mut fixture = fixture();
    bind_fixture(&mut fixture, "mob-1").await;
    let key = member_key("mob-1", "worker-1");
    record_materialized_member(
        &mut fixture.authority,
        &fixture.persistence,
        &key,
        materialized_row("mob-1", "worker-1", 7, 11),
    )
    .await
    .expect("current member materializes");

    for index in 0..128u64 {
        let input_id = format!("terminal-{index}");
        assert_eq!(
            reserve_pending(&mut fixture, &key, &input_id, 7, 11, index + 1).await,
            TurnOutcomePendingReservationDisposition::Reserved {
                window_start: index + 1
            }
        );
        assert_eq!(
            record_turn_outcome_journal(
                &mut fixture.authority,
                &fixture.persistence,
                &key,
                &outcome_row(&input_id, 7, 11, index + 1),
            )
            .await
            .expect("terminal records"),
            TurnOutcomeRecordDisposition::Recorded
        );
    }
    for index in 0..128u64 {
        let input_id = format!("pending-{index}");
        assert_eq!(
            reserve_pending(&mut fixture, &key, &input_id, 7, 11, 1_000 + index,).await,
            TurnOutcomePendingReservationDisposition::Reserved {
                window_start: 1_000 + index
            }
        );
    }
    assert_eq!(
        reserve_pending(&mut fixture, &key, "overflow", 7, 11, 9_999).await,
        TurnOutcomePendingReservationDisposition::JournalFull
    );

    let durable = fixture
        .persistence
        .load("mob-1")
        .await
        .expect("durable row loads")
        .expect("binding remains");
    assert_eq!(durable.turn_outcomes["worker-1"].len(), 128);
    assert_eq!(durable.turn_outcome_pending["worker-1"].len(), 128);

    let recovered = recover_or_create_binding_authority(&fixture.persistence)
        .await
        .expect("mixed journal state recovers from sqlite");
    assert_eq!(recovered.state().turn_outcome_terminal_seqs.len(), 128);
    assert_eq!(
        recovered.state().turn_outcome_pending_window_starts.len(),
        128
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn ack_tombstones_are_lifecycle_ledger_not_evicting_live_quota() {
    let mut fixture = fixture();
    bind_fixture(&mut fixture, "mob-ack-ledger").await;
    let key = member_key("mob-ack-ledger", "worker-1");
    record_materialized_member(
        &mut fixture.authority,
        &fixture.persistence,
        &key,
        materialized_row("mob-ack-ledger", "worker-1", 7, 11),
    )
    .await
    .expect("member materializes");

    // This is deliberately above the 256 live Pending+terminal bound. ACK
    // tombstones cannot be evicted without reopening delayed duplicate
    // execution; they are reclaimed only with the exact residency lifecycle.
    let mut state = fixture.authority.state().clone();
    for index in 0..300 {
        state.turn_outcome_acknowledged.insert(
            meerkat_mob::machines::mob_host_binding_authority::TurnKey::new(
                AuthorityMobId::from("mob-ack-ledger"),
                AuthorityAgentIdentity::from("worker-1"),
                meerkat_mob::machines::mob_host_binding_authority::Generation(7),
                meerkat_mob::machines::mob_host_binding_authority::FenceToken(11),
                meerkat_mob::machines::mob_host_binding_authority::InputId::from(format!(
                    "ack-{index}"
                )),
            ),
            true,
        );
    }
    let mut recovered = MobHostBindingAuthorityAuthority::recover_from_state(state)
        .expect("300 lifecycle tombstones do not violate the live-row quota");
    let transition = MobHostBindingAuthorityMutator::apply(
        &mut recovered,
        MobHostBindingAuthorityInput::ReserveTurnOutcomePending {
            turn_key: meerkat_mob::machines::mob_host_binding_authority::TurnKey::new(
                AuthorityMobId::from("mob-ack-ledger"),
                AuthorityAgentIdentity::from("worker-1"),
                meerkat_mob::machines::mob_host_binding_authority::Generation(7),
                meerkat_mob::machines::mob_host_binding_authority::FenceToken(11),
                meerkat_mob::machines::mob_host_binding_authority::InputId::from("fresh"),
            ),
            window_start: 1,
        },
    )
    .expect("fresh live row remains admissible");
    assert!(matches!(
        transition.effects(),
        [meerkat_mob::machines::mob_host_binding_authority::MobHostBindingAuthorityEffect::TurnOutcomePendingReserved { .. }]
    ));
    assert_eq!(
        recovered.state().turn_outcome_acknowledged.len(),
        300,
        "no local eviction is legal while delayed delivery remains addressable"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn recovery_folds_durable_rows_and_corrupt_rows_abort_typed() {
    let mut fixture = fixture();
    let supervisor = supervisor_identity("supervisor-a");
    let token = fixture.token.current().to_string();
    let observations = bind_observations(&fixture, "mob-1", &supervisor, 7, &token);
    serve_host_bind(
        &mut fixture.authority,
        &fixture.persistence,
        &mut fixture.token,
        observations,
    )
    .await
    .expect("bind serves");

    // A rebuilt daemon recovers the binding through the generated
    // recover_from_state seam.
    let recovered = recover_or_create_binding_authority(&fixture.persistence)
        .await
        .expect("recovery succeeds");
    let mob = AuthorityMobId::from("mob-1");
    assert_eq!(recovered.state().supervisor_epochs.get(&mob), Some(&7));
    assert_eq!(
        recovered.state().supervisor_peer_ids.get(&mob),
        Some(&AuthorityPeerId::from(supervisor.peer_id.as_str()))
    );

    // A corrupt row aborts recovery typed — never a half-recovered daemon.
    fixture
        .store
        .put_mob_host_binding_if_absent("mob-corrupt", b"not json")
        .await
        .expect("seed corrupt row");
    // `expect_err` needs `Debug` on the Ok type, and the generated
    // `MobHostBindingAuthorityAuthority` deliberately has none — match instead.
    let err = match recover_or_create_binding_authority(&fixture.persistence).await {
        Err(err) => err,
        Ok(_) => panic!("corrupt row must abort recovery"),
    };
    assert!(matches!(err, MobHostActorError::RecordSerde { .. }));
}

#[tokio::test(flavor = "multi_thread")]
async fn persistence_witness_gates_writes_to_the_accepting_transition() {
    let fixture = fixture();
    let mob = AuthorityMobId::from("mob-1");

    // A rejecting transition mints no witness.
    let mut rejecting = MobHostBindingAuthorityAuthority::new();
    let transition = MobHostBindingAuthorityMutator::apply(
        &mut rejecting,
        MobHostBindingAuthorityInput::ResolveHostBind {
            mob_id: mob.clone(),
            supervisor_peer_id: AuthorityPeerId::from("peer-a"),
            supervisor_signing_key: AuthorityPeerSigningKey::from([1u8; 32]),
            epoch: 1,
            binding_generation: 1,
            sender_matches_supervisor: false,
            address_matches: true,
            token_valid: true,
        },
    )
    .expect("reject transition fires");
    assert!(HostBindingPersistenceAuthority::from_transition(&mob, &transition).is_err());

    // An accepting transition mints a witness bound to ITS tuple; a
    // divergent record is refused at the write seam.
    let mut accepting = MobHostBindingAuthorityAuthority::new();
    let transition = MobHostBindingAuthorityMutator::apply(
        &mut accepting,
        MobHostBindingAuthorityInput::ResolveHostBind {
            mob_id: mob.clone(),
            supervisor_peer_id: AuthorityPeerId::from("peer-a"),
            supervisor_signing_key: AuthorityPeerSigningKey::from([1u8; 32]),
            epoch: 1,
            binding_generation: 1,
            sender_matches_supervisor: true,
            address_matches: true,
            token_valid: true,
        },
    )
    .expect("accept transition fires");
    let witness =
        HostBindingPersistenceAuthority::from_transition(&mob, &transition).expect("witness mints");
    let forged = MobHostBindingRecord {
        supervisor_peer_id: "peer-a".to_string(),
        supervisor_signing_key: [1u8; 32],
        epoch: 99,
        binding_generation: 1,
        accepted_capabilities: None,
        materialized: std::collections::BTreeMap::new(),
        released: std::collections::BTreeMap::new(),
        turn_outcome_pending: std::collections::BTreeMap::new(),
        turn_outcomes: std::collections::BTreeMap::new(),
        turn_outcome_acknowledged: std::collections::BTreeMap::new(),
        tracked_input_cancellations: std::collections::BTreeMap::new(),
    };
    let err = fixture
        .persistence
        .put_if_absent("mob-1", &forged, &witness)
        .await
        .expect_err("forged epoch must be refused by the witness");
    assert!(matches!(err, MobHostActorError::Witness { .. }));
}
