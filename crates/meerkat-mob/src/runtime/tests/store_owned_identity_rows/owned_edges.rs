//! Owned edges (0.9.11): the edges an identity change declares, each owned
//! by the identity whose change declared it, realized while both its ends
//! are seated, and ended only by its owner's change, a policy reload that
//! denies it, the removal of either end and the mob's destroy (R3.1 to R3.7,
//! and the rulings on an edge's end and realization).

use super::*;
use crate::identity::{OwnedEdgeAdmission, OwnedIdentityEdge};
use crate::runtime::{OwnedEdgeEndOutcome, StoreOwnedIdentityUpdate};

/// An edge to `peer`, admitted by the peer rule `team-peers`.
fn edge(peer: &str) -> OwnedIdentityEdge {
    OwnedIdentityEdge::new(
        peer,
        OwnedEdgeAdmission::rule("team-peers").expect("a peer rule id"),
    )
}

/// A host surface's create of `name` on the worker profile, listing
/// `peers`, as `operation_id`.
fn create_with_peers(operation_id: &str, name: &str, peers: &[&str]) -> StoreOwnedIdentityCreate {
    StoreOwnedIdentityCreate::new(
        operation(operation_id),
        IdentityChangeRequesterInput::host("console"),
        vec![
            CreatedIdentityRequest::new(spec(name, "worker"))
                .with_peers(peers.iter().map(|peer| edge(peer)).collect()),
        ],
    )
}

impl StoreOwnedMob {
    /// A host surface's update of `name`'s peers as `operation_id`, its spec
    /// unchanged: the edges to `add` declared, those to `remove` ended.
    async fn update_peers(
        &self,
        operation_id: &str,
        name: &str,
        add: &[&str],
        remove: &[&str],
    ) -> Result<crate::runtime::IdentityUpdateOutcome, MobError> {
        let spec = self.row(name).await.spec.expect("the row holds its spec");
        let expected = spec.spec_digest().expect("the spec's digest");
        tokio::time::timeout(
            STEP,
            self.handle.update_store_owned_identity(
                StoreOwnedIdentityUpdate::new(
                    operation(operation_id),
                    IdentityChangeRequesterInput::host("console"),
                    spec,
                    expected,
                )
                .with_peers_add(add.iter().map(|peer| edge(peer)).collect())
                .with_peers_remove(remove.iter().map(|peer| id(peer)).collect()),
                ALLOW,
            ),
        )
        .await
        .expect("the update answers")
    }

    /// The peers `name`'s row owns edges to.
    async fn owned_peers(&self, name: &str) -> BTreeSet<AgentIdentity> {
        self.row(name)
            .await
            .owned_edges
            .into_iter()
            .map(|owned| owned.peer)
            .collect()
    }

    /// Wait, within one step, until `a` and `b` are wired (`wired`) or not.
    async fn wired_within(&self, a: &str, b: &str, wired: bool) {
        let deadline = tokio::time::Instant::now() + STEP;
        while self.wired_to(a).await.contains(&id(b)) != wired {
            assert!(
                tokio::time::Instant::now() < deadline,
                "{a} and {b} never became {}: {a} is wired to {:?}",
                if wired { "wired" } else { "unwired" },
                self.wired_to(a).await
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// A plain unwire of `a` and `b`: the owner it is refused for, or `None`
    /// when it unwired the edge.
    async fn plain_unwire_owner(&self, a: &str, b: &str) -> Option<AgentIdentity> {
        match tokio::time::timeout(STEP, self.handle.unwire(id(a), id(b)))
            .await
            .expect("the unwire answers")
        {
            Err(MobError::OwnedIdentityEdge { owner, .. }) => Some(owner),
            Ok(()) => None,
            Err(other) => panic!("the plain unwire of {a} and {b} failed: {other:?}"),
        }
    }

    /// The host's end of the edge `owner` holds to `peer`: its policy reload
    /// denies it.
    async fn deny(&self, owner: &str, peer: &str) -> OwnedEdgeEndOutcome {
        tokio::time::timeout(
            STEP,
            self.handle.end_denied_identity_edge(id(owner), id(peer)),
        )
        .await
        .expect("the end answers")
        .unwrap_or_else(|error| panic!("the end of {owner}'s edge to {peer} failed: {error:?}"))
    }
}

/// Row (RED): a create's peer is an owned edge: the creating identity's row
/// holds it with the rule that admitted it, the edge is wired, and after a
/// restart it is still that identity's, wired, and refused a plain unwire.
///
/// RED with no owner recorded (the declaration's and the recovery's record
/// of the owner dropped): the edge is never wired.
#[tokio::test]
async fn an_owned_edge_survives_restart_with_its_owner_and_rule() {
    let mut mob = StoreOwnedMob::new("owned-restart").await;
    committed(
        mob.create(host_create("create-beta", &["beta"]), ALLOW)
            .await,
    );
    committed(
        mob.create(create_with_peers("create-alpha", "alpha", &["beta"]), ALLOW)
            .await,
    );
    assert_eq!(
        mob.row("alpha").await.owned_edges,
        vec![edge("beta")],
        "alpha's row owns the edge, with its rule"
    );
    assert!(
        mob.owned_peers("beta").await.is_empty(),
        "beta owns nothing"
    );
    mob.wired_within("alpha", "beta", true).await;

    mob.restart_as(MobCompositionAuthority::Authoritative).await;
    assert_eq!(mob.row("alpha").await.owned_edges, vec![edge("beta")]);
    mob.wired_within("alpha", "beta", true).await;
    assert_eq!(
        mob.plain_unwire_owner("beta", "alpha").await,
        Some(id("alpha")),
        "the restarted edge is alpha's"
    );
}

/// Row (RED): a second declaration of an owned edge gives no custody: the
/// other end's update listing it commits, its row owns nothing, and its
/// change cannot end the edge.
///
/// RED with no owner recorded: the second declaration is declared too, and
/// the other end's row owns the edge.
#[tokio::test]
async fn a_second_declaration_gives_no_custody() {
    let mob = StoreOwnedMob::new("owned-second").await;
    committed(
        mob.create(host_create("create-beta", &["beta"]), ALLOW)
            .await,
    );
    committed(
        mob.create(create_with_peers("create-alpha", "alpha", &["beta"]), ALLOW)
            .await,
    );
    let updated = mob
        .update_peers("beta-adds-alpha", "beta", &["alpha"], &[])
        .await
        .expect("beta's update commits");
    assert!(matches!(
        updated.change,
        IdentityChangeOutcome::Committed { .. }
    ));
    assert!(
        mob.owned_peers("beta").await.is_empty(),
        "beta's declaration gives it no custody"
    );
    assert_eq!(mob.owned_peers("alpha").await, BTreeSet::from([id("beta")]));
    let refused = refusal(
        mob.update_peers("beta-removes-alpha", "beta", &[], &["alpha"])
            .await,
    );
    assert!(
        invalid_request_naming(&refused, "owns no edge"),
        "{refused:?}"
    );
    assert_eq!(
        mob.plain_unwire_owner("alpha", "beta").await,
        Some(id("alpha"))
    );
}

/// Row (RED): an edge to an identity no member holds (archived) is declared
/// and left unrealized, and the identity's seat (its unarchive) realizes it.
///
/// RED with no owner recorded: the seat wires nothing.
#[tokio::test]
async fn an_edge_to_an_unseated_identity_is_realized_at_its_seat() {
    let mob = StoreOwnedMob::new("owned-unseated").await;
    committed(
        mob.create(host_create("create-gamma", &["gamma"]), ALLOW)
            .await,
    );
    committed(mob.archive("archive-gamma", "gamma").await);
    mob.unseated_for("gamma", UnseatedEntryCause::Archived)
        .await;
    committed(
        mob.create(
            create_with_peers("create-alpha", "alpha", &["gamma"]),
            ALLOW,
        )
        .await,
    );
    assert_eq!(
        mob.owned_peers("alpha").await,
        BTreeSet::from([id("gamma")])
    );
    assert!(
        !mob.wired_to("alpha").await.contains(&id("gamma")),
        "an unseated end leaves the edge unrealized"
    );
    committed(mob.unarchive("unarchive-gamma", "gamma").await);
    mob.wired_within("alpha", "gamma", true).await;
}

/// Row (RED): an identity's change ends its own edges only: an update that
/// removes another identity's edge is refused and the edge stays its
/// owner's; the owner's own change ends it, and its realization is unwired.
///
/// RED with no owner recorded: a plain unwire of the other identity's edge
/// is admitted.
#[tokio::test]
async fn two_owners_cannot_remove_each_others_edges() {
    let mob = StoreOwnedMob::new("owned-two-owners").await;
    committed(
        mob.create(host_create("create-gamma", &["gamma"]), ALLOW)
            .await,
    );
    committed(
        mob.create(
            create_with_peers("create-alpha", "alpha", &["gamma"]),
            ALLOW,
        )
        .await,
    );
    committed(
        mob.create(create_with_peers("create-beta", "beta", &["alpha"]), ALLOW)
            .await,
    );
    // beta owns beta-alpha, alpha owns alpha-gamma.
    let refused = refusal(
        mob.update_peers("alpha-removes-beta", "alpha", &[], &["beta"])
            .await,
    );
    assert!(
        invalid_request_naming(&refused, "owns no edge"),
        "{refused:?}"
    );
    assert_eq!(mob.owned_peers("beta").await, BTreeSet::from([id("alpha")]));
    assert_eq!(
        mob.plain_unwire_owner("alpha", "beta").await,
        Some(id("beta"))
    );
    assert_eq!(
        mob.plain_unwire_owner("gamma", "alpha").await,
        Some(id("alpha"))
    );
    mob.wired_within("alpha", "beta", true).await;
    mob.wired_within("alpha", "gamma", true).await;

    mob.update_peers("beta-removes-alpha", "beta", &[], &["alpha"])
        .await
        .expect("the owner's change ends its edge");
    assert!(mob.owned_peers("beta").await.is_empty());
    mob.wired_within("alpha", "beta", false).await;
    assert_eq!(
        mob.owned_peers("alpha").await,
        BTreeSet::from([id("gamma")]),
        "alpha's own edge stays"
    );
}

/// Row (RED): no change of a third identity ends an owned edge: its update
/// with its own peers, its archive, its unarchive and its removal leave the
/// edge its owner's and wired.
///
/// RED with no owner recorded: a plain unwire of the edge is admitted.
#[tokio::test]
async fn an_owned_edge_survives_any_change_of_a_third_identity() {
    let mob = StoreOwnedMob::new("owned-third").await;
    committed(
        mob.create(host_create("create-beta-gamma", &["beta", "gamma"]), ALLOW)
            .await,
    );
    committed(
        mob.create(create_with_peers("create-alpha", "alpha", &["beta"]), ALLOW)
            .await,
    );
    mob.update_peers("gamma-adds-beta", "gamma", &["beta"], &[])
        .await
        .expect("gamma's update commits");
    committed(mob.archive("archive-gamma", "gamma").await);
    mob.unseated_for("gamma", UnseatedEntryCause::Archived)
        .await;
    committed(mob.unarchive("unarchive-gamma", "gamma").await);
    committed(mob.remove("remove-gamma", "gamma").await);
    mob.removed("gamma").await;
    assert_eq!(mob.owned_peers("alpha").await, BTreeSet::from([id("beta")]));
    assert_eq!(
        mob.plain_unwire_owner("alpha", "beta").await,
        Some(id("alpha"))
    );
    mob.wired_within("alpha", "beta", true).await;
}

/// Row (RED): the removal of an end ends that end's owned edges, whichever
/// end owns them, and no other: the owner's row loses the edge in the
/// removal's own store transaction, the removed identity's tombstone owns
/// nothing, and the other edges stay.
///
/// RED with no owner recorded: no edge is wired.
#[tokio::test]
async fn an_endpoint_removal_ends_its_edges_and_no_other() {
    let mob = StoreOwnedMob::new("owned-endpoint-removal").await;
    committed(
        mob.create(host_create("create-beta-gamma", &["beta", "gamma"]), ALLOW)
            .await,
    );
    committed(
        mob.create(
            create_with_peers("create-alpha", "alpha", &["beta", "gamma"]),
            ALLOW,
        )
        .await,
    );
    mob.update_peers("beta-adds-gamma", "beta", &["gamma"], &[])
        .await
        .expect("beta's update commits");
    // alpha owns alpha-beta and alpha-gamma; beta owns beta-gamma.
    mob.wired_within("alpha", "beta", true).await;
    mob.wired_within("beta", "gamma", true).await;
    committed(mob.remove("remove-beta", "beta").await);
    mob.removed("beta").await;
    assert_eq!(
        mob.owned_peers("alpha").await,
        BTreeSet::from([id("gamma")]),
        "alpha's edge to beta ended with beta"
    );
    assert!(
        mob.row("beta").await.owned_edges.is_empty(),
        "beta's tombstone owns nothing"
    );
    mob.wired_within("alpha", "beta", false).await;
    assert_eq!(
        mob.plain_unwire_owner("alpha", "gamma").await,
        Some(id("alpha")),
        "alpha's edge to gamma stays"
    );
}

/// Row (RED): a removal ends its identity's owned edges in its own store
/// transaction, with its tombstone: a store that refuses the removal's write
/// leaves both the edge and the identity as they were (neither written),
/// and the removal's retry writes both.
///
/// RED with no owner recorded: a plain unwire of the edge is admitted.
#[tokio::test]
async fn a_removal_ends_its_owned_edges_with_its_tombstone_in_one_transaction() {
    let mut storage = MobStorage::in_memory();
    let _failing = FailingChangeCommit::wrap(&mut storage, |write| {
        write.receipt.change == IdentityChangeKind::Remove
    });
    let mob = StoreOwnedMob::build(
        store_owned_definition("owned-removal-transaction"),
        storage,
        |builder| builder,
    )
    .await;
    committed(
        mob.create(host_create("create-beta", &["beta"]), ALLOW)
            .await,
    );
    committed(
        mob.create(create_with_peers("create-alpha", "alpha", &["beta"]), ALLOW)
            .await,
    );
    assert!(mob.remove("remove-beta", "beta").await.is_err());
    assert_eq!(
        mob.owned_peers("alpha").await,
        BTreeSet::from([id("beta")]),
        "a refused write ends no edge"
    );
    assert_eq!(
        mob.row("beta").await.presence,
        CreatedIdentityPresence::Present,
        "and writes no tombstone"
    );
    assert_eq!(
        mob.plain_unwire_owner("alpha", "beta").await,
        Some(id("alpha")),
        "the machine still holds the edge alpha's"
    );

    committed(mob.remove("remove-beta-again", "beta").await);
    mob.removed("beta").await;
    assert!(mob.owned_peers("alpha").await.is_empty());
    assert_eq!(
        mob.row("beta").await.presence,
        CreatedIdentityPresence::Removed
    );
}

/// Row (RED): an archived identity's owned edges stay declared, unrealized
/// while it is archived, and its unarchive realizes them again with no new
/// declaration.
///
/// RED with no owner recorded: no edge is wired.
#[tokio::test]
async fn an_archived_identitys_edges_return_at_unarchive() {
    let mob = StoreOwnedMob::new("owned-archive").await;
    committed(
        mob.create(host_create("create-beta", &["beta"]), ALLOW)
            .await,
    );
    committed(
        mob.create(create_with_peers("create-alpha", "alpha", &["beta"]), ALLOW)
            .await,
    );
    mob.wired_within("alpha", "beta", true).await;
    committed(mob.archive("archive-alpha", "alpha").await);
    mob.unseated_for("alpha", UnseatedEntryCause::Archived)
        .await;
    assert!(
        !mob.wired_to("beta").await.contains(&id("alpha")),
        "the archive unrealized the edge"
    );
    assert_eq!(
        mob.owned_peers("alpha").await,
        BTreeSet::from([id("beta")]),
        "and kept it declared"
    );
    committed(mob.unarchive("unarchive-alpha", "alpha").await);
    mob.wired_within("alpha", "beta", true).await;
    assert_eq!(
        mob.plain_unwire_owner("beta", "alpha").await,
        Some(id("alpha"))
    );
}

/// Row (RED): an unlisted roster-owned identity's owned edge needs no
/// durable record of the unlisting: the owner's row keeps the edge, nothing
/// else is written, and the identity's re-listing (its seat) realizes the
/// edge again.
///
/// RED with no owner recorded: no edge is wired.
#[tokio::test]
async fn an_unlisted_identitys_owned_edge_returns_at_its_relisting() {
    let mob = StoreOwnedMob::new("owned-unlist").await;
    mob.declare(&["listed"]).await;
    committed(
        mob.create(
            create_with_peers("create-alpha", "alpha", &["listed"]),
            ALLOW,
        )
        .await,
    );
    mob.wired_within("alpha", "listed", true).await;
    let rows_before = mob.rows().await;
    tokio::time::timeout(STEP, mob.handle.unlist_declared_identity(&id("listed")))
        .await
        .expect("the unlist answers")
        .expect("the unlist runs");
    let deadline = tokio::time::Instant::now() + STEP;
    while mob.is_member("listed").await {
        assert!(tokio::time::Instant::now() < deadline, "listed never left");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(
        mob.rows().await,
        rows_before,
        "the unlisting writes no row: alpha's keeps the edge"
    );
    mob.declare(&["listed"]).await;
    mob.wired_within("alpha", "listed", true).await;
    assert_eq!(
        mob.plain_unwire_owner("listed", "alpha").await,
        Some(id("alpha"))
    );
}

/// Row: an edge enables no tool: a profile that denies the peer messaging
/// tools still declares them denied in the build of an identity whose owned
/// edge is wired.
///
/// A guard: with no owner recorded its edge is never wired, and the row
/// stops before its tool assertion.
#[tokio::test]
async fn an_edge_enables_no_tool() {
    const PEER_TOOLS: [&str; 2] = ["send_message", "send_request"];
    let mut definition = store_owned_definition("owned-no-tool");
    definition
        .profiles
        .get_mut(&ProfileName::from("worker"))
        .and_then(ProfileBinding::as_inline_mut)
        .expect("inline worker profile")
        .tools
        .deny = PEER_TOOLS.iter().map(|name| (*name).to_string()).collect();
    let mob = StoreOwnedMob::build(definition, MobStorage::in_memory(), |builder| builder).await;
    committed(
        mob.create(host_create("create-beta", &["beta"]), ALLOW)
            .await,
    );
    committed(
        mob.create(create_with_peers("create-alpha", "alpha", &["beta"]), ALLOW)
            .await,
    );
    mob.wired_within("alpha", "beta", true).await;
    tokio::time::timeout(STEP, mob.handle.ensure_member_live(&id("alpha")))
        .await
        .expect("the wake answers")
        .expect("the wake builds the member");
    let build = last_member_build(&mob.service, &id("alpha")).await;
    assert_eq!(
        build.declared_deny,
        PEER_TOOLS.iter().map(|name| (*name).to_string()).collect(),
        "the edge leaves the profile's denied peer tools denied"
    );
}

/// Row (RED): the host's policy reload ends an owned edge it denies,
/// whatever the custody of the edge's other end: the owner's row loses it
/// and its realization is unwired. An edge another identity owns, or none
/// does, changes nothing.
///
/// RED with no owner recorded: no edge is wired.
#[tokio::test]
async fn a_reload_that_denies_an_edge_removes_it_for_either_custody() {
    let mob = StoreOwnedMob::new("owned-reload").await;
    mob.declare(&["listed"]).await;
    committed(
        mob.create(host_create("create-beta", &["beta"]), ALLOW)
            .await,
    );
    committed(
        mob.create(
            create_with_peers("create-alpha", "alpha", &["beta", "listed"]),
            ALLOW,
        )
        .await,
    );
    mob.wired_within("alpha", "beta", true).await;
    mob.wired_within("alpha", "listed", true).await;

    assert_eq!(
        mob.deny("beta", "alpha").await,
        OwnedEdgeEndOutcome::NotOwner
    );
    assert_eq!(
        mob.deny("beta", "listed").await,
        OwnedEdgeEndOutcome::Absent
    );

    assert_eq!(
        mob.deny("alpha", "listed").await,
        OwnedEdgeEndOutcome::Ended
    );
    assert_eq!(mob.owned_peers("alpha").await, BTreeSet::from([id("beta")]));
    mob.wired_within("alpha", "listed", false).await;

    assert_eq!(mob.deny("alpha", "beta").await, OwnedEdgeEndOutcome::Ended);
    assert!(mob.owned_peers("alpha").await.is_empty());
    mob.wired_within("alpha", "beta", false).await;
    assert_eq!(mob.deny("alpha", "beta").await, OwnedEdgeEndOutcome::Absent);
}

/// Row (RED): a create wires only the peers it lists: its listed peer, and
/// neither another identity nor its requester; an agent requester that is
/// an identity may list itself, and its edge is the created identity's.
///
/// RED with no owner recorded: the listed peer is never wired.
#[tokio::test]
async fn a_create_wires_only_the_peers_it_lists() {
    let mob = StoreOwnedMob::new("owned-only-listed").await;
    committed(
        mob.create(
            host_create("create-creator-beta-gamma", &["creator", "beta", "gamma"]),
            ALLOW,
        )
        .await,
    );
    committed(
        mob.create(create_with_peers("create-alpha", "alpha", &["beta"]), ALLOW)
            .await,
    );
    mob.wired_within("alpha", "beta", true).await;
    assert_eq!(
        mob.wired_to("alpha").await,
        BTreeSet::from([id("beta")]),
        "alpha is wired to its listed peer only"
    );

    mob.handle
        .ensure_member_live(&id("creator"))
        .await
        .expect("wake the creator");
    let session = mob.session("creator").await;
    let child = spec("child", "worker");
    let grant = mob
        .admit(&session, "create-child", std::slice::from_ref(&child))
        .await
        .expect("the creator's grant admits the worker profile");
    committed(
        mob.create(
            StoreOwnedIdentityCreate::new(
                operation("create-child"),
                IdentityChangeRequesterInput::Agent { grant },
                vec![CreatedIdentityRequest::new(child).with_peers(vec![edge("creator")])],
            ),
            ALLOW,
        )
        .await,
    );
    mob.wired_within("child", "creator", true).await;
    assert_eq!(
        mob.wired_to("child").await,
        BTreeSet::from([id("creator")]),
        "the requester is wired to the identity it created only because the create lists it"
    );
    assert_eq!(
        mob.owned_peers("child").await,
        BTreeSet::from([id("creator")])
    );
    assert!(mob.owned_peers("creator").await.is_empty());
}

/// Row (RED): a create's peer that is no identity of the mob (a worker
/// member, or nothing) is refused `peer_unknown`, and nothing is written; a
/// create may list another identity of the same create.
///
/// RED with no owner recorded: the pair's second declaration is declared
/// too.
#[tokio::test]
async fn a_peer_that_is_no_identity_is_refused_peer_unknown() {
    let mob = StoreOwnedMob::new("owned-peer-unknown").await;
    mob.spawn_requester("worker-one").await;
    for peer in ["worker-one", "nobody"] {
        let refused = refusal(
            mob.create(create_with_peers("create-alpha", "alpha", &[peer]), ALLOW)
                .await,
        );
        assert_eq!(
            refused,
            IdentityChangeRefusal::PeerUnknown {
                identity: id("alpha"),
                peer: id(peer),
            }
        );
    }
    assert!(mob.rows().await.is_empty(), "nothing is written");

    let both = StoreOwnedIdentityCreate::new(
        operation("create-pair"),
        IdentityChangeRequesterInput::host("console"),
        vec![
            CreatedIdentityRequest::new(spec("alpha", "worker")).with_peers(vec![edge("beta")]),
            CreatedIdentityRequest::new(spec("beta", "worker")).with_peers(vec![edge("alpha")]),
        ],
    );
    committed(mob.create(both, ALLOW).await);
    assert_eq!(mob.owned_peers("alpha").await, BTreeSet::from([id("beta")]));
    assert!(
        mob.owned_peers("beta").await.is_empty(),
        "the pair's second declaration gives no custody"
    );
    mob.wired_within("alpha", "beta", true).await;
}

/// Row (RED): an owned edge to an unlisted roster-owned identity binds no
/// worker that takes the identity's free name: the edge stays declared, the
/// worker's spawn and the mob's realization pass wire nothing for it, a
/// plain wire and unwire of the pair are the worker's own, and once the
/// worker is gone the identity's re-listing realizes the edge again.
///
/// RED with every owned edge in force whatever its ends: the worker is
/// wired by the edge.
#[tokio::test]
async fn a_worker_that_takes_an_unlisted_peers_name_gets_no_owned_edge() {
    let mob = StoreOwnedMob::new("owned-unlisted-name").await;
    mob.declare(&["listed"]).await;
    committed(
        mob.create(
            create_with_peers("create-alpha", "alpha", &["listed"]),
            ALLOW,
        )
        .await,
    );
    mob.wired_within("alpha", "listed", true).await;
    tokio::time::timeout(STEP, mob.handle.unlist_declared_identity(&id("listed")))
        .await
        .expect("the unlist answers")
        .expect("the unlist runs");
    let deadline = tokio::time::Instant::now() + STEP;
    while mob.is_member("listed").await {
        assert!(tokio::time::Instant::now() < deadline, "listed never left");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    tokio::time::timeout(
        STEP,
        mob.handle
            .spawn_spec(SpawnMemberSpec::new("worker", id("listed"))),
    )
    .await
    .expect("the spawn answers")
    .expect("a worker takes the free name");
    assert!(
        matches!(
            mob.read("listed"),
            IdentityEntryRead::NotAnEntry { worker: Some(_) }
        ),
        "{:?}",
        mob.read("listed")
    );
    mob.handle.realize_owned_identity_edges().await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(
        !mob.wired_to("alpha").await.contains(&id("listed")),
        "alpha's owned edge to the unlisted identity wired the worker that took its name"
    );
    assert_eq!(
        mob.owned_peers("alpha").await,
        BTreeSet::from([id("listed")]),
        "alpha's row keeps the edge declared"
    );
    tokio::time::timeout(STEP, mob.handle.wire(id("alpha"), id("listed")))
        .await
        .expect("the wire answers")
        .expect("a plain wire of the worker");
    mob.wired_within("alpha", "listed", true).await;
    assert_eq!(
        mob.plain_unwire_owner("alpha", "listed").await,
        None,
        "the worker's plain wiring takes a plain unwire"
    );
    mob.wired_within("alpha", "listed", false).await;

    tokio::time::timeout(STEP, mob.handle.retire(id("listed")))
        .await
        .expect("the retirement answers")
        .expect("retire the worker");
    let deadline = tokio::time::Instant::now() + STEP;
    while mob.is_member("listed").await {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the worker never left"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    mob.declare(&["listed"]).await;
    mob.wired_within("alpha", "listed", true).await;
    assert_eq!(
        mob.plain_unwire_owner("alpha", "listed").await,
        Some(id("alpha")),
        "the re-listed identity's edge is alpha's again"
    );
}
