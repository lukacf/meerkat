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
//!   member retires through the ordinary verb, and the next start is clean;
//!   the same holds for the journal of a mob that was stopped before its host
//!   went away, which starts stopped and lifts;
//! * a member whose recovered session binding comes up, at a start, on the
//!   key another member holds is listed broken alone, and the mob starts;
//! * while a spawn's activation has not published the member's trusted peer,
//!   the member's key has no wake route, whether the activation then ends in
//!   a member, in a rollback, or meets a retire on the way.
//!
//! One row pins a known limitation: the runtime of a refused member is built
//! before MobMachine judges its endpoint, and building it takes the in-process
//! route of the key from the member that holds it.

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
        Self::with_definition(sample_definition(), label).await
    }

    /// A mob whose workers are wired to each other, so a spawn's activation
    /// starts the new member's peer ingress before it wires it.
    async fn wired(label: &str) -> Self {
        let mut definition = sample_definition();
        definition.wiring.role_wiring = vec![RoleWiringRule {
            a: ProfileName::from("worker"),
            b: ProfileName::from("worker"),
        }];
        Self::with_definition(definition, label).await
    }

    async fn with_definition(definition: MobDefinition, label: &str) -> Self {
        let service = Arc::new(MockSessionService::new());
        let _adapter = service.enable_runtime_adapter();
        let storage = MobStorage::in_memory();
        let events = storage.events.clone();
        let runtime_metadata = storage.runtime_metadata.clone();
        let handle = MobBuilder::new(with_unique_mob_id(definition, label), storage)
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

    /// Spawn `identity` on a new session that loads a key of its own.
    async fn spawn_on_own_key(&self, identity: &AgentIdentity) -> SessionId {
        let session = SessionId::new();
        tokio::time::timeout(STEP, self.handle.spawn_spec(spec_on(identity, &session)))
            .await
            .expect("the spawn answers")
            .expect("a member on a key of its own spawns");
        session
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

    /// The host process goes away and comes back: the mob's actor ends, the
    /// session service restarts over its durable state with no live session,
    /// and the start restores every member. Each session in `on_shared_key`
    /// comes up on the shared key, as every local member of a realm on
    /// socket comms does.
    async fn restart_host(&mut self, on_shared_key: &[&SessionId]) -> Result<(), MobError> {
        let restarted = Arc::new(self.service.cold_restart_preserving_durable_state().await);
        let _adapter = restarted.enable_runtime_adapter();
        crash_stop_and_release_routes(self.handle.clone()).await;
        for session in on_shared_key {
            restarted.set_comms_identity_seed(session, SHARED_KEY).await;
        }
        self.service = restarted;
        let builder = MobBuilder::for_resume(MobStorage::with_events_and_runtime_metadata(
            self.events.clone(),
            self.runtime_metadata.clone(),
        ))
        .with_session_service(self.service.clone())
        .notify_orchestrator_on_resume(false)
        .with_member_restore_policy(MemberRestorePolicy::Eager);
        self.handle = tokio::time::timeout(STEP, Box::pin(builder.resume()))
            .await
            .expect("the start answers")?;
        Ok(())
    }

    /// Start a spawn of `identity` on `session`, which loads the shared key,
    /// and park the actor right before it admits the member: the spawn's
    /// ladder is open and its endpoint observation is held.
    async fn spawn_parked_before_admission(
        &self,
        identity: &AgentIdentity,
        session: &SessionId,
    ) -> (tokio::task::JoinHandle<Result<(), MobError>>, ActorPark) {
        self.service
            .set_comms_identity_seed(session, SHARED_KEY)
            .await;
        self.hold_comms_runtime_reads();
        let spawn = {
            let handle = self.handle.clone();
            let spec = spec_on(identity, session);
            tokio::spawn(async move { handle.spawn_spec(spec).await.map(|_| ()) })
        };
        tokio::time::timeout(STEP, async {
            while self.spawn_phase(identity) != Some(dsl::SpawnExecPhase::Opened) {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the spawn opens its ladder while its endpoint is observed");
        let before_admission = self.park_actor().await;
        assert_eq!(
            self.spawn_phase(identity),
            Some(dsl::SpawnExecPhase::Opened),
            "the actor is parked before the member is admitted"
        );
        (spawn, before_admission)
    }

    /// Let the admission parked by [`Self::spawn_parked_before_admission`]
    /// run, and stop the actor at `in_window`, a park queued behind it: the
    /// actor reaches it right after the admission's turn, before the
    /// activation's first stage is committed. Returns the member's endpoint
    /// key, which MobMachine holds from the admission, after checking that
    /// the key has no wake route.
    async fn enter_spawn_window(
        &self,
        identity: &AgentIdentity,
        before_admission: ActorPark,
        in_window: QueuedActorPark,
    ) -> (ActorPark, meerkat_comms::PubKey) {
        self.release_comms_runtime_reads_and_wait().await;
        before_admission.release().await;
        let in_window = in_window.entered().await;
        assert_eq!(
            self.spawn_phase(identity),
            Some(dsl::SpawnExecPhase::MembershipCommitted),
            "the member is admitted and its activation has not ended"
        );
        let key = self
            .endpoint_key(identity)
            .expect("MobMachine holds the member's endpoint from its membership commit");
        assert!(
            !self.has_key_wake_route(&key),
            "the key has no wake route before the activation published the member's trusted peer"
        );
        (in_window, key)
    }

    /// The transport key of the endpoint MobMachine records for `identity`.
    fn endpoint_key(&self, identity: &AgentIdentity) -> Option<meerkat_comms::PubKey> {
        self.recorded_endpoint(identity)
            .map(|endpoint| meerkat_comms::PubKey(endpoint.signing_key.0))
    }

    /// Whether `key` has a wake route in the mob's namespace.
    fn has_key_wake_route(&self, key: &meerkat_comms::PubKey) -> bool {
        meerkat_comms::InprocRegistry::global()
            .has_wake_route_in_namespace(&self.wake_namespace(), key)
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
    let (spawn, before_admission) = mob.spawn_parked_before_admission(&alice, &session).await;
    let in_window = mob.queue_actor_park().await;
    let (in_window, key) = mob
        .enter_spawn_window(&alice, before_admission, in_window)
        .await;

    in_window.release().await;
    tokio::time::timeout(STEP, spawn)
        .await
        .expect("the spawn answers")
        .expect("the spawn task ends")
        .expect("the member spawns");
    assert!(
        mob.has_key_wake_route(&key),
        "the activation published the key's wake route"
    );
    assert_eq!(
        mob.key_routes_held_back().await,
        0,
        "no key route is held back after the spawn"
    );
}

/// A spawn that fails after its window is rolled back with nothing held
/// back: inside the window the member's key has no wake route; the
/// activation then publishes the member's trusted peer, cannot start the
/// member's peer ingress and rolls the spawn back. Afterwards no key route
/// is held back, MobMachine holds nothing of the member, its key has no wake
/// route and is free for another member, and the wake route of the member
/// that was there before is as it was.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_spawn_rolled_back_after_its_window_holds_back_no_key_route() {
    let mob = KeyMob::wired("shared-key-window-rollback").await;
    let (bob, alice, dave) = (id("bob"), id("alice"), id("dave"));
    mob.spawn_on_own_key(&bob).await;
    let bob_endpoint = mob
        .recorded_endpoint(&bob)
        .expect("the earlier member's endpoint is recorded");
    let bob_key = mob.endpoint_key(&bob).expect("the earlier member's key");
    assert!(
        mob.has_key_wake_route(&bob_key),
        "the earlier member's key has its wake route"
    );

    let session = SessionId::new();
    let (spawn, before_admission) = mob.spawn_parked_before_admission(&alice, &session).await;
    let in_window = mob.queue_actor_park().await;
    let (in_window, key) = mob
        .enter_spawn_window(&alice, before_admission, in_window)
        .await;
    assert_ne!(key, bob_key, "the two members hold different keys");

    // From here no member's comms runtime can be read: the peer ingress the
    // new member's wiring needs cannot start, which rolls the spawn back.
    mob.service.set_comms_runtime_missing_after_successes(0);
    in_window.release().await;
    let failure = tokio::time::timeout(STEP, spawn)
        .await
        .expect("the spawn answers")
        .expect("the spawn task ends")
        .expect_err("a member whose peer ingress cannot start does not spawn");
    mob.service
        .set_comms_runtime_missing_after_successes(usize::MAX);

    assert_eq!(
        mob.key_routes_held_back().await,
        0,
        "no key route is held back after the failed spawn: {failure}"
    );
    assert!(
        !mob.machine_holds(&alice) && mob.recorded_endpoint(&alice).is_none(),
        "MobMachine holds nothing of the rolled back member: {failure}"
    );
    assert!(
        !mob.has_key_wake_route(&key),
        "the rolled back member's key has no wake route"
    );
    assert_eq!(
        mob.recorded_endpoint(&bob),
        Some(bob_endpoint),
        "the earlier member keeps its endpoint"
    );
    assert!(
        mob.has_key_wake_route(&bob_key),
        "the earlier member's key keeps its wake route"
    );
    assert_eq!(
        mob.listed()
            .await
            .into_iter()
            .map(|(member, status, _)| (member, status))
            .collect::<Vec<_>>(),
        vec![(bob.to_string(), MobMemberStatus::Active)],
        "only the earlier member is listed, active"
    );

    // The key is free: another member spawns on it and holds its route.
    mob.spawn_on_shared_key(&dave)
        .await
        .expect("another member spawns on the key the rolled back member carried");
    assert_eq!(mob.endpoint_key(&dave), Some(key));
    assert!(
        mob.has_key_wake_route(&key),
        "the key's wake route is the new member's"
    );
    assert_eq!(mob.key_routes_held_back().await, 0);
}

/// A retire that reaches the actor inside a spawn's window retires nothing
/// and holds back nothing. The spawning member is seated in the roster only
/// by its activation, so a caller's `retire` finds no incarnation there and
/// asks MobMachine about an absent identity; MobMachine still maps the
/// identity and refuses, which the caller reads as the spawn still in
/// progress. The key has no wake route in the window, the spawn then
/// completes and publishes it, no key route is held back, and the wake route
/// of the member that was there before is as it was.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_retire_inside_a_spawns_window_retires_nothing_and_holds_back_no_key_route() {
    let mob = KeyMob::new("shared-key-window-retire").await;
    let (bob, alice) = (id("bob"), id("alice"));
    mob.spawn_on_own_key(&bob).await;
    let bob_endpoint = mob
        .recorded_endpoint(&bob)
        .expect("the earlier member's endpoint is recorded");
    let bob_key = mob.endpoint_key(&bob).expect("the earlier member's key");

    let session = SessionId::new();
    let (spawn, before_admission) = mob.spawn_parked_before_admission(&alice, &session).await;
    let in_window = mob.queue_actor_park().await;
    // Queued behind the window's park and ahead of the activation's first
    // stage result: the retire is the actor's first command in the window.
    let retire = mob
        .handle
        .enqueue_actor_command_for_test(|reply_tx| MobCommand::RetireAbsent {
            agent_identity: alice.clone(),
            reply_tx,
        })
        .await
        .expect("queue the retire");
    let (in_window, key) = mob
        .enter_spawn_window(&alice, before_admission, in_window)
        .await;
    assert!(
        mob.handle
            .get_member(&alice)
            .await
            .expect("the roster answers")
            .is_none(),
        "the spawning member is not seated in the roster inside its window"
    );

    in_window.release().await;
    let refusal = tokio::time::timeout(STEP, retire)
        .await
        .expect("the retire answers")
        .expect("the actor answers the retire")
        .expect_err("MobMachine refuses the retire of an identity whose spawn is running");
    assert!(
        matches!(
            crate::runtime::handle::retire_absent_refusal(&alice, refusal),
            MobError::LifecycleOperationPending { .. }
        ),
        "the caller reads the spawn as still in progress"
    );
    tokio::time::timeout(STEP, spawn)
        .await
        .expect("the spawn answers")
        .expect("the spawn task ends")
        .expect("the member spawns");

    assert!(
        mob.has_key_wake_route(&key),
        "the activation published the key's wake route"
    );
    assert_eq!(
        mob.key_routes_held_back().await,
        0,
        "no key route is held back after the spawn"
    );
    assert_eq!(
        mob.recorded_endpoint(&bob),
        Some(bob_endpoint),
        "the earlier member keeps its endpoint"
    );
    assert!(
        mob.has_key_wake_route(&bob_key),
        "the earlier member's key keeps its wake route"
    );
    assert_eq!(
        mob.listed()
            .await
            .into_iter()
            .map(|(member, status, _)| (member, status))
            .collect::<Vec<_>>(),
        vec![
            (alice.to_string(), MobMemberStatus::Active),
            (bob.to_string(), MobMemberStatus::Active)
        ],
        "both members are listed, active"
    );
}

/// A member whose session binding was recovered without an endpoint owes the
/// next start its endpoint. When its session comes up on the transport key
/// another member holds, the start records that member broken alone, with
/// the reason a caller may read, and goes on: the key's holder keeps its
/// endpoint and its wake route, the broken member carries no comms identity,
/// a second start answers the same, the broken member retires through the
/// ordinary verb, and the start after that is clean.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_start_breaks_only_the_member_whose_recovered_binding_comes_up_on_another_members_key() {
    let mut mob = KeyMob::new("shared-key-recovered-binding").await;
    let (alice, carol) = (id("alice"), id("carol"));
    let alice_session = mob
        .spawn_on_shared_key(&alice)
        .await
        .expect("the key's member spawns");
    let carol_session = mob.spawn_on_own_key(&carol).await;
    let alice_endpoint = mob
        .recorded_endpoint(&alice)
        .expect("the holder's endpoint is recorded");
    let alice_key = mob.endpoint_key(&alice).expect("the holder's key");
    let alice_peer_id = mob
        .handle
        .get_member(&alice)
        .await
        .expect("the roster answers")
        .and_then(|entry| entry.peer_id())
        .expect("the holder's peer id");
    let carol_runtime_id = mob
        .handle
        .get_member(&carol)
        .await
        .expect("the roster answers")
        .expect("the second member is a member")
        .agent_runtime_id
        .clone();
    // A binding recovered without its endpoint, as an earlier release
    // recorded it: the next start registers the endpoint the session comes
    // up with.
    mob.events
        .append(NewMobEvent {
            mob_id: mob.handle.mob_id().clone(),
            timestamp: None,
            kind: MobEventKind::MemberSessionBindingRecovered(
                crate::event::MemberSessionBindingRecoveredEvent::new(
                    carol.clone(),
                    carol_runtime_id,
                    carol_session.clone(),
                ),
            ),
        })
        .await
        .expect("append the recovered binding");

    for start in ["the first start", "a second start with nothing cleaned"] {
        mob.restart_host(&[&alice_session, &carol_session])
            .await
            .unwrap_or_else(|error| {
                panic!("{start}: one member's refused endpoint does not fail the start: {error}")
            });
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
            "{start}: the holder is active and the other member broken: {listed:?}"
        );
        let reason = listed[1].2.clone().expect("the broken member has a reason");
        assert!(
            reason.contains("retire"),
            "{start}: the reason says the way out: {reason}"
        );
        assert_caller_text(&reason, &carol, &alice, &alice_endpoint);
        let broken = mob
            .handle
            .get_member(&carol)
            .await
            .expect("the roster answers")
            .expect("the broken member is a member");
        assert!(
            broken.peer_id().is_none() && broken.transport_public_key().is_none(),
            "{start}: the broken member's roster entry carries no comms identity"
        );
        assert_ne!(
            mob.recorded_endpoint(&carol)
                .map(|endpoint| endpoint.peer_id),
            Some(alice_endpoint.peer_id.clone()),
            "{start}: MobMachine records the key for its holder alone"
        );
        assert_eq!(
            mob.recorded_endpoint(&alice),
            Some(alice_endpoint.clone()),
            "{start}: the holder keeps its endpoint"
        );
        assert_eq!(
            mob.handle
                .get_member(&alice)
                .await
                .expect("the roster answers")
                .and_then(|entry| entry.peer_id()),
            Some(alice_peer_id),
            "{start}: the holder keeps its peer id"
        );
        assert!(
            mob.has_key_wake_route(&alice_key),
            "{start}: the key's wake route is published for its holder"
        );
    }

    // The way out is the ordinary verb, and the start after it is clean.
    tokio::time::timeout(STEP, mob.handle.retire(carol.clone()))
        .await
        .expect("the retire answers")
        .expect("the broken member retires");
    mob.restart_host(&[&alice_session])
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

/// Known limitation, pinned. A refused member's comms runtime is built before
/// MobMachine judges its endpoint (the key is read from the built runtime),
/// and a runtime built on a key takes the key's in-process route from the
/// runtime that held it. The refusal rolls the refused member back, which
/// removes its own route and puts nothing back: the member that holds the
/// key is not reachable in process until the mob starts again. MobMachine
/// and the caller's answer are as for any refused member.
///
/// When the route is kept or restored, this row's last assertion is the one
/// that changes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refused_member_leaves_the_keys_holder_without_its_in_process_route() {
    let (handle, service) = create_test_mob_with_real_comms(with_unique_mob_id(
        sample_definition(),
        "shared-key-in-process-route",
    ))
    .await;
    let (alice, carol) = (id("alice"), id("carol"));
    let keypair = meerkat_comms::Keypair::generate();
    let key = keypair.public_key();
    let (alice_session, carol_session) = (SessionId::new(), SessionId::new());
    {
        let mut keypairs = service.session_keypairs.write().await;
        keypairs.insert(alice_session.clone(), keypair.clone());
        keypairs.insert(carol_session.clone(), keypair);
    }
    tokio::time::timeout(STEP, handle.spawn_spec(spec_on(&alice, &alice_session)))
        .await
        .expect("the spawn answers")
        .expect("the key's first member spawns");
    let registry = meerkat_comms::InprocRegistry::global();
    let holder_route = registry
        .get_name_by_pubkey(&key)
        .expect("the holder is reachable in process under its key");
    assert!(
        holder_route.ends_with("/alice"),
        "the key's in-process route is the holder's: {holder_route}"
    );
    let holder_endpoint = handle
        .machine_state_watch_rx
        .borrow()
        .member_peer_endpoints
        .get(&dsl::AgentIdentity::from_domain(&alice))
        .cloned()
        .expect("the holder's endpoint is recorded");

    let refused = tokio::time::timeout(STEP, handle.spawn_spec(spec_on(&carol, &carol_session)))
        .await
        .expect("the second spawn answers")
        .expect_err("the second member of the key is refused");
    assert_plain_refusal(&refused, &alice, &carol, &holder_endpoint);
    assert_eq!(
        handle
            .machine_state_watch_rx
            .borrow()
            .member_peer_endpoints
            .get(&dsl::AgentIdentity::from_domain(&alice))
            .cloned(),
        Some(holder_endpoint),
        "the holder keeps its endpoint in MobMachine"
    );

    // The limitation: the key has no in-process route any more.
    assert_eq!(
        registry.get_name_by_pubkey(&key),
        None,
        "the refused member's build took the key's in-process route and its rollback removed it"
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

/// The same journal, of a mob that was stopped before its host went away, as
/// every mob is that an upgrade finds: the start records the row's member
/// broken while its seeded MobMachine is no longer Running, and must not fail
/// on it. The mob starts stopped, twice in a row, with the row's member
/// listed broken and the holder untouched; it lifts; the broken member
/// retires through the ordinary verb, and the next start is clean.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stopped_mobs_journal_with_a_row_of_another_members_key_starts_and_lifts() {
    let mut mob = KeyMob::new("shared-key-stopped-journal").await;
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
    tokio::time::timeout(STEP, mob.handle.stop())
        .await
        .expect("the stop answers")
        .expect("the mob stops");

    for start in ["the first start", "the second start"] {
        mob.reconstruct(None)
            .await
            .unwrap_or_else(|error| panic!("{start} of the stopped mob: {error}"));
        assert_eq!(
            mob.handle.status().await.expect("the mob answers"),
            MobState::Stopped,
            "{start}: the mob starts as it was left, stopped"
        );
        let listed = mob.listed().await;
        assert_eq!(
            listed
                .iter()
                .map(|(member, _, _)| member.as_str())
                .collect::<Vec<_>>(),
            vec!["alice", "carol"],
            "{start}: both members are listed: {listed:?}"
        );
        assert!(
            listed[0].1 != MobMemberStatus::Broken && listed[0].2.is_none(),
            "{start}: the holder is not broken: {listed:?}"
        );
        assert_eq!(
            listed[1].1,
            MobMemberStatus::Broken,
            "{start}: the row's member is broken: {listed:?}"
        );
        let reason = listed[1].2.clone().expect("the broken member has a reason");
        assert!(
            reason.contains("retire"),
            "{start}: the reason says the way out: {reason}"
        );
        assert_caller_text(&reason, &carol, &alice, &alice_endpoint);
        assert_eq!(
            mob.recorded_endpoint(&alice),
            Some(alice_endpoint.clone()),
            "{start}: the holder keeps its endpoint"
        );
        assert_eq!(
            mob.recorded_endpoint(&carol),
            None,
            "{start}: the row's member holds no endpoint"
        );
    }

    // The mob lifts with the broken member in it.
    tokio::time::timeout(STEP, mob.handle.resume())
        .await
        .expect("the lift answers")
        .expect("the stopped mob lifts");
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
        "after the lift the holder is active and the row's member broken: {listed:?}"
    );
    assert_eq!(
        mob.recorded_endpoint(&alice),
        Some(alice_endpoint.clone()),
        "the lift leaves the holder its endpoint"
    );

    // The way out is the ordinary verb, and the next start is clean.
    tokio::time::timeout(STEP, mob.handle.retire(carol.clone()))
        .await
        .expect("the retire answers")
        .expect("the broken member retires");
    assert!(
        !mob.machine_holds(&carol),
        "the retired member is gone from MobMachine"
    );
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
