//! Rows for wiring with a member whose wake has not bound it.
//!
//! A wake creates the member's session (and with it its comms runtime) off
//! the actor and binds its operation record afterwards; MobMachine holds the
//! member Waking and not Bound until the wake's plan is applied. In that
//! span the member has a comms runtime and nothing that may be wired
//! through it. Endpoint resolution used to look the comms runtime up before
//! it considered MobMachine's recorded endpoint (it fell back to the
//! recorded endpoint only when no comms runtime existed), so every caller
//! that met the member in that span was answered `missing generated owner
//! operation binding`. The rows hold a wake in exactly that span with the
//! provision's test hold and pin what each caller gets.

use super::*;
use crate::runtime::provisioner::provision_finish_test_hooks::{self, BindHold};

/// A released (Dormant) worker whose next wake holds between the creation
/// of its session and the binding of its operation record.
async fn peer_held_before_bind(fixture: &WakeFixture, label: &str) -> (AgentIdentity, BindHold) {
    let peer = fixture.spawn(label).await;
    fixture.release(&peer).await;
    let hold = provision_finish_test_hooks::hold_before_bind(&fixture.session(&peer).await);
    (peer, hold)
}

fn start_wake(
    fixture: &WakeFixture,
    peer: &AgentIdentity,
) -> tokio::task::JoinHandle<Result<(), MobError>> {
    let handle = fixture.handle.clone();
    let woken = peer.clone();
    tokio::spawn(async move { handle.ensure_member_live(&woken).await })
}

/// Wait until `peer`'s wake reached its bind, and check the span the rows
/// are about: MobMachine holds the peer Waking and not Bound, and its comms
/// runtime exists.
async fn wait_held_before_bind(fixture: &WakeFixture, peer: &AgentIdentity, hold: &mut BindHold) {
    tokio::time::timeout(WAIT, hold.entered.wait_for(|entered| *entered))
        .await
        .expect("the peer's wake reaches its bind")
        .expect("the bind hold stays armed");
    let key = mob_dsl::AgentIdentity::from_domain(peer);
    {
        let state = fixture.handle.machine_state_watch_rx.borrow();
        assert_eq!(
            state.member_materialization.get(&key).copied(),
            Some(mob_dsl::MemberMaterializationPhase::Waking),
            "the held peer is Waking"
        );
        assert!(
            !state.member_wake_bound.contains(&key),
            "the held peer's wake is not Bound"
        );
    }
    let session = fixture.session(peer).await;
    assert!(
        fixture.service.comms_runtime(&session).await.is_some(),
        "the held wake built the peer's comms runtime"
    );
}

/// Let the held wake bind and wait for it to end Live.
async fn finish_held_wake(
    fixture: &WakeFixture,
    peer: &AgentIdentity,
    hold: BindHold,
    wake: tokio::task::JoinHandle<Result<(), MobError>>,
) {
    hold.release.send_replace(true);
    tokio::time::timeout(WAIT, wake)
        .await
        .expect("the held wake answers once it may bind")
        .expect("the wake task ends")
        .expect("the held wake succeeds");
    fixture
        .wait_residency(peer, Some(mob_dsl::MemberMaterializationPhase::Live))
        .await;
}

/// A durable turn-driven worker's spawn, answered as the caller sees it.
async fn spawn_worker(fixture: &WakeFixture, label: &str) -> (AgentIdentity, Result<(), MobError>) {
    let member = AgentIdentity::from(format!("{label}-{}", Uuid::new_v4().simple()));
    let mut spec = SpawnMemberSpec::new("worker", member.clone());
    spec.continuity_intent = SpawnContinuityIntent::DurableIdentity {
        continuity_key: member.to_string(),
    };
    let spawned = tokio::time::timeout(WAIT, fixture.handle.spawn_spec(spec))
        .await
        .expect("the spawn answers")
        .map(|_| ());
    (member, spawned)
}

fn edge_held(fixture: &WakeFixture, a: &AgentIdentity, b: &AgentIdentity) -> bool {
    fixture
        .handle
        .machine_state_watch_rx
        .borrow()
        .wiring_edges
        .iter()
        .any(|edge| {
            (edge.a.0 == a.as_str() && edge.b.0 == b.as_str())
                || (edge.a.0 == b.as_str() && edge.b.0 == a.as_str())
        })
}

/// The peer ids `member`'s own comms runtime trusts.
async fn trusted_by(fixture: &WakeFixture, member: &AgentIdentity) -> BTreeSet<String> {
    fixture
        .comms(member)
        .await
        .peers()
        .await
        .into_iter()
        .map(|peer| peer.peer_id.to_string())
        .collect()
}

/// The peer id MobMachine recorded for `member`.
fn recorded_peer_id(fixture: &WakeFixture, member: &AgentIdentity) -> String {
    fixture
        .handle
        .machine_state_watch_rx
        .borrow()
        .member_peer_ids
        .get(&mob_dsl::AgentIdentity::from_domain(member))
        .expect("the member's recorded peer id")
        .0
        .clone()
}

/// One message each way between two Live members is delivered once: each
/// side trusts the other.
async fn assert_both_directions_deliver(
    fixture: &WakeFixture,
    a: &AgentIdentity,
    b: &AgentIdentity,
) {
    for (from, to) in [(a, b), (b, a)] {
        let body = token("we-both-ways");
        fixture
            .raw_peer_send(from, to, &body)
            .await
            .expect("the message is admitted over the edge");
        assert_eq!(
            fixture.delivered_count(to, &body).await,
            1,
            "{from}'s message reaches {to} once"
        );
    }
}

/// A member spawned while a peer it is wired to by role is being woken, and
/// that wake has not bound the peer yet, is admitted with its declared edge:
/// the member trusts the peer's recorded endpoint, and the peer's wake
/// realizes its own side from MobMachine's edges.
///
/// RED before the fix: the spawn's wiring looked the peer's comms runtime
/// up, found it, and asked to mark the peer ready on an operation record
/// the wake had not bound: `WiringError("... missing generated owner
/// operation binding")`, the member rolled back.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_spawn_wired_by_role_to_a_peer_whose_wake_has_not_bound_it_takes_its_declared_edge() {
    let fixture = kickoff_notice_fixture("we-spawn-unbound").await;
    let (peer, mut hold) = peer_held_before_bind(&fixture, "we-spawn-peer").await;
    let wake = start_wake(&fixture, &peer);
    wait_held_before_bind(&fixture, &peer, &mut hold).await;

    let (member, spawned) = spawn_worker(&fixture, "we-spawn-member").await;
    assert!(
        spawned.is_ok(),
        "a spawn wired by role to a peer whose wake has not bound it is admitted: {spawned:?}"
    );
    assert!(
        edge_held(&fixture, &member, &peer),
        "MobMachine holds the declared edge"
    );
    assert_eq!(
        fixture.residency(&peer),
        Some(mob_dsl::MemberMaterializationPhase::Waking),
        "the spawn neither waited for the peer's wake nor ended it"
    );
    assert!(
        trusted_by(&fixture, &member)
            .await
            .contains(&recorded_peer_id(&fixture, &peer)),
        "the member trusts the peer's recorded endpoint"
    );

    finish_held_wake(&fixture, &peer, hold, wake).await;
    assert_both_directions_deliver(&fixture, &member, &peer).await;
    fixture.finish().await;
}

/// A kickoff notice owed to a peer whose wake has not bound it is delivered
/// once, like the notice that started the wake.
///
/// RED before the fix: the started notice was prepared against the peer's
/// comms runtime, failed `missing generated owner operation binding`, and
/// was dropped with a record; the peer held only the starting notice.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_kickoff_notice_to_a_peer_whose_wake_has_not_bound_it_is_delivered_once() {
    let fixture = kickoff_notice_fixture("we-notice-unbound").await;
    let (peer, mut hold) = peer_held_before_bind(&fixture, "we-notice-peer").await;

    // The member's starting notice wakes the peer; the wake holds at its bind.
    fixture.client.block();
    let member = spawn_kickoff_member(&fixture, "we-notice-member").await;
    wait_held_before_bind(&fixture, &peer, &mut hold).await;

    // The kickoff resolves while the peer is held: its started notice is
    // owed to a peer that is Waking, not Bound, with a comms runtime.
    fixture.client.release();
    wait_kickoff_started(&fixture, &member).await;
    assert!(
        actor_answer_time(&fixture).await < ACTOR_ANSWER_BOUND,
        "the notice does not hold the actor"
    );
    assert_eq!(
        fixture.residency(&peer),
        Some(mob_dsl::MemberMaterializationPhase::Waking),
        "the peer's wake is still held"
    );

    hold.release.send_replace(true);
    fixture
        .wait_residency(&peer, Some(mob_dsl::MemberMaterializationPhase::Live))
        .await;
    let received = kickoff_notices_settled(
        &fixture,
        std::slice::from_ref(&peer),
        std::slice::from_ref(&member),
        WAIT,
    )
    .await;
    assert_eq!(
        received,
        vec![starting_then_started()],
        "the peer holds the member's kickoff notices once each, in order"
    );
    fixture.finish().await;
}

/// A host peer message to a member whose wake has not bound it joins that
/// wake and is delivered once.
///
/// RED before the fix: the send answered `Internal("... missing generated
/// owner operation binding")` and nothing was sent.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_host_peer_message_to_a_peer_whose_wake_has_not_bound_it_is_delivered_once() {
    let fixture = WakeFixture::new("we-send-unbound").await;
    let sender = fixture.spawn("we-send-sender").await;
    let peer = fixture.spawn("we-send-peer").await;
    fixture.wire(&sender, &peer).await;
    fixture.release(&peer).await;
    let mut hold = provision_finish_test_hooks::hold_before_bind(&fixture.session(&peer).await);
    let wake = start_wake(&fixture, &peer);
    wait_held_before_bind(&fixture, &peer, &mut hold).await;

    let body = token("we-send");
    let mut send = {
        let handle = fixture.handle.clone();
        let (from, to, body) = (sender.clone(), peer.clone(), body.clone());
        tokio::spawn(async move {
            handle
                .send_peer_message(from, to, body.as_str(), HandlingMode::Queue)
                .await
        })
    };
    // While the wake is held the send waits for it (or is already handed
    // off): it is never refused. A refusal answers well inside this wait.
    let early = match tokio::time::timeout(Duration::from_secs(2), &mut send).await {
        Ok(answered) => Some(
            answered
                .expect("the send task ends")
                .expect("a host peer message to a peer whose wake has not bound it is handed off"),
        ),
        Err(_) => None,
    };
    assert!(
        actor_answer_time(&fixture).await < ACTOR_ANSWER_BOUND,
        "the send does not hold the actor while the peer's wake is held"
    );

    finish_held_wake(&fixture, &peer, hold, wake).await;
    let receipt = match early {
        Some(receipt) => receipt,
        None => tokio::time::timeout(WAIT, send)
            .await
            .expect("the send answers once the peer is woken")
            .expect("the send task ends")
            .expect("a host peer message to a peer whose wake has not bound it is handed off"),
    };
    assert_eq!(receipt.to, peer);
    assert_eq!(fixture.delivered_count(&peer, &body).await, 1);
    fixture.finish().await;
}

/// A spawn whose wiring fails for another reason (its edge cannot be
/// journaled) is rolled back at once beside a peer whose wake has not bound
/// it: the rollback's endpoint stage reads the peer's recorded endpoint.
///
/// RED before the fix: the spawn already failed on the peer's missing
/// binding, and with the resolver alone fixed the rollback's own endpoint
/// stage failed on it (`rollback remains owned for retry`), leaving the
/// member seated until a later attempt.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_spawn_rollback_beside_a_peer_whose_wake_has_not_bound_it_completes_at_once() {
    let events = Arc::new(FaultInjectedMobEventStore::new());
    let fixture = WakeFixture::build_with_storage(
        "we-rollback-unbound",
        |_| {},
        |definition| {
            definition.wiring.role_wiring = vec![RoleWiringRule {
                a: ProfileName::from("worker"),
                b: ProfileName::from("worker"),
            }];
        },
        false,
        None,
        MobStorage::with_events(events.clone()),
    )
    .await;
    let (peer, mut hold) = peer_held_before_bind(&fixture, "we-rollback-peer").await;
    let wake = start_wake(&fixture, &peer);
    wait_held_before_bind(&fixture, &peer, &mut hold).await;

    events.fail_appends_for("MembersWired").await;
    let (member, spawned) = spawn_worker(&fixture, "we-rollback-member").await;
    events.allow_appends_for("MembersWired").await;
    let refused = spawned.expect_err("the spawn's edge cannot be journaled");
    let text = refused.to_string();
    assert!(
        !text.contains("missing generated owner operation binding"),
        "the refusal is the journal's, not the peer's missing binding: {refused:?}"
    );
    assert!(
        !text.contains("rollback remains owned for retry"),
        "the rollback completed on its first attempt: {refused:?}"
    );
    assert!(
        fixture
            .handle
            .get_member(&member)
            .await
            .expect("roster read")
            .is_none(),
        "the refused member is rolled back when its spawn answers"
    );
    assert!(
        !edge_held(&fixture, &member, &peer),
        "no edge of the refused member is left"
    );

    finish_held_wake(&fixture, &peer, hold, wake).await;
    fixture.finish().await;
}

/// Two wired members woken together realize their edge with no repair
/// owed: the first one's topology step meets the second while its wake has
/// not bound it, and trusts its recorded endpoint.
///
/// RED before the fix: that step failed `missing generated owner operation
/// binding` and MobMachine recorded the edge as owing a repair.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_members_woken_together_realize_their_edge_with_no_repair_owed() {
    let fixture = WakeFixture::new("we-pair-unbound").await;
    let first = fixture.spawn("we-pair-first").await;
    let second = fixture.spawn("we-pair-second").await;
    fixture.wire(&first, &second).await;
    fixture.release(&first).await;
    fixture.release(&second).await;
    let mut hold = provision_finish_test_hooks::hold_before_bind(&fixture.session(&second).await);
    let second_wake = start_wake(&fixture, &second);
    wait_held_before_bind(&fixture, &second, &mut hold).await;

    fixture
        .ensure_live(&first)
        .await
        .expect("the first member wakes while the second's wake is held");
    {
        let state = fixture.handle.machine_state_watch_rx.borrow();
        assert!(
            state.owed_edge_repairs.is_empty(),
            "the woken member's edge to a peer whose wake has not bound it owes no repair: {:?}",
            state.owed_edge_repairs
        );
    }
    assert!(
        trusted_by(&fixture, &first)
            .await
            .contains(&recorded_peer_id(&fixture, &second)),
        "the woken member trusts the held peer's recorded endpoint"
    );

    finish_held_wake(&fixture, &second, hold, second_wake).await;
    assert_both_directions_deliver(&fixture, &first, &second).await;
    fixture.finish().await;
}

/// Control: a spawn wired by role to a peer whose wake IS Bound (its plan is
/// applied, its readiness is owed) installs both sides at once, on the
/// peer's own comms runtime too, before the wake ends. The peer's topology
/// step may already have planned its edges by then, so a Bound member is
/// never treated as not materialized.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_spawn_wired_to_a_peer_whose_wake_is_bound_installs_both_sides_before_the_wake_ends() {
    let fixture = kickoff_notice_fixture("we-spawn-bound").await;
    let (peer, readiness) = peer_held_at_wake_readiness(&fixture, "we-bound-peer").await;
    let wake = start_wake(&fixture, &peer);
    wait_wake_bound(&fixture, &peer).await;

    let (member, spawned) = spawn_worker(&fixture, "we-bound-member").await;
    assert!(
        spawned.is_ok(),
        "a spawn wired to a Bound waking peer is admitted: {spawned:?}"
    );
    assert_eq!(
        fixture.residency(&peer),
        Some(mob_dsl::MemberMaterializationPhase::Waking),
        "the peer's wake is still held at its readiness"
    );
    assert!(
        trusted_by(&fixture, &peer)
            .await
            .contains(&recorded_peer_id(&fixture, &member)),
        "the Bound peer's own comms runtime trusts the member before its wake ends"
    );
    assert!(
        trusted_by(&fixture, &member)
            .await
            .contains(&recorded_peer_id(&fixture, &peer)),
        "the member trusts the Bound peer"
    );

    let _ = readiness.send(true);
    tokio::time::timeout(WAIT, wake)
        .await
        .expect("the wake answers")
        .expect("the wake task ends")
        .expect("the wake succeeds");
    assert_both_directions_deliver(&fixture, &member, &peer).await;
    fixture.finish().await;
}

/// A spawn that already wired a peer whose wake has not bound it, and is
/// then refused (its next edge cannot be journaled), is rolled back at once:
/// the rollback's endpoint stage takes that peer by its recorded endpoint.
/// The member is gone when the spawn answers, no edge of it is left, and a
/// member spawned afterwards is wired to both peers.
///
/// Both of the member's peers are held in the same span, so whichever is
/// wired first is the one the rollback has to undo. A peer's failed endpoint
/// fails a spawn rollback only for a peer the spawn had wired, which is why
/// the journal fault lets one edge through.
///
/// RED under the mutant that reads the recorded endpoint for a Dormant
/// member only in the retirement endpoint stage: the rollback looks the
/// wired peer's comms runtime up, fails on its unbound operation record,
/// stays owned for a later attempt, and the spawn answers `rollback remains
/// owned for retry` with the member seated on its one edge.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_spawn_that_wired_a_peer_whose_wake_has_not_bound_it_is_rolled_back_at_once() {
    let events = Arc::new(FaultInjectedMobEventStore::new());
    let fixture = WakeFixture::build_with_storage(
        "we-rollback-wired",
        |_| {},
        |definition| {
            definition.wiring.role_wiring = vec![RoleWiringRule {
                a: ProfileName::from("worker"),
                b: ProfileName::from("worker"),
            }];
        },
        false,
        None,
        MobStorage::with_events(events.clone()),
    )
    .await;
    let (first, mut first_hold) = peer_held_before_bind(&fixture, "we-wired-first").await;
    let (second, mut second_hold) = peer_held_before_bind(&fixture, "we-wired-second").await;
    let first_wake = start_wake(&fixture, &first);
    let second_wake = start_wake(&fixture, &second);
    wait_held_before_bind(&fixture, &first, &mut first_hold).await;
    wait_held_before_bind(&fixture, &second, &mut second_hold).await;

    // One of the member's two edges is journaled, the next one is refused.
    events.fail_appends_for_after("MembersWired", 1).await;
    let (member, spawned) = spawn_worker(&fixture, "we-wired-member").await;
    events.allow_appends_for("MembersWired").await;
    let refused = spawned.expect_err("the spawn's second edge cannot be journaled");
    let text = refused.to_string();
    assert!(
        !text.contains("rollback remains owned for retry"),
        "the rollback completed on its first attempt: {refused:?}"
    );
    assert!(
        !text.contains("missing generated owner operation binding"),
        "the refusal is the journal's: {refused:?}"
    );
    assert!(
        fixture
            .handle
            .get_member(&member)
            .await
            .expect("roster read")
            .is_none(),
        "the refused member is rolled back when its spawn answers"
    );
    assert!(
        !edge_held(&fixture, &member, &first) && !edge_held(&fixture, &member, &second),
        "no edge of the refused member is left"
    );

    finish_held_wake(&fixture, &first, first_hold, first_wake).await;
    finish_held_wake(&fixture, &second, second_hold, second_wake).await;

    // Nothing of the refused spawn is left behind on either peer: a member
    // spawned afterwards takes both declared edges, and each edge carries a
    // message both ways.
    let (later, spawned) = spawn_worker(&fixture, "we-wired-later").await;
    assert!(
        spawned.is_ok(),
        "a spawn after the rollback is admitted: {spawned:?}"
    );
    assert!(
        edge_held(&fixture, &later, &first) && edge_held(&fixture, &later, &second),
        "the later member holds both declared edges"
    );
    assert_both_directions_deliver(&fixture, &later, &first).await;
    assert_both_directions_deliver(&fixture, &later, &second).await;
    fixture.finish().await;
}

/// How many `mob.peer_retired` notices of `from` `peer`'s transcript holds.
async fn retired_notices(
    fixture: &WakeFixture,
    peer: &AgentIdentity,
    from: &AgentIdentity,
) -> usize {
    let session = fixture.session(peer).await;
    fixture
        .transcript(&session)
        .await
        .split("Peer lifecycle notice from ")
        .skip(1)
        .filter(|notice| {
            notice.split_once("Kind: ").is_some_and(|(sender, rest)| {
                sender.contains(from.as_str()) && rest.starts_with("mob.peer_retired")
            })
        })
        .count()
}

/// A spawn whose second edge cannot be journaled is rolled back while the
/// wakes of its two peers end. The rollback is held at its first
/// `spawn-rollback` step, after it has observed the peers' endpoints and
/// before it removes MobMachine's edges; meanwhile both peers become Live.
/// `waking`: the peers' wakes are held before their bind when the spawn runs
/// and let go at the hold; otherwise the peers are Dormant then and are woken
/// at the hold.
///
/// A peer the rollback observed from its recorded endpoint has no comms
/// runtime the rollback could clean. When that peer's wake binds it before
/// the rollback removes the edge, the wake's topology step realizes the edge
/// MobMachine still holds, and the peer then trusts a member that is rolled
/// back: it holds two rows under the member's name once the identity is
/// spawned again, and cannot address the new one.
///
/// The rows hold the settled state, whether the rollback ended at once or by
/// a later attempt: the member gone, no edge, no repair owed, neither peer
/// holding a trust row for the member, at most one retired notice for each
/// attempt, and the same identity spawned again with a message both ways on
/// each edge.
async fn spawn_rollback_whose_peers_wakes_end_meanwhile(label: &'static str, waking: bool) {
    let events = Arc::new(FaultInjectedMobEventStore::new());
    let fixture = WakeFixture::build_with_storage(
        label,
        |_| {},
        |definition| {
            definition.wiring.role_wiring = vec![RoleWiringRule {
                a: ProfileName::from("worker"),
                b: ProfileName::from("worker"),
            }];
        },
        false,
        None,
        MobStorage::with_events(events.clone()),
    )
    .await;
    let first = fixture.spawn("endpoint-meanwhile-first").await;
    let second = fixture.spawn("endpoint-meanwhile-second").await;
    fixture.release(&first).await;
    fixture.release(&second).await;
    let mut holds = Vec::new();
    let mut wakes = Vec::new();
    if waking {
        for peer in [&first, &second] {
            holds.push(provision_finish_test_hooks::hold_before_bind(
                &fixture.session(peer).await,
            ));
        }
        for peer in [&first, &second] {
            wakes.push(start_wake(&fixture, peer));
        }
        for (peer, hold) in [&first, &second].into_iter().zip(holds.iter_mut()) {
            wait_held_before_bind(&fixture, peer, hold).await;
        }
    }

    // One of the member's two edges is journaled, the next one is refused,
    // and the rollback is held once it has observed the peers' endpoints.
    events.fail_appends_for_after("MembersWired", 1).await;
    let member = AgentIdentity::from(format!(
        "endpoint-meanwhile-member-{}",
        Uuid::new_v4().simple()
    ));
    let member_spec = || {
        let mut spec = SpawnMemberSpec::new("worker", member.clone());
        spec.continuity_intent = SpawnContinuityIntent::DurableIdentity {
            continuity_key: member.to_string(),
        };
        spec
    };
    let rollback_held =
        crate::runtime::actor::retirement_stage_hold::install(&member, "spawn-rollback");
    let spawning = {
        let handle = fixture.handle.clone();
        let spec = member_spec();
        tokio::spawn(async move { handle.spawn_spec(spec).await.map(|_| ()) })
    };
    tokio::time::timeout(WAIT, rollback_held.entered())
        .await
        .expect("the spawn's rollback reaches its held step");
    let member_peer_id = recorded_peer_id(&fixture, &member);

    // The peers' wakes end while the rollback is held.
    if waking {
        for hold in &holds {
            hold.release.send_replace(true);
        }
    } else {
        for peer in [&first, &second] {
            wakes.push(start_wake(&fixture, peer));
        }
    }
    for wake in wakes {
        tokio::time::timeout(WAIT, wake)
            .await
            .expect("the peer's wake answers while the rollback is held")
            .expect("the wake task ends")
            .expect("the peer's wake succeeds");
    }
    let live = Some(mob_dsl::MemberMaterializationPhase::Live);
    fixture.wait_residency(&first, live).await;
    fixture.wait_residency(&second, live).await;

    rollback_held.release();
    let refused = tokio::time::timeout(WAIT, spawning)
        .await
        .expect("the spawn answers")
        .expect("the spawn task ends")
        .expect_err("the spawn's second edge cannot be journaled");
    events.allow_appends_for("MembersWired").await;

    // The rollback settles: at once, or by its next attempt when a peer's
    // wake bound the peer after the rollback had observed it.
    let gone = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if fixture
                .handle
                .get_member(&member)
                .await
                .expect("roster read")
                .is_none()
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await;
    assert!(
        gone.is_ok(),
        "the refused member is rolled back: {refused:?}"
    );
    assert!(
        !edge_held(&fixture, &member, &first) && !edge_held(&fixture, &member, &second),
        "no edge of the refused member is left"
    );
    for peer in [&first, &second] {
        assert!(
            !trusted_by(&fixture, peer).await.contains(&member_peer_id),
            "{peer} holds no trust row for the rolled-back member (the spawn answered {refused:?})"
        );
        let retired = retired_notices(&fixture, peer, &member).await;
        assert!(
            retired <= 2,
            "{peer} holds at most one retired notice of the member for each attempt: {retired}"
        );
    }
    {
        let state = fixture.handle.machine_state_watch_rx.borrow();
        assert!(
            state.owed_edge_repairs.is_empty(),
            "no repair is owed after the rollback: {:?}",
            state.owed_edge_repairs
        );
    }

    // The caller's retry: the same identity is spawned again, takes both
    // declared edges, and each edge carries a message both ways by name.
    let respawned = tokio::time::timeout(WAIT, fixture.handle.spawn_spec(member_spec()))
        .await
        .expect("the respawn answers")
        .map(|_| ());
    assert!(
        respawned.is_ok(),
        "the same identity is admitted after the rollback: {respawned:?}"
    );
    assert!(
        edge_held(&fixture, &member, &first) && edge_held(&fixture, &member, &second),
        "the respawned member holds both declared edges"
    );
    assert_both_directions_deliver(&fixture, &member, &first).await;
    assert_both_directions_deliver(&fixture, &member, &second).await;
    fixture.finish().await;
}

/// The peers are Waking and not Bound when the rollback observes them, and
/// their wakes bind them before the rollback removes its edges.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_spawn_rollback_whose_wired_peers_wake_binds_meanwhile_leaves_that_peer_no_trust_row() {
    spawn_rollback_whose_peers_wakes_end_meanwhile("endpoint-meanwhile-waking", true).await;
}

/// The peers are Dormant when the rollback observes them, and are woken
/// before the rollback removes its edges.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_spawn_rollback_whose_wired_peer_is_woken_meanwhile_leaves_that_peer_no_trust_row() {
    spawn_rollback_whose_peers_wakes_end_meanwhile("endpoint-meanwhile-dormant", false).await;
}
