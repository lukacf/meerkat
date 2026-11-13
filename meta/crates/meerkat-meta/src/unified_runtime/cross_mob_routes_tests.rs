//! Rows for the link table and the wake forwarder. They drive the forwarder
//! with scripted answers of the remote member's owner; the rows of
//! `tests/cross_mob_wire_local_delivery.rs` drive it with two real mobs.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use meerkat_comms::{Inbox, Keypair, PeerMeta};
use tokio::sync::Notify;

use super::*;

/// A content send that observed the member missing, from a sender that is
/// not being built.
const CONTENT: WakeShape = WakeShape {
    input_class: PeerWakeInputClass::Content,
    sender_waking: false,
    observed_missing: true,
};

/// A content send from a member that is itself being built.
const FROM_A_MEMBER_BEING_BUILT: WakeShape = WakeShape {
    sender_waking: true,
    ..CONTENT
};

/// A silent lifecycle notice.
const LIFECYCLE_NOTICE: WakeShape = WakeShape {
    input_class: PeerWakeInputClass::Lifecycle,
    ..CONTENT
};

/// A mob's own owner of its wake routes. These rows never wake through it:
/// the scripted `ask` stands for its answer.
struct MobOwner {
    waking: AtomicBool,
}

impl InprocPeerWaker for MobOwner {
    fn wake(&self, _request: PeerWakeRequest) -> PeerWakeFuture {
        denied(PeerWakeDenial::Refused(PeerWakeRefusal::MobUnavailable))
    }

    fn member_is_waking(&self, _member: &str) -> bool {
        self.waking.load(Ordering::SeqCst)
    }
}

/// The part of the runtime that runs `namespace`'s mob: these rows start it
/// on first use, as a runtime's start-time read does.
fn part_of(namespace: &str) -> Arc<CrossMobWakeRoutes> {
    routes_of(namespace).unwrap_or_else(|| start_part(namespace))
}

/// The runtime that runs `namespace`'s mob shut down.
fn shut_down(namespace: &str) {
    if let Some(part) = routes_of(namespace) {
        clear_part(&part);
    }
}

fn owner_weak(owner: &Arc<MobOwner>) -> Weak<dyn InprocPeerWaker> {
    let owner: Arc<dyn InprocPeerWaker> = Arc::clone(owner) as Arc<dyn InprocPeerWaker>;
    Arc::downgrade(&owner)
}

/// Two mobs of this process: alice of mob A is live, bob of mob B is seated
/// and not built (his mob published its wake route for him).
struct Pair {
    mob_a: String,
    mob_b: String,
    ns_a: String,
    ns_b: String,
    alice_name: String,
    bob_name: String,
    alice: PubKey,
    bob: PubKey,
    bob_owner: Arc<MobOwner>,
    _alice_inbox: Inbox,
}

fn comms_name(mob_id: &str, member: &str) -> String {
    meerkat_core::MemberCommsName::new(mob_id, "worker", member)
        .expect("comms name")
        .to_string()
}

fn link(member: &str, local: &str, remote: &str, remote_key: PubKey) -> Link {
    Link {
        member: member.to_string(),
        local: local.to_string(),
        remote: remote.to_string(),
        remote_key,
    }
}

impl Pair {
    fn new(tag: &str) -> Self {
        let mob_a = format!("xmr-{tag}-a");
        let mob_b = format!("xmr-{tag}-b");
        let ns_a = mob_inproc_namespace_for_id(&mob_a).expect("namespace a");
        let ns_b = mob_inproc_namespace_for_id(&mob_b).expect("namespace b");
        let alice_name = comms_name(&mob_a, "alice");
        let bob_name = comms_name(&mob_b, "bob");
        let alice = Keypair::generate().public_key();
        let bob = Keypair::generate().public_key();
        let (alice_inbox, alice_sender) = Inbox::new_transport_only();
        let registry = InprocRegistry::global();
        assert!(
            !registry
                .register_with_meta_in_namespace(
                    &ns_a,
                    alice_name.clone(),
                    alice,
                    alice_sender,
                    PeerMeta::default(),
                )
                .is_rejected()
        );
        let bob_owner = Arc::new(MobOwner {
            waking: AtomicBool::new(false),
        });
        assert!(registry.publish_wake_route(&ns_b, bob, "bob", owner_weak(&bob_owner)));
        Self {
            mob_a,
            mob_b,
            ns_a,
            ns_b,
            alice_name,
            bob_name,
            alice,
            bob,
            bob_owner,
            _alice_inbox: alice_inbox,
        }
    }

    /// Mob A's row of alice names bob.
    fn a_holds(&self) {
        part_of(&self.ns_a).set_member_links(
            "alice",
            vec![link("alice", &self.alice_name, &self.bob_name, self.bob)],
        );
    }

    /// Mob B's row of bob names alice.
    fn b_holds(&self) {
        part_of(&self.ns_b).set_member_links(
            "bob",
            vec![link("bob", &self.bob_name, &self.alice_name, self.alice)],
        );
    }

    /// A content send of alice to bob that observed him missing.
    fn forward(
        &self,
        ask: impl FnOnce(&dyn InprocPeerWaker, String) -> PeerWakeFuture + Send + 'static,
    ) -> PeerWakeFuture {
        part_of(&self.ns_a).forward(self.alice, self.bob, &self.bob_name, CONTENT, ask)
    }

    fn route_to_bob(&self) -> Option<PubKey> {
        InprocRegistry::global()
            .peers_in_namespace(&self.ns_a)
            .into_iter()
            .find(|peer| peer.name == self.bob_name)
            .map(|peer| peer.pubkey)
    }

    /// Bob's own mob builds him: his inbox is registered in his namespace.
    fn build_bob(&self) -> Inbox {
        let (inbox, sender) = Inbox::new_transport_only();
        assert!(
            !InprocRegistry::global()
                .register_with_meta_in_namespace(
                    &self.ns_b,
                    self.bob_name.clone(),
                    self.bob,
                    sender,
                    PeerMeta::default(),
                )
                .is_rejected()
        );
        inbox
    }
}

impl Drop for Pair {
    fn drop(&mut self) {
        shut_down(&self.ns_a);
        shut_down(&self.ns_b);
        let registry = InprocRegistry::global();
        registry.unregister_in_namespace(&self.ns_a, &self.alice);
        registry.unregister_in_namespace(&self.ns_b, &self.bob);
        registry.retract_wake_route(&self.ns_b, &self.bob, &owner_weak(&self.bob_owner));
    }
}

fn never_asked(
    asked: &Arc<AtomicUsize>,
) -> impl FnOnce(&dyn InprocPeerWaker, String) -> PeerWakeFuture + Send + 'static {
    let asked = Arc::clone(asked);
    move |_owner, _member| {
        asked.fetch_add(1, Ordering::SeqCst);
        Box::pin(std::future::ready(Ok(PeerWakeAnswer::Ready)))
    }
}

#[tokio::test]
async fn k_waiting_sends_share_one_wake_of_the_remote_member() {
    let pair = Pair::new("k-waiters");
    pair.a_holds();
    pair.b_holds();
    let asked = Arc::new(AtomicUsize::new(0));
    let asked_member = Arc::new(Mutex::new(Vec::<String>::new()));
    let release = Arc::new(Notify::new());

    // K sends wait on bob. Each brings its own request; one is asked of
    // bob's mob, under that mob's own name for him.
    let waiting: Vec<PeerWakeFuture> = (0..8)
        .map(|_| {
            let asked = Arc::clone(&asked);
            let asked_member = Arc::clone(&asked_member);
            let release = Arc::clone(&release);
            pair.forward(move |_owner, member| {
                asked.fetch_add(1, Ordering::SeqCst);
                locked(&asked_member).push(member);
                Box::pin(async move {
                    release.notified().await;
                    Ok(PeerWakeAnswer::Ready)
                })
            })
        })
        .collect();
    assert_eq!(
        asked.load(Ordering::SeqCst),
        1,
        "one wake for K waiting sends"
    );
    assert_eq!(*locked(&asked_member), vec!["bob".to_string()]);
    assert_eq!(locked(&part_of(&pair.ns_a).state).pending.len(), 1);
    assert_eq!(
        pair.route_to_bob(),
        None,
        "no route before the member is ready"
    );

    // Bob's mob builds him and answers ready: every waiting send gets that
    // answer, and mob A's namespace routes bob's name to his inbox of now.
    let _bob_inbox = pair.build_bob();
    release.notify_one();
    let answers = futures::future::join_all(waiting).await;
    assert_eq!(answers.len(), 8);
    for answer in answers {
        assert_eq!(answer, Ok(PeerWakeAnswer::Ready));
    }
    assert_eq!(asked.load(Ordering::SeqCst), 1);
    assert_eq!(pair.route_to_bob(), Some(pair.bob));
    assert!(
        locked(&part_of(&pair.ns_a).state).pending.is_empty(),
        "an answered wake is no longer pending"
    );

    // The next send that misses bob is a new wake.
    let again = pair.forward(never_asked(&asked)).await;
    assert_eq!(again, Ok(PeerWakeAnswer::Ready));
    assert_eq!(asked.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn a_link_only_one_mob_holds_asks_nobody_and_installs_nothing() {
    let pair = Pair::new("one-sided");
    let asked = Arc::new(AtomicUsize::new(0));
    let _bob_inbox = pair.build_bob();

    // Neither mob holds the link.
    assert_eq!(
        pair.forward(never_asked(&asked)).await,
        Err(PeerWakeDenial::NotAMember)
    );
    // Only the sender's mob holds it.
    pair.a_holds();
    assert_eq!(
        pair.forward(never_asked(&asked)).await,
        Err(PeerWakeDenial::NotAMember)
    );
    // Only the recipient's mob holds it.
    shut_down(&pair.ns_a);
    pair.b_holds();
    assert_eq!(
        pair.forward(never_asked(&asked)).await,
        Err(PeerWakeDenial::NotAMember)
    );
    assert_eq!(
        asked.load(Ordering::SeqCst),
        0,
        "nothing was asked of bob's mob"
    );
    assert_eq!(pair.route_to_bob(), None, "nothing was installed");
    assert!(locked(&part_of(&pair.ns_a).state).pending.is_empty());

    // Both hold it: the wake is forwarded.
    pair.a_holds();
    assert_eq!(
        pair.forward(never_asked(&asked)).await,
        Ok(PeerWakeAnswer::Ready)
    );
    assert_eq!(asked.load(Ordering::SeqCst), 1);
    assert_eq!(pair.route_to_bob(), Some(pair.bob));
}

#[tokio::test]
async fn the_link_is_held_for_the_sender_and_the_keys_the_rows_name() {
    let pair = Pair::new("exact");
    pair.a_holds();
    pair.b_holds();
    let asked = Arc::new(AtomicUsize::new(0));
    let registry = InprocRegistry::global();
    let routes = part_of(&pair.ns_a);

    // Another live member of mob A, which holds no link to bob.
    let carol = Keypair::generate().public_key();
    let (_carol_inbox, carol_sender) = Inbox::new_transport_only();
    assert!(
        !registry
            .register_with_meta_in_namespace(
                &pair.ns_a,
                comms_name(&pair.mob_a, "carol"),
                carol,
                carol_sender,
                PeerMeta::default(),
            )
            .is_rejected()
    );
    let from_carol = routes.forward(
        carol,
        pair.bob,
        &pair.bob_name,
        CONTENT,
        never_asked(&asked),
    );
    assert_eq!(from_carol.await, Err(PeerWakeDenial::NotAMember));

    // A sender this namespace does not route at all.
    let stranger = Keypair::generate().public_key();
    let from_stranger = routes.forward(
        stranger,
        pair.bob,
        &pair.bob_name,
        CONTENT,
        never_asked(&asked),
    );
    assert_eq!(from_stranger.await, Err(PeerWakeDenial::NotAMember));

    // Another key under bob's name.
    let other_key = Keypair::generate().public_key();
    let to_other_key = routes.forward(
        pair.alice,
        other_key,
        &pair.bob_name,
        CONTENT,
        never_asked(&asked),
    );
    assert_eq!(to_other_key.await, Err(PeerWakeDenial::NotAMember));

    // Bob's mob names alice under another key than the sender's.
    part_of(&pair.ns_b).set_member_links(
        "bob",
        vec![link("bob", &pair.bob_name, &pair.alice_name, other_key)],
    );
    assert_eq!(
        pair.forward(never_asked(&asked)).await,
        Err(PeerWakeDenial::NotAMember)
    );
    assert_eq!(asked.load(Ordering::SeqCst), 0);
    registry.unregister_in_namespace(&pair.ns_a, &carol);
}

#[tokio::test]
async fn an_unwire_and_a_stopped_runtime_end_the_forwarding() {
    let pair = Pair::new("removed");
    pair.a_holds();
    pair.b_holds();
    let asked = Arc::new(AtomicUsize::new(0));
    let registry = InprocRegistry::global();
    assert!(registry.has_wake_route_in_namespace(&pair.ns_a, &pair.bob));
    assert!(registry.has_wake_route_in_namespace(&pair.ns_b, &pair.alice));

    // Bob is unwired from alice: mob B's link goes before its row changes.
    drop_member_link(&pair.mob_b, &AgentIdentity::from("bob"), &pair.alice_name);
    assert!(!registry.has_wake_route_in_namespace(&pair.ns_b, &pair.alice));
    assert_eq!(
        pair.forward(never_asked(&asked)).await,
        Err(PeerWakeDenial::NotAMember)
    );

    // Wired again, then mob B's runtime shuts down: its whole part goes,
    // and the route mob A held to bob's inbox goes with it.
    pair.b_holds();
    let _bob_inbox = pair.build_bob();
    assert_eq!(
        pair.forward(never_asked(&asked)).await,
        Ok(PeerWakeAnswer::Ready)
    );
    assert_eq!(pair.route_to_bob(), Some(pair.bob));
    shut_down(&pair.ns_b);
    assert_eq!(pair.route_to_bob(), None);
    assert!(!registry.has_wake_route_in_namespace(&pair.ns_b, &pair.alice));
    assert!(
        registry.has_wake_route_in_namespace(&pair.ns_a, &pair.bob),
        "mob A's row still names bob"
    );
    assert_eq!(
        pair.forward(never_asked(&asked)).await,
        Err(PeerWakeDenial::NotAMember)
    );
    assert_eq!(asked.load(Ordering::SeqCst), 1);

    // Mob A's runtime shuts down too: its wake route is retracted.
    shut_down(&pair.ns_a);
    assert!(!registry.has_wake_route_in_namespace(&pair.ns_a, &pair.bob));
    assert!(routes_of(&pair.ns_a).is_none());
}

#[tokio::test]
async fn the_remote_mobs_refusal_is_passed_on_and_a_gone_mob_is_unavailable() {
    let pair = Pair::new("refused");
    pair.a_holds();
    pair.b_holds();
    let asked = Arc::new(AtomicUsize::new(0));

    // Bob's mob refuses (it is stopped): the same typed refusal, no route.
    let refused = {
        let asked = Arc::clone(&asked);
        pair.forward(move |_owner, _member| {
            asked.fetch_add(1, Ordering::SeqCst);
            denied(PeerWakeDenial::Refused(PeerWakeRefusal::MobNotRunning))
        })
    };
    assert_eq!(
        refused.await,
        Err(PeerWakeDenial::Refused(PeerWakeRefusal::MobNotRunning))
    );
    assert_eq!(asked.load(Ordering::SeqCst), 1);
    assert_eq!(pair.route_to_bob(), None);

    // Bob's mob is gone without retracting its route: nothing can be asked.
    let gone = Pair::new("gone");
    gone.a_holds();
    gone.b_holds();
    let registry = InprocRegistry::global();
    {
        let short_lived = Arc::new(MobOwner {
            waking: AtomicBool::new(false),
        });
        registry.publish_wake_route(&gone.ns_b, gone.bob, "bob", owner_weak(&short_lived));
    }
    assert_eq!(
        gone.forward(never_asked(&asked)).await,
        Err(PeerWakeDenial::Refused(PeerWakeRefusal::MobUnavailable))
    );
    // Bob left his mob (its route is retracted): no member.
    let stale: Weak<dyn InprocPeerWaker> = registry
        .wake_route_owner_in_namespace(&gone.ns_b, &gone.bob)
        .expect("the gone owner's route")
        .1;
    registry.retract_wake_route(&gone.ns_b, &gone.bob, &stale);
    assert_eq!(
        gone.forward(never_asked(&asked)).await,
        Err(PeerWakeDenial::NotAMember)
    );
    assert_eq!(asked.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn an_owner_under_another_name_than_the_linked_member_is_not_asked() {
    let pair = Pair::new("renamed");
    pair.a_holds();
    pair.b_holds();
    let asked = Arc::new(AtomicUsize::new(0));
    let registry = InprocRegistry::global();

    // Bob's key is published by his mob under another member's name: the
    // linked member is `bob`, so nobody is asked.
    let other = Arc::new(MobOwner {
        waking: AtomicBool::new(true),
    });
    assert!(registry.publish_wake_route(&pair.ns_b, pair.bob, "mallory", owner_weak(&other)));
    assert_eq!(
        pair.forward(never_asked(&asked)).await,
        Err(PeerWakeDenial::NotAMember)
    );
    assert!(!part_of(&pair.ns_a).member_is_waking(&pair.bob_name));

    // The route under bob's key is another forwarder: nobody is asked.
    let chained = part_of(&pair.ns_b).waker();
    assert!(registry.publish_wake_route(&pair.ns_b, pair.bob, "bob", chained.clone()));
    assert_eq!(
        pair.forward(never_asked(&asked)).await,
        Err(PeerWakeDenial::NotAMember)
    );
    assert!(!part_of(&pair.ns_a).member_is_waking(&pair.bob_name));
    assert_eq!(asked.load(Ordering::SeqCst), 0);
    assert!(registry.retract_wake_route(&pair.ns_b, &pair.bob, &chained));
}

#[tokio::test]
async fn a_waiter_that_leaves_does_not_end_the_wake_and_the_last_one_forgets_it() {
    let pair = Pair::new("leaving");
    pair.a_holds();
    pair.b_holds();
    let asked = Arc::new(AtomicUsize::new(0));
    let release = Arc::new(Notify::new());
    let hanging = |asked: &Arc<AtomicUsize>, release: &Arc<Notify>| {
        let asked = Arc::clone(asked);
        let release = Arc::clone(release);
        move |_owner: &dyn InprocPeerWaker, _member: String| -> PeerWakeFuture {
            asked.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                release.notified().await;
                Ok(PeerWakeAnswer::Ready)
            })
        }
    };
    let pending = |pair: &Pair| -> Option<usize> {
        locked(&part_of(&pair.ns_a).state)
            .pending
            .get(&(pair.bob, true))
            .map(|pending| pending.waiters)
    };

    // Two sends wait on one wake. One leaves (its budget ended): the wake
    // stays for the other, which gets its answer.
    let first = pair.forward(hanging(&asked, &release));
    let second = pair.forward(hanging(&asked, &release));
    assert_eq!(pending(&pair), Some(2));
    drop(first);
    assert_eq!(pending(&pair), Some(1));
    let _bob_inbox = pair.build_bob();
    release.notify_one();
    assert_eq!(second.await, Ok(PeerWakeAnswer::Ready));
    assert_eq!(asked.load(Ordering::SeqCst), 1);
    assert_eq!(pending(&pair), None);

    // Bob's mob never answers and every waiting send leaves: the pending
    // wake is forgotten, and the next send asks anew.
    let never = Arc::new(Notify::new());
    let waiting: Vec<PeerWakeFuture> = (0..3)
        .map(|_| pair.forward(hanging(&asked, &never)))
        .collect();
    assert_eq!(asked.load(Ordering::SeqCst), 2);
    assert_eq!(pending(&pair), Some(3));
    drop(waiting);
    assert_eq!(pending(&pair), None);
    let again = pair.forward(hanging(&asked, &never));
    assert_eq!(asked.load(Ordering::SeqCst), 3);
    assert_eq!(pending(&pair), Some(1));
    drop(again);
    assert_eq!(pending(&pair), None);
}

#[tokio::test]
async fn a_runtime_closes_only_the_part_it_started() {
    let pair = Pair::new("owned");
    let asked = Arc::new(AtomicUsize::new(0));
    // A runtime of mob B was dropped; the runtime built after it starts its
    // own part and reads the rows again.
    let earlier = start_part(&pair.ns_b);
    earlier.set_member_links(
        "bob",
        vec![link("bob", &pair.bob_name, &pair.alice_name, pair.alice)],
    );
    let later = start_part(&pair.ns_b);
    assert!(
        !InprocRegistry::global().has_wake_route_in_namespace(&pair.ns_b, &pair.alice),
        "the earlier part's route went with it"
    );
    pair.a_holds();
    pair.b_holds();
    let _bob_inbox = pair.build_bob();
    assert_eq!(
        pair.forward(never_asked(&asked)).await,
        Ok(PeerWakeAnswer::Ready)
    );
    assert_eq!(pair.route_to_bob(), Some(pair.bob));

    // The dropped runtime's teardown runs late: it closes its own part and
    // nothing of the later runtime's.
    close_part(&earlier);
    clear_part(&earlier);
    assert!(
        routes_of(&pair.ns_b).is_some_and(|current| Arc::ptr_eq(&current, &later)),
        "the later runtime's part is still the namespace's"
    );
    assert!(InprocRegistry::global().has_wake_route_in_namespace(&pair.ns_b, &pair.alice));
    assert_eq!(
        pair.route_to_bob(),
        Some(pair.bob),
        "its routes were not swept"
    );
    assert_eq!(
        pair.forward(never_asked(&asked)).await,
        Ok(PeerWakeAnswer::Ready)
    );

    // A wire on a mob whose runtime never ran the start-time read holds no
    // link: no part is created for it.
    shut_down(&pair.ns_b);
    drop_member_link(&pair.mob_b, &AgentIdentity::from("bob"), &pair.alice_name);
    assert!(routes_of(&pair.ns_b).is_none());
}

#[tokio::test]
async fn a_link_that_left_while_its_wake_was_pending_gets_no_route() {
    let pair = Pair::new("left-pending");
    pair.a_holds();
    pair.b_holds();
    let release = Arc::new(Notify::new());
    let waiting = {
        let release = Arc::clone(&release);
        pair.forward(move |_owner, _member| {
            Box::pin(async move {
                release.notified().await;
                Ok(PeerWakeAnswer::Ready)
            })
        })
    };
    // Alice is unwired from bob while bob's mob is still building him.
    drop_member_link(&pair.mob_a, &AgentIdentity::from("alice"), &pair.bob_name);
    let _bob_inbox = pair.build_bob();
    release.notify_one();
    assert_eq!(waiting.await, Err(PeerWakeDenial::NotAMember));
    assert_eq!(
        pair.route_to_bob(),
        None,
        "no route for a link that is gone"
    );
}

#[tokio::test]
async fn a_wake_under_another_name_than_the_links_is_not_forwarded() {
    let pair = Pair::new("other-name");
    pair.a_holds();
    pair.b_holds();
    let asked = Arc::new(AtomicUsize::new(0));
    // The request names another member of bob's mob than the one the link
    // under bob's key names.
    let other_name = comms_name(&pair.mob_b, "carol");
    let renamed = part_of(&pair.ns_a).forward(
        pair.alice,
        pair.bob,
        &other_name,
        CONTENT,
        never_asked(&asked),
    );
    assert_eq!(renamed.await, Err(PeerWakeDenial::NotAMember));
    assert_eq!(asked.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_send_that_shares_no_wake_is_asked_for_itself() {
    let pair = Pair::new("unshared");
    pair.a_holds();
    pair.b_holds();
    let asked = Arc::new(AtomicUsize::new(0));
    let routes = part_of(&pair.ns_a);
    let consumed = |asked: &Arc<AtomicUsize>| {
        let asked = Arc::clone(asked);
        move |_owner: &dyn InprocPeerWaker, _member: String| -> PeerWakeFuture {
            asked.fetch_add(1, Ordering::SeqCst);
            Box::pin(std::future::ready(Ok(
                PeerWakeAnswer::ControlNoticeConsumed,
            )))
        }
    };
    let first = routes.forward(
        pair.alice,
        pair.bob,
        &pair.bob_name,
        FROM_A_MEMBER_BEING_BUILT,
        consumed(&asked),
    );
    let second = routes.forward(
        pair.alice,
        pair.bob,
        &pair.bob_name,
        FROM_A_MEMBER_BEING_BUILT,
        consumed(&asked),
    );
    assert_eq!(asked.load(Ordering::SeqCst), 2);
    assert!(locked(&routes.state).pending.is_empty());
    assert_eq!(first.await, Ok(PeerWakeAnswer::ControlNoticeConsumed));
    assert_eq!(second.await, Ok(PeerWakeAnswer::ControlNoticeConsumed));
    assert_eq!(
        pair.route_to_bob(),
        None,
        "a consumed notice installs no route"
    );
}

#[tokio::test]
async fn a_wake_route_lives_from_the_first_link_of_a_key_to_its_last() {
    let pair = Pair::new("refcount");
    let registry = InprocRegistry::global();
    let routes = part_of(&pair.ns_a);
    let carol_name = comms_name(&pair.mob_a, "carol");
    routes.set_member_links(
        "alice",
        vec![link("alice", &pair.alice_name, &pair.bob_name, pair.bob)],
    );
    routes.set_member_links(
        "carol",
        vec![link("carol", &carol_name, &pair.bob_name, pair.bob)],
    );
    let (published_as, owner) = registry
        .wake_route_owner_in_namespace(&pair.ns_a, &pair.bob)
        .expect("the forwarder's route");
    assert_eq!(published_as, pair.bob_name);
    assert!(Weak::ptr_eq(&owner, &routes.waker()));

    routes.drop_link("alice", &pair.bob_name);
    assert!(registry.has_wake_route_in_namespace(&pair.ns_a, &pair.bob));
    routes.drop_link("carol", &pair.bob_name);
    assert!(!registry.has_wake_route_in_namespace(&pair.ns_a, &pair.bob));

    // A key a member of mob A itself is seated under keeps its own mob's
    // route: no link is held for it, and a wire to it is refused.
    let own = Arc::new(MobOwner {
        waking: AtomicBool::new(false),
    });
    let seated = Keypair::generate().public_key();
    registry.publish_wake_route(&pair.ns_a, seated, "dave", owner_weak(&own));
    routes.set_member_links(
        "alice",
        vec![link("alice", &pair.alice_name, &pair.bob_name, seated)],
    );
    assert!(locked(&routes.state).links.is_empty());
    let (still, still_owner) = registry
        .wake_route_owner_in_namespace(&pair.ns_a, &seated)
        .expect("the mob's own route");
    assert_eq!(still, "dave");
    assert!(Weak::ptr_eq(&still_owner, &owner_weak(&own)));
    let spec = TrustedPeerDescriptor::test_only_unsigned(
        pair.bob_name.clone(),
        seated.to_peer_id().to_string(),
        format!("inproc://{}", pair.bob_name),
    )
    .expect("descriptor")
    .with_pubkey(*seated.as_bytes());
    let refusal = refuse_locally_seated_key(&pair.mob_a, &spec).expect_err("refused");
    assert!(
        refusal.to_string().contains("already holds that key"),
        "{refusal}"
    );
    let free = TrustedPeerDescriptor::test_only_unsigned(
        pair.bob_name.clone(),
        pair.bob.to_peer_id().to_string(),
        format!("inproc://{}", pair.bob_name),
    )
    .expect("descriptor")
    .with_pubkey(*pair.bob.as_bytes());
    refuse_locally_seated_key(&pair.mob_a, &free).expect("a key no local member holds");
    registry.retract_wake_route(&pair.ns_a, &seated, &owner_weak(&own));
}

#[tokio::test]
async fn the_remote_members_residency_is_read_from_its_own_mob_for_a_whole_link() {
    let pair = Pair::new("waking");
    let routes = part_of(&pair.ns_a);
    pair.bob_owner.waking.store(true, Ordering::SeqCst);

    // A link only mob A holds reads nothing of mob B.
    pair.a_holds();
    assert!(!routes.member_is_waking(&pair.bob_name));
    pair.b_holds();
    assert!(routes.member_is_waking(&pair.bob_name));
    pair.bob_owner.waking.store(false, Ordering::SeqCst);
    assert!(!routes.member_is_waking(&pair.bob_name));
}

#[tokio::test]
async fn a_silent_lifecycle_notice_is_not_forwarded() {
    let pair = Pair::new("notice");
    pair.a_holds();
    pair.b_holds();
    let asked = Arc::new(AtomicUsize::new(0));
    let _bob_inbox = pair.build_bob();

    // A whole link, and a notice instead of a content send: answered as a
    // key that is no member, with nothing asked of bob's mob.
    let notice = part_of(&pair.ns_a).forward(
        pair.alice,
        pair.bob,
        &pair.bob_name,
        LIFECYCLE_NOTICE,
        never_asked(&asked),
    );
    assert_eq!(notice.await, Err(PeerWakeDenial::NotAMember));
    assert_eq!(asked.load(Ordering::SeqCst), 0);
    assert_eq!(pair.route_to_bob(), None);
    assert!(locked(&part_of(&pair.ns_a).state).pending.is_empty());

    // The content send over the same link is forwarded.
    assert_eq!(
        pair.forward(never_asked(&asked)).await,
        Ok(PeerWakeAnswer::Ready)
    );
    assert_eq!(asked.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_link_the_other_mob_dropped_while_the_wake_was_pending_gets_no_route() {
    let pair = Pair::new("dropped-remote");
    pair.a_holds();
    pair.b_holds();
    let release = Arc::new(Notify::new());
    let waiting = {
        let release = Arc::clone(&release);
        pair.forward(move |_owner, _member| {
            Box::pin(async move {
                release.notified().await;
                Ok(PeerWakeAnswer::Ready)
            })
        })
    };
    // Bob is unwired from alice by his own mob while he is being built.
    drop_member_link(&pair.mob_b, &AgentIdentity::from("bob"), &pair.alice_name);
    let _bob_inbox = pair.build_bob();
    release.notify_one();
    assert_eq!(waiting.await, Err(PeerWakeDenial::NotAMember));
    assert_eq!(
        pair.route_to_bob(),
        None,
        "no route for a link that is gone"
    );
}

#[tokio::test]
async fn senders_that_share_a_wake_are_each_answered_for_their_own_link() {
    let pair = Pair::new("own-link");
    let registry = InprocRegistry::global();
    let asked = Arc::new(AtomicUsize::new(0));
    let release = Arc::new(Notify::new());
    // Alice and carol of mob A are both wired to bob, and bob to both.
    let carol = Keypair::generate().public_key();
    let carol_name = comms_name(&pair.mob_a, "carol");
    let (_carol_inbox, carol_sender) = Inbox::new_transport_only();
    assert!(
        !registry
            .register_with_meta_in_namespace(
                &pair.ns_a,
                carol_name.clone(),
                carol,
                carol_sender,
                PeerMeta::default(),
            )
            .is_rejected()
    );
    pair.a_holds();
    part_of(&pair.ns_a).set_member_links(
        "carol",
        vec![link("carol", &carol_name, &pair.bob_name, pair.bob)],
    );
    part_of(&pair.ns_b).set_member_links(
        "bob",
        vec![
            link("bob", &pair.bob_name, &pair.alice_name, pair.alice),
            link("bob", &pair.bob_name, &carol_name, carol),
        ],
    );
    let hanging = |asked: &Arc<AtomicUsize>, release: &Arc<Notify>| {
        let asked = Arc::clone(asked);
        let release = Arc::clone(release);
        move |_owner: &dyn InprocPeerWaker, _member: String| -> PeerWakeFuture {
            asked.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                release.notified().await;
                Ok(PeerWakeAnswer::Ready)
            })
        }
    };
    let from_alice = pair.forward(hanging(&asked, &release));
    let from_carol = part_of(&pair.ns_a).forward(
        carol,
        pair.bob,
        &pair.bob_name,
        CONTENT,
        hanging(&asked, &release),
    );
    assert_eq!(asked.load(Ordering::SeqCst), 1, "one wake for both senders");

    // Alice is unwired from bob while the wake is pending. Carol's link is
    // whole: she is answered ready and the route is installed. Alice is
    // answered as no member.
    drop_member_link(&pair.mob_a, &AgentIdentity::from("alice"), &pair.bob_name);
    let _bob_inbox = pair.build_bob();
    release.notify_one();
    assert_eq!(from_carol.await, Ok(PeerWakeAnswer::Ready));
    assert_eq!(from_alice.await, Err(PeerWakeDenial::NotAMember));
    assert_eq!(pair.route_to_bob(), Some(pair.bob));
    assert_eq!(asked.load(Ordering::SeqCst), 1);
    registry.unregister_in_namespace(&pair.ns_a, &carol);
}

#[tokio::test]
async fn a_runtime_that_shut_down_takes_only_the_inbox_routes_links_name() {
    let pair = Pair::new("swept");
    pair.a_holds();
    pair.b_holds();
    let registry = InprocRegistry::global();
    let asked = Arc::new(AtomicUsize::new(0));
    let _bob_inbox = pair.build_bob();
    assert_eq!(
        pair.forward(never_asked(&asked)).await,
        Ok(PeerWakeAnswer::Ready)
    );
    assert_eq!(pair.route_to_bob(), Some(pair.bob));

    // A route in mob A's namespace under a name of mob B that no link of
    // mob A names (somebody else's route).
    let foreign = Keypair::generate().public_key();
    let foreign_name = comms_name(&pair.mob_b, "dave");
    let (_foreign_inbox, foreign_sender) = Inbox::new_transport_only();
    assert!(
        !registry
            .register_with_meta_in_namespace(
                &pair.ns_a,
                foreign_name.clone(),
                foreign,
                foreign_sender,
                PeerMeta::default(),
            )
            .is_rejected()
    );

    // A third mob's runtime starts and shuts down: nothing of the two
    // others moves.
    let third = start_part("mob.xmr-swept-c");
    clear_part(&third);
    assert_eq!(pair.route_to_bob(), Some(pair.bob));

    // Mob B's runtime shuts down: mob A drops the inbox route of its link
    // to bob, and leaves the route no link names.
    shut_down(&pair.ns_b);
    assert_eq!(pair.route_to_bob(), None);
    assert_eq!(
        registry.get_name_by_pubkey_in_namespace(&pair.ns_a, &foreign),
        Some(foreign_name)
    );
    registry.unregister_in_namespace(&pair.ns_a, &foreign);
}

#[tokio::test]
async fn a_key_that_lost_its_last_link_loses_its_inbox_route() {
    let pair = Pair::new("last-link");
    let registry = InprocRegistry::global();
    let asked = Arc::new(AtomicUsize::new(0));
    let routes = part_of(&pair.ns_a);
    // Alice and carol of mob A are both wired to bob, and bob to alice.
    let carol_name = comms_name(&pair.mob_a, "carol");
    pair.a_holds();
    routes.set_member_links(
        "carol",
        vec![link("carol", &carol_name, &pair.bob_name, pair.bob)],
    );
    pair.b_holds();
    let _bob_inbox = pair.build_bob();
    assert_eq!(
        pair.forward(never_asked(&asked)).await,
        Ok(PeerWakeAnswer::Ready)
    );
    assert_eq!(pair.route_to_bob(), Some(pair.bob));

    // Alice is unwired from bob. Carol's link still names the key: its wake
    // route and its inbox route stay.
    drop_member_link(&pair.mob_a, &AgentIdentity::from("alice"), &pair.bob_name);
    assert!(registry.has_wake_route_in_namespace(&pair.ns_a, &pair.bob));
    assert_eq!(pair.route_to_bob(), Some(pair.bob));

    // Carol is unwired too: the key lost its last link, and the inbox route
    // goes with the wake route.
    drop_member_link(&pair.mob_a, &AgentIdentity::from("carol"), &pair.bob_name);
    assert!(!registry.has_wake_route_in_namespace(&pair.ns_a, &pair.bob));
    assert_eq!(
        pair.route_to_bob(),
        None,
        "the inbox route outlived the last link to its key"
    );

    // Only the route under the link's own name goes. A route the namespace
    // holds under the key with another name is somebody else's, and stays.
    pair.a_holds();
    let (_other_inbox, other_sender) = Inbox::new_transport_only();
    assert!(
        !registry
            .register_with_meta_in_namespace(
                &pair.ns_a,
                "somebody-else",
                pair.bob,
                other_sender,
                PeerMeta::default(),
            )
            .is_rejected()
    );
    drop_member_link(&pair.mob_a, &AgentIdentity::from("alice"), &pair.bob_name);
    assert!(!registry.has_wake_route_in_namespace(&pair.ns_a, &pair.bob));
    assert_eq!(
        registry.get_name_by_pubkey_in_namespace(&pair.ns_a, &pair.bob),
        Some("somebody-else".to_string())
    );
    registry.unregister_in_namespace(&pair.ns_a, &pair.bob);
}

#[tokio::test]
async fn a_part_that_was_replaced_or_closed_takes_no_link() {
    let pair = Pair::new("closed");
    let registry = InprocRegistry::global();
    let bob_names_alice = || vec![link("bob", &pair.bob_name, &pair.alice_name, pair.alice)];

    // A runtime of mob B was dropped, and the runtime built after it started
    // its own part. A write that still holds the earlier part adds nothing:
    // no link, and no wake route under the earlier part's name.
    let earlier = start_part(&pair.ns_b);
    let later = start_part(&pair.ns_b);
    earlier.set_member_links("bob", bob_names_alice());
    let held_by_earlier = locked(&earlier.state).links.len();
    let published_by_earlier =
        registry.retract_wake_route(&pair.ns_b, &pair.alice, &earlier.waker());
    assert_eq!(held_by_earlier, 0, "a replaced part took a link");
    assert!(!published_by_earlier, "a replaced part published a route");

    // The later runtime's part takes the link.
    later.set_member_links("bob", bob_names_alice());
    assert!(registry.has_wake_route_in_namespace(&pair.ns_b, &pair.alice));

    // Its runtime closes it: the link and the route go, and a write that
    // still holds the part adds nothing.
    close_part(&later);
    assert!(!registry.has_wake_route_in_namespace(&pair.ns_b, &pair.alice));
    later.set_member_links("bob", bob_names_alice());
    assert!(locked(&later.state).links.is_empty());
    assert!(!registry.has_wake_route_in_namespace(&pair.ns_b, &pair.alice));
}
