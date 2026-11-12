//! Members whose comms runtimes load one transport key.
//!
//! A host on socket comms gives every local member of a realm the same key,
//! and MobMachine admits a peer id for one identity only. These rows pin what
//! the mob does with the second member of such a key:
//!
//! * its spawn is refused in plain text before its `MemberSpawned` row is
//!   written, and the refusal leaves nothing of the member behind, whether
//!   the key's holder is live or dormant;
//! * a journal that already holds such a row (an earlier release wrote it
//!   before MobMachine refused the endpoint) starts again: the row's member is
//!   listed broken with the reason, the key's holder is untouched, the broken
//!   member retires through the ordinary verb, and the next start is clean.

use super::*;
use crate::machines::mob_machine as dsl;
use crate::runtime::MemberRestorePolicy;
use crate::runtime::handle::MobMemberStatus;

const STEP: Duration = Duration::from_mins(1);

/// The seed of the one key both members' sessions load.
const SHARED_KEY: &str = "one-realm-transport-key";

/// What a refusal must not show a caller: the machine's own rejection.
const MACHINE_WORDS: [&str; 3] = ["DSL authority", "guard rejected", "MobMachine"];

fn id(name: &str) -> AgentIdentity {
    AgentIdentity::from(name)
}

/// A fresh turn-driven worker on the session `session`.
fn spec_on(identity: &AgentIdentity, session: &SessionId) -> SpawnMemberSpec {
    let mut spec = SpawnMemberSpec::new("worker", identity.clone());
    spec.runtime_mode = Some(crate::MobRuntimeMode::TurnDriven);
    spec.fresh_session_id = Some(session.clone());
    spec
}

/// A running mob over the mock session service, whose journal and runtime
/// metadata outlive its handle.
struct KeyMob {
    handle: MobHandle,
    service: Arc<MockSessionService>,
    events: Arc<dyn MobEventStore>,
    runtime_metadata: Arc<dyn MobRuntimeMetadataStore>,
}

impl KeyMob {
    async fn new(label: &str) -> Self {
        let service = Arc::new(MockSessionService::new());
        let _adapter = service.enable_runtime_adapter();
        let storage = MobStorage::in_memory();
        let events = storage.events.clone();
        let runtime_metadata = storage.runtime_metadata.clone();
        let handle = MobBuilder::new(with_unique_mob_id(sample_definition(), label), storage)
            .with_session_service(service.clone())
            .create()
            .await
            .expect("create the mob");
        Self {
            handle,
            service,
            events,
            runtime_metadata,
        }
    }

    /// Spawn `identity` on a new session that loads the shared key.
    async fn spawn_on_shared_key(&self, identity: &AgentIdentity) -> Result<SessionId, MobError> {
        let session = SessionId::new();
        self.service
            .set_comms_identity_seed(&session, SHARED_KEY)
            .await;
        tokio::time::timeout(STEP, self.handle.spawn_spec(spec_on(identity, &session)))
            .await
            .expect("the spawn answers")
            .map(|_| session)
    }

    /// The mob's actor goes away and a new handle is reconstructed from the
    /// same journal over the same session service.
    async fn reconstruct(&mut self, policy: Option<MemberRestorePolicy>) -> Result<(), MobError> {
        crash_stop_and_release_routes(self.handle.clone()).await;
        let builder = MobBuilder::for_resume(MobStorage::with_events_and_runtime_metadata(
            self.events.clone(),
            self.runtime_metadata.clone(),
        ))
        .with_session_service(self.service.clone())
        .notify_orchestrator_on_resume(false);
        let builder = match policy {
            Some(policy) => builder.with_member_restore_policy(policy),
            None => builder,
        };
        self.handle = tokio::time::timeout(STEP, Box::pin(builder.resume()))
            .await
            .expect("the start answers")?;
        Ok(())
    }

    /// The identities of the journal's `MemberSpawned` rows, in order.
    async fn spawned_rows(&self) -> Vec<String> {
        self.events
            .replay_all()
            .await
            .expect("read the journal")
            .into_iter()
            .filter_map(|event| match event.kind {
                MobEventKind::MemberSpawned(spawned) => Some(spawned.agent_identity.to_string()),
                _ => None,
            })
            .collect()
    }

    /// The endpoint MobMachine records for `identity`.
    fn recorded_endpoint(&self, identity: &AgentIdentity) -> Option<dsl::MemberPeerEndpoint> {
        self.handle
            .machine_state_watch_rx
            .borrow()
            .member_peer_endpoints
            .get(&dsl::AgentIdentity::from_domain(identity))
            .cloned()
    }

    /// Whether MobMachine holds anything of `identity`: a member, a session
    /// binding, an open spawn or a staged one.
    fn machine_holds(&self, identity: &AgentIdentity) -> bool {
        let identity = dsl::AgentIdentity::from_domain(identity);
        let state = self.handle.machine_state_watch_rx.borrow();
        state.identity_to_runtime.contains_key(&identity)
            || state.member_session_bindings.contains_key(&identity)
            || state.spawn_exec_phase.contains_key(&identity)
            || state.pending_spawn_sessions.contains_key(&identity)
    }

    /// Each listed member with its status and error text.
    async fn listed(&self) -> Vec<(String, MobMemberStatus, Option<String>)> {
        let mut listed = self
            .handle
            .list_members()
            .await
            .into_iter()
            .map(|entry| (entry.agent_identity.to_string(), entry.status, entry.error))
            .collect::<Vec<_>>();
        listed.sort_by(|a, b| a.0.cmp(&b.0));
        listed
    }

    /// Append the row an earlier release wrote for `identity` before
    /// MobMachine refused its endpoint: an unterminated `MemberSpawned` on
    /// `session` whose endpoint carries the peer id and key of `holder`'s.
    async fn append_unterminated_row_with_the_key_of(
        &self,
        identity: &AgentIdentity,
        session: &SessionId,
        holder: &AgentIdentity,
    ) {
        let events = self.events.replay_all().await.expect("read the journal");
        let (holder_endpoint, holder_fence) = events
            .iter()
            .rev()
            .find_map(|event| match &event.kind {
                MobEventKind::MemberSpawned(spawned) if &spawned.agent_identity == holder => {
                    Some((
                        spawned
                            .member_peer_endpoint()
                            .cloned()
                            .expect("the holder's row records its endpoint"),
                        spawned.fence_token,
                    ))
                }
                _ => None,
            })
            .expect("the holder's MemberSpawned row");
        let mob_id = self.handle.mob_id().clone();
        let comms_name = format!("{mob_id}/worker/{identity}");
        let endpoint = TrustedPeerDescriptor {
            peer_id: holder_endpoint.peer_id,
            name: PeerName::new(comms_name.clone()).expect("a peer name"),
            address: meerkat_core::comms::PeerAddress::parse(format!("inproc://{comms_name}"))
                .expect("an address"),
            pubkey: holder_endpoint.pubkey,
        };
        let mut spawned = crate::event::MemberSpawnedEvent::new(
            identity.clone(),
            crate::ids::Generation::INITIAL,
            crate::ids::FenceToken::new(holder_fence.get() + 1),
            crate::ids::AgentRuntimeId::initial(identity.clone()),
            ProfileName::from("worker"),
        )
        .with_bridge_member_ref(Some(crate::event::MemberRef::from_bridge_session_id(
            session.clone(),
        )))
        .with_member_peer_endpoint(Some(endpoint));
        spawned.runtime_mode = crate::MobRuntimeMode::TurnDriven;
        self.events
            .append(NewMobEvent {
                mob_id,
                timestamp: None,
                kind: MobEventKind::MemberSpawned(spawned),
            })
            .await
            .expect("append the row");
    }
}

/// The refusal of the second member of a key: plain text naming both
/// members, never the machine's own rejection.
fn assert_plain_refusal(
    refused: &MobError,
    holder: &AgentIdentity,
    refused_member: &AgentIdentity,
) {
    let MobError::WiringError(text) = refused else {
        panic!("the second member of a key is refused as a wiring error: {refused:?}");
    };
    assert!(
        text.contains(holder.as_str()) && text.contains(refused_member.as_str()),
        "the refusal names both members: {text}"
    );
    for word in MACHINE_WORDS {
        assert!(
            !text.contains(word),
            "the refusal is plain text, not the machine's rejection ({word}): {text}"
        );
    }
}

/// A member whose comms runtime loads the key a live member's endpoint
/// already carries is refused before its `MemberSpawned` row is written: no
/// row, nothing of it in MobMachine or the roster, its session rolled back,
/// the holder's endpoint as it was, and the identity spawnable again on a key
/// of its own.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_member_whose_transport_key_a_live_member_holds_is_refused_before_its_row() {
    let mob = KeyMob::new("shared-key-live").await;
    let (alice, carol) = (id("alice"), id("carol"));
    mob.spawn_on_shared_key(&alice)
        .await
        .expect("the key's first member spawns");
    let alice_endpoint = mob
        .recorded_endpoint(&alice)
        .expect("the first member's endpoint is recorded");
    let rows_before = mob.spawned_rows().await;

    let carol_session = SessionId::new();
    mob.service
        .set_comms_identity_seed(&carol_session, SHARED_KEY)
        .await;
    let refused =
        tokio::time::timeout(STEP, mob.handle.spawn_spec(spec_on(&carol, &carol_session)))
            .await
            .expect("the second spawn answers")
            .expect_err("the second member of the key is refused");
    assert_plain_refusal(&refused, &alice, &carol);

    assert_eq!(
        mob.spawned_rows().await,
        rows_before,
        "no MemberSpawned row is written for the refused member"
    );
    assert!(
        !mob.machine_holds(&carol),
        "MobMachine holds nothing of the refused member"
    );
    assert_eq!(
        mob.listed()
            .await
            .into_iter()
            .map(|(member, status, _)| (member, status))
            .collect::<Vec<_>>(),
        vec![(alice.to_string(), MobMemberStatus::Active)],
        "only the key's holder is listed, active"
    );
    assert!(
        mob.service.test_is_session_archived(&carol_session).await,
        "the refused member's session is rolled back"
    );
    assert_eq!(
        mob.recorded_endpoint(&alice),
        Some(alice_endpoint),
        "the holder keeps its endpoint"
    );

    // The identity is spawnable again, on a key of its own.
    let own_session = SessionId::new();
    tokio::time::timeout(STEP, mob.handle.spawn_spec(spec_on(&carol, &own_session)))
        .await
        .expect("the spawn answers")
        .expect("the refused identity spawns on a key of its own");
    assert_eq!(
        mob.spawned_rows().await,
        vec![alice.to_string(), carol.to_string()]
    );
}

/// The same refusal when the key's holder has no live runtime: a dormant
/// member's recorded endpoint holds the key as a live one's does.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_member_whose_transport_key_a_dormant_member_holds_is_refused_before_its_row() {
    let mut mob = KeyMob::new("shared-key-dormant").await;
    let (alice, carol) = (id("alice"), id("carol"));
    mob.spawn_on_shared_key(&alice)
        .await
        .expect("the key's first member spawns");
    mob.reconstruct(Some(MemberRestorePolicy::OnDemand))
        .await
        .expect("the mob starts again");
    mob.handle.status().await.expect("the mob answers");
    assert_eq!(
        mob.handle.member_materialization_for_test(&alice),
        Some(dsl::MemberMaterializationPhase::Dormant),
        "the lazy start leaves the holder dormant"
    );
    let alice_endpoint = mob
        .recorded_endpoint(&alice)
        .expect("the dormant holder's endpoint is recorded");
    let rows_before = mob.spawned_rows().await;

    let refused = mob
        .spawn_on_shared_key(&carol)
        .await
        .expect_err("the second member of the key is refused");
    assert_plain_refusal(&refused, &alice, &carol);
    assert_eq!(
        mob.spawned_rows().await,
        rows_before,
        "no MemberSpawned row is written for the refused member"
    );
    assert!(
        !mob.machine_holds(&carol),
        "MobMachine holds nothing of the refused member"
    );
    assert_eq!(mob.recorded_endpoint(&alice), Some(alice_endpoint));
    assert_eq!(
        mob.handle.member_materialization_for_test(&alice),
        Some(dsl::MemberMaterializationPhase::Dormant),
        "the refusal wakes nobody"
    );
}

/// A journal that holds an unterminated `MemberSpawned` row whose endpoint
/// carries another member's key (written by a release that judged the
/// endpoint after the row): the mob starts, the row's member is listed broken
/// with a reason that names the holder and says what to do, the holder is as
/// a start leaves it, the broken member retires through the ordinary verb,
/// and the next start is clean.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_journal_with_a_row_of_another_members_key_starts_and_the_row_is_cleanable() {
    let mut mob = KeyMob::new("shared-key-journal").await;
    let (alice, carol) = (id("alice"), id("carol"));
    mob.spawn_on_shared_key(&alice)
        .await
        .expect("the key's first member spawns");
    let alice_endpoint = mob
        .recorded_endpoint(&alice)
        .expect("the first member's endpoint is recorded");
    let carol_session = SessionId::new();
    mob.service
        .replace_live_session(Session::with_id(carol_session.clone()))
        .await;
    mob.append_unterminated_row_with_the_key_of(&carol, &carol_session, &alice)
        .await;

    mob.reconstruct(None)
        .await
        .expect("the mob starts on a journal that holds the row");
    mob.handle.status().await.expect("the mob answers");

    let listed = mob.listed().await;
    assert_eq!(
        listed
            .iter()
            .map(|(member, status, _)| (member.as_str(), *status))
            .collect::<Vec<_>>(),
        vec![
            ("alice", MobMemberStatus::Active),
            ("carol", MobMemberStatus::Broken)
        ],
        "the holder is active and the row's member broken: {listed:?}"
    );
    let reason = listed[1].2.clone().expect("the broken member has a reason");
    assert!(
        reason.contains("alice") && reason.contains("carol") && reason.contains("retire"),
        "the reason names both members and the way out: {reason}"
    );
    for word in MACHINE_WORDS {
        assert!(!reason.contains(word), "the reason is plain text: {reason}");
    }
    assert_eq!(
        mob.recorded_endpoint(&alice),
        Some(alice_endpoint.clone()),
        "the holder keeps its endpoint"
    );
    assert_eq!(
        mob.recorded_endpoint(&carol),
        None,
        "the row's member holds no endpoint"
    );

    // The way out is the ordinary verb.
    tokio::time::timeout(STEP, mob.handle.retire(carol.clone()))
        .await
        .expect("the retire answers")
        .expect("the broken member retires");
    assert_eq!(
        mob.listed()
            .await
            .into_iter()
            .map(|(member, status, _)| (member, status))
            .collect::<Vec<_>>(),
        vec![(alice.to_string(), MobMemberStatus::Active)]
    );
    assert!(
        !mob.machine_holds(&carol),
        "the retired member is gone from MobMachine"
    );

    // And the next start is clean.
    mob.reconstruct(None)
        .await
        .expect("the mob starts again after the cleanup");
    mob.handle.status().await.expect("the mob answers");
    assert_eq!(
        mob.listed().await,
        vec![(alice.to_string(), MobMemberStatus::Active, None)],
        "only the holder is left, with no error"
    );
    assert_eq!(mob.recorded_endpoint(&alice), Some(alice_endpoint));
}
