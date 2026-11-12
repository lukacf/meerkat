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
use crate::runtime::state::MobCommand;

const STEP: Duration = Duration::from_mins(1);

/// How long, in milliseconds, a read of a member's comms runtime is held
/// while spawns are brought to one point (see
/// [`KeyMob::hold_comms_runtime_reads`]).
const OBSERVATION_HOLD_MS: u64 = 500;

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

    /// The phase of `identity`'s spawn ladder in MobMachine.
    fn spawn_phase(&self, identity: &AgentIdentity) -> Option<dsl::SpawnExecPhase> {
        self.handle
            .machine_state_watch_rx
            .borrow()
            .spawn_exec_phase
            .get(&dsl::AgentIdentity::from_domain(identity))
            .cloned()
    }

    /// Hold every read of a member's comms runtime for
    /// [`OBSERVATION_HOLD_MS`]: a spawn's endpoint observation (off the
    /// actor) starts with one.
    fn hold_comms_runtime_reads(&self) {
        self.service
            .set_comms_runtime_delay_for_next_calls(u64::MAX, OBSERVATION_HOLD_MS);
    }

    /// Hold no further read, and wait until the reads held now have ended
    /// and the observations they belong to have finished.
    async fn release_comms_runtime_reads_and_wait(&self) {
        self.service.set_comms_runtime_delay_for_next_calls(0, 0);
        tokio::time::sleep(Duration::from_millis(OBSERVATION_HOLD_MS * 4)).await;
    }

    /// Park the actor: it takes no command and commits no off-actor result
    /// until the park is released.
    async fn park_actor(&self) -> ActorPark {
        self.queue_actor_park().await.entered().await
    }

    /// Queue a park of the actor behind the commands already queued for it.
    async fn queue_actor_park(&self) -> QueuedActorPark {
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let ended = self
            .handle
            .enqueue_actor_command_for_test(|reply_tx| MobCommand::ParkActorForObservationTest {
                entered_tx,
                release_rx,
                reply_tx,
            })
            .await
            .expect("queue the park");
        QueuedActorPark {
            entered_rx,
            park: ActorPark { release_tx, ended },
        }
    }

    /// The inproc namespace the mob publishes its members' wake routes in.
    fn wake_namespace(&self) -> String {
        meerkat_core::mob_realm_id(self.handle.mob_id().as_str())
            .expect("the mob id is a realm identity")
            .as_str()
            .to_string()
    }

    /// How many incarnations have their key's wake route held back for a
    /// spawn activation.
    async fn key_routes_held_back(&self) -> usize {
        self.handle
            .enqueue_actor_command_for_test(|reply_tx| MobCommand::SpawnActivationCustodyProbe {
                reply_tx,
            })
            .await
            .expect("queue the census")
            .await
            .expect("the census answers")
            .key_routes_held_back
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

/// A park queued for the actor (see [`KeyMob::queue_actor_park`]).
struct QueuedActorPark {
    entered_rx: tokio::sync::oneshot::Receiver<()>,
    park: ActorPark,
}

impl QueuedActorPark {
    /// Wait until the actor reached the park.
    async fn entered(self) -> ActorPark {
        tokio::time::timeout(STEP, self.entered_rx)
            .await
            .expect("the actor reaches the park")
            .expect("the actor parks");
        self.park
    }
}

/// A parked actor (see [`KeyMob::park_actor`]).
struct ActorPark {
    release_tx: tokio::sync::oneshot::Sender<()>,
    ended: tokio::sync::oneshot::Receiver<Result<(), MobError>>,
}

impl ActorPark {
    /// Let the actor go on, and wait until it left the park.
    async fn release(self) {
        self.release_tx.send(()).expect("the actor is still parked");
        tokio::time::timeout(STEP, self.ended)
            .await
            .expect("the park answers")
            .expect("the actor answers the park")
            .expect("the park ends");
    }
}

/// What a caller reads about a member whose key another member holds (a
/// spawn refusal, a broken member's reason): plain text that names that
/// member alone. The caller may not be allowed to see the key's holder, so
/// the text carries neither the holder's name nor the key's peer id, and
/// never the machine's own rejection.
fn assert_caller_text(
    text: &str,
    named: &AgentIdentity,
    holder: &AgentIdentity,
    holder_endpoint: &dsl::MemberPeerEndpoint,
) {
    assert!(
        text.contains(named.as_str()) && text.contains("comms key"),
        "the text names the member and its comms key: {text}"
    );
    assert!(
        !text.contains(holder.as_str()),
        "the text does not name the key's holder: {text}"
    );
    assert!(
        !text.contains(holder_endpoint.peer_id.0.as_str()),
        "the text does not carry the key's peer id: {text}"
    );
    for word in MACHINE_WORDS {
        assert!(
            !text.contains(word),
            "the text is plain, not the machine's rejection ({word}): {text}"
        );
    }
}

/// The refusal of the second member of a key: a wiring error whose text is
/// what [`assert_caller_text`] allows.
fn assert_plain_refusal(
    refused: &MobError,
    holder: &AgentIdentity,
    refused_member: &AgentIdentity,
    holder_endpoint: &dsl::MemberPeerEndpoint,
) {
    let MobError::WiringError(text) = refused else {
        panic!("the second member of a key is refused as a wiring error: {refused:?}");
    };
    assert_caller_text(text, refused_member, holder, holder_endpoint);
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
    assert_plain_refusal(&refused, &alice, &carol, &alice_endpoint);

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
    assert_plain_refusal(&refused, &alice, &carol, &alice_endpoint);
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

/// Two members whose comms runtimes load one key, admitted back to back (the
/// parallel shape of `spawn_many`): MobMachine holds the first one's endpoint
/// from the step that commits its membership, so the one admitted second is
/// refused before its `MemberSpawned` row as a member spawned later is. One
/// member is admitted, the other is refused as a wiring error with nothing of
/// it left behind, and a second start of the mob is clean.
///
/// The two admissions are brought back to back, in either order: both
/// endpoint observations run off the actor while the actor is parked, so both
/// members are admitted before the first one's activation takes another actor
/// turn.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_members_of_one_key_admitted_back_to_back_admit_one_and_refuse_the_other_before_its_row()
 {
    let mut mob = KeyMob::new("shared-key-in-flight").await;
    let members = [
        (id("alice"), SessionId::new()),
        (id("carol"), SessionId::new()),
    ];
    for (_, session) in &members {
        mob.service
            .set_comms_identity_seed(session, SHARED_KEY)
            .await;
    }
    mob.hold_comms_runtime_reads();
    let spawns = members
        .iter()
        .map(|(identity, session)| {
            let handle = mob.handle.clone();
            let spec = spec_on(identity, session);
            tokio::spawn(async move { handle.spawn_spec(spec).await.map(|_| ()) })
        })
        .collect::<Vec<_>>();
    let both_ladders_open = |mob: &KeyMob| {
        members
            .iter()
            .all(|(identity, _)| mob.spawn_phase(identity) == Some(dsl::SpawnExecPhase::Opened))
    };
    tokio::time::timeout(STEP, async {
        while !both_ladders_open(&mob) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("both spawns open their ladders while their endpoints are observed");
    let park = mob.park_actor().await;
    assert!(
        both_ladders_open(&mob),
        "the actor is parked before either member is admitted"
    );
    mob.release_comms_runtime_reads_and_wait().await;
    park.release().await;

    let mut admitted = Vec::new();
    let mut refused = Vec::new();
    for ((identity, session), spawn) in members.iter().zip(spawns) {
        match tokio::time::timeout(STEP, spawn)
            .await
            .expect("the spawn answers")
            .expect("the spawn task ends")
        {
            Ok(()) => admitted.push(identity.clone()),
            Err(error) => refused.push((identity.clone(), session.clone(), error)),
        }
    }
    let rows = mob.spawned_rows().await;
    assert!(
        admitted.len() == 1 && refused.len() == 1,
        "one member of the key is admitted and the other refused: admitted={admitted:?} refused={refused:?} rows={rows:?}"
    );
    let winner = &admitted[0];
    let (loser, loser_session, refusal) = &refused[0];
    assert!(
        matches!(refusal, MobError::WiringError(_)) && rows == vec![winner.to_string()],
        "the member admitted second is refused before its row, as a wiring error: rows={rows:?} refusal={refusal:?}"
    );
    let winner_endpoint = mob
        .recorded_endpoint(winner)
        .expect("the admitted member holds the key's endpoint");
    assert_plain_refusal(refusal, winner, loser, &winner_endpoint);
    assert!(
        !mob.machine_holds(loser),
        "MobMachine holds nothing of the refused member"
    );
    assert!(
        mob.recorded_endpoint(winner).is_some() && mob.recorded_endpoint(loser).is_none(),
        "the admitted member alone holds the key's endpoint"
    );
    assert_eq!(
        mob.listed()
            .await
            .into_iter()
            .map(|(member, status, _)| (member, status))
            .collect::<Vec<_>>(),
        vec![(winner.to_string(), MobMemberStatus::Active)],
        "only the admitted member is listed, active"
    );
    assert!(
        mob.service.test_is_session_archived(loser_session).await,
        "the refused member's session is rolled back"
    );
    assert_eq!(
        mob.key_routes_held_back().await,
        0,
        "no key route is held back after the two spawns"
    );

    // A second start is clean: one member, not broken.
    mob.reconstruct(None).await.expect("the mob starts again");
    mob.handle.status().await.expect("the mob answers");
    assert_eq!(
        mob.listed().await,
        vec![(winner.to_string(), MobMemberStatus::Active, None)],
        "the second start lists the admitted member alone, with no error"
    );
}

/// Inside a spawn's window (its membership is committed, its activation has
/// not published its trusted peer yet) MobMachine holds the member's
/// endpoint, and the key has no wake route: a peer send to the key is
/// answered from the member's live route alone (its inbox while the route
/// stands, peer not found when it does not), as it was when the activation
/// itself registered the endpoint. The key's wake route appears once the
/// activation published the trusted peer, and nothing is held back after the
/// spawn.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_spawning_members_key_has_no_wake_route_until_its_activation_publishes_its_peer() {
    let mob = KeyMob::new("shared-key-window").await;
    let alice = id("alice");
    let session = SessionId::new();
    mob.service
        .set_comms_identity_seed(&session, SHARED_KEY)
        .await;
    mob.hold_comms_runtime_reads();
    let spawn = {
        let handle = mob.handle.clone();
        let spec = spec_on(&alice, &session);
        tokio::spawn(async move { handle.spawn_spec(spec).await.map(|_| ()) })
    };
    tokio::time::timeout(STEP, async {
        while mob.spawn_phase(&alice) != Some(dsl::SpawnExecPhase::Opened) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the spawn opens its ladder while its endpoint is observed");
    let before_admission = mob.park_actor().await;
    assert_eq!(
        mob.spawn_phase(&alice),
        Some(dsl::SpawnExecPhase::Opened),
        "the actor is parked before the member is admitted"
    );
    // Queued behind the first park and ahead of everything the admission
    // dispatches: the actor reaches it right after the admission's turn,
    // before the activation's first stage is committed.
    let in_window = mob.queue_actor_park().await;
    mob.release_comms_runtime_reads_and_wait().await;
    before_admission.release().await;
    let in_window = in_window.entered().await;

    assert_eq!(
        mob.spawn_phase(&alice),
        Some(dsl::SpawnExecPhase::MembershipCommitted),
        "the member is admitted and its activation has not ended"
    );
    let endpoint = mob
        .recorded_endpoint(&alice)
        .expect("MobMachine holds the member's endpoint from its membership commit");
    let key = meerkat_comms::PubKey(endpoint.signing_key.0);
    let namespace = mob.wake_namespace();
    let registry = meerkat_comms::InprocRegistry::global();
    assert!(
        !registry.has_wake_route_in_namespace(&namespace, &key),
        "the key has no wake route before the activation published the member's trusted peer"
    );

    in_window.release().await;
    tokio::time::timeout(STEP, spawn)
        .await
        .expect("the spawn answers")
        .expect("the spawn task ends")
        .expect("the member spawns");
    assert!(
        registry.has_wake_route_in_namespace(&namespace, &key),
        "the activation published the key's wake route"
    );
    assert_eq!(
        mob.key_routes_held_back().await,
        0,
        "no key route is held back after the spawn"
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
        reason.contains("retire"),
        "the reason says the way out: {reason}"
    );
    assert_caller_text(&reason, &carol, &alice, &alice_endpoint);
    // The read the layer above decides a key's holder from after a restart:
    // the broken member carries no comms identity.
    let tolerated = mob
        .handle
        .get_member(&carol)
        .await
        .expect("the roster answers")
        .expect("the broken member is a member");
    assert!(
        tolerated.peer_id().is_none() && tolerated.transport_public_key().is_none(),
        "the broken member's roster entry carries no comms identity"
    );
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
