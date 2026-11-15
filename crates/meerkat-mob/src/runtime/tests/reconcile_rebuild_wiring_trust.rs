//! A member the identity reconcile pass rebuilds keeps the wiring trust of
//! the edges the mob holds for it.
//!
//! The pass replaces a member whose realization no longer matches its
//! identity's declaration (an adoption, and every later change of the
//! declaration): it retires the member and rebuilds it on its session. The
//! retirement is the respawn's: the mob keeps the member's edges and takes
//! the retired incarnation's trust off the members it was wired to. The
//! rebuild then restores, on both ends, the trust of every edge the mob
//! still holds for the member, as a respawn does: the pair is left as a
//! second wiring call would leave it. It writes no structure: an edge the
//! mob does not hold is not created, and an edge whose restore fails is
//! owed a repair that the mob retries, without failing the rebuild.
//!
//! The members here run on mock comms runtimes: a row reads each end's trust
//! rows and its peers, and hands the sender's runtime a peer message, which
//! the mock refuses for a peer it does not trust. A message read from the
//! receiver's conversation is rowed in the crate's `cold_restart_mob_resume`
//! integration target.

use super::identity_owned_wiring_reconcile::{
    AFTER_START, STEP, WiringFixture, assert_converged, edge,
};
use super::*;
use crate::event::MobEventKind;
use crate::identity::{
    DesiredIdentityEdge, IdentityConvergenceStatus, IdentityReconcileDecision,
    IdentityWiringCustody,
};
use crate::machines::mob_machine as mob_dsl;
use meerkat_core::comms::GeneratedCommsTrustAuthoritySourceKind;

/// One wiring trust row: the trusted peer's name, peer id and address.
type TrustRow = (String, String, String);

/// A seated worker with a comms runtime on a key of its own.
struct Member {
    identity: AgentIdentity,
    session: SessionId,
    /// The name its peers trust it under.
    comms_name: String,
}

async fn seat(fixture: &WiringFixture, name: &str) -> Member {
    let (identity, session) = fixture.seat_with_comms(name).await;
    Member {
        identity,
        session,
        comms_name: test_comms_name_for(&fixture.mob_id, "worker", name),
    }
}

/// Join two members by a wiring call.
async fn wire(fixture: &WiringFixture, left: &Member, right: &Member) {
    fixture
        .handle
        .wire(left.identity.clone(), right.identity.clone())
        .await
        .unwrap_or_else(|error| panic!("wire {} and {}: {error}", left.identity, right.identity));
}

/// The member's comms runtime, as the session service holds it.
async fn comms(fixture: &WiringFixture, member: &Member) -> Arc<dyn CoreCommsRuntime> {
    fixture
        .service
        .comms_runtime(&member.session)
        .await
        .unwrap_or_else(|| panic!("{} runs with a comms runtime", member.identity))
}

/// The wiring trust rows the member's comms runtime holds.
async fn trust_rows(fixture: &WiringFixture, member: &Member) -> BTreeSet<TrustRow> {
    comms(fixture, member)
        .await
        .trusted_peer_projection_snapshot_for_source(
            GeneratedCommsTrustAuthoritySourceKind::MobMachineMemberTrustWiring,
        )
        .await
        .unwrap_or_else(|error| panic!("{}: read the trust rows: {error}", member.identity))
        .into_iter()
        .map(|row| {
            (
                row.name.as_str().to_string(),
                row.peer_id.to_string(),
                row.address.to_string(),
            )
        })
        .collect()
}

/// The trust rows of each of `members`, in order.
async fn trust_of(fixture: &WiringFixture, members: &[&Member]) -> Vec<BTreeSet<TrustRow>> {
    let mut rows = Vec::with_capacity(members.len());
    for member in members {
        rows.push(trust_rows(fixture, member).await);
    }
    rows
}

/// The names in a member's trust rows.
async fn trusted_names(fixture: &WiringFixture, member: &Member) -> BTreeSet<String> {
    trust_rows(fixture, member)
        .await
        .into_iter()
        .map(|(name, _, _)| name)
        .collect()
}

/// Hand `from`'s comms runtime a peer message for `to`: `from` lists `to`
/// among its peers and its runtime takes the message.
async fn peer_message(fixture: &WiringFixture, from: &Member, to: &Member) -> Result<(), String> {
    let comms = comms(fixture, from).await;
    let peers = comms.peers().await;
    let Some(peer) = peers
        .iter()
        .find(|peer| peer.name.as_str() == to.comms_name)
    else {
        return Err(format!(
            "{} lists no peer {}; its peers: {:?}",
            from.identity,
            to.comms_name,
            peers
                .iter()
                .map(|peer| peer.name.as_str().to_string())
                .collect::<Vec<_>>()
        ));
    };
    comms
        .send(CommsCommand::PeerMessage {
            objective_id: None,
            content_taint: None,
            to: PeerRoute::with_display_name(peer.peer_id, peer.name.clone()),
            body: format!("a message from {} to {}", from.identity, to.identity),
            blocks: None,
            handling_mode: HandlingMode::Queue,
            in_reply_to: None,
        })
        .await
        .map(|_| ())
        .map_err(|error| format!("{} cannot message {}: {error}", from.identity, to.identity))
}

/// The pair is wired as its members and the mob see it: each end trusts the
/// other, lists it among its peers and can message it, and the member list
/// shows the edge.
async fn assert_wired_pair(fixture: &WiringFixture, left: &Member, right: &Member, when: &str) {
    for (holder, peer) in [(left, right), (right, left)] {
        let names = trusted_names(fixture, holder).await;
        assert!(
            names.contains(&peer.comms_name),
            "{when}: {} trusts {}; it trusts {names:?}",
            holder.identity,
            peer.identity
        );
        if let Err(error) = peer_message(fixture, holder, peer).await {
            panic!("{when}: {error}");
        }
    }
    assert_listed_wired(fixture, left, right, when).await;
}

/// The member list shows the edge on both ends.
async fn assert_listed_wired(fixture: &WiringFixture, left: &Member, right: &Member, when: &str) {
    let members = fixture.handle.list_members().await;
    for (holder, peer) in [(left, right), (right, left)] {
        let entry = members
            .iter()
            .find(|entry| entry.agent_identity == holder.identity)
            .unwrap_or_else(|| panic!("{when}: {} is listed", holder.identity));
        assert!(
            entry.wired_to.contains(&peer.identity),
            "{when}: the member list shows {} wired to {}: {:?}",
            holder.identity,
            peer.identity,
            entry.wired_to
        );
    }
}

/// Neither end trusts the other or can message it.
async fn assert_silenced(fixture: &WiringFixture, left: &Member, right: &Member) {
    for (holder, peer) in [(left, right), (right, left)] {
        assert!(
            !trusted_names(fixture, holder)
                .await
                .contains(&peer.comms_name),
            "{} holds no trust row for {}",
            holder.identity,
            peer.identity
        );
        assert!(
            peer_message(fixture, holder, peer).await.is_err(),
            "{} cannot message {}",
            holder.identity,
            peer.identity
        );
    }
}

/// Take each end's trust row for the other off its comms runtime, leaving
/// the mob's edge as it is: the state of a pair whose member was rebuilt
/// without its trust.
async fn silence(left: &Member, right: &Member) {
    for (holder, peer) in [(left, right), (right, left)] {
        let comms = persistent_mock_comms_runtime(&holder.session)
            .unwrap_or_else(|| panic!("{} was built with a comms runtime", holder.identity));
        let peer_ids: Vec<String> = comms
            .trusted_peers
            .read()
            .await
            .iter()
            .filter(|(_, row)| row.name.as_str() == peer.comms_name)
            .map(|(peer_id, _)| peer_id.clone())
            .collect();
        for peer_id in peer_ids {
            comms
                .remove_trusted_peer(&peer_id)
                .await
                .unwrap_or_else(|error| panic!("take the trust row off: {error}"));
        }
    }
}

/// The member edges the mob holds.
fn held_edges(fixture: &WiringFixture) -> BTreeSet<mob_dsl::WiringEdge> {
    fixture
        .handle
        .machine_state_watch_rx
        .borrow()
        .wiring_edges
        .iter()
        .cloned()
        .collect()
}

fn held_edge(left: &Member, right: &Member) -> mob_dsl::WiringEdge {
    mob_dsl::WiringEdge::new(
        mob_dsl::AgentIdentity::from_domain(&left.identity),
        mob_dsl::AgentIdentity::from_domain(&right.identity),
    )
}

/// How many wired rows the mob's event store holds.
async fn stored_wired_rows(fixture: &WiringFixture) -> usize {
    fixture
        .events
        .replay_all()
        .await
        .expect("read the mob's events")
        .iter()
        .filter(|event| {
            matches!(
                event.kind,
                MobEventKind::MembersWired { .. } | MobEventKind::MembersWiredBatch { .. }
            )
        })
        .count()
}

/// The edges `adopted`'s declaration owns under `custody`: under
/// identity-owned custody the smaller-named end owns the pair's edge, and
/// its declaration states it; the larger-named end owns nothing, and under
/// the default custody a declaration states no edge.
fn owned_under(
    custody: IdentityWiringCustody,
    adopt_smaller: bool,
    smaller: &Member,
    larger: &Member,
) -> Vec<DesiredIdentityEdge> {
    if custody == IdentityWiringCustody::IdentityOwned && adopt_smaller {
        vec![edge(&smaller.identity, &larger.identity)]
    } else {
        Vec::new()
    }
}

/// Two members are joined by a wiring call, then one end is adopted: the
/// pass rebuilds it, it converges, and both ends hold the trust rows they
/// held before, list each other and can message each other.
async fn an_adoption_keeps_the_pairs_trust(
    label: &str,
    adopt_smaller: bool,
    custody: IdentityWiringCustody,
) {
    let fixture = WiringFixture::create(label).await;
    let smaller = seat(&fixture, "worker-a").await;
    let larger = seat(&fixture, "worker-b").await;
    wire(&fixture, &smaller, &larger).await;
    assert_wired_pair(&fixture, &smaller, &larger, "after the wiring call").await;
    let before = trust_of(&fixture, &[&smaller, &larger]).await;

    let adopted = if adopt_smaller { &smaller } else { &larger };
    let owned = owned_under(custody, adopt_smaller, &smaller, &larger);
    let builds = persistent_mock_comms_builds(&adopted.session);
    fixture
        .adopt(&adopted.identity, &adopted.session, custody, &owned)
        .await;
    assert_converged(
        &adopted.identity,
        &fixture.settled(&adopted.identity, 1).await,
    );
    assert!(
        persistent_mock_comms_builds(&adopted.session) > builds,
        "the adoption rebuilt {}",
        adopted.identity
    );

    assert_eq!(
        trust_of(&fixture, &[&smaller, &larger]).await,
        before,
        "both ends hold the trust rows they held before the adoption"
    );
    assert_wired_pair(&fixture, &smaller, &larger, "after the adoption").await;

    fixture.handle.shutdown().await.expect("shutdown the mob");
}

/// One end of a wired pair is an adopted identity, and its owner changes its
/// declaration: the pass rebuilds it on the change, the change becomes
/// active, and both ends hold the trust rows they held before.
///
/// The adoption comes before the wiring call, so the change is the pair's
/// first rebuild, except for the end that owns the edge under identity-owned
/// custody: its declaration states an edge that must exist, so it is adopted
/// after the wiring call.
async fn a_change_keeps_the_pairs_trust(
    label: &str,
    adopt_smaller: bool,
    custody: IdentityWiringCustody,
) {
    let fixture = WiringFixture::create(label).await;
    let smaller = seat(&fixture, "worker-a").await;
    let larger = seat(&fixture, "worker-b").await;
    let adopted = if adopt_smaller { &smaller } else { &larger };
    let owned = owned_under(custody, adopt_smaller, &smaller, &larger);
    let wired_first = !owned.is_empty();

    if wired_first {
        wire(&fixture, &smaller, &larger).await;
    }
    fixture
        .adopt(&adopted.identity, &adopted.session, custody, &owned)
        .await;
    assert_converged(
        &adopted.identity,
        &fixture.settled(&adopted.identity, 1).await,
    );
    if !wired_first {
        wire(&fixture, &smaller, &larger).await;
    }
    assert_wired_pair(&fixture, &smaller, &larger, "before the change").await;
    let before = trust_of(&fixture, &[&smaller, &larger]).await;

    let builds = persistent_mock_comms_builds(&adopted.session);
    fixture.change(&adopted.identity).await;
    let changed = fixture.settled(&adopted.identity, 2).await;
    assert_converged(&adopted.identity, &changed);
    assert_eq!(changed.active_intent_revision, Some(2));
    assert!(
        fixture.shell_is_on(&adopted.session).await,
        "{} runs on its changed declaration",
        adopted.identity
    );
    assert!(
        persistent_mock_comms_builds(&adopted.session) > builds,
        "the change rebuilt {}",
        adopted.identity
    );

    assert_eq!(
        trust_of(&fixture, &[&smaller, &larger]).await,
        before,
        "both ends hold the trust rows they held before the change"
    );
    assert_wired_pair(&fixture, &smaller, &larger, "after the change").await;

    fixture.handle.shutdown().await.expect("shutdown the mob");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_wired_pair_keeps_its_trust_when_its_smaller_named_end_is_adopted() {
    an_adoption_keeps_the_pairs_trust(
        "rebuild-trust-adopt-smaller",
        true,
        IdentityWiringCustody::ExternalManaged,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_wired_pair_keeps_its_trust_when_its_larger_named_end_is_adopted() {
    an_adoption_keeps_the_pairs_trust(
        "rebuild-trust-adopt-larger",
        false,
        IdentityWiringCustody::ExternalManaged,
    )
    .await;
}

/// Under identity-owned custody the smaller-named end owns the edge the
/// host wired first, and its declaration states it: the rebuild restores
/// both ends' trust, so the pass finds the edge it holds the adopter to
/// realized and the adopter converges.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_wired_pair_keeps_its_trust_when_the_end_that_owns_its_edge_is_adopted() {
    an_adoption_keeps_the_pairs_trust(
        "rebuild-trust-adopt-owner",
        true,
        IdentityWiringCustody::IdentityOwned,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_wired_pair_keeps_its_trust_when_its_larger_named_end_is_adopted_under_identity_owned_custody()
 {
    an_adoption_keeps_the_pairs_trust(
        "rebuild-trust-adopt-owned-larger",
        false,
        IdentityWiringCustody::IdentityOwned,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_wired_pair_keeps_its_trust_when_its_smaller_named_ends_declaration_changes() {
    a_change_keeps_the_pairs_trust(
        "rebuild-trust-change-smaller",
        true,
        IdentityWiringCustody::ExternalManaged,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_wired_pair_keeps_its_trust_when_its_larger_named_ends_declaration_changes() {
    a_change_keeps_the_pairs_trust(
        "rebuild-trust-change-larger",
        false,
        IdentityWiringCustody::ExternalManaged,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_wired_pair_keeps_its_trust_when_the_declaration_of_the_end_that_owns_its_edge_changes() {
    a_change_keeps_the_pairs_trust(
        "rebuild-trust-change-owner",
        true,
        IdentityWiringCustody::IdentityOwned,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_wired_pair_keeps_its_trust_when_its_larger_named_ends_declaration_changes_under_identity_owned_custody()
 {
    a_change_keeps_the_pairs_trust(
        "rebuild-trust-change-owned-larger",
        false,
        IdentityWiringCustody::IdentityOwned,
    )
    .await;
}

/// A member wired to two peers is adopted: it keeps both, and each of them
/// keeps it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_member_wired_to_two_peers_keeps_both_when_it_is_adopted() {
    let fixture = WiringFixture::create("rebuild-trust-two-peers").await;
    let first = seat(&fixture, "worker-a").await;
    let middle = seat(&fixture, "worker-b").await;
    let last = seat(&fixture, "worker-c").await;
    wire(&fixture, &first, &middle).await;
    wire(&fixture, &middle, &last).await;
    let before = trust_of(&fixture, &[&first, &middle, &last]).await;

    fixture
        .adopt(
            &middle.identity,
            &middle.session,
            IdentityWiringCustody::ExternalManaged,
            &[],
        )
        .await;
    assert_converged(
        &middle.identity,
        &fixture.settled(&middle.identity, 1).await,
    );

    assert_eq!(
        trust_of(&fixture, &[&first, &middle, &last]).await,
        before,
        "every member holds the trust rows it held before the adoption"
    );
    assert_wired_pair(&fixture, &first, &middle, "after the adoption").await;
    assert_wired_pair(&fixture, &middle, &last, "after the adoption").await;
    assert!(
        !trusted_names(&fixture, &first)
            .await
            .contains(&last.comms_name),
        "the two peers were never wired to each other"
    );

    fixture.handle.shutdown().await.expect("shutdown the mob");
}

/// The control: a member with no edge is adopted and rebuilt with no trust
/// row, and a wired pair beside it is left as it was.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_member_with_no_edge_is_rebuilt_with_no_peer() {
    let fixture = WiringFixture::create("rebuild-trust-no-edge").await;
    let alone = seat(&fixture, "worker-a").await;
    let left = seat(&fixture, "worker-b").await;
    let right = seat(&fixture, "worker-c").await;
    wire(&fixture, &left, &right).await;
    let before = trust_of(&fixture, &[&left, &right]).await;
    let edges = held_edges(&fixture);

    let builds = persistent_mock_comms_builds(&alone.session);
    fixture
        .adopt(
            &alone.identity,
            &alone.session,
            IdentityWiringCustody::ExternalManaged,
            &[],
        )
        .await;
    assert_converged(&alone.identity, &fixture.settled(&alone.identity, 1).await);
    assert!(
        persistent_mock_comms_builds(&alone.session) > builds,
        "the adoption rebuilt the member"
    );

    assert!(
        trust_rows(&fixture, &alone).await.is_empty(),
        "a member with no edge trusts nobody"
    );
    assert_eq!(trust_of(&fixture, &[&left, &right]).await, before);
    assert_wired_pair(&fixture, &left, &right, "after the other member's adoption").await;
    assert_eq!(
        held_edges(&fixture),
        edges,
        "the mob holds the edges it held"
    );

    fixture.handle.shutdown().await.expect("shutdown the mob");
}

/// The rebuild restores the edges the mob holds and writes no structure: a
/// pair that was unwired before the adoption stays unwired, the mob holds
/// the one edge it held, and its store holds no further wired row.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_rebuild_creates_no_edge_the_mob_does_not_hold() {
    let fixture = WiringFixture::create("rebuild-trust-held-edges-only").await;
    let adopted = seat(&fixture, "worker-a").await;
    let kept = seat(&fixture, "worker-b").await;
    let unwired = seat(&fixture, "worker-c").await;
    wire(&fixture, &adopted, &kept).await;
    wire(&fixture, &adopted, &unwired).await;
    fixture
        .handle
        .unwire(adopted.identity.clone(), unwired.identity.clone())
        .await
        .expect("unwire the second pair");
    let edges = held_edges(&fixture);
    assert_eq!(edges, BTreeSet::from([held_edge(&adopted, &kept)]));
    let wired_rows = stored_wired_rows(&fixture).await;

    fixture
        .adopt(
            &adopted.identity,
            &adopted.session,
            IdentityWiringCustody::ExternalManaged,
            &[],
        )
        .await;
    assert_converged(
        &adopted.identity,
        &fixture.settled(&adopted.identity, 1).await,
    );

    assert_eq!(
        held_edges(&fixture),
        edges,
        "the mob holds the edge it held, and no other"
    );
    assert_eq!(
        stored_wired_rows(&fixture).await,
        wired_rows,
        "the rebuild stores no wired row"
    );
    assert_eq!(
        trusted_names(&fixture, &adopted).await,
        BTreeSet::from([kept.comms_name.clone()]),
        "the rebuilt member trusts the peer of the edge the mob holds, and no other"
    );
    assert!(
        trust_rows(&fixture, &unwired).await.is_empty(),
        "the unwired member trusts nobody"
    );
    assert!(
        peer_message(&fixture, &adopted, &unwired).await.is_err(),
        "the unwired pair stays unwired"
    );
    assert_wired_pair(&fixture, &adopted, &kept, "after the adoption").await;

    fixture.handle.shutdown().await.expect("shutdown the mob");
}

/// An edge whose restore fails does not fail the rebuild: the member is
/// rebuilt and converges, the mob owes the edge a repair, and its repair
/// timer realizes the edge once the cause is gone.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_edge_whose_restore_fails_is_owed_a_repair_and_the_rebuild_completes() {
    let fixture = WiringFixture::create("rebuild-trust-failed-edge").await;
    let adopted = seat(&fixture, "worker-a").await;
    let peer = seat(&fixture, "worker-b").await;
    wire(&fixture, &adopted, &peer).await;
    let before = trust_of(&fixture, &[&adopted, &peer]).await;
    let held = held_edge(&adopted, &peer);

    // The peer's comms runtime takes no trust row for now.
    let peer_comms =
        persistent_mock_comms_runtime(&peer.session).expect("the peer's comms runtime");
    peer_comms.set_behavior(MockCommsBehavior {
        fail_add_trust: true,
        ..MockCommsBehavior::default()
    });

    let builds = persistent_mock_comms_builds(&adopted.session);
    fixture
        .adopt(
            &adopted.identity,
            &adopted.session,
            IdentityWiringCustody::ExternalManaged,
            &[],
        )
        .await;
    assert_converged(
        &adopted.identity,
        &fixture.settled(&adopted.identity, 1).await,
    );
    assert!(
        persistent_mock_comms_builds(&adopted.session) > builds,
        "the member is rebuilt although one of its edges failed"
    );
    let mut state = fixture.handle.machine_state_watch_rx.clone();
    tokio::time::timeout(
        STEP,
        state.wait_for(|state| state.owed_edge_repairs.contains(&held)),
    )
    .await
    .expect("the failed edge is owed a repair")
    .expect("the machine state stays published");
    assert_listed_wired(&fixture, &adopted, &peer, "while the repair is owed").await;

    peer_comms.set_behavior(MockCommsBehavior::default());
    tokio::time::timeout(
        AFTER_START,
        state.wait_for(|state| !state.owed_edge_repairs.contains(&held)),
    )
    .await
    .expect("the repair timer realizes the edge")
    .expect("the machine state stays published");
    assert_eq!(
        trust_of(&fixture, &[&adopted, &peer]).await,
        before,
        "the repaired pair holds the trust rows it held before the adoption"
    );
    assert_wired_pair(&fixture, &adopted, &peer, "after the repair").await;

    fixture.handle.shutdown().await.expect("shutdown the mob");
}

/// The rebuild leaves a pair as a second wiring call does: the trust rows
/// both ends hold after an adoption equal the rows the same wiring call,
/// made again on a pair without them, leaves. The second call adds no edge
/// and stores no wired row.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_rebuild_leaves_a_pair_as_a_second_wiring_call_does() {
    let fixture = WiringFixture::create("rebuild-trust-as-second-wire").await;
    let adopted = seat(&fixture, "worker-a").await;
    let peer = seat(&fixture, "worker-b").await;
    wire(&fixture, &adopted, &peer).await;
    let edges = held_edges(&fixture);
    let wired_rows = stored_wired_rows(&fixture).await;

    fixture
        .adopt(
            &adopted.identity,
            &adopted.session,
            IdentityWiringCustody::ExternalManaged,
            &[],
        )
        .await;
    assert_converged(
        &adopted.identity,
        &fixture.settled(&adopted.identity, 1).await,
    );
    let rebuilt = trust_of(&fixture, &[&adopted, &peer]).await;

    silence(&adopted, &peer).await;
    assert_silenced(&fixture, &adopted, &peer).await;
    wire(&fixture, &adopted, &peer).await;
    let wired_again = trust_of(&fixture, &[&adopted, &peer]).await;
    assert_wired_pair(&fixture, &adopted, &peer, "after the second wiring call").await;

    assert_eq!(
        rebuilt, wired_again,
        "the rebuild leaves the trust rows a second wiring call leaves"
    );
    assert_eq!(held_edges(&fixture), edges, "the second call adds no edge");
    assert_eq!(
        stored_wired_rows(&fixture).await,
        wired_rows,
        "the second call stores no wired row"
    );

    fixture.handle.shutdown().await.expect("shutdown the mob");
}

/// A pair that lost its trust before (its adopted end was rebuilt without
/// it) has it back at the adopted end's next change: the rebuild restores
/// what the mob holds, whatever the pair's trust rows were.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pair_without_its_trust_has_it_back_at_the_adopted_ends_next_change() {
    let fixture = WiringFixture::create("rebuild-trust-next-change").await;
    let adopted = seat(&fixture, "worker-a").await;
    let peer = seat(&fixture, "worker-b").await;
    fixture
        .adopt(
            &adopted.identity,
            &adopted.session,
            IdentityWiringCustody::ExternalManaged,
            &[],
        )
        .await;
    assert_converged(
        &adopted.identity,
        &fixture.settled(&adopted.identity, 1).await,
    );
    wire(&fixture, &adopted, &peer).await;
    let before = trust_of(&fixture, &[&adopted, &peer]).await;
    silence(&adopted, &peer).await;
    assert_silenced(&fixture, &adopted, &peer).await;
    assert_listed_wired(&fixture, &adopted, &peer, "without its trust").await;

    fixture.change(&adopted.identity).await;
    let changed = fixture.settled(&adopted.identity, 2).await;
    assert_converged(&adopted.identity, &changed);
    assert_eq!(changed.active_intent_revision, Some(2));

    assert_eq!(
        trust_of(&fixture, &[&adopted, &peer]).await,
        before,
        "both ends hold the trust rows the wiring call gave them"
    );
    assert_wired_pair(&fixture, &adopted, &peer, "after the change").await;

    fixture.handle.shutdown().await.expect("shutdown the mob");
}

/// A store carried over a restart with a wired pair whose adopted end was
/// rebuilt without its trust: the edge is held, the adoption is recorded and
/// converged, and neither end trusted the other when the mob stopped. Once
/// the next start has materialized both members, each holds the trust rows
/// the wiring call gave it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pair_that_stopped_without_its_trust_has_it_back_after_the_next_start() {
    let mut fixture = WiringFixture::create("rebuild-trust-carried").await;
    let adopted = seat(&fixture, "worker-a").await;
    let peer = seat(&fixture, "worker-b").await;
    wire(&fixture, &adopted, &peer).await;
    let before = trust_of(&fixture, &[&adopted, &peer]).await;
    fixture
        .adopt(
            &adopted.identity,
            &adopted.session,
            IdentityWiringCustody::ExternalManaged,
            &[],
        )
        .await;
    let converged = fixture.settled(&adopted.identity, 1).await;
    assert_converged(&adopted.identity, &converged);
    silence(&adopted, &peer).await;
    assert_silenced(&fixture, &adopted, &peer).await;
    assert_listed_wired(&fixture, &adopted, &peer, "before the stop").await;

    fixture.restart().await;
    let after_epoch = converged.lease_epoch.expect("the adoption held a lease");
    assert_converged(
        &adopted.identity,
        &fixture
            .settled_within(&adopted.identity, 1, after_epoch, AFTER_START)
            .await,
    );
    for member in [&adopted, &peer] {
        fixture
            .handle
            .ensure_member_live(&member.identity)
            .await
            .unwrap_or_else(|error| panic!("{} is live after the start: {error}", member.identity));
    }

    assert_eq!(
        trust_of(&fixture, &[&adopted, &peer]).await,
        before,
        "both ends hold the trust rows the wiring call gave them"
    );
    assert_wired_pair(&fixture, &adopted, &peer, "after the start").await;

    fixture.handle.shutdown().await.expect("shutdown the mob");
}

/// A store carried over a restart with a change that committed and did not
/// become active on the far end of another identity's owned edge, the two
/// joined by a wiring call before either was adopted. The next start makes
/// the change active: the pass rebuilds the far end, and the pair holds the
/// trust rows the wiring call gave it. The edge's owner converges too.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_far_end_rebuilt_at_the_next_start_keeps_its_peer() {
    let mut fixture = WiringFixture::create("rebuild-trust-far-end-carried").await;
    let owner = seat(&fixture, "worker-a").await;
    let far_end = seat(&fixture, "worker-b").await;
    wire(&fixture, &owner, &far_end).await;
    let before = trust_of(&fixture, &[&owner, &far_end]).await;
    let owned_edge = edge(&owner.identity, &far_end.identity);

    fixture
        .adopt(
            &far_end.identity,
            &far_end.session,
            IdentityWiringCustody::IdentityOwned,
            &[],
        )
        .await;
    let adopted = fixture.settled(&far_end.identity, 1).await;
    assert_converged(&far_end.identity, &adopted);
    fixture
        .adopt(
            &owner.identity,
            &owner.session,
            IdentityWiringCustody::IdentityOwned,
            std::slice::from_ref(&owned_edge),
        )
        .await;
    let owner_adopted = fixture.settled(&owner.identity, 1).await;
    assert_converged(&owner.identity, &owner_adopted);
    fixture.handle.shutdown().await.expect("shutdown the mob");

    // The stored state a host leaves when the far end's change committed and
    // the pass then reported it repair-blocked for wiring: the change at
    // desired revision 2 with revision 1 active, and that pass's status.
    fixture.change_in_the_store(&far_end.identity).await;
    fixture
        .identity_status
        .replace_identity_convergence_status(
            &fixture.mob_id,
            &IdentityConvergenceStatus {
                identity: far_end.identity.clone(),
                intent_revision: Some(2),
                active_intent_revision: Some(1),
                lease_epoch: adopted.lease_epoch,
                decision: Some(IdentityReconcileDecision::RepairBlocked),
                observed_at_ms: adopted.observed_at_ms,
                detail: Some(
                    "generated obligation ReconcileWiring has no actuator in the retained sealed-intent reconciliation slice"
                        .to_string(),
                ),
            },
        )
        .await
        .expect("record the carried status");

    fixture.start_again().await;
    let after_epoch = adopted.lease_epoch.expect("the adoption held a lease");
    let started = fixture
        .settled_within(&far_end.identity, 2, after_epoch, AFTER_START)
        .await;
    assert_converged(&far_end.identity, &started);
    assert_eq!(started.active_intent_revision, Some(2));
    assert!(
        fixture.shell_is_on(&far_end.session).await,
        "the change committed before the start is active"
    );
    let owner_after_epoch = owner_adopted
        .lease_epoch
        .expect("the adoption held a lease");
    assert_converged(
        &owner.identity,
        &fixture
            .settled_within(&owner.identity, 1, owner_after_epoch, AFTER_START)
            .await,
    );
    for member in [&owner, &far_end] {
        fixture
            .handle
            .ensure_member_live(&member.identity)
            .await
            .unwrap_or_else(|error| panic!("{} is live after the start: {error}", member.identity));
    }

    assert_eq!(
        trust_of(&fixture, &[&owner, &far_end]).await,
        before,
        "both ends hold the trust rows the wiring call gave them"
    );
    assert_wired_pair(&fixture, &owner, &far_end, "after the start").await;

    fixture.handle.shutdown().await.expect("shutdown the mob");
}

/// An agent that stays repair-blocked for its wiring is rebuilt by the pass
/// like any other, and the rebuild restores the trust of the edges the mob
/// holds for it. Under identity-owned custody an agent that a wiring call
/// joined to a larger-named member is held to that edge at its next change:
/// it reads repair-blocked and its change is not active, both ends still
/// hold each other's trust and can message each other, and a host delivery
/// to the agent stays refused while it is blocked.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_still_blocked_agent_keeps_the_trust_of_its_wired_peer() {
    let fixture = WiringFixture::create("rebuild-trust-still-blocked").await;
    let blocked = seat(&fixture, "worker-c").await;
    let larger = seat(&fixture, "worker-d").await;
    fixture
        .adopt(
            &blocked.identity,
            &blocked.session,
            IdentityWiringCustody::IdentityOwned,
            &[],
        )
        .await;
    assert_converged(
        &blocked.identity,
        &fixture.settled(&blocked.identity, 1).await,
    );
    wire(&fixture, &blocked, &larger).await;
    assert_wired_pair(&fixture, &blocked, &larger, "before the change").await;
    let before = trust_of(&fixture, &[&blocked, &larger]).await;

    let builds = persistent_mock_comms_builds(&blocked.session);
    fixture.change(&blocked.identity).await;
    let held = fixture.settled(&blocked.identity, 2).await;
    assert_eq!(
        held.decision,
        Some(IdentityReconcileDecision::RepairBlocked),
        "the agent is held to the edge its declaration does not state: {held:?}"
    );
    assert_eq!(
        held.active_intent_revision,
        Some(1),
        "the change is not active"
    );
    assert!(
        persistent_mock_comms_builds(&blocked.session) > builds,
        "the pass rebuilt the agent although it stays blocked"
    );

    assert_eq!(
        trust_of(&fixture, &[&blocked, &larger]).await,
        before,
        "both ends hold the trust rows they held before the change"
    );
    assert_wired_pair(&fixture, &blocked, &larger, "while the agent is blocked").await;
    let delivery = match fixture.handle.member(&blocked.identity).await {
        Ok(member) => member
            .send("a message", HandlingMode::Queue)
            .await
            .map(|_| ()),
        Err(error) => Err(error),
    };
    assert!(
        matches!(
            delivery,
            Err(MobError::IdentityConvergenceAdmissionClosed { .. })
        ),
        "a host delivery to the blocked agent is refused: {delivery:?}"
    );

    fixture.handle.shutdown().await.expect("shutdown the mob");
}

/// A wiring call still in flight when the pass comes to rebuild one of its
/// members settles first: the pass waits for it, as a retire or a respawn
/// does, so the rebuild's restore never repairs an edge whose wiring call
/// then takes it back. Once both have settled, each end holds a trust row
/// for the other exactly when the mob holds the edge.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_wiring_call_in_flight_settles_before_the_pass_rebuilds_its_member() {
    let fixture = WiringFixture::create("rebuild-trust-wire-in-flight").await;
    let adopted = seat(&fixture, "worker-a").await;
    let peer = seat(&fixture, "worker-b").await;

    // The wiring call's trust step on the adopted member's runtime is held,
    // so the call is in flight: the mob holds the edge, the trust is not
    // installed yet.
    let gate = Arc::new(TestRuntimeControlBarrier::new());
    persistent_mock_comms_runtime(&adopted.session)
        .expect("the adopted member's comms runtime")
        .park_trust_mutations(gate.clone());
    let wiring = {
        let handle = fixture.handle.clone();
        let (member, other) = (adopted.identity.clone(), peer.identity.clone());
        tokio::spawn(async move { handle.wire(member, other).await })
    };
    tokio::time::timeout(STEP, async {
        while gate.boundary_calls.load(Ordering::Acquire) == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the wiring call reaches its trust step");

    let builds = persistent_mock_comms_builds(&adopted.session);
    fixture
        .adopt(
            &adopted.identity,
            &adopted.session,
            IdentityWiringCustody::ExternalManaged,
            &[],
        )
        .await;
    // Whether the pass rebuilt the member under the wiring call.
    let rebuilt_under_the_call = tokio::time::timeout(Duration::from_secs(2), async {
        while persistent_mock_comms_builds(&adopted.session) == builds {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .is_ok();

    gate.release_all();
    let wired = tokio::time::timeout(STEP, wiring)
        .await
        .expect("the wiring call answers")
        .expect("the wiring task ends");
    assert_converged(
        &adopted.identity,
        &fixture.settled(&adopted.identity, 1).await,
    );

    let held = held_edges(&fixture).contains(&held_edge(&adopted, &peer));
    let adopted_trusts_peer = trusted_names(&fixture, &adopted)
        .await
        .contains(&peer.comms_name);
    let peer_trusts_adopted = trusted_names(&fixture, &peer)
        .await
        .contains(&adopted.comms_name);
    assert_eq!(
        (adopted_trusts_peer, peer_trusts_adopted),
        (held, held),
        "each end holds a trust row for the other exactly when the mob holds the edge \
         (edge held: {held}; the wiring call answered {wired:?})"
    );
    assert!(
        !rebuilt_under_the_call,
        "the pass waits for the wiring call before it rebuilds the member"
    );
    wired.expect("the wiring call completes");
    assert_wired_pair(&fixture, &adopted, &peer, "after both settled").await;

    fixture.handle.shutdown().await.expect("shutdown the mob");
}

/// The same wiring call rolls back while the pass waits for it: its second
/// trust step, on the peer's runtime, fails once the adoption is recorded,
/// so the call takes its edge back and removes the row it installed. The
/// pass then rebuilds the member, and nothing is left of the pair: the mob
/// holds no edge for it and neither end holds a trust row for the other.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_wiring_call_that_rolls_back_while_the_pass_waits_leaves_no_trust_row() {
    let fixture = WiringFixture::create("rebuild-trust-wire-rolls-back").await;
    let adopted = seat(&fixture, "worker-a").await;
    let peer = seat(&fixture, "worker-b").await;

    // The wiring call is in flight: its first trust step, on the adopted
    // member's runtime, is held.
    let gate = Arc::new(TestRuntimeControlBarrier::new());
    persistent_mock_comms_runtime(&adopted.session)
        .expect("the adopted member's comms runtime")
        .park_trust_mutations(gate.clone());
    let wiring = {
        let handle = fixture.handle.clone();
        let (member, other) = (adopted.identity.clone(), peer.identity.clone());
        tokio::spawn(async move { handle.wire(member, other).await })
    };
    tokio::time::timeout(STEP, async {
        while gate.boundary_calls.load(Ordering::Acquire) == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the wiring call reaches its trust step");
    assert!(
        held_edges(&fixture).contains(&held_edge(&adopted, &peer)),
        "the mob holds the edge while the wiring call is in flight"
    );

    let builds = persistent_mock_comms_builds(&adopted.session);
    fixture
        .adopt(
            &adopted.identity,
            &adopted.session,
            IdentityWiringCustody::ExternalManaged,
            &[],
        )
        .await;
    // Whether the pass rebuilt the member under the wiring call.
    let rebuilt_under_the_call = tokio::time::timeout(Duration::from_secs(2), async {
        while persistent_mock_comms_builds(&adopted.session) == builds {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .is_ok();

    // The call's second trust step fails, so the call rolls back.
    let peer_comms =
        persistent_mock_comms_runtime(&peer.session).expect("the peer's comms runtime");
    peer_comms.set_behavior(MockCommsBehavior {
        fail_add_trust: true,
        ..MockCommsBehavior::default()
    });
    gate.release_all();
    let wired = tokio::time::timeout(STEP, wiring)
        .await
        .expect("the wiring call answers")
        .expect("the wiring task ends");
    assert!(wired.is_err(), "the wiring call rolls back: {wired:?}");
    // The peer takes trust rows again: a later add for the pair would show.
    peer_comms.set_behavior(MockCommsBehavior::default());
    assert_converged(
        &adopted.identity,
        &fixture.settled(&adopted.identity, 1).await,
    );
    assert!(
        persistent_mock_comms_builds(&adopted.session) > builds,
        "the pass rebuilds the member once the wiring call has settled"
    );

    assert!(
        !held_edges(&fixture).contains(&held_edge(&adopted, &peer)),
        "the mob holds no edge for the pair after the rollback"
    );
    let adopted_trusts_peer = trusted_names(&fixture, &adopted)
        .await
        .contains(&peer.comms_name);
    let peer_trusts_adopted = trusted_names(&fixture, &peer)
        .await
        .contains(&adopted.comms_name);
    assert_eq!(
        (adopted_trusts_peer, peer_trusts_adopted),
        (false, false),
        "neither end holds a trust row for a pair the mob holds no edge for"
    );
    assert!(
        !rebuilt_under_the_call,
        "the pass waits for the wiring call before it rebuilds the member"
    );
    let members = fixture.handle.list_members().await;
    for (holder, other) in [(&adopted, &peer), (&peer, &adopted)] {
        let entry = members
            .iter()
            .find(|entry| entry.agent_identity == holder.identity)
            .unwrap_or_else(|| panic!("{} is listed", holder.identity));
        assert!(
            !entry.wired_to.contains(&other.identity),
            "the member list shows no edge between the pair: {:?}",
            entry.wired_to
        );
    }

    fixture.handle.shutdown().await.expect("shutdown the mob");
}

/// A wiring call that arrives while the pass's retirement of one of its
/// members is starting is not admitted under it: a retirement's stages are
/// effects of the member, and a wire or unwire naming a member waits for the
/// member's effect in flight. So the call cannot take an edge while the
/// member is being retired and rebuilt. Once the retirement has gone on and
/// the call has answered, each end holds a trust row for the other exactly
/// when the mob holds the edge.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_wiring_call_that_arrives_while_the_retirement_starts_waits_for_it() {
    use crate::runtime::actor::retirement_stage_hold;

    let fixture = WiringFixture::create("rebuild-trust-wire-during-retire").await;
    let adopted = seat(&fixture, "worker-a").await;
    let peer = seat(&fixture, "worker-b").await;

    // The retirement's first stage is held: the pass has decided to retire
    // the member, and the member is not marked retiring yet.
    let retirement =
        retirement_stage_hold::install(&adopted.identity, "retire-live-mutation-drain");
    let builds = persistent_mock_comms_builds(&adopted.session);
    fixture
        .adopt(
            &adopted.identity,
            &adopted.session,
            IdentityWiringCustody::ExternalManaged,
            &[],
        )
        .await;
    tokio::time::timeout(STEP, retirement.entered())
        .await
        .expect("the pass starts the member's retirement");

    // A wiring call for a new edge arrives now. It is not admitted while the
    // retirement's stage is in flight: the mob holds no edge for the pair
    // and the call has not answered.
    let wiring = {
        let handle = fixture.handle.clone();
        let (member, other) = (adopted.identity.clone(), peer.identity.clone());
        tokio::spawn(async move { handle.wire(member, other).await })
    };
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        !held_edges(&fixture).contains(&held_edge(&adopted, &peer)),
        "the mob holds no edge for the pair while the retirement's stage is in flight"
    );
    assert!(
        !wiring.is_finished(),
        "the wiring call waits for the member's retirement"
    );
    assert_eq!(
        persistent_mock_comms_builds(&adopted.session),
        builds,
        "the member is not rebuilt while its retirement is held"
    );

    // The retirement goes on; the pass rebuilds the member; the call answers.
    retirement.release();
    assert_converged(
        &adopted.identity,
        &fixture.settled(&adopted.identity, 1).await,
    );
    assert!(
        persistent_mock_comms_builds(&adopted.session) > builds,
        "the pass rebuilds the member"
    );
    let wired = tokio::time::timeout(STEP, wiring)
        .await
        .expect("the wiring call answers")
        .expect("the wiring task ends");

    let held = held_edges(&fixture).contains(&held_edge(&adopted, &peer));
    let adopted_trusts_peer = trusted_names(&fixture, &adopted)
        .await
        .contains(&peer.comms_name);
    let peer_trusts_adopted = trusted_names(&fixture, &peer)
        .await
        .contains(&adopted.comms_name);
    assert_eq!(
        (adopted_trusts_peer, peer_trusts_adopted),
        (held, held),
        "each end holds a trust row for the other exactly when the mob holds the edge \
         (edge held: {held}; the wiring call answered {wired:?})"
    );
    assert_eq!(
        wired.is_ok(),
        held,
        "the mob holds the edge exactly when the wiring call succeeded: {wired:?}"
    );

    fixture.handle.shutdown().await.expect("shutdown the mob");
}

/// The pair is apart: the mob holds no edge for it, neither end holds a
/// trust row for the other or can message it, and the member list shows no
/// edge between the two.
async fn assert_apart(fixture: &WiringFixture, left: &Member, right: &Member, when: &str) {
    assert!(
        !held_edges(fixture).contains(&held_edge(left, right)),
        "{when}: the mob holds no edge for the pair"
    );
    assert_silenced(fixture, left, right).await;
    let members = fixture.handle.list_members().await;
    for (holder, peer) in [(left, right), (right, left)] {
        let entry = members
            .iter()
            .find(|entry| entry.agent_identity == holder.identity)
            .unwrap_or_else(|| panic!("{when}: {} is listed", holder.identity));
        assert!(
            !entry.wired_to.contains(&peer.identity),
            "{when}: the member list shows no edge between the pair: {:?}",
            entry.wired_to
        );
    }
}

/// A pair that was wired and then unwired on purpose stays apart: through
/// the adoption of one of its members, through that member's declaration
/// change and through a stop and start, the mob holds no edge for the pair,
/// neither end holds a trust row for the other or can message it, and the
/// member list shows no edge. The rebuild restores what the mob holds, and
/// a wiring that an unwire ended is not among it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pair_unwired_on_purpose_stays_apart_through_adoption_change_and_restart() {
    let mut fixture = WiringFixture::create("rebuild-trust-unwired-stays-apart").await;
    let adopted = seat(&fixture, "worker-a").await;
    let other = seat(&fixture, "worker-b").await;
    wire(&fixture, &adopted, &other).await;
    assert_wired_pair(&fixture, &adopted, &other, "after the wiring call").await;
    fixture
        .handle
        .unwire(adopted.identity.clone(), other.identity.clone())
        .await
        .expect("unwire the pair");
    assert_apart(&fixture, &adopted, &other, "after the unwire").await;

    let builds = persistent_mock_comms_builds(&adopted.session);
    fixture
        .adopt(
            &adopted.identity,
            &adopted.session,
            IdentityWiringCustody::ExternalManaged,
            &[],
        )
        .await;
    assert_converged(
        &adopted.identity,
        &fixture.settled(&adopted.identity, 1).await,
    );
    assert!(
        persistent_mock_comms_builds(&adopted.session) > builds,
        "the adoption rebuilt the member"
    );
    assert_apart(&fixture, &adopted, &other, "after the adoption").await;

    let builds = persistent_mock_comms_builds(&adopted.session);
    fixture.change(&adopted.identity).await;
    let changed = fixture.settled(&adopted.identity, 2).await;
    assert_converged(&adopted.identity, &changed);
    assert!(
        persistent_mock_comms_builds(&adopted.session) > builds,
        "the change rebuilt the member"
    );
    assert_apart(&fixture, &adopted, &other, "after the declaration change").await;

    fixture.restart().await;
    let after_epoch = changed.lease_epoch.expect("the change held a lease");
    assert_converged(
        &adopted.identity,
        &fixture
            .settled_within(&adopted.identity, 2, after_epoch, AFTER_START)
            .await,
    );
    for member in [&adopted, &other] {
        fixture
            .handle
            .ensure_member_live(&member.identity)
            .await
            .unwrap_or_else(|error| panic!("{} is live after the start: {error}", member.identity));
    }
    assert_apart(&fixture, &adopted, &other, "after the start").await;

    fixture.handle.shutdown().await.expect("shutdown the mob");
}
