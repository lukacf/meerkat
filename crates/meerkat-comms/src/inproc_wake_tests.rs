//! Wake routes: a send to a seated member with no live route waits for the
//! member's owner to wake it, inside the sender's own budget, then hands the
//! envelope off once; every refusal is typed and hands nothing off.
//!
//! These rows drive the registry with scripted owners. The owner's own
//! machine verdicts are exercised by the mob crate's wake rows.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use crate::classify::test_support;
use crate::inbox::Inbox;
use crate::trust::TrustStore;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

const BUDGET: Duration = Duration::from_secs(30);

fn message(body: &str) -> MessageKind {
    MessageKind::Message {
        objective_id: None,
        content_taint: None,
        blocks: None,
        body: body.to_string(),
        handling_mode: None,
        in_reply_to: None,
    }
}

/// A response keeps capacity-only semantics: admitted means `Queued`, with no
/// drain needed, which keeps these rows about routing rather than ingress.
fn response() -> MessageKind {
    MessageKind::Response {
        in_reply_to: Uuid::new_v4(),
        status: crate::types::Status::Completed,
        result: serde_json::json!({ "ok": true }),
        blocks: None,
        content_taint: None,
        handling_mode: None,
        objective_id: None,
    }
}

fn queued(inbox: &Inbox) -> usize {
    inbox
        .classified_snapshot()
        .map_or(0, |snapshot| snapshot.total_count)
}

fn open_inbox() -> (Inbox, InboxSender) {
    Inbox::new_classified(test_support::classification_context(
        TrustStore::new(),
        false,
    ))
}

/// What a scripted owner does when it is asked to wake its member.
enum OnWake {
    /// Register `sender` as the member's live route, then answer ready.
    Materialize {
        registry: Arc<InprocRegistry>,
        namespace: String,
        name: String,
        pubkey: PubKey,
        sender: std::sync::Mutex<Option<InboxSender>>,
    },
    /// Release the stale generation, register the fresh one, answer ready
    /// (an owner rebuilding a member whose old inbox closed).
    Replace {
        registry: Arc<InprocRegistry>,
        namespace: String,
        name: String,
        pubkey: PubKey,
        stale: InboxSender,
        fresh: std::sync::Mutex<Option<InboxSender>>,
    },
    /// Answer ready without registering anything.
    ReadyWithoutRoute,
    /// Answer with this refusal.
    Refuse(PeerWakeRefusal),
    /// Answer that the key is no longer a member (a stale route).
    NotAMember,
    /// Answer that the owner consumed a silent control notice.
    ConsumeNotice,
    /// Never answer.
    Hang,
}

struct ScriptedOwner {
    on_wake: OnWake,
    waking: AtomicBool,
    calls: AtomicUsize,
    waking_asked: AtomicUsize,
    requests: parking_lot::Mutex<Vec<PeerWakeRequest>>,
}

impl ScriptedOwner {
    fn new(on_wake: OnWake) -> Arc<Self> {
        Arc::new(Self {
            on_wake,
            waking: AtomicBool::new(false),
            calls: AtomicUsize::new(0),
            waking_asked: AtomicUsize::new(0),
            requests: parking_lot::Mutex::new(Vec::new()),
        })
    }

    fn waking(self: Arc<Self>) -> Arc<Self> {
        self.waking.store(true, Ordering::SeqCst);
        self
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    fn weak(self: &Arc<Self>) -> Weak<dyn InprocPeerWaker> {
        let owner: Arc<dyn InprocPeerWaker> = Arc::clone(self) as Arc<dyn InprocPeerWaker>;
        Arc::downgrade(&owner)
    }
}

impl InprocPeerWaker for ScriptedOwner {
    fn wake(&self, request: PeerWakeRequest) -> PeerWakeFuture {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.requests.lock().push(request);
        let answer = match &self.on_wake {
            OnWake::Materialize {
                registry,
                namespace,
                name,
                pubkey,
                sender,
            } => {
                if let Some(sender) = sender.lock().unwrap().take() {
                    let outcome = registry.register_with_meta_in_namespace(
                        namespace,
                        name.clone(),
                        *pubkey,
                        sender,
                        PeerMeta::default(),
                    );
                    assert!(!outcome.is_rejected(), "{outcome:?}");
                }
                Some(Ok(PeerWakeAnswer::Ready))
            }
            OnWake::Replace {
                registry,
                namespace,
                name,
                pubkey,
                stale,
                fresh,
            } => {
                registry.unregister_sender_in_namespace(namespace, pubkey, stale);
                if let Some(fresh) = fresh.lock().unwrap().take() {
                    let outcome = registry.register_with_meta_in_namespace(
                        namespace,
                        name.clone(),
                        *pubkey,
                        fresh,
                        PeerMeta::default(),
                    );
                    assert!(!outcome.is_rejected(), "{outcome:?}");
                }
                Some(Ok(PeerWakeAnswer::Ready))
            }
            OnWake::ReadyWithoutRoute => Some(Ok(PeerWakeAnswer::Ready)),
            OnWake::Refuse(refusal) => Some(Err(PeerWakeDenial::Refused(refusal.clone()))),
            OnWake::NotAMember => Some(Err(PeerWakeDenial::NotAMember)),
            OnWake::ConsumeNotice => Some(Ok(PeerWakeAnswer::ControlNoticeConsumed)),
            OnWake::Hang => None,
        };
        Box::pin(async move {
            match answer {
                Some(answer) => answer,
                None => std::future::pending().await,
            }
        })
    }

    fn member_is_waking(&self, _member: &str) -> bool {
        self.waking_asked.fetch_add(1, Ordering::SeqCst);
        self.waking.load(Ordering::SeqCst)
    }
}

/// A dormant member's key has only a wake route. The send
/// asks the owner once, then hands the envelope off once to the inbox the
/// wake registered; a second send finds the live route and wakes nothing.
#[tokio::test]
async fn a_send_to_a_dormant_member_wakes_it_once_then_delivers_once() {
    let registry = Arc::new(InprocRegistry::new());
    let member = make_keypair();
    let (inbox, inbox_sender) = open_inbox();
    let owner = ScriptedOwner::new(OnWake::Materialize {
        registry: Arc::clone(&registry),
        namespace: "realm".to_string(),
        name: "worker-1".to_string(),
        pubkey: member.public_key(),
        sender: std::sync::Mutex::new(Some(inbox_sender)),
    });
    assert!(registry.publish_wake_route("realm", member.public_key(), "worker-1", owner.weak()));
    let sender = make_keypair();

    let first = registry
        .send_to_pubkey_in_namespace_with_id_wait(
            "realm",
            &sender,
            &member.public_key(),
            Uuid::new_v4(),
            response(),
            true,
            BUDGET,
            None,
        )
        .await
        .expect("the wake answered ready and the envelope was handed off");
    assert_eq!(
        first.outcome,
        InprocDeliveryOutcome::Resolved(IngressDeliveryOutcome::Queued)
    );
    assert_eq!(owner.calls(), 1);
    let request = owner.requests.lock()[0].clone();
    assert_eq!(request.member, "worker-1");
    assert_eq!(request.recipient, member.public_key());
    assert_eq!(request.sender, sender.public_key());
    assert!(request.observed_missing);
    assert!(!request.sender_waking);
    assert_eq!(request.input_class, PeerWakeInputClass::Content);

    registry
        .send_to_pubkey_in_namespace_with_id_wait(
            "realm",
            &sender,
            &member.public_key(),
            Uuid::new_v4(),
            response(),
            true,
            BUDGET,
            None,
        )
        .await
        .expect("the live route serves the second send");
    assert_eq!(owner.calls(), 1, "a live route is never woken");
    assert_eq!(
        queued(&inbox),
        2,
        "each envelope was handed off exactly once"
    );
}

/// A woken member receives the governed origin its sender carried: the
/// envelope handed off after the wake keeps the origin, beside the
/// in-process sender authentication, as a delivery to a live member does.
#[tokio::test]
async fn a_woken_member_receives_the_governed_origin_its_sender_carried() {
    let registry = Arc::new(InprocRegistry::new());
    let member = make_keypair();
    let (inbox, inbox_sender) = open_inbox();
    let owner = ScriptedOwner::new(OnWake::Materialize {
        registry: Arc::clone(&registry),
        namespace: "realm".to_string(),
        name: "worker-1".to_string(),
        pubkey: member.public_key(),
        sender: std::sync::Mutex::new(Some(inbox_sender)),
    });
    assert!(registry.publish_wake_route("realm", member.public_key(), "worker-1", owner.weak()));
    let origin =
        meerkat_core::GovernedSendOrigin::__from_runtime_facts(Arc::new("staged run facts"));

    registry
        .send_to_pubkey_in_namespace_with_id_wait(
            "realm",
            &make_keypair(),
            &member.public_key(),
            Uuid::new_v4(),
            response(),
            true,
            BUDGET,
            Some(origin.clone()),
        )
        .await
        .expect("the wake answered ready and the envelope was handed off");
    assert_eq!(owner.calls(), 1, "the member was dormant and was woken");

    let claim = crate::runtime::comms_runtime::test_peer_ingress_queue_claim(
        inbox
            .try_claim_one_classified()
            .expect("the envelope handed off after the wake"),
    );
    assert_eq!(
        claim.sender_authentication(),
        meerkat_core::PeerIngressSenderAuthentication::InProcess
    );
    assert!(
        claim
            .governed_send_origin()
            .is_some_and(|received| Arc::ptr_eq(
                received.__runtime_facts(),
                origin.__runtime_facts()
            )),
        "the woken member's claim holds the sender's own origin: {:?}",
        claim.governed_send_origin()
    );
}

/// A key with neither route is not a member.
#[tokio::test]
async fn a_key_with_no_route_at_all_is_peer_not_found() {
    let registry = InprocRegistry::new();
    let result = registry
        .send_to_pubkey_in_namespace_with_id_wait(
            "realm",
            &make_keypair(),
            &make_keypair().public_key(),
            Uuid::new_v4(),
            message("hello"),
            true,
            BUDGET,
            None,
        )
        .await;
    assert!(matches!(result, Err(InprocSendError::PeerNotFound(_))));
}

/// A refused wake is typed and hands nothing off.
#[tokio::test]
async fn a_refused_wake_is_typed_and_never_peer_not_found() {
    let registry = InprocRegistry::new();
    let member = make_keypair();
    let refusal = PeerWakeRefusal::MemberBroken {
        reason: "missing durable session snapshot".to_string(),
        hold: None,
    };
    let owner = ScriptedOwner::new(OnWake::Refuse(refusal.clone()));
    registry.publish_wake_route("realm", member.public_key(), "worker-1", owner.weak());

    let result = registry
        .send_to_pubkey_in_namespace_with_id_wait(
            "realm",
            &make_keypair(),
            &member.public_key(),
            Uuid::new_v4(),
            message("hello"),
            true,
            BUDGET,
            None,
        )
        .await;
    match result {
        Err(InprocSendError::WakeRefused(actual)) => assert_eq!(actual, refusal),
        other => panic!("expected the typed refusal, got {other:?}"),
    }
}

/// A closed live route of a seated member is stale. It takes
/// the wake path (observed missing), and the envelope reaches the member's
/// next inbox exactly once; the closed generation never received it.
#[tokio::test]
async fn a_closed_stale_live_route_takes_the_wake_path() {
    let registry = Arc::new(InprocRegistry::new());
    let member = make_keypair();
    let (stale_inbox, stale_sender) = open_inbox();
    registry.register_with_meta_in_namespace(
        "realm",
        "worker-1",
        member.public_key(),
        stale_sender.clone(),
        PeerMeta::default(),
    );
    drop(stale_inbox);
    let (fresh_inbox, fresh_sender) = open_inbox();
    let owner = ScriptedOwner::new(OnWake::Replace {
        registry: Arc::clone(&registry),
        namespace: "realm".to_string(),
        name: "worker-1".to_string(),
        pubkey: member.public_key(),
        stale: stale_sender,
        fresh: std::sync::Mutex::new(Some(fresh_sender)),
    });
    registry.publish_wake_route("realm", member.public_key(), "worker-1", owner.weak());

    let delivered = registry
        .send_to_pubkey_in_namespace_with_id_wait(
            "realm",
            &make_keypair(),
            &member.public_key(),
            Uuid::new_v4(),
            response(),
            true,
            BUDGET,
            None,
        )
        .await
        .expect("the fresh generation receives the envelope");
    assert_eq!(
        delivered.outcome,
        InprocDeliveryOutcome::Resolved(IngressDeliveryOutcome::Queued)
    );
    assert_eq!(owner.calls(), 1);
    assert!(owner.requests.lock()[0].observed_missing);
    assert_eq!(queued(&fresh_inbox), 1, "handed off exactly once");
}

/// A member that answers ready but has no route twice is
/// `materialization_lost` after exactly two wakes, and nothing was handed off.
#[tokio::test]
async fn ready_without_a_route_twice_is_materialization_lost() {
    let registry = InprocRegistry::new();
    let member = make_keypair();
    let owner = ScriptedOwner::new(OnWake::ReadyWithoutRoute);
    registry.publish_wake_route("realm", member.public_key(), "worker-1", owner.weak());

    let result = registry
        .send_to_pubkey_in_namespace_with_id_wait(
            "realm",
            &make_keypair(),
            &member.public_key(),
            Uuid::new_v4(),
            message("hello"),
            true,
            BUDGET,
            None,
        )
        .await;
    assert!(matches!(
        result,
        Err(InprocSendError::WakeRefused(
            PeerWakeRefusal::MaterializationLost
        ))
    ));
    assert_eq!(
        owner.calls(),
        2,
        "one wake for the miss, one re-wake after ready"
    );
}

/// The sender's budget bounds the wait for the wake. It
/// ends as `wake_timed_out` (not delivered); the owner's wake goes on.
#[tokio::test(start_paused = true)]
async fn a_pending_wake_ends_with_the_senders_budget_as_wake_timed_out() {
    let registry = InprocRegistry::new();
    let member = make_keypair();
    let owner = ScriptedOwner::new(OnWake::Hang);
    registry.publish_wake_route("realm", member.public_key(), "worker-1", owner.weak());

    let result = registry
        .send_to_pubkey_in_namespace_with_id_wait(
            "realm",
            &make_keypair(),
            &member.public_key(),
            Uuid::new_v4(),
            message("hello"),
            true,
            Duration::from_secs(5),
            None,
        )
        .await;
    assert!(matches!(
        result,
        Err(InprocSendError::WakeRefused(PeerWakeRefusal::WakeTimedOut))
    ));
    assert_eq!(owner.calls(), 1);
}

/// A sender that is itself being woken never waits on a
/// recipient that is waking too, even through the recipient's early live
/// route: the recipient's owner is asked first with `sender_waking`, answers
/// from its machine, and nothing is handed off.
#[tokio::test]
async fn a_waking_sender_never_waits_on_a_waking_recipient() {
    let registry = InprocRegistry::new();
    let sender = make_keypair();
    let recipient = make_keypair();
    let sender_owner = ScriptedOwner::new(OnWake::Hang).waking();
    registry.publish_wake_route("realm", sender.public_key(), "a", sender_owner.weak());
    let recipient_owner =
        ScriptedOwner::new(OnWake::Refuse(PeerWakeRefusal::SenderWaking)).waking();
    registry.publish_wake_route("realm", recipient.public_key(), "b", recipient_owner.weak());
    // The recipient's inbox is already registered (its wake is in flight).
    let (inbox, inbox_sender) = open_inbox();
    registry.register_with_meta_in_namespace(
        "realm",
        "b",
        recipient.public_key(),
        inbox_sender,
        PeerMeta::default(),
    );

    let result = registry
        .send_to_pubkey_in_namespace_with_id_wait(
            "realm",
            &sender,
            &recipient.public_key(),
            Uuid::new_v4(),
            message("from inside a build"),
            true,
            BUDGET,
            None,
        )
        .await;
    assert!(matches!(
        result,
        Err(InprocSendError::WakeRefused(PeerWakeRefusal::SenderWaking))
    ));
    let request = recipient_owner.requests.lock()[0].clone();
    assert!(request.sender_waking);
    assert!(!request.observed_missing);
    assert_eq!(queued(&inbox), 0, "nothing was handed off");
    assert_eq!(
        sender_owner.calls(),
        0,
        "the sender's own owner is only observed"
    );
}

/// A wake route is namespace-scoped. A member dormant in namespace A is
/// not wakeable from namespace B: `PeerNotFound`, and its owner is not asked.
#[tokio::test]
async fn a_wake_route_never_crosses_namespaces() {
    let registry = InprocRegistry::new();
    let member = make_keypair();
    let owner = ScriptedOwner::new(OnWake::ReadyWithoutRoute);
    registry.publish_wake_route("realm-a", member.public_key(), "worker-1", owner.weak());

    let result = registry
        .send_to_pubkey_in_namespace_with_id_wait(
            "realm-b",
            &make_keypair(),
            &member.public_key(),
            Uuid::new_v4(),
            message("hello"),
            true,
            BUDGET,
            None,
        )
        .await;
    assert!(matches!(result, Err(InprocSendError::PeerNotFound(_))));
    assert_eq!(owner.calls(), 0);
}

/// A wake route whose owner is gone (its mob actor exited without
/// retracting it) is a member nothing in this process can wake: typed
/// `mob_unavailable`, never `PeerNotFound`.
#[tokio::test]
async fn a_route_whose_owner_is_gone_is_mob_unavailable() {
    let registry = InprocRegistry::new();
    let member = make_keypair();
    let owner = ScriptedOwner::new(OnWake::ReadyWithoutRoute);
    registry.publish_wake_route("realm", member.public_key(), "worker-1", owner.weak());
    drop(owner);

    let result = registry
        .send_to_pubkey_in_namespace_with_id_wait(
            "realm",
            &make_keypair(),
            &member.public_key(),
            Uuid::new_v4(),
            message("hello"),
            true,
            BUDGET,
            None,
        )
        .await;
    assert!(matches!(
        result,
        Err(InprocSendError::WakeRefused(
            PeerWakeRefusal::MobUnavailable
        ))
    ));
}

/// A seated member whose owner is gone is still served by its live route,
/// and that hand-off keeps the governed origin its sender carried, beside
/// the in-process sender authentication.
#[tokio::test]
async fn an_owner_gone_member_served_by_its_live_route_receives_the_governed_origin() {
    let registry = InprocRegistry::new();
    let member = make_keypair();
    let (inbox, inbox_sender) = open_inbox();
    let outcome = registry.register_with_meta_in_namespace(
        "realm",
        "worker-1".to_string(),
        member.public_key(),
        inbox_sender,
        PeerMeta::default(),
    );
    assert!(!outcome.is_rejected(), "{outcome:?}");
    let owner = ScriptedOwner::new(OnWake::ReadyWithoutRoute);
    assert!(registry.publish_wake_route("realm", member.public_key(), "worker-1", owner.weak()));
    drop(owner);
    let origin =
        meerkat_core::GovernedSendOrigin::__from_runtime_facts(Arc::new("staged run facts"));

    registry
        .send_to_pubkey_in_namespace_with_id_wait(
            "realm",
            &make_keypair(),
            &member.public_key(),
            Uuid::new_v4(),
            response(),
            true,
            BUDGET,
            Some(origin.clone()),
        )
        .await
        .expect("the live route serves the member whose owner is gone");

    let claim = crate::runtime::comms_runtime::test_peer_ingress_queue_claim(
        inbox
            .try_claim_one_classified()
            .expect("the envelope the live route took"),
    );
    assert_eq!(
        claim.sender_authentication(),
        meerkat_core::PeerIngressSenderAuthentication::InProcess
    );
    assert!(
        claim
            .governed_send_origin()
            .is_some_and(|received| Arc::ptr_eq(
                received.__runtime_facts(),
                origin.__runtime_facts()
            )),
        "the owner-gone member's claim holds the sender's own origin: {:?}",
        claim.governed_send_origin()
    );
}

/// Retraction is owner-exact: another owner cannot retract a route it did not
/// publish, and a retracted key is no longer a member here.
#[test]
fn wake_route_retraction_is_owner_exact() {
    let registry = InprocRegistry::new();
    let member = make_keypair().public_key();
    let owner = ScriptedOwner::new(OnWake::ReadyWithoutRoute);
    let stranger = ScriptedOwner::new(OnWake::ReadyWithoutRoute);
    assert!(registry.publish_wake_route("realm", member, "worker-1", owner.weak()));
    assert!(!registry.retract_wake_route("realm", &member, &stranger.weak()));
    assert!(registry.has_wake_route_in_namespace("realm", &member));
    assert!(registry.retract_wake_route("realm", &member, &owner.weak()));
    assert!(!registry.has_wake_route_in_namespace("realm", &member));
    assert!(!registry.publish_wake_route("realm", PubKey([0u8; 32]), "zero", owner.weak()));
}

/// The owner read answers the published owner and the owner's name for the
/// member, only for the namespace and key the route was published under, and
/// no longer after the retract.
#[test]
fn the_wake_route_owner_read_answers_the_published_owner_and_member_name() {
    let registry = InprocRegistry::new();
    let member = make_keypair().public_key();
    let other_key = make_keypair().public_key();
    let owner = ScriptedOwner::new(OnWake::ReadyWithoutRoute);
    let other_owner = ScriptedOwner::new(OnWake::ReadyWithoutRoute);
    assert!(registry.publish_wake_route("realm-b", member, "worker-1", owner.weak()));
    assert!(registry.publish_wake_route("realm-b", other_key, "worker-2", other_owner.weak()));

    let (name, found) = registry
        .wake_route_owner_in_namespace("realm-b", &member)
        .expect("the published route is read");
    assert_eq!(name, "worker-1");
    assert!(Weak::ptr_eq(&found, &owner.weak()));
    assert!(!Weak::ptr_eq(&found, &other_owner.weak()));
    let (other_name, other_found) = registry
        .wake_route_owner_in_namespace("realm-b", &other_key)
        .expect("each key reads its own route");
    assert_eq!(other_name, "worker-2");
    assert!(Weak::ptr_eq(&other_found, &other_owner.weak()));

    assert!(
        registry
            .wake_route_owner_in_namespace("realm-a", &member)
            .is_none(),
        "another namespace has no route for the key"
    );
    assert!(
        registry
            .wake_route_owner_in_namespace("realm-b", &make_keypair().public_key())
            .is_none(),
        "another key has no route"
    );

    assert!(registry.retract_wake_route("realm-b", &member, &owner.weak()));
    assert!(
        registry
            .wake_route_owner_in_namespace("realm-b", &member)
            .is_none(),
        "a retracted route is no longer read"
    );
    assert!(
        registry
            .wake_route_owner_in_namespace("realm-b", &other_key)
            .is_some(),
        "the other key's route stays"
    );
}

/// The owner read alone wakes nobody and changes nothing: no owner is asked,
/// no route appears in the reader's namespace, no live route is created, and
/// a send from the other namespace still finds no member.
#[tokio::test]
async fn the_wake_route_owner_read_alone_wakes_nobody_and_publishes_nothing() {
    let registry = InprocRegistry::new();
    let sender = make_keypair();
    let member = make_keypair().public_key();
    let owner = ScriptedOwner::new(OnWake::ReadyWithoutRoute);
    assert!(registry.publish_wake_route("realm-b", member, "worker-1", owner.weak()));

    for _ in 0..3 {
        assert!(
            registry
                .wake_route_owner_in_namespace("realm-b", &member)
                .is_some()
        );
    }
    assert_eq!(owner.calls(), 0, "a read asks no owner");
    assert!(owner.requests.lock().is_empty());
    assert!(!registry.has_wake_route_in_namespace("realm-a", &member));
    assert!(registry.has_wake_route_in_namespace("realm-b", &member));
    assert!(
        registry
            .get_by_pubkey_in_namespace("realm-b", &member)
            .is_none(),
        "a read hands out and creates no inbox"
    );
    assert!(registry.peers_in_namespace("realm-a").is_empty());
    assert!(registry.peers_in_namespace("realm-b").is_empty());

    let refused = registry
        .send_to_pubkey_in_namespace_with_id_wait(
            "realm-a",
            &sender,
            &member,
            Uuid::new_v4(),
            message("across"),
            false,
            BUDGET,
            None,
        )
        .await
        .expect_err("the registry itself never wakes a member from another namespace");
    assert!(
        matches!(refused, InprocSendError::PeerNotFound(_)),
        "{refused:?}"
    );
    assert_eq!(owner.calls(), 0);
}

/// One key under two namespaces with two owners (a host forwarder's route in
/// the sender's namespace, the member's own owner in the member's): each
/// read answers the asked namespace's own route. A namespace that exists
/// and lacks the key answers `None`, with no fallback to another namespace,
/// and so does a namespace that does not exist, which the read does not
/// create. A re-published route is read as it is now.
#[test]
fn the_wake_route_owner_read_answers_the_asked_namespace_only() {
    let registry = InprocRegistry::new();
    let member = make_keypair().public_key();
    let other_key = make_keypair().public_key();
    let forwarder = ScriptedOwner::new(OnWake::ReadyWithoutRoute);
    let owner = ScriptedOwner::new(OnWake::ReadyWithoutRoute);
    let bystander = ScriptedOwner::new(OnWake::ReadyWithoutRoute);
    assert!(registry.publish_wake_route("realm-a", member, "mob-b/worker/bob", forwarder.weak()));
    assert!(registry.publish_wake_route("realm-b", member, "bob", owner.weak()));
    assert!(registry.publish_wake_route("realm-c", other_key, "carol", bystander.weak()));

    let (name_a, found_a) = registry
        .wake_route_owner_in_namespace("realm-a", &member)
        .expect("realm-a's own route");
    assert_eq!(name_a, "mob-b/worker/bob");
    assert!(Weak::ptr_eq(&found_a, &forwarder.weak()));
    let (name_b, found_b) = registry
        .wake_route_owner_in_namespace("realm-b", &member)
        .expect("realm-b's own route");
    assert_eq!(name_b, "bob");
    assert!(Weak::ptr_eq(&found_b, &owner.weak()));

    // realm-c exists and has no route for the key: no other namespace
    // answers for it.
    assert!(
        registry
            .wake_route_owner_in_namespace("realm-c", &member)
            .is_none()
    );
    // A namespace that does not exist answers `None` and is not created.
    let namespaces_before = registry.state.read().namespaces.len();
    assert!(
        registry
            .wake_route_owner_in_namespace("realm-d", &member)
            .is_none()
    );
    assert!(!registry.state.read().namespaces.contains_key("realm-d"));
    assert_eq!(registry.state.read().namespaces.len(), namespaces_before);

    // The key's route in realm-b is published again by a successor under
    // another name: the read answers the route of now.
    let successor = ScriptedOwner::new(OnWake::ReadyWithoutRoute);
    assert!(registry.publish_wake_route("realm-b", member, "bob-2", successor.weak()));
    let (name_now, found_now) = registry
        .wake_route_owner_in_namespace("realm-b", &member)
        .expect("the re-published route");
    assert_eq!(name_now, "bob-2");
    assert!(Weak::ptr_eq(&found_now, &successor.weak()));
    assert!(!Weak::ptr_eq(&found_now, &owner.weak()));
    // realm-a's route is untouched by it.
    let (_, still_a) = registry
        .wake_route_owner_in_namespace("realm-a", &member)
        .expect("realm-a's route");
    assert!(Weak::ptr_eq(&still_a, &forwarder.weak()));
}

/// The read asks nothing of the owner (neither `wake` nor
/// `member_is_waking`) and keeps no strong reference to it. A route whose
/// owner is gone still answers `Some`, with a `Weak` that does not upgrade.
#[test]
fn the_wake_route_owner_read_asks_nothing_of_the_owner_and_keeps_no_reference() {
    let registry = InprocRegistry::new();
    let member = make_keypair().public_key();
    let owner = ScriptedOwner::new(OnWake::ReadyWithoutRoute);
    assert!(registry.publish_wake_route("realm-b", member, "worker-1", owner.weak()));
    let weak_before = Arc::weak_count(&owner);

    let (_, found) = registry
        .wake_route_owner_in_namespace("realm-b", &member)
        .expect("the published route");
    assert_eq!(owner.calls(), 0, "the read does not wake");
    assert_eq!(
        owner.waking_asked.load(Ordering::SeqCst),
        0,
        "the read does not ask the owner's residency"
    );
    assert_eq!(
        Arc::strong_count(&owner),
        1,
        "the read keeps no strong reference"
    );
    assert_eq!(
        Arc::weak_count(&owner),
        weak_before + 1,
        "the answer is one more Weak"
    );
    drop(found);
    assert_eq!(Arc::weak_count(&owner), weak_before);
    assert_eq!(Arc::strong_count(&owner), 1);

    // The owner is gone without retracting its route.
    drop(owner);
    let (name, gone) = registry
        .wake_route_owner_in_namespace("realm-b", &member)
        .expect("a route whose owner is gone is still read");
    assert_eq!(name, "worker-1");
    assert!(gone.upgrade().is_none(), "nothing can wake the member");
}

/// A host forwarder as the registry's docs describe it: published in the
/// sender's namespace under the member's key, it reads the member's own
/// owner in the member's namespace, hands it the same request under the
/// owner's name for the member, and once the owner answered ready makes the
/// member's inbox reachable from the sender's namespace.
struct HostForwarder {
    registry: Arc<InprocRegistry>,
    local: String,
    remote: String,
    alias: String,
    forwarded: AtomicUsize,
}

impl InprocPeerWaker for HostForwarder {
    fn wake(&self, request: PeerWakeRequest) -> PeerWakeFuture {
        let Some((member, owner)) = self
            .registry
            .wake_route_owner_in_namespace(&self.remote, &request.recipient)
        else {
            return Box::pin(async { Err(PeerWakeDenial::NotAMember) });
        };
        let Some(owner) = owner.upgrade() else {
            return Box::pin(async {
                Err(PeerWakeDenial::Refused(PeerWakeRefusal::MobUnavailable))
            });
        };
        self.forwarded.fetch_add(1, Ordering::SeqCst);
        let recipient = request.recipient;
        let mut forwarded = request;
        forwarded.member = member;
        let asked = owner.wake(forwarded);
        drop(owner);
        let registry = Arc::clone(&self.registry);
        let (local, remote, alias) = (self.local.clone(), self.remote.clone(), self.alias.clone());
        Box::pin(async move {
            let answer = asked.await?;
            if answer == PeerWakeAnswer::Ready
                && let Some(inbox) = registry.get_by_pubkey_in_namespace(&remote, &recipient)
            {
                let outcome = registry.register_with_meta_in_namespace(
                    &local,
                    alias,
                    recipient,
                    inbox,
                    PeerMeta::default(),
                );
                assert!(!outcome.is_rejected(), "{outcome:?}");
            }
            Ok(answer)
        })
    }

    fn member_is_waking(&self, _member: &str) -> bool {
        false
    }
}

fn forwarder_weak(forwarder: &Arc<HostForwarder>) -> Weak<dyn InprocPeerWaker> {
    let forwarder: Arc<dyn InprocPeerWaker> = Arc::clone(forwarder) as Arc<dyn InprocPeerWaker>;
    Arc::downgrade(&forwarder)
}

/// The host-composition use of the owner read, end to end: a send scoped to
/// realm-a reaches a member seated in realm-b through a forwarder. The
/// member's own owner gets the registry's request unchanged but for its own
/// name for the member, with the sender's residency (`sender_waking`), and
/// the envelope is handed off once.
#[tokio::test]
async fn a_host_forwarder_hands_the_same_request_to_the_members_own_owner() {
    let registry = Arc::new(InprocRegistry::new());
    let member = make_keypair();
    let (inbox, inbox_sender) = open_inbox();
    let owner = ScriptedOwner::new(OnWake::Materialize {
        registry: Arc::clone(&registry),
        namespace: "realm-b".to_string(),
        name: "worker-1".to_string(),
        pubkey: member.public_key(),
        sender: std::sync::Mutex::new(Some(inbox_sender)),
    });
    assert!(registry.publish_wake_route("realm-b", member.public_key(), "worker-1", owner.weak()));
    let forwarder = Arc::new(HostForwarder {
        registry: Arc::clone(&registry),
        local: "realm-a".to_string(),
        remote: "realm-b".to_string(),
        alias: "mob-b/worker/worker-1".to_string(),
        forwarded: AtomicUsize::new(0),
    });
    assert!(registry.publish_wake_route(
        "realm-a",
        member.public_key(),
        "mob-b/worker/worker-1",
        forwarder_weak(&forwarder),
    ));
    // The sender is itself being woken, as its own owner in realm-a
    // publishes it.
    let sender = make_keypair();
    let sender_owner = ScriptedOwner::new(OnWake::ReadyWithoutRoute).waking();
    assert!(registry.publish_wake_route(
        "realm-a",
        sender.public_key(),
        "alice",
        sender_owner.weak()
    ));

    let envelope_id = Uuid::new_v4();
    let delivered = registry
        .send_to_pubkey_in_namespace_with_id_wait(
            "realm-a",
            &sender,
            &member.public_key(),
            envelope_id,
            response(),
            true,
            BUDGET,
            None,
        )
        .await
        .expect("the forwarded wake answered ready and the envelope was handed off");
    assert_eq!(
        delivered.outcome,
        InprocDeliveryOutcome::Resolved(IngressDeliveryOutcome::Queued)
    );
    assert_eq!(forwarder.forwarded.load(Ordering::SeqCst), 1);
    assert_eq!(owner.calls(), 1);
    let request = owner.requests.lock()[0].clone();
    assert_eq!(
        request.member, "worker-1",
        "the owner's own name for the member"
    );
    assert_eq!(request.recipient, member.public_key());
    assert_eq!(request.sender, sender.public_key());
    assert_eq!(request.envelope_id, envelope_id);
    assert!(request.observed_missing);
    assert!(
        request.sender_waking,
        "the sender's residency reaches the owner"
    );
    assert_eq!(request.input_class, PeerWakeInputClass::Content);
    assert_eq!(queued(&inbox), 1);
    assert_eq!(sender_owner.calls(), 0);
}

/// A sender with no namespace resolves a wake route in any namespace and
/// fails closed when the key has one in more than one. A host forwarder's
/// route beside the owner's is such a second route: a member that is not
/// built is then no longer found by a sender with no namespace, while a
/// send scoped to the member's own namespace still wakes it.
#[tokio::test]
async fn a_forwarder_route_hides_a_member_that_is_not_built_from_a_sender_with_no_namespace() {
    let registry = Arc::new(InprocRegistry::new());
    let sender = make_keypair();
    let materializing = |name: &str, pubkey: PubKey, sender: InboxSender| {
        ScriptedOwner::new(OnWake::Materialize {
            registry: Arc::clone(&registry),
            namespace: "realm-b".to_string(),
            name: name.to_string(),
            pubkey,
            sender: std::sync::Mutex::new(Some(sender)),
        })
    };

    // Only its owner's route: a sender with no namespace wakes the member.
    let alone = make_keypair().public_key();
    let (alone_inbox, alone_sender) = open_inbox();
    let alone_owner = materializing("worker-1", alone, alone_sender);
    assert!(registry.publish_wake_route("realm-b", alone, "worker-1", alone_owner.weak()));
    registry
        .send_to_pubkey_any_namespace_with_id_wait(
            &sender,
            &alone,
            Uuid::new_v4(),
            response(),
            true,
            BUDGET,
            None,
        )
        .await
        .expect("one route in any namespace is the route");
    assert_eq!(alone_owner.calls(), 1);
    assert_eq!(queued(&alone_inbox), 1);

    // Its owner's route in realm-b and a forwarder's in realm-a.
    let linked = make_keypair().public_key();
    let (linked_inbox, linked_sender) = open_inbox();
    let linked_owner = materializing("worker-2", linked, linked_sender);
    let forwarder = ScriptedOwner::new(OnWake::ReadyWithoutRoute);
    assert!(registry.publish_wake_route("realm-b", linked, "worker-2", linked_owner.weak()));
    assert!(registry.publish_wake_route(
        "realm-a",
        linked,
        "mob-b/worker/worker-2",
        forwarder.weak()
    ));
    let refused = registry
        .send_to_pubkey_any_namespace_with_id_wait(
            &sender,
            &linked,
            Uuid::new_v4(),
            response(),
            true,
            BUDGET,
            None,
        )
        .await
        .expect_err("two routes in any namespace are no route");
    assert!(
        matches!(refused, InprocSendError::PeerNotFound(_)),
        "{refused:?}"
    );
    assert_eq!(linked_owner.calls(), 0);
    assert_eq!(forwarder.calls(), 0);

    // A send scoped to the member's own namespace is not affected.
    registry
        .send_to_pubkey_in_namespace_with_id_wait(
            "realm-b",
            &sender,
            &linked,
            Uuid::new_v4(),
            response(),
            true,
            BUDGET,
            None,
        )
        .await
        .expect("the member's own namespace wakes it");
    assert_eq!(linked_owner.calls(), 1);
    assert_eq!(forwarder.calls(), 0);
    assert_eq!(queued(&linked_inbox), 1);
}

/// A wake route never claims a name: a live registration under the member's
/// name still succeeds, and a lifecycle notice classifies as lifecycle.
#[test]
fn a_wake_route_claims_no_name_and_classifies_lifecycle_notices() {
    let registry = InprocRegistry::new();
    let member = make_keypair().public_key();
    let owner = ScriptedOwner::new(OnWake::ReadyWithoutRoute);
    registry.publish_wake_route("realm", member, "worker-1", owner.weak());
    let (_inbox, sender) = open_inbox();
    assert!(
        !registry
            .register_with_meta_in_namespace(
                "realm",
                "worker-1",
                member,
                sender,
                PeerMeta::default()
            )
            .is_rejected()
    );
    let lifecycle = MessageKind::Lifecycle {
        kind: meerkat_core::comms::PeerLifecycleKind::PeerAdded,
        params: serde_json::json!({}),
    };
    assert_eq!(
        PeerWakeInputClass::of(&lifecycle),
        PeerWakeInputClass::Lifecycle
    );
    let kickoff = MessageKind::Lifecycle {
        kind: meerkat_core::comms::PeerLifecycleKind::KickoffPending,
        params: serde_json::json!({}),
    };
    assert_eq!(
        PeerWakeInputClass::of(&kickoff),
        PeerWakeInputClass::Content
    );
    assert_eq!(
        PeerWakeInputClass::of(&message("m")),
        PeerWakeInputClass::Content
    );
}

/// A live route closed for peer ingress stops resolving, so a
/// send meets the member's owner; reopening restores the same generation with
/// its metadata.
#[tokio::test]
async fn a_closed_live_route_routes_to_the_owner_until_reopened() {
    let registry = InprocRegistry::new();
    let member = make_keypair();
    let (inbox, inbox_sender) = open_inbox();
    registry.register_with_meta_in_namespace(
        "realm",
        "worker-1",
        member.public_key(),
        inbox_sender.clone(),
        PeerMeta::default().with_description("kept"),
    );
    let owner = ScriptedOwner::new(OnWake::Refuse(PeerWakeRefusal::MobNotRunning));
    registry.publish_wake_route("realm", member.public_key(), "worker-1", owner.weak());

    assert!(registry.close_sender_in_namespace("realm", &member.public_key(), &inbox_sender));
    let refused = registry
        .send_to_pubkey_in_namespace_with_id_wait(
            "realm",
            &make_keypair(),
            &member.public_key(),
            Uuid::new_v4(),
            response(),
            true,
            BUDGET,
            None,
        )
        .await;
    assert!(matches!(
        refused,
        Err(InprocSendError::WakeRefused(PeerWakeRefusal::MobNotRunning))
    ));
    assert_eq!(queued(&inbox), 0);

    assert!(registry.reopen_sender_in_namespace("realm", &member.public_key(), &inbox_sender));
    let peers = registry.peers_in_namespace("realm");
    assert_eq!(peers.len(), 1);
    assert_eq!(peers[0].meta.description.as_deref(), Some("kept"));
    registry
        .send_to_pubkey_in_namespace_with_id_wait(
            "realm",
            &make_keypair(),
            &member.public_key(),
            Uuid::new_v4(),
            response(),
            true,
            BUDGET,
            None,
        )
        .await
        .expect("the reopened route delivers");
    assert_eq!(queued(&inbox), 1);
    assert_eq!(owner.calls(), 1);
}

/// A member whose wake is in flight already has its inbox
/// registered. A send from a live sender that finds that early live route
/// joins the wake through the owner first (no observed miss), and hands off
/// only after the owner answered ready.
#[tokio::test]
async fn an_early_live_route_of_a_waking_member_joins_its_wake_first() {
    let registry = InprocRegistry::new();
    let member = make_keypair();
    let (inbox, inbox_sender) = open_inbox();
    registry.register_with_meta_in_namespace(
        "realm",
        "worker-1",
        member.public_key(),
        inbox_sender,
        PeerMeta::default(),
    );
    let owner = ScriptedOwner::new(OnWake::ReadyWithoutRoute).waking();
    registry.publish_wake_route("realm", member.public_key(), "worker-1", owner.weak());

    registry
        .send_to_pubkey_in_namespace_with_id_wait(
            "realm",
            &make_keypair(),
            &member.public_key(),
            Uuid::new_v4(),
            response(),
            true,
            BUDGET,
            None,
        )
        .await
        .expect("delivered once the owner answered ready");
    assert_eq!(owner.calls(), 1);
    let request = owner.requests.lock()[0].clone();
    assert!(!request.observed_missing);
    assert!(!request.sender_waking);
    assert_eq!(queued(&inbox), 1);
}

/// A stale wake route (its owner no longer holds the key as a member) is a
/// non-member: `PeerNotFound`, the only construction of it on the wake path.
#[tokio::test]
async fn a_stale_route_whose_owner_denies_membership_is_peer_not_found() {
    let registry = InprocRegistry::new();
    let member = make_keypair();
    let owner = ScriptedOwner::new(OnWake::NotAMember);
    registry.publish_wake_route("realm", member.public_key(), "gone", owner.weak());
    let result = registry
        .send_to_pubkey_in_namespace_with_id_wait(
            "realm",
            &make_keypair(),
            &member.public_key(),
            Uuid::new_v4(),
            message("hello"),
            true,
            BUDGET,
            None,
        )
        .await;
    assert!(matches!(result, Err(InprocSendError::PeerNotFound(_))));
}

/// A silent lifecycle notice to a dormant member is
/// consumed by its owner without a wake, as a live member's drain consumes
/// one; the sender gets the volatile hand-off fact, and no inbox received
/// anything.
#[tokio::test]
async fn a_silent_lifecycle_notice_to_a_dormant_member_is_consumed_by_its_owner() {
    let registry = InprocRegistry::new();
    let member = make_keypair();
    let owner = ScriptedOwner::new(OnWake::ConsumeNotice);
    registry.publish_wake_route("realm", member.public_key(), "worker-1", owner.weak());
    let notice = MessageKind::Lifecycle {
        kind: meerkat_core::comms::PeerLifecycleKind::PeerAdded,
        params: serde_json::json!({}),
    };
    let delivery = registry
        .send_to_pubkey_in_namespace_with_id_wait(
            "realm",
            &make_keypair(),
            &member.public_key(),
            Uuid::new_v4(),
            notice,
            true,
            BUDGET,
            None,
        )
        .await
        .expect("the owner consumed the notice");
    assert_eq!(
        delivery.outcome,
        InprocDeliveryOutcome::Resolved(IngressDeliveryOutcome::VolatileHandedOff)
    );
    assert_eq!(
        owner.requests.lock()[0].input_class,
        PeerWakeInputClass::Lifecycle
    );
}

fn make_keypair() -> Keypair {
    Keypair::generate()
}

/// Two inbox generations of one member, both finalized by one runtime
/// session: the inbox a member had, and the one its re-wake installed.
fn runtime_backed_inbox_generations() -> (
    (Inbox, InboxSender),
    (Inbox, InboxSender),
    Arc<meerkat_runtime::TestPeerIngressRuntimeFinalizer>,
) {
    let (handle, finalizer) = meerkat_runtime::test_peer_comms_handle_and_runtime_finalizer();
    let context = || {
        Arc::new(crate::classify::IngressClassificationContext {
            require_peer_auth: false,
            trusted_peers: Arc::new(parking_lot::RwLock::new(TrustStore::new())),
            peer_comms_handle: Arc::new(parking_lot::RwLock::new(Some(Arc::clone(&handle)))),
            inproc_namespace: None,
            durable_runtime_consumer: true,
        })
    };
    (
        Inbox::new_classified(context()),
        Inbox::new_classified(context()),
        Arc::new(finalizer),
    )
}

/// Claim the next classified head of `inbox` and finalize it through the
/// member's runtime.
fn finalize_next(
    inbox: Arc<Inbox>,
    finalizer: Arc<meerkat_runtime::TestPeerIngressRuntimeFinalizer>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let claimed = loop {
            if let Some(claimed) = inbox.try_claim_one_classified() {
                break claimed;
            }
            tokio::task::yield_now().await;
        };
        let claim = crate::runtime::comms_runtime::test_peer_ingress_queue_claim(claimed);
        finalizer
            .finalize(claim)
            .await
            .expect("the member's runtime finalizes the claim");
    })
}

/// A durable message the member's runtime already admitted, sent again
/// through a stale route (as a sender does when the closing inbox lost the
/// receipt), takes the wake path with its own envelope id: the re-wake's
/// inbox hands it to the same runtime, which resolves it `Deduplicated`. The
/// member gets one input.
#[tokio::test]
async fn a_re_wake_hands_an_admitted_message_off_with_its_id_and_it_is_deduplicated() {
    let registry = Arc::new(InprocRegistry::new());
    let member = make_keypair();
    let sender = make_keypair();
    let ((first_inbox, first_sender), (fresh_inbox, fresh_sender), finalizer) =
        runtime_backed_inbox_generations();
    let first_inbox = Arc::new(first_inbox);
    let fresh_inbox = Arc::new(fresh_inbox);
    registry.register_with_meta_in_namespace(
        "realm",
        "worker-1",
        member.public_key(),
        first_sender.clone(),
        PeerMeta::default(),
    );
    let envelope_id = Uuid::new_v4();

    let first_finalize = finalize_next(Arc::clone(&first_inbox), Arc::clone(&finalizer));
    let first = registry
        .send_to_pubkey_in_namespace_with_id_wait(
            "realm",
            &sender,
            &member.public_key(),
            envelope_id,
            message("admitted once"),
            true,
            BUDGET,
            None,
        )
        .await
        .expect("the live inbox admits the message");
    first_finalize.await.expect("the first admission completes");
    assert!(matches!(
        first.outcome,
        InprocDeliveryOutcome::Resolved(IngressDeliveryOutcome::DurablyResolved(
            meerkat_core::PeerIngressTerminalOutcomeKind::Accepted
        ))
    ));

    // The member's inbox closes; its owner re-wakes it into a fresh inbox of
    // the same runtime session.
    drop(first_inbox);
    let owner = ScriptedOwner::new(OnWake::Replace {
        registry: Arc::clone(&registry),
        namespace: "realm".to_string(),
        name: "worker-1".to_string(),
        pubkey: member.public_key(),
        stale: first_sender,
        fresh: std::sync::Mutex::new(Some(fresh_sender)),
    });
    registry.publish_wake_route("realm", member.public_key(), "worker-1", owner.weak());

    let retry_finalize = finalize_next(Arc::clone(&fresh_inbox), Arc::clone(&finalizer));
    let retried = registry
        .send_to_pubkey_in_namespace_with_id_wait(
            "realm",
            &sender,
            &member.public_key(),
            envelope_id,
            message("admitted once"),
            true,
            BUDGET,
            None,
        )
        .await
        .expect("the re-wake hands the message off");
    retry_finalize.await.expect("the redelivery is finalized");
    assert_eq!(owner.calls(), 1, "the stale route took one wake");
    assert_eq!(retried.envelope_id, envelope_id, "the same envelope id");
    assert!(
        matches!(
            retried.outcome,
            InprocDeliveryOutcome::Resolved(IngressDeliveryOutcome::DurablyResolved(
                meerkat_core::PeerIngressTerminalOutcomeKind::Deduplicated
            ))
        ),
        "the member's runtime admits the message once: {:?}",
        retried.outcome
    );
}
