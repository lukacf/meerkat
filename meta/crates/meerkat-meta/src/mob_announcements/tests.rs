//! Store conformance and owner behaviour for mob announcements.
//!
//! The conformance cases run against any [`AnnouncementStore`]; here they run
//! against the SQLite store, both fresh and reopened from disk. The owner
//! cases drive [`AnnouncementService`] against a scripted native owner that
//! models Meerkat's continuation contract: one binding per key, identical
//! replays answered with the original receipt, other content refused, and
//! status read from the binding only.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use super::availability::{
    BlockCause, EvidenceClass, MemberDeparture, NotServedHereCause, SuspendCause,
};
use super::native::{
    MemberMobEnd, MemberMobScan, MemberProblem, MemberRoute, MobOrigin, MobOriginFacts,
    MobPresence, NativeDelivery, NativeStatus, NativeStatusError, NativeSubmitError,
    NativeUnavailable, PrepareError, continuation_for,
};
use super::owner::{
    AnnouncementConfig, AnnouncementError, AnnouncementService, NativeObservation, RoundReport,
};
use super::sqlite::SqliteAnnouncementStore;
use super::store::AnnouncementStore;
use super::types::*;

/// Declarations and role-selected subscriptions, over this module's harness.
#[path = "declarations_tests.rs"]
mod declarations_tests;

fn sid(value: &str) -> SubscriptionId {
    SubscriptionId::new(value).unwrap()
}

pub(super) fn op(value: &str) -> OperationId {
    OperationId::new(value).unwrap()
}

/// A member departure, as a peer's signed answer gives it.
fn departed() -> SubscriptionEnd {
    SubscriptionEnd::MemberDeparted {
        departure: MemberDeparture::from_signed_departure(MobId::new("mob-b").unwrap()),
    }
}

/// The mob of the gateway these tests run as: its members are local.
pub(super) const OWN_MOB: &str = "payments";

/// The kept route of a member of [`OWN_MOB`].
fn local_route() -> MemberRouteRecord {
    MemberRouteRecord {
        route: MemberRoute::Local,
        incarnation: None,
    }
}

pub(super) fn member(value: &str) -> Member {
    Member::new(mob(OWN_MOB), MemberIdentity::new(value).unwrap())
}

pub(super) fn mob(value: &str) -> MobId {
    MobId::new(value).unwrap()
}

fn topics(values: &[&str]) -> BTreeSet<Topic> {
    values
        .iter()
        .map(|value| Topic::new(*value).unwrap())
        .collect()
}

pub(super) fn announce_request(
    mob_id: &str,
    key: &str,
    topic: &[&str],
    body: &str,
) -> AnnounceRequest {
    AnnounceRequest {
        mob_id: mob(mob_id),
        announced_by: Announcer::Host,
        source_key: SourceKey::new(key).unwrap(),
        topics: topics(topic),
        body: body.to_string(),
        correlation_id: None,
        causes: Vec::new(),
    }
}

/// Every mob these tests announce into. A subscription made with [`put`]
/// names them all, so only a test that narrows one sees a mob left out.
const TEST_MOBS: &[&str] = &[
    "payments", "events", "builds", "refunds", "a-newer", "z-older", "a-mob", "b-mob",
];

pub(super) fn put(id: &str, to: &str, topic: &[&str], start: StartFrontier) -> PutSubscription {
    PutSubscription {
        id: sid(id),
        member: member(to),
        selector: Selector {
            topics: topics(topic),
            mobs: TEST_MOBS.iter().map(|name| mob(name)).collect(),
        },
        start,
        expected_revision: None,
        route: Some(local_route()),
        member_mob_cursor: None,
        child_limits: ChildMemberLimits::default(),
        delivery: SubscriptionDelivery::default(),
    }
}

pub(super) fn policy() -> SealingPolicy {
    SealingPolicy {
        max_count: 256,
        max_bytes: 1_048_576,
        max_delay_ms: 1_000,
        min_wake_interval_ms: 0,
    }
}

pub(super) fn batch_id(value: &str) -> BatchId {
    BatchId::new(value).unwrap()
}

fn receipt(id: &str) -> NativeReceipt {
    NativeReceipt {
        delivery_id: format!("continuation:{id}"),
        address: "rt:member:test:agent:1".into(),
        submission_digest: format!("digest-{id}"),
        committed_at_ms: 1,
    }
}

fn temp_store() -> (tempfile::TempDir, SqliteAnnouncementStore) {
    let dir = tempfile::tempdir().unwrap();
    let store =
        SqliteAnnouncementStore::open(dir.path().join("mob_announcements.sqlite3")).unwrap();
    (dir, store)
}

// ---------------------------------------------------------------------------
// Store conformance
// ---------------------------------------------------------------------------

async fn announcements_are_sequenced_per_mob_and_bound_to_content(store: &dyn AnnouncementStore) {
    let first = store
        .announce(announce_request("payments", "k1", &["ios"], "one"), 10)
        .await
        .unwrap();
    assert_eq!(first.announcement.sequence, 1);
    assert!(!first.replayed);
    let second = store
        .announce(announce_request("payments", "k2", &["ios"], "two"), 11)
        .await
        .unwrap();
    assert_eq!(second.announcement.sequence, 2);
    let other = store
        .announce(announce_request("builds", "k1", &["ios"], "one"), 12)
        .await
        .unwrap();
    assert_eq!(
        other.announcement.sequence, 1,
        "sequences of independent mobs are never shared"
    );
    let replay = store
        .announce(announce_request("payments", "k1", &["ios"], "one"), 99)
        .await
        .unwrap();
    assert!(replay.replayed);
    assert_eq!(
        replay.announcement, first.announcement,
        "a replay returns the original"
    );
    let conflict = store
        .announce(announce_request("payments", "k1", &["ios"], "changed"), 99)
        .await;
    assert_eq!(
        conflict,
        Err(AnnouncementStoreError::AnnouncementConflict {
            existing: Box::new(first.announcement.clone()),
        })
    );
    assert_eq!(
        store
            .announcement(&mob("payments"), &SourceKey::new("k1").unwrap())
            .await
            .unwrap(),
        Some(first.announcement)
    );
}

async fn capture_commits_with_the_announcement(store: &dyn AnnouncementStore) {
    store
        .announce(announce_request("payments", "before", &["ios"], "old"), 1)
        .await
        .unwrap();
    store
        .put_subscription(
            &op("o1"),
            put("s1", "agent:a", &["ios"], StartFrontier::Now),
            2,
        )
        .await
        .unwrap();
    let mut scoped = put("s2", "agent:b", &["ios"], StartFrontier::Now);
    scoped.selector.mobs = [mob("builds")].into();
    store.put_subscription(&op("o2"), scoped, 2).await.unwrap();
    let matched = store
        .announce(
            announce_request("payments", "after", &["ios", "billing"], "new"),
            3,
        )
        .await
        .unwrap();
    assert_eq!(matched.captured_for, vec![sid("s1")]);
    let unmatched = store
        .announce(announce_request("payments", "other", &["android"], "x"), 4)
        .await
        .unwrap();
    assert!(unmatched.captured_for.is_empty());
    let backlogs = store.backlogs(10).await.unwrap();
    assert_eq!(backlogs.len(), 1, "only s1's member owes anything");
    assert_eq!(backlogs[0].member, member("agent:a"));
    assert_eq!(
        backlogs[0].eligible_count, 1,
        "a subscription starting now captures nothing announced before it"
    );
}

async fn explicit_frontiers_replay_or_report_a_gap(store: &dyn AnnouncementStore) {
    for key in ["a", "b", "c"] {
        store
            .announce(announce_request("payments", key, &["ios"], key), 1)
            .await
            .unwrap();
    }
    let replaying = store
        .put_subscription(
            &op("o1"),
            put(
                "s1",
                "agent:a",
                &["ios"],
                StartFrontier::After {
                    frontier: [(mob("payments"), 1)].into(),
                },
            ),
            5,
        )
        .await
        .unwrap();
    assert_eq!(replaying.frontier.get(&mob("payments")), Some(&1));
    assert_eq!(
        store.backlogs(10).await.unwrap()[0].eligible_count,
        2,
        "sequences 2 and 3 are replayed"
    );
    store
        .change_subscription(&op("o2"), &sid("s1"), 1, SubscriptionChange::Cancel, 6)
        .await
        .unwrap();
    // Nothing is owed any more, so retention removes the whole prefix.
    store.prune(10_000, 0, 100).await.unwrap();
    let gap = store
        .put_subscription(
            &op("o3"),
            put(
                "s2",
                "agent:a",
                &["ios"],
                StartFrontier::After {
                    frontier: [(mob("payments"), 0)].into(),
                },
            ),
            10_001,
        )
        .await;
    assert_eq!(
        gap,
        Err(AnnouncementStoreError::Gap {
            mob_id: mob("payments"),
            earliest_retained: 4,
        }),
        "a frontier older than retained history is a typed gap, never a jump"
    );
}

async fn frontiers_outside_a_mob_are_refused(store: &dyn AnnouncementStore) {
    for key in ["k1", "k2"] {
        store
            .announce(announce_request("payments", key, &["ios"], key), 1)
            .await
            .unwrap();
    }
    let after = |frontier: &[(&str, u64)]| StartFrontier::After {
        frontier: frontier
            .iter()
            .map(|(name, sequence)| (mob(name), *sequence))
            .collect(),
    };
    assert_eq!(
        store
            .put_subscription(
                &op("o1"),
                put("s1", "agent:a", &["ios"], after(&[("payments", 3)])),
                2,
            )
            .await,
        Err(AnnouncementStoreError::FrontierBeyondHead {
            mob_id: mob("payments"),
            head: 2,
        }),
        "a frontier beyond the head would skip announcements not made yet; it is refused, \
         never clamped"
    );
    assert_eq!(
        store
            .put_subscription(
                &op("o2"),
                put("s2", "agent:a", &["ios"], after(&[("refunds", 1)])),
                2,
            )
            .await,
        Err(AnnouncementStoreError::FrontierBeyondHead {
            mob_id: mob("refunds"),
            head: 0,
        }),
        "a mob that has announced nothing yet is at head 0"
    );
    let mut narrow = put("s3", "agent:a", &["ios"], after(&[("refunds", 0)]));
    narrow.selector.mobs = [mob("payments")].into();
    assert!(
        matches!(
            store.put_subscription(&op("o3"), narrow, 2).await,
            Err(AnnouncementStoreError::Invalid(_))
        ),
        "a frontier may name only the mobs the subscription names"
    );
    // A frontier at the head, or at 0 for a mob that has announced nothing
    // yet, captures from the next announcement.
    let subscription = store
        .put_subscription(
            &op("o4"),
            put(
                "s4",
                "agent:a",
                &["ios"],
                after(&[("payments", 2), ("refunds", 0)]),
            ),
            2,
        )
        .await
        .unwrap();
    assert_eq!(subscription.frontier.get(&mob("payments")), Some(&2));
    let refund = store
        .announce(announce_request("refunds", "r1", &["ios"], "r1"), 3)
        .await
        .unwrap();
    assert_eq!(refund.captured_for, vec![sid("s4")]);
}

async fn subscription_changes_are_revisioned_and_idempotent(store: &dyn AnnouncementStore) {
    let created = store
        .put_subscription(
            &op("o1"),
            put("s1", "agent:a", &["ios"], StartFrontier::Now),
            1,
        )
        .await
        .unwrap();
    assert_eq!(created.revision, 1);
    assert_eq!(
        store
            .put_subscription(
                &op("o1"),
                put("s1", "agent:a", &["ios"], StartFrontier::Now),
                2
            )
            .await
            .unwrap(),
        created,
        "the same operation replays its original result"
    );
    assert_eq!(
        store
            .put_subscription(
                &op("o1"),
                put("s1", "agent:a", &["web"], StartFrontier::Now),
                2
            )
            .await,
        Err(AnnouncementStoreError::OperationReused)
    );
    assert_eq!(
        store
            .put_subscription(
                &op("o2"),
                put("s1", "agent:a", &["ios"], StartFrontier::Now),
                2
            )
            .await,
        Err(AnnouncementStoreError::StaleRevision { current: 1 }),
        "creating an existing subscription names no revision"
    );
    let mut update = put("s1", "agent:a", &["web"], StartFrontier::Now);
    update.expected_revision = Some(7);
    assert_eq!(
        store.put_subscription(&op("o3"), update.clone(), 3).await,
        Err(AnnouncementStoreError::StaleRevision { current: 1 })
    );
    update.expected_revision = Some(1);
    update.member = member("agent:b");
    assert_eq!(
        store.put_subscription(&op("o4"), update.clone(), 3).await,
        Err(AnnouncementStoreError::MemberMismatch)
    );
    update.member = member("agent:a");
    let updated = store.put_subscription(&op("o5"), update, 3).await.unwrap();
    assert_eq!(updated.revision, 2);
    assert_eq!(updated.selector_revision, 2);
    let paused = store
        .change_subscription(&op("o6"), &sid("s1"), 2, SubscriptionChange::Pause, 4)
        .await
        .unwrap();
    assert_eq!(paused.subscription.state, SubscriptionState::Paused);
    assert_eq!(
        store
            .change_subscription(&op("o6"), &sid("s1"), 2, SubscriptionChange::Pause, 5)
            .await
            .unwrap(),
        paused
    );
    let cancelled = store
        .change_subscription(&op("o7"), &sid("s1"), 3, SubscriptionChange::Cancel, 6)
        .await
        .unwrap();
    assert_eq!(cancelled.subscription.state, SubscriptionState::Cancelled);
    assert_eq!(
        store
            .change_subscription(&op("o8"), &sid("s1"), 4, SubscriptionChange::Resume, 7)
            .await,
        Err(AnnouncementStoreError::SubscriptionCancelled)
    );
}

async fn paused_subscriptions_capture_and_hold(store: &dyn AnnouncementStore) {
    store
        .put_subscription(
            &op("o1"),
            put("s1", "agent:a", &["ios"], StartFrontier::Now),
            1,
        )
        .await
        .unwrap();
    store
        .change_subscription(&op("o2"), &sid("s1"), 1, SubscriptionChange::Pause, 2)
        .await
        .unwrap();
    for index in 0..5 {
        let outcome = store
            .announce(
                announce_request("payments", &format!("k{index}"), &["ios"], "x"),
                3,
            )
            .await
            .unwrap();
        assert_eq!(
            outcome.captured_for,
            vec![sid("s1")],
            "a paused subscription captures"
        );
    }
    assert!(
        store.backlogs(10).await.unwrap().is_empty(),
        "a paused subscription's announcements are not deliverable"
    );
    assert_eq!(
        store
            .seal_batch(&member("agent:a"), &batch_id("b1"), &policy(), 1_000_000)
            .await
            .unwrap(),
        None
    );
    store
        .change_subscription(&op("o3"), &sid("s1"), 2, SubscriptionChange::Resume, 4)
        .await
        .unwrap();
    let backlog = store.backlogs(10).await.unwrap().remove(0);
    assert_eq!(backlog.eligible_count, 5);
    assert_eq!(backlog.flush_requested_at_ms, Some(4), "a resume flushes");
    assert_eq!(policy().due_at(&backlog), Some(0));
}

async fn sealing_binds_exact_members_once(store: &dyn AnnouncementStore) {
    for id in ["s1", "s2"] {
        store
            .put_subscription(&op(id), put(id, "agent:a", &["ios"], StartFrontier::Now), 1)
            .await
            .unwrap();
    }
    store
        .announce(announce_request("payments", "k1", &["ios"], "shared"), 2)
        .await
        .unwrap();
    assert_eq!(
        store
            .seal_batch(&member("agent:a"), &batch_id("b0"), &policy(), 500)
            .await
            .unwrap(),
        None,
        "not due before the delay"
    );
    let batch = store
        .seal_batch(&member("agent:a"), &batch_id("b1"), &policy(), 1_002)
        .await
        .unwrap()
        .expect("due after the delay");
    assert_eq!(
        batch.items.len(),
        2,
        "each subscription's capture is an item"
    );
    assert_eq!(
        batch
            .body
            .matches("Announcement #1 in mob \"payments\"")
            .count(),
        1,
        "one announcement is rendered once"
    );
    assert_eq!(batch.content_digest, content_digest(&batch.body));
    assert_eq!(batch.state, BatchState::Sealed);
    let backlog_after = store.backlogs(10).await.unwrap();
    assert!(backlog_after.is_empty(), "everything owed is in the batch");
    store
        .announce(announce_request("payments", "k2", &["ios"], "later"), 1_003)
        .await
        .unwrap();
    assert!(
        store.backlogs(10).await.unwrap().is_empty(),
        "a member with a batch outstanding takes no place in the backlog page"
    );
    assert_eq!(
        store
            .seal_batch(&member("agent:a"), &batch_id("b2"), &policy(), 9_999_999)
            .await
            .unwrap(),
        None
    );
    assert_eq!(store.batch(&batch_id("b1")).await.unwrap(), Some(batch));
}

async fn transitions_settle_owed_announcements(store: &dyn AnnouncementStore) {
    store
        .put_subscription(
            &op("o1"),
            put("s1", "agent:a", &["ios"], StartFrontier::Now),
            1,
        )
        .await
        .unwrap();
    store
        .announce(announce_request("payments", "k1", &["ios"], "x"), 2)
        .await
        .unwrap();
    store
        .seal_batch(&member("agent:a"), &batch_id("b1"), &policy(), 2_000)
        .await
        .unwrap()
        .unwrap();
    let id = batch_id("b1");
    assert_eq!(
        store
            .transition_batch(&id, BatchStateTag::Committed, BatchEvent::BeginSubmit, 3)
            .await,
        Err(AnnouncementStoreError::BatchMoved {
            current: BatchStateTag::Sealed,
        })
    );
    store
        .transition_batch(&id, BatchStateTag::Sealed, BatchEvent::BeginSubmit, 3)
        .await
        .unwrap();
    store
        .transition_batch(
            &id,
            BatchStateTag::Submitting,
            BatchEvent::NativeCommitted {
                receipt: receipt("b1"),
            },
            4,
        )
        .await
        .unwrap();
    assert_eq!(store.unsettled_batches(None, 10).await.unwrap().len(), 1);
    let delivered = store
        .transition_batch(
            &id,
            BatchStateTag::Committed,
            BatchEvent::NativeApplied {
                receipt: receipt("b1"),
                session_id: "session".into(),
                input_id: "input".into(),
            },
            5,
        )
        .await
        .unwrap();
    assert_eq!(delivered.state.tag(), BatchStateTag::Delivered);
    assert!(store.unsettled_batches(None, 10).await.unwrap().is_empty());
    let report = store.prune(10_000_000, 0, 100).await.unwrap();
    assert_eq!(
        report.announcements, 1,
        "a settled announcement no longer holds retention"
    );
    assert_eq!(report.batches, 1);
}

async fn stranded_batches_hand_over_to_a_successor(store: &dyn AnnouncementStore) {
    store
        .put_subscription(
            &op("o1"),
            put("s1", "agent:a", &["ios"], StartFrontier::Now),
            1,
        )
        .await
        .unwrap();
    store
        .announce(announce_request("payments", "k1", &["ios"], "x"), 2)
        .await
        .unwrap();
    let id = batch_id("b1");
    store
        .seal_batch(&member("agent:a"), &id, &policy(), 2_000)
        .await
        .unwrap()
        .unwrap();
    store
        .transition_batch(&id, BatchStateTag::Sealed, BatchEvent::BeginSubmit, 3)
        .await
        .unwrap();
    store
        .transition_batch(
            &id,
            BatchStateTag::Submitting,
            BatchEvent::NativeCommitted {
                receipt: receipt("b1"),
            },
            4,
        )
        .await
        .unwrap();
    let stranded = store
        .transition_batch(
            &id,
            BatchStateTag::Committed,
            BatchEvent::NativeStranded {
                receipt: receipt("b1"),
                successor: Some(SuccessorBatch {
                    id: batch_id("b2"),
                    body: "redelivery".into(),
                    gaps: Vec::new(),
                    deferred: Vec::new(),
                }),
            },
            5,
        )
        .await
        .unwrap();
    assert_eq!(stranded.successor, Some(batch_id("b2")));
    let successor = store.batch(&batch_id("b2")).await.unwrap().unwrap();
    assert_eq!(successor.predecessor, Some(id));
    assert_eq!(
        successor.items, stranded.items,
        "the same announcements move on"
    );
    assert_eq!(successor.state, BatchState::Sealed);
    let report = store.prune(10_000_000, 0, 100).await.unwrap();
    assert_eq!(
        report.announcements, 0,
        "announcements still owed through the successor keep their announcement"
    );
}

async fn cancellation_settles_what_was_never_dispatched(store: &dyn AnnouncementStore) {
    store
        .put_subscription(
            &op("o1"),
            put("s1", "agent:a", &["ios"], StartFrontier::Now),
            1,
        )
        .await
        .unwrap();
    store
        .announce(announce_request("payments", "k1", &["ios"], "x"), 2)
        .await
        .unwrap();
    store
        .seal_batch(&member("agent:a"), &batch_id("sealed"), &policy(), 2_000)
        .await
        .unwrap()
        .unwrap();
    store
        .announce(announce_request("payments", "k2", &["ios"], "y"), 3)
        .await
        .unwrap();
    let outcome = store
        .change_subscription(&op("o2"), &sid("s1"), 1, SubscriptionChange::Cancel, 4)
        .await
        .unwrap();
    assert!(
        outcome.unresolved_batches.is_empty(),
        "a sealed batch nothing wants any more is cancelled, not left owed"
    );
    assert_eq!(
        store
            .batch(&batch_id("sealed"))
            .await
            .unwrap()
            .unwrap()
            .state,
        BatchState::Cancelled
    );
    assert!(store.backlogs(10).await.unwrap().is_empty());
    let report = store.prune(10_000_000, 0, 100).await.unwrap();
    assert_eq!(report.announcements, 2);

    store
        .put_subscription(
            &op("o3"),
            put("s2", "agent:a", &["ios"], StartFrontier::Now),
            5,
        )
        .await
        .unwrap();
    store
        .announce(announce_request("payments", "k3", &["ios"], "z"), 6)
        .await
        .unwrap();
    let id = batch_id("dispatching");
    store
        .seal_batch(&member("agent:a"), &id, &policy(), 2_000)
        .await
        .unwrap()
        .unwrap();
    store
        .transition_batch(&id, BatchStateTag::Sealed, BatchEvent::BeginSubmit, 7)
        .await
        .unwrap();
    let outcome = store
        .change_subscription(&op("o4"), &sid("s2"), 1, SubscriptionChange::Cancel, 8)
        .await
        .unwrap();
    assert_eq!(
        outcome.unresolved_batches,
        vec![id.clone()],
        "a batch that may have reached the native owner is reported, never erased"
    );
    assert_eq!(
        store.batch(&id).await.unwrap().unwrap().state.tag(),
        BatchStateTag::Submitting
    );
}

async fn retention_keeps_a_contiguous_history(store: &dyn AnnouncementStore) {
    store
        .put_subscription(
            &op("o1"),
            put("s1", "agent:a", &["ios"], StartFrontier::Now),
            1,
        )
        .await
        .unwrap();
    store
        .announce(announce_request("payments", "k1", &["ios"], "owed"), 2)
        .await
        .unwrap();
    store
        .announce(announce_request("payments", "k2", &["web"], "free"), 3)
        .await
        .unwrap();
    let report = store.prune(1_000_000, 0, 100).await.unwrap();
    assert_eq!(
        report.announcements, 0,
        "an owed announcement blocks retention behind it, so history has no holes"
    );
    assert!(
        store
            .announcement(&mob("payments"), &SourceKey::new("k2").unwrap())
            .await
            .unwrap()
            .is_some()
    );
}

async fn retention_prunes_the_oldest_first_across_mobs(store: &dyn AnnouncementStore) {
    store
        .announce(announce_request("z-older", "k1", &["ios"], "first"), 1)
        .await
        .unwrap();
    store
        .announce(announce_request("a-newer", "k1", &["ios"], "second"), 2)
        .await
        .unwrap();
    let report = store.prune(1_000_000, 0, 1).await.unwrap();
    assert_eq!(report.announcements, 1);
    assert!(
        store
            .announcement(&mob("z-older"), &SourceKey::new("k1").unwrap())
            .await
            .unwrap()
            .is_none(),
        "the oldest announcement goes first, whatever its mob_id is named"
    );
    assert!(
        store
            .announcement(&mob("a-newer"), &SourceKey::new("k1").unwrap())
            .await
            .unwrap()
            .is_some()
    );
}

async fn retention_passes_report_when_more_work_is_left(store: &dyn AnnouncementStore) {
    // Three members owe the first announcement; each one's batch of it is
    // cancelled, so all three batches can go once the announcement has gone.
    for (index, agent) in ["agent:a", "agent:b", "agent:c"].iter().enumerate() {
        store
            .put_subscription(
                &op(&format!("o{index}")),
                put(&format!("s{index}"), agent, &["ios"], StartFrontier::Now),
                1,
            )
            .await
            .unwrap();
    }
    for index in 0..3 {
        let topic = if index == 0 { "ios" } else { "web" };
        store
            .announce(
                announce_request("payments", &format!("k{index}"), &[topic], "old"),
                1,
            )
            .await
            .unwrap();
    }
    for (index, agent) in ["agent:a", "agent:b", "agent:c"].iter().enumerate() {
        let id = batch_id(&format!("b{index}"));
        store
            .seal_batch(&member(agent), &id, &policy(), 2_000)
            .await
            .unwrap()
            .unwrap();
        store
            .transition_batch(&id, BatchStateTag::Sealed, BatchEvent::Cancel, 3_000)
            .await
            .unwrap();
    }
    let first = store.prune(1_000_000, 0, 2).await.unwrap();
    assert_eq!(
        (first.announcements, first.batches, first.operation_receipts),
        (2, 2, 2),
        "each kind has its own limit"
    );
    assert!(first.more, "a pass that stopped at its limit says so");
    let second = store.prune(1_000_000, 0, 2).await.unwrap();
    assert_eq!(
        (
            second.announcements,
            second.batches,
            second.operation_receipts
        ),
        (1, 1, 1)
    );
    assert!(!second.more, "nothing is left for another pass");
}

pub(super) fn peer_member(identity: &str, peer: &str) -> Member {
    Member::new(mob(peer), MemberIdentity::new(identity).unwrap())
}

/// A subscription for `to`, kept on the route the owner gives a member of
/// its mob: this gateway's own, or a peer's.
pub(super) fn put_for(id: &str, to: Member, topic: &[&str]) -> PutSubscription {
    let route = if to.mob_id.as_str() == OWN_MOB {
        MemberRoute::Local
    } else {
        MemberRoute::Peer
    };
    PutSubscription {
        member: to,
        route: Some(MemberRouteRecord {
            route,
            incarnation: None,
        }),
        ..put(id, "agent:a", topic, StartFrontier::Now)
    }
}

async fn remote_batches_fit_one_delivery_request(store: &dyn AnnouncementStore) {
    let remote = peer_member("agent:a", "mob-b");
    store
        .put_subscription(&op("o1"), put_for("s1", remote.clone(), &["ios"]), 1)
        .await
        .unwrap();
    store
        .put_subscription(
            &op("o2"),
            put("s2", "agent:c", &["ios"], StartFrontier::Now),
            1,
        )
        .await
        .unwrap();
    // Forty announcements of about 2 KiB: more than one delivery request
    // carries, far less than the byte cap.
    let body = "x".repeat(2_000);
    for index in 0..40_u64 {
        store
            .announce(
                announce_request("payments", &format!("k{index}"), &["ios"], &body),
                2 + index,
            )
            .await
            .unwrap();
    }
    let batch = store
        .seal_batch(&remote, &batch_id("b1"), &policy(), 1_000_000)
        .await
        .unwrap()
        .unwrap();
    assert!(
        !batch.items.is_empty() && batch.items.len() < 40,
        "the wire budget bounds the batch: {}",
        batch.items.len()
    );
    assert_eq!(batch.member, remote);
    assert!(wire_len(&batch.body) <= MAX_REMOTE_BODY_WIRE_BYTES);
    // A successor carrying the same announcements fits too.
    let mut items = Vec::new();
    for item in &batch.items {
        let announcement = store
            .announcement(&item.mob_id, &item.source_key)
            .await
            .unwrap()
            .unwrap();
        items.push(render_item(&announcement));
    }
    let successor_id = BatchId::new(format!("batch-{}", uuid::Uuid::new_v4())).unwrap();
    let successor = render_batch(&successor_id, Some(&batch.id), &[], &items);
    assert!(wire_len(&successor) <= MAX_REMOTE_BODY_WIRE_BYTES);
    // A local member's batch is bounded by its byte cap alone.
    let local = store
        .seal_batch(&member("agent:c"), &batch_id("b2"), &policy(), 1_000_000)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(local.items.len(), 40);
}

async fn oversized_renderings_are_refused(store: &dyn AnnouncementStore) {
    // Line breaks render far larger than their byte count.
    let body = "\n".repeat(MAX_BODY_BYTES);
    assert!(matches!(
        store
            .announce(announce_request("payments", "k1", &["ios"], &body), 1)
            .await,
        Err(AnnouncementStoreError::TooLarge(_))
    ));
    let body = "x".repeat(MAX_BODY_BYTES);
    store
        .announce(announce_request("payments", "k2", &["ios"], &body), 2)
        .await
        .unwrap();
}

async fn the_unsettled_walk_resumes_after_a_position(store: &dyn AnnouncementStore) {
    for (index, agent) in ["agent:a", "agent:b", "agent:c"].iter().enumerate() {
        let id = format!("s{index}");
        let topic = format!("t{index}");
        store
            .put_subscription(
                &op(&format!("o{index}")),
                put(&id, agent, &[topic.as_str()], StartFrontier::Now),
                1,
            )
            .await
            .unwrap();
        store
            .announce(
                announce_request("events", &format!("k{index}"), &[topic.as_str()], "x"),
                2,
            )
            .await
            .unwrap();
        store
            .seal_batch(
                &member(agent),
                &batch_id(&format!("b{index}")),
                &policy(),
                2_000 + index as u64,
            )
            .await
            .unwrap()
            .expect("due once the delay passed");
    }
    let all = store.unsettled_batches(None, 10).await.unwrap();
    let ids = |batches: &[Batch]| {
        batches
            .iter()
            .map(|batch| batch.id.to_string())
            .collect::<Vec<_>>()
    };
    assert_eq!(ids(&all), ["b0", "b1", "b2"], "(sealed time, id) order");
    let after = BatchPosition::of(&all[0]);
    assert_eq!(
        ids(&store.unsettled_batches(Some(&after), 10).await.unwrap()),
        ["b1", "b2"],
        "the walk resumes after the position"
    );
    let end = BatchPosition::of(&all[2]);
    assert!(
        store
            .unsettled_batches(Some(&end), 10)
            .await
            .unwrap()
            .is_empty()
    );
    store
        .announce(announce_request("events", "k0-more", &["t0"], "y"), 2_010)
        .await
        .unwrap();
    assert!(
        store.backlogs(10).await.unwrap().is_empty(),
        "agent:a's new announcement waits behind its outstanding batch and takes no place"
    );
}

async fn lifecycle_ends_subscriptions_on_final_facts(store: &dyn AnnouncementStore) {
    store
        .put_subscription(
            &op("o1"),
            put("s1", "agent:a", &["ios"], StartFrontier::Now),
            1,
        )
        .await
        .unwrap();
    store
        .put_subscription(
            &op("o2"),
            put("s2", "agent:b", &["ios"], StartFrontier::Now),
            1,
        )
        .await
        .unwrap();
    store
        .announce(announce_request("payments", "k1", &["ios"], "x"), 2)
        .await
        .unwrap();
    store
        .seal_batch(&member("agent:a"), &batch_id("sealed"), &policy(), 2_000)
        .await
        .unwrap()
        .unwrap();
    store
        .announce(announce_request("payments", "k2", &["ios"], "y"), 3)
        .await
        .unwrap();
    let ended = store
        .end_member(&member("agent:a"), departed(), 4)
        .await
        .unwrap();
    assert_eq!(ended, vec![sid("s1")]);
    let s1 = store.subscription(&sid("s1")).await.unwrap().unwrap();
    assert_eq!(s1.state, SubscriptionState::Ended);
    assert_eq!(s1.ended, Some(departed()));
    assert_eq!(s1.revision, 2, "ending is a change at a new revision");
    assert_eq!(
        store
            .batch(&batch_id("sealed"))
            .await
            .unwrap()
            .unwrap()
            .state,
        BatchState::Cancelled,
        "a sealed batch nothing wants any more is cancelled"
    );
    let owing = store
        .backlogs(10)
        .await
        .unwrap()
        .into_iter()
        .map(|backlog| backlog.member)
        .collect::<Vec<_>>();
    assert_eq!(
        owing,
        vec![member("agent:b")],
        "what the ended subscription captured is settled"
    );
    let later = store
        .announce(announce_request("payments", "k3", &["ios"], "z"), 5)
        .await
        .unwrap();
    assert_eq!(
        later.captured_for,
        vec![sid("s2")],
        "an ended subscription captures nothing"
    );
    let ended_refusal = AnnouncementStoreError::SubscriptionEnded { end: departed() };
    assert_eq!(
        store
            .change_subscription(&op("o3"), &sid("s1"), 2, SubscriptionChange::Resume, 6)
            .await
            .unwrap_err(),
        ended_refusal
    );
    let mut replace = put("s1", "agent:a", &["ios"], StartFrontier::Now);
    replace.expected_revision = Some(2);
    assert_eq!(
        store
            .put_subscription(&op("o4"), replace, 6)
            .await
            .unwrap_err(),
        ended_refusal
    );
    assert!(
        store
            .end_member(&member("agent:a"), departed(), 7)
            .await
            .unwrap()
            .is_empty(),
        "nothing live is left to end"
    );
}

async fn destroyed_mobs_leave_the_subscriptions_that_name_them(store: &dyn AnnouncementStore) {
    let mut both = put("s1", "agent:a", &["ios"], StartFrontier::Now);
    both.selector.mobs = [mob("payments"), mob("builds")].into();
    store.put_subscription(&op("o1"), both, 1).await.unwrap();
    let mut only = put("s2", "agent:b", &["ios"], StartFrontier::Now);
    only.selector.mobs = [mob("builds")].into();
    store.put_subscription(&op("o2"), only, 1).await.unwrap();
    store
        .announce(announce_request("builds", "k1", &["ios"], "owed"), 2)
        .await
        .unwrap();
    assert_eq!(
        store.known_mobs().await.unwrap(),
        [mob("payments"), mob("builds")].into()
    );
    let changed = store
        .drop_ended_mobs(&[mob("builds")].into(), 3)
        .await
        .unwrap();
    assert_eq!(changed, vec![sid("s1"), sid("s2")]);
    let s1 = store.subscription(&sid("s1")).await.unwrap().unwrap();
    assert_eq!(s1.state, SubscriptionState::Active);
    assert_eq!(s1.selector.mobs, [mob("payments")].into());
    assert_eq!(s1.ended_mobs, [mob("builds")].into());
    assert_eq!(s1.revision, 2);
    assert!(!s1.frontier.contains_key(&mob("builds")));
    let s2 = store.subscription(&sid("s2")).await.unwrap().unwrap();
    assert_eq!(s2.state, SubscriptionState::Ended);
    assert_eq!(s2.ended, Some(SubscriptionEnd::MobsEnded));
    assert_eq!(s2.ended_mobs, [mob("builds")].into());
    let backlogs = store.backlogs(10).await.unwrap();
    assert_eq!(backlogs.len(), 1);
    assert_eq!(backlogs[0].member, member("agent:a"));
    assert_eq!(
        backlogs[0].eligible_count, 1,
        "what was captured from a destroyed mob stays owed"
    );
    assert_eq!(
        store.known_mobs().await.unwrap(),
        [mob("payments")].into(),
        "the destroyed mob's log is marked ended"
    );
    assert!(
        store
            .drop_ended_mobs(&[mob("builds")].into(), 4)
            .await
            .unwrap()
            .is_empty(),
        "dropping again changes nothing"
    );
    let again = store
        .announce(announce_request("builds", "k2", &["ios"], "a new mob"), 5)
        .await
        .unwrap();
    assert!(
        again.captured_for.is_empty(),
        "a mob created later under the same id is another mob"
    );
    assert_eq!(
        store.known_mobs().await.unwrap(),
        [mob("payments"), mob("builds")].into(),
        "announcing into a hosted mob opens its log again"
    );
}

async fn retention_removes_what_ended_subscriptions_and_destroyed_mobs_leave(
    store: &dyn AnnouncementStore,
) {
    let mut only = put("s1", "agent:a", &["ios"], StartFrontier::Now);
    only.selector.mobs = [mob("builds")].into();
    store.put_subscription(&op("o1"), only, 1).await.unwrap();
    store
        .announce(announce_request("builds", "k1", &["ios"], "x"), 2)
        .await
        .unwrap();
    store
        .drop_ended_mobs(&[mob("builds")].into(), 3)
        .await
        .unwrap();
    let early = store.prune(4, 1_000, 100).await.unwrap();
    assert_eq!(
        (early.announcements, early.subscriptions),
        (0, 0),
        "nothing goes before the window has passed"
    );
    assert!(store.subscription(&sid("s1")).await.unwrap().is_some());
    let report = store.prune(1_000_000, 0, 100).await.unwrap();
    assert_eq!(report.announcements, 1);
    assert_eq!(report.subscriptions, 1);
    assert!(!report.more);
    assert!(store.subscription(&sid("s1")).await.unwrap().is_none());
    assert!(
        store.known_mobs().await.unwrap().is_empty(),
        "the destroyed mob's log is removed once it retains nothing"
    );
    let fresh = store
        .announce(announce_request("builds", "k1", &["ios"], "y"), 5)
        .await
        .unwrap();
    assert_eq!(
        (fresh.announcement.sequence, fresh.replayed),
        (1, false),
        "a mob created later under the same id starts a log of its own"
    );
}

async fn retention_reads_only_mobs_with_old_announcements(store: &dyn AnnouncementStore) {
    // Fifty quiet mobs with recent announcements, and one with a backlog of
    // old ones that takes two passes.
    for index in 0..50 {
        store
            .announce(
                announce_request(&format!("quiet-{index:02}"), "k", &["ios"], "recent"),
                900_000,
            )
            .await
            .unwrap();
    }
    for index in 0..5_u64 {
        store
            .announce(
                announce_request("busy", &format!("k{index}"), &["ios"], "old"),
                10 + index,
            )
            .await
            .unwrap();
    }
    let first = store.prune(1_000_000, 500_000, 3).await.unwrap();
    assert_eq!((first.announcements, first.more), (3, true));
    let second = store.prune(1_000_000, 500_000, 3).await.unwrap();
    assert_eq!((second.announcements, second.more), (2, false));
    for index in 0..50 {
        assert!(
            store
                .announcement(
                    &mob(&format!("quiet-{index:02}")),
                    &SourceKey::new("k").unwrap()
                )
                .await
                .unwrap()
                .is_some(),
            "a quiet mob keeps its history"
        );
    }
    let mut at_head = put(
        "s1",
        "agent:a",
        &["ios"],
        StartFrontier::After {
            frontier: [(mob("busy"), 5)].into(),
        },
    );
    at_head.selector.mobs = [mob("busy"), mob("quiet-07")].into();
    store
        .put_subscription(&op("o1"), at_head, 2_000_000)
        .await
        .unwrap();
    let mut behind = put(
        "s2",
        "agent:a",
        &["ios"],
        StartFrontier::After {
            frontier: [(mob("busy"), 4)].into(),
        },
    );
    behind.selector.mobs = [mob("busy")].into();
    assert_eq!(
        store.put_subscription(&op("o2"), behind, 2_000_000).await,
        Err(AnnouncementStoreError::Gap {
            mob_id: mob("busy"),
            earliest_retained: 6,
        }),
        "the busy mob's earliest retained sequence moved past what was pruned"
    );
    let mut quiet = put(
        "s3",
        "agent:a",
        &["ios"],
        StartFrontier::After {
            frontier: [(mob("quiet-07"), 0)].into(),
        },
    );
    quiet.selector.mobs = [mob("quiet-07")].into();
    store
        .put_subscription(&op("o3"), quiet, 2_000_000)
        .await
        .expect("a quiet mob still retains its first announcement");
}

/// Which store a conformance case runs against.
#[derive(Debug, Clone, Copy)]
enum ConformanceBackend {
    /// A database file, as a state directory keeps it.
    Disk,
    /// A host store realm's in-memory copy. Once the case ran, a new process
    /// opens the realm again and must find every table as the case left it.
    HostStore,
}

async fn run_conformance<F, Fut>(backend: ConformanceBackend, name: &str, case: F)
where
    F: Fn(Arc<dyn AnnouncementStore>) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    eprintln!("announcement store conformance ({backend:?}): {name}");
    match backend {
        ConformanceBackend::Disk => {
            let (_dir, store) = temp_store();
            case(Arc::new(store)).await;
        }
        ConformanceBackend::HostStore => {
            let host = Arc::new(meerkat_store_conformance::InMemoryHostStore::new());
            let process = super::host_store_tests::Process::open(&host).await;
            case(Arc::new(process.store.clone())).await;
            super::host_store_tests::assert_restart_keeps_every_table(process, name).await;
        }
    }
}

#[tokio::test]
async fn sqlite_store_conforms() {
    conformance_suite(ConformanceBackend::Disk).await;
}

/// The store conformance cases over a host store realm's copy of the
/// tables: each case on a fresh realm, then a restart as a new process that
/// finds every table, row ids and AUTOINCREMENT marks included, as the case
/// left it.
#[tokio::test]
async fn the_host_store_copy_conforms_and_a_restart_finds_every_table() {
    conformance_suite(ConformanceBackend::HostStore).await;
}

async fn conformance_suite(backend: ConformanceBackend) {
    run_conformance(backend, "announcements", |store| async move {
        announcements_are_sequenced_per_mob_and_bound_to_content(store.as_ref()).await;
    })
    .await;
    run_conformance(backend, "capture", |store| async move {
        capture_commits_with_the_announcement(store.as_ref()).await;
    })
    .await;
    run_conformance(backend, "frontiers", |store| async move {
        explicit_frontiers_replay_or_report_a_gap(store.as_ref()).await;
    })
    .await;
    run_conformance(backend, "frontiers outside a mob", |store| async move {
        frontiers_outside_a_mob_are_refused(store.as_ref()).await;
    })
    .await;
    run_conformance(backend, "revisions", |store| async move {
        subscription_changes_are_revisioned_and_idempotent(store.as_ref()).await;
    })
    .await;
    run_conformance(backend, "pause", |store| async move {
        paused_subscriptions_capture_and_hold(store.as_ref()).await;
    })
    .await;
    run_conformance(backend, "sealing", |store| async move {
        sealing_binds_exact_members_once(store.as_ref()).await;
    })
    .await;
    run_conformance(backend, "transitions", |store| async move {
        transitions_settle_owed_announcements(store.as_ref()).await;
    })
    .await;
    run_conformance(backend, "stranded", |store| async move {
        stranded_batches_hand_over_to_a_successor(store.as_ref()).await;
    })
    .await;
    run_conformance(backend, "cancellation", |store| async move {
        cancellation_settles_what_was_never_dispatched(store.as_ref()).await;
    })
    .await;
    run_conformance(backend, "retention", |store| async move {
        retention_keeps_a_contiguous_history(store.as_ref()).await;
    })
    .await;
    run_conformance(backend, "wire budget", |store| async move {
        remote_batches_fit_one_delivery_request(store.as_ref()).await;
    })
    .await;
    run_conformance(backend, "item size", |store| async move {
        oversized_renderings_are_refused(store.as_ref()).await;
    })
    .await;
    run_conformance(backend, "unsettled walk", |store| async move {
        the_unsettled_walk_resumes_after_a_position(store.as_ref()).await;
    })
    .await;
    run_conformance(backend, "retention order", |store| async move {
        retention_prunes_the_oldest_first_across_mobs(store.as_ref()).await;
    })
    .await;
    run_conformance(backend, "retention budgets", |store| async move {
        retention_passes_report_when_more_work_is_left(store.as_ref()).await;
    })
    .await;
    run_conformance(backend, "lifecycle", |store| async move {
        lifecycle_ends_subscriptions_on_final_facts(store.as_ref()).await;
    })
    .await;
    run_conformance(backend, "destroyed mobs", |store| async move {
        destroyed_mobs_leave_the_subscriptions_that_name_them(store.as_ref()).await;
    })
    .await;
    run_conformance(backend, "retention of ended rows", |store| async move {
        retention_removes_what_ended_subscriptions_and_destroyed_mobs_leave(store.as_ref()).await;
    })
    .await;
    run_conformance(backend, "retention reads", |store| async move {
        retention_reads_only_mobs_with_old_announcements(store.as_ref()).await;
    })
    .await;
    run_conformance(backend, "lineage", |store| async move {
        lineage_is_recorded_once_per_incarnation(store.as_ref()).await;
    })
    .await;
    run_conformance(backend, "expiry", |store| async move {
        expiry_settles_what_waited_past_the_window(store.as_ref()).await;
    })
    .await;
    run_conformance(
        backend,
        "retention of native-held bodies",
        |store| async move {
            retention_drops_bodies_a_native_held_batch_carries(store.as_ref()).await;
        },
    )
    .await;
    run_conformance(backend, "member routes", |store| async move {
        member_routes_are_kept_and_member_mob_logs_read_on(store.as_ref()).await;
    })
    .await;
    run_conformance(
        backend,
        "gap lines within a peer request",
        |store| async move {
            gap_lines_fit_what_the_items_leave_of_a_peer_request(store.as_ref()).await;
        },
    )
    .await;
    run_conformance(
        backend,
        "gap lines of a batch that reports nothing",
        |store| async move {
            gaps_of_a_batch_that_reports_nothing_go_back(store.as_ref()).await;
        },
    )
    .await;
    run_conformance(backend, "child member limits", |store| async move {
        child_member_subscriptions_are_bounded(store.as_ref()).await;
    })
    .await;
    run_conformance(
        backend,
        "gap lines a successor defers",
        |store| async move {
            gap_lines_a_successor_defers_reach_the_next_batch(store.as_ref()).await;
        },
    )
    .await;
    run_conformance(backend, "unconfirmed batches", |store| async move {
        unconfirmed_batches_settle_what_they_held_and_keep_a_late_outcome(store.as_ref()).await;
    })
    .await;
}

/// A batch whose delivery was not confirmed in time settles what it held,
/// as not confirmed, and hands that and its own gap lines to the member's
/// next batch. Its late reads stay due until one names no next read, and
/// what a late read finds is kept on it without moving it.
async fn unconfirmed_batches_settle_what_they_held_and_keep_a_late_outcome(
    store: &dyn AnnouncementStore,
) {
    store
        .put_subscription(
            &op("o1"),
            put("s1", "agent:a", &["ios"], StartFrontier::Now),
            1,
        )
        .await
        .unwrap();
    let agent = member("agent:a");
    store
        .announce(announce_request("payments", "k1", &["ios"], "x"), 2)
        .await
        .unwrap();
    store
        .seal_batch(&agent, &batch_id("b1"), &policy(), 2_000)
        .await
        .unwrap()
        .unwrap();
    let b1 = batch_id("b1");
    store
        .transition_batch(&b1, BatchStateTag::Sealed, BatchEvent::BeginSubmit, 3)
        .await
        .unwrap();
    let unverified = store
        .transition_batch(
            &b1,
            BatchStateTag::Submitting,
            BatchEvent::ExpireUnverified,
            4,
        )
        .await
        .unwrap();
    assert_eq!(
        unverified.state,
        BatchState::ExpiredUnverified { late: None }
    );
    assert!(store.unsettled_batches(None, 10).await.unwrap().is_empty());
    assert_eq!(
        store
            .late_reads_due(4, 10)
            .await
            .unwrap()
            .into_iter()
            .map(|batch| batch.id)
            .collect::<Vec<_>>(),
        vec![b1.clone()]
    );
    assert!(store.late_reads_due(3, 10).await.unwrap().is_empty());

    store
        .announce(announce_request("payments", "k2", &["ios"], "y"), 5)
        .await
        .unwrap();
    let b2 = store
        .seal_batch(&agent, &batch_id("b2"), &policy(), 4_000)
        .await
        .unwrap()
        .expect("the member's next batch seals");
    assert_eq!(
        b2.gaps,
        vec![BatchGap {
            mob_id: mob("payments"),
            count: 1,
            reason: GapReason::DeliveryNotConfirmed,
        }]
    );
    assert!(
        b2.body.contains("neither confirmed nor ruled out"),
        "{}",
        b2.body
    );
    store
        .transition_batch(&b2.id, BatchStateTag::Sealed, BatchEvent::BeginSubmit, 6)
        .await
        .unwrap();
    store
        .transition_batch(
            &b2.id,
            BatchStateTag::Submitting,
            BatchEvent::NativeCommitted {
                receipt: receipt("b2"),
            },
            7,
        )
        .await
        .unwrap();
    let unconfirmed = store
        .transition_batch(
            &b2.id,
            BatchStateTag::Committed,
            BatchEvent::ExpireUnconfirmed,
            8,
        )
        .await
        .unwrap();
    assert_eq!(
        unconfirmed.state,
        BatchState::CommittedUnconfirmed {
            receipt: receipt("b2"),
            late: None,
        },
        "the receipt is kept"
    );

    let applied = LateOutcome {
        status: NativeStatus::Applied {
            receipt: receipt("b1"),
            session_id: "session".into(),
            input_id: "input".into(),
        },
        at_ms: 9,
    };
    let kept = store
        .record_late_read(&b1, Some(applied.clone()), None)
        .await
        .unwrap();
    assert_eq!(
        kept.state,
        BatchState::ExpiredUnverified {
            late: Some(applied.clone())
        }
    );
    assert_eq!(kept.updated_at_ms, 4, "a late read moves nothing");
    assert_eq!(store.batch(&b1).await.unwrap(), Some(kept));
    store
        .record_late_read(&b2.id, None, Some(20))
        .await
        .unwrap();
    assert!(
        store.late_reads_due(19, 10).await.unwrap().is_empty(),
        "a read that names no next one ends the reads; the next is due when named"
    );
    assert_eq!(store.late_reads_due(20, 10).await.unwrap().len(), 1);

    store
        .announce(announce_request("payments", "k3", &["ios"], "z"), 10)
        .await
        .unwrap();
    let b3 = store
        .seal_batch(&agent, &batch_id("b3"), &policy(), 6_000)
        .await
        .unwrap()
        .expect("the member's next batch seals");
    assert_eq!(
        b3.gaps,
        vec![
            BatchGap {
                mob_id: mob("payments"),
                count: 1,
                reason: GapReason::AdmissionNotConfirmed,
            },
            BatchGap {
                mob_id: mob("payments"),
                count: 1,
                reason: GapReason::DeliveryNotConfirmed,
            },
        ],
        "what b2 held, and the gap line it carried, which may not have been read"
    );
    assert!(matches!(
        store.record_late_read(&b3.id, Some(applied), None).await,
        Err(AnnouncementStoreError::Invalid(_))
    ));
    assert!(
        matches!(
            store.record_late_read(&b3.id, None, Some(30)).await,
            Err(AnnouncementStoreError::Invalid(_))
        ),
        "no late read is scheduled for a batch that keeps no late outcome"
    );
    assert!(
        store
            .late_reads_due(u64::from(u32::MAX), 10)
            .await
            .unwrap()
            .iter()
            .all(|batch| batch.id != b3.id)
    );
}

async fn gap_lines_a_successor_defers_reach_the_next_batch(store: &dyn AnnouncementStore) {
    store
        .put_subscription(
            &op("s1"),
            put("s1", "agent:a", &["ios"], StartFrontier::Now),
            1_000,
        )
        .await
        .unwrap();
    store
        .announce(announce_request("payments", "k1", &["ios"], "x"), 1_000)
        .await
        .unwrap();
    store
        .seal_batch(&member("agent:a"), &batch_id("b1"), &policy(), 2_000)
        .await
        .unwrap()
        .unwrap();
    let deliver = |id: &'static str| async move {
        for (expected, event) in [
            (BatchStateTag::Sealed, BatchEvent::BeginSubmit),
            (
                BatchStateTag::Submitting,
                BatchEvent::NativeCommitted {
                    receipt: receipt(id),
                },
            ),
        ] {
            store
                .transition_batch(&batch_id(id), expected, event, 3_000)
                .await
                .unwrap();
        }
    };
    deliver("b1").await;
    let deferred = vec![expired_gap(3)];
    store
        .transition_batch(
            &batch_id("b1"),
            BatchStateTag::Committed,
            BatchEvent::NativeStranded {
                receipt: receipt("b1"),
                successor: Some(SuccessorBatch {
                    id: batch_id("b2"),
                    body: "redelivery".into(),
                    gaps: Vec::new(),
                    deferred: deferred.clone(),
                }),
            },
            4_000,
        )
        .await
        .unwrap();
    deliver("b2").await;
    store
        .transition_batch(
            &batch_id("b2"),
            BatchStateTag::Committed,
            BatchEvent::NativeApplied {
                receipt: receipt("b2"),
                session_id: "session".into(),
                input_id: "input".into(),
            },
            5_000,
        )
        .await
        .unwrap();
    store
        .announce(announce_request("payments", "k2", &["ios"], "y"), 6_000)
        .await
        .unwrap();
    let next = store
        .seal_batch(&member("agent:a"), &batch_id("b3"), &policy(), 7_000)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(next.gaps, deferred);
}

async fn gap_lines_fit_what_the_items_leave_of_a_peer_request(store: &dyn AnnouncementStore) {
    let peer = peer_member("agent:p", "mob-b");
    // Ids near the longest a mob id may be, so each gap line is long: 150
    // of them make more than a peer request holds next to a large item.
    let long = |index: usize| mob(&format!("{}-{index:03}", "m".repeat(240)));
    let lines = 150_usize;
    let indexes = (0..lines).collect::<Vec<_>>();
    // A selector names at most 64 mobs: three subscriptions name the long
    // mobs, and one the mob the later announcements go to.
    for (part, chunk) in indexes.chunks(50).enumerate() {
        let id = format!("p{part}");
        store
            .put_subscription(
                &op(&id),
                PutSubscription {
                    selector: Selector {
                        topics: topics(&["ios"]),
                        mobs: chunk.iter().map(|index| long(*index)).collect(),
                    },
                    ..put_for(&id, peer.clone(), &["ios"])
                },
                1_000,
            )
            .await
            .unwrap();
    }
    store
        .put_subscription(
            &op("p-payments"),
            PutSubscription {
                selector: Selector {
                    topics: topics(&["ios"]),
                    mobs: BTreeSet::from([mob("payments")]),
                },
                ..put_for("p-payments", peer.clone(), &["ios"])
            },
            1_000,
        )
        .await
        .unwrap();
    for index in 0..lines {
        store
            .announce(
                announce_request(long(index).as_str(), "k", &["ios"], "old"),
                1_000,
            )
            .await
            .unwrap();
    }
    let report = store
        .expire(1_000 + WINDOW, WINDOW, 0, 1_000)
        .await
        .unwrap();
    assert_eq!(report.captures, 150);
    // One announcement as large as a body may be: what it leaves of the
    // request holds some of the gap lines, not all.
    let now = 1_000 + WINDOW + 10;
    store
        .announce(
            announce_request("payments", "big", &["ios"], &"x".repeat(MAX_BODY_BYTES)),
            now,
        )
        .await
        .unwrap();
    let first = store
        .seal_batch(&peer, &batch_id("b1"), &policy(), now + 1_000)
        .await
        .unwrap()
        .unwrap();
    assert!(wire_len(&first.body) <= MAX_REMOTE_BODY_WIRE_BYTES);
    let reported = first.gaps.iter().map(|gap| gap.count).sum::<u64>();
    assert!(reported > 0 && reported < 150, "{reported}");
    // Once it is delivered, the rest ride on the member's next batch.
    for (expected, event) in [
        (BatchStateTag::Sealed, BatchEvent::BeginSubmit),
        (
            BatchStateTag::Submitting,
            BatchEvent::NativeCommitted {
                receipt: receipt("b1"),
            },
        ),
        (
            BatchStateTag::Committed,
            BatchEvent::NativeApplied {
                receipt: receipt("b1"),
                session_id: "session".into(),
                input_id: "input".into(),
            },
        ),
    ] {
        store
            .transition_batch(&batch_id("b1"), expected, event, now + 1_500)
            .await
            .unwrap();
    }
    store
        .announce(
            announce_request("payments", "small", &["ios"], "x"),
            now + 2_000,
        )
        .await
        .unwrap();
    let second = store
        .seal_batch(&peer, &batch_id("b2"), &policy(), now + 5_000)
        .await
        .unwrap()
        .unwrap();
    assert!(wire_len(&second.body) <= MAX_REMOTE_BODY_WIRE_BYTES);
    let rest = second.gaps.iter().map(|gap| gap.count).sum::<u64>();
    assert_eq!(reported + rest, 150);
}

async fn gaps_of_a_batch_that_reports_nothing_go_back(store: &dyn AnnouncementStore) {
    let subscribe = |id: &str, mob_id: &str| PutSubscription {
        selector: Selector {
            topics: topics(&["ios"]),
            mobs: BTreeSet::from([mob(mob_id)]),
        },
        ..put(id, "agent:a", &["ios"], StartFrontier::Now)
    };
    store
        .put_subscription(&op("sa"), subscribe("sa", "payments"), 1_000)
        .await
        .unwrap();
    store
        .put_subscription(&op("sb"), subscribe("sb", "events"), 1_000)
        .await
        .unwrap();
    store
        .announce(announce_request("payments", "old", &["ios"], "old"), 1_000)
        .await
        .unwrap();
    store.expire(1_000 + WINDOW, WINDOW, 0, 100).await.unwrap();
    let now = 1_000 + WINDOW + 10;
    store
        .announce(announce_request("events", "k1", &["ios"], "x"), now)
        .await
        .unwrap();
    let carrier = store
        .seal_batch(&member("agent:a"), &batch_id("b1"), &policy(), now + 1_000)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(carrier.gaps, vec![expired_gap(1)]);
    // The one subscription the batch holds items for is cancelled before it
    // is submitted: the batch is cancelled and its gap line goes back.
    store
        .change_subscription(
            &op("cb"),
            &sid("sb"),
            1,
            SubscriptionChange::Cancel,
            now + 2_000,
        )
        .await
        .unwrap();
    assert_eq!(
        store.batch(&batch_id("b1")).await.unwrap().unwrap().state,
        BatchState::Cancelled
    );
    store
        .announce(
            announce_request("payments", "k2", &["ios"], "y"),
            now + 3_000,
        )
        .await
        .unwrap();
    let next = store
        .seal_batch(&member("agent:a"), &batch_id("b2"), &policy(), now + 5_000)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(next.gaps, vec![expired_gap(1)]);
    // A batch the native owner refuses hands its gap line on as well.
    store
        .transition_batch(
            &batch_id("b2"),
            BatchStateTag::Sealed,
            BatchEvent::BeginSubmit,
            now + 6_000,
        )
        .await
        .unwrap();
    store
        .transition_batch(
            &batch_id("b2"),
            BatchStateTag::Submitting,
            BatchEvent::NativeRefused {
                reason: "refused".into(),
                receipt: None,
            },
            now + 7_000,
        )
        .await
        .unwrap();
    store
        .announce(
            announce_request("payments", "k3", &["ios"], "z"),
            now + 8_000,
        )
        .await
        .unwrap();
    let after_refusal = store
        .seal_batch(&member("agent:a"), &batch_id("b3"), &policy(), now + 10_000)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after_refusal.gaps, vec![expired_gap(1)]);
}

async fn child_member_subscriptions_are_bounded(store: &dyn AnnouncementStore) {
    let limits = ChildMemberLimits {
        subscriptions: 3,
        per_mob: 2,
    };
    let child_put = |id: &str, identity: &str, mobs: &[&str]| PutSubscription {
        member: peer_member(identity, "builds"),
        route: Some(MemberRouteRecord {
            route: MemberRoute::Child,
            incarnation: Some(incarnation("owner-1", 10)),
        }),
        member_mob_cursor: Some(1),
        child_limits: limits,
        selector: Selector {
            topics: topics(&["ios"]),
            mobs: mobs.iter().map(|name| mob(name)).collect(),
        },
        ..put(id, "agent:a", &["ios"], StartFrontier::Now)
    };
    store
        .put_subscription(&op("c1"), child_put("c1", "w1", &["payments"]), 1)
        .await
        .unwrap();
    store
        .put_subscription(&op("c2"), child_put("c2", "w2", &["payments"]), 1)
        .await
        .unwrap();
    // A third naming the same mob would make one announcement into it owed
    // to three of them.
    assert_eq!(
        store
            .put_subscription(&op("c3"), child_put("c3", "w3", &["payments"]), 1)
            .await
            .unwrap_err(),
        AnnouncementStoreError::ChildMemberLimit {
            limit: 2,
            mob_id: Some(mob("payments")),
        }
    );
    assert!(store.subscription(&sid("c3")).await.unwrap().is_none());
    // Replacing one does not count it against itself.
    let replaced = store
        .put_subscription(
            &op("c1-r"),
            PutSubscription {
                expected_revision: Some(1),
                ..child_put("c1", "w1", &["payments", "events"])
            },
            2,
        )
        .await
        .unwrap();
    assert_eq!(replaced.revision, 2);
    store
        .put_subscription(&op("c4"), child_put("c4", "w4", &["events"]), 3)
        .await
        .unwrap();
    // The gateway's bound counts every live one.
    assert_eq!(
        store
            .put_subscription(&op("c5"), child_put("c5", "w5", &["refunds"]), 4)
            .await
            .unwrap_err(),
        AnnouncementStoreError::ChildMemberLimit {
            limit: 3,
            mob_id: None,
        }
    );
    // An ended or cancelled one no longer counts; nor does a member of
    // this gateway's own mob.
    store
        .change_subscription(&op("c2-x"), &sid("c2"), 1, SubscriptionChange::Cancel, 5)
        .await
        .unwrap();
    store
        .put_subscription(&op("c5-b"), child_put("c5", "w5", &["refunds"]), 6)
        .await
        .unwrap();
    store
        .put_subscription(
            &op("own"),
            PutSubscription {
                child_limits: limits,
                ..put("own", "agent:a", &["ios"], StartFrontier::Now)
            },
            7,
        )
        .await
        .unwrap();
}

fn worker() -> Member {
    peer_member("worker-1", "builds")
}

async fn member_routes_are_kept_and_member_mob_logs_read_on(store: &dyn AnnouncementStore) {
    let child = MemberRouteRecord {
        route: MemberRoute::Child,
        incarnation: Some(incarnation("owner-1", 10)),
    };
    let child_put = |id: &str, cursor: u64| PutSubscription {
        member: worker(),
        route: Some(child.clone()),
        member_mob_cursor: Some(cursor),
        ..put(id, "agent:a", &["ios"], StartFrontier::Now)
    };
    store
        .put_subscription(&op("c1"), child_put("c1", 5), 1)
        .await
        .unwrap();
    assert_eq!(
        store.member_route(&worker()).await.unwrap(),
        Some(child.clone())
    );
    // A member is never rerouted, and nothing is recorded for the refusal.
    assert!(matches!(
        store
            .put_subscription(&op("c2"), put_for("c2", worker(), &["ios"]), 2)
            .await,
        Err(AnnouncementStoreError::RouteKept {
            kept: MemberRoute::Child
        })
    ));
    assert!(store.subscription(&sid("c2")).await.unwrap().is_none());
    store
        .put_subscription(&op("c3"), child_put("c3", 9), 3)
        .await
        .unwrap();
    assert_eq!(
        store.child_member_routes().await.unwrap(),
        vec![(worker(), child.clone())]
    );
    // The log is read on from the earliest live cursor, then from where
    // the last read ended; never back.
    assert_eq!(
        store.member_mob_check_from(&worker()).await.unwrap(),
        Some(5)
    );
    store.record_member_mob_check(&worker(), 7).await.unwrap();
    assert_eq!(
        store.member_mob_check_from(&worker()).await.unwrap(),
        Some(7)
    );
    store.record_member_mob_check(&worker(), 6).await.unwrap();
    assert_eq!(
        store.member_mob_check_from(&worker()).await.unwrap(),
        Some(7)
    );
    // An end at cursor 8 ends only what was made before it.
    let completed = SubscriptionEnd::MemberMobEnded {
        why: "completed".into(),
    };
    assert_eq!(
        store
            .end_member_before(&worker(), 8, completed.clone(), 4)
            .await
            .unwrap(),
        vec![sid("c1")]
    );
    let ended = store.subscription(&sid("c1")).await.unwrap().unwrap();
    assert_eq!(ended.ended, Some(completed));
    assert_eq!(
        store.subscription(&sid("c3")).await.unwrap().unwrap().state,
        SubscriptionState::Active
    );
    assert_eq!(
        store.member_mob_check_from(&worker()).await.unwrap(),
        Some(9),
        "the earliest live subscription was made after the last read"
    );
    // Unbinding ends the rest and drops the route: another can be kept.
    assert_eq!(
        store
            .unbind_member_route(
                &worker(),
                SubscriptionEnd::MemberMobEnded {
                    why: "destroyed".into()
                },
                5
            )
            .await
            .unwrap(),
        vec![sid("c3")]
    );
    assert_eq!(store.member_route(&worker()).await.unwrap(), None);
    assert!(store.child_member_routes().await.unwrap().is_empty());
    assert_eq!(store.member_mob_check_from(&worker()).await.unwrap(), None);
    store
        .put_subscription(&op("c4"), put_for("c4", worker(), &["ios"]), 6)
        .await
        .unwrap();
    // A batch carries the route its member is kept on.
    store
        .announce(announce_request("payments", "k1", &["ios"], "x"), 7)
        .await
        .unwrap();
    let batch = store
        .seal_batch(&worker(), &batch_id("b1"), &policy(), 2_000)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(batch.route, MemberRoute::Peer);
}

fn incarnation(owner: &str, first_event_at_ms: u64) -> MobIncarnation {
    MobIncarnation {
        owner_session: Some(owner.to_string()),
        first_event_at_ms,
    }
}

fn lineage_record(
    mob_id: &str,
    parent: Option<&str>,
    incarnation: MobIncarnation,
    at: u64,
) -> MobLineage {
    MobLineage {
        mob_id: mob(mob_id),
        incarnation,
        parent: parent.map(mob),
        parent_incarnation: None,
        creator: parent.map(|_| "creator".to_string()),
        recorded_at_ms: at,
    }
}

async fn lineage_is_recorded_once_per_incarnation(store: &dyn AnnouncementStore) {
    let first = lineage_record("events", Some("payments"), incarnation("s1", 10), 100);
    assert!(store.record_lineage(&first, 100).await.unwrap());
    // Derived once: the same incarnation keeps its first record.
    let again = lineage_record("events", Some("builds"), incarnation("s1", 10), 200);
    assert!(!store.record_lineage(&again, 200).await.unwrap());
    assert_eq!(store.lineage().await.unwrap(), vec![first.clone()]);
    // Another incarnation (same id, created again) replaces it.
    let second = lineage_record("events", Some("refunds"), incarnation("s1", 20), 300);
    assert!(store.record_lineage(&second, 300).await.unwrap());
    assert_eq!(store.lineage().await.unwrap(), vec![second.clone()]);
    let child = lineage_record("builds", Some("events"), incarnation("s2", 30), 300);
    let unrelated = lineage_record("refunds", None, incarnation("s3", 40), 300);
    store.record_lineage(&child, 300).await.unwrap();
    store.record_lineage(&unrelated, 300).await.unwrap();
    store
        .end_lineage(&BTreeSet::from([mob("events"), mob("refunds")]), 400)
        .await
        .unwrap();
    assert_eq!(
        store.lineage().await.unwrap(),
        vec![child.clone()],
        "ended records are not live"
    );
    // Retention removes ended records before the window, except the
    // records of a mob a live record still names as parent.
    let report = store.prune(10_000, 1_000, 100).await.unwrap();
    assert_eq!(
        report.lineage, 1,
        "refunds; events is still named by builds"
    );
    assert_eq!(store.lineage().await.unwrap(), vec![child.clone()]);
    store
        .end_lineage(&BTreeSet::from([mob("builds")]), 500)
        .await
        .unwrap();
    let report = store.prune(10_000, 1_000, 100).await.unwrap();
    assert_eq!(report.lineage, 3, "both events records and builds");
    assert!(store.lineage().await.unwrap().is_empty());
    let report = store.prune(10_000, 1_000, 100).await.unwrap();
    assert_eq!(report.lineage, 0);
}

#[tokio::test]
async fn the_sqlite_store_survives_reopening() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mob_announcements.sqlite3");
    {
        let store = SqliteAnnouncementStore::open(&path).unwrap();
        store
            .put_subscription(
                &op("o1"),
                put("s1", "agent:a", &["ios"], StartFrontier::Now),
                1,
            )
            .await
            .unwrap();
        store
            .announce(announce_request("payments", "k1", &["ios"], "x"), 2)
            .await
            .unwrap();
        store
            .seal_batch(&member("agent:a"), &batch_id("b1"), &policy(), 2_000)
            .await
            .unwrap()
            .unwrap();
        store
            .transition_batch(
                &batch_id("b1"),
                BatchStateTag::Sealed,
                BatchEvent::BeginSubmit,
                3,
            )
            .await
            .unwrap();
    }
    let reopened = SqliteAnnouncementStore::open(&path).unwrap();
    let unsettled = reopened.unsettled_batches(None, 10).await.unwrap();
    assert_eq!(unsettled.len(), 1);
    assert_eq!(unsettled[0].state, BatchState::Submitting { attempt: 1 });
    assert_eq!(
        reopened
            .put_subscription(
                &op("o1"),
                put("s1", "agent:a", &["ios"], StartFrontier::Now),
                9
            )
            .await
            .unwrap()
            .revision,
        1,
        "operation receipts survive the restart"
    );
}

#[tokio::test]
async fn the_ledgered_disk_store_opens_twice() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mob_announcements.sqlite3");
    {
        let store = super::storage::open_disk_store(path.clone()).unwrap();
        store
            .announce(announce_request("payments", "k1", &["ios"], "x"), 1)
            .await
            .unwrap();
    }
    let store = super::storage::open_disk_store(path).unwrap();
    assert!(
        store
            .announcement(&mob("payments"), &SourceKey::new("k1").unwrap())
            .await
            .unwrap()
            .is_some()
    );
}

// ---------------------------------------------------------------------------
// Owner behaviour against a scripted native owner
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Ledger {
    /// key -> (exact delivery, receipt, applied)
    bindings: BTreeMap<String, (String, NativeReceipt, bool)>,
    submits: u64,
    lose_next_ack: bool,
    refuse_owner_retired: bool,
    /// Every submission meets no live incarnation until this is cleared.
    retired: bool,
    /// Every submission is answered that nothing was committed, for this
    /// cause and on this evidence, until this is cleared.
    refuse: Option<(SuspendCause, EvidenceClass)>,
    /// Every status read gives this error until it is cleared.
    status_error: Option<NativeStatusError>,
    strand: BTreeSet<String>,
    /// Native status reads, for pacing checks.
    status_reads: u64,
    /// Native input reads.
    input_reads: u64,
    /// Every submission brings no answer that can be verified, and commits
    /// nothing: the peer predates keyed inject, or its answers are lost.
    unanswered: bool,
}

pub(super) struct ScriptedNative {
    ledger: Mutex<Ledger>,
    /// The mob this gateway names as its own, if any.
    own: Option<MobId>,
    pub(super) governed: bool,
    pub(super) known: BTreeSet<String>,
    /// Registered identities this gateway's runtime does not serve.
    unusable: BTreeSet<String>,
    /// Peers this gateway can call authenticated.
    peers: BTreeSet<String>,
    /// Mobs the mob owner records as destroyed.
    destroyed: Mutex<BTreeSet<String>>,
    /// Members that cannot take a delivery now, and why.
    pub(super) waiting: Mutex<BTreeMap<String, SuspendCause>>,
    /// Identities a member preparation was asked for, in order.
    prepared: Mutex<Vec<String>>,
    /// The next submission's final answer that its member departed.
    submit_gone: Mutex<Option<MemberDeparture>>,
    apply_on_commit: bool,
    /// What Meerkat's records say about the mobs members created, by mob.
    origins: Mutex<BTreeMap<String, MobOriginFacts>>,
    /// Mobs whose origin cannot be read now.
    unreadable_origins: Mutex<BTreeSet<String>>,
    /// Whether every hosted mob has been restored.
    restored: std::sync::atomic::AtomicBool,
    /// Members on the rosters of mobs members created, as `(mob, identity)`.
    child_roster: Mutex<BTreeSet<(String, String)>>,
    /// Mobs members created that are held until their owner resolves.
    held: Mutex<BTreeSet<String>>,
    /// Mobs members created whose state reads completed now.
    completed: Mutex<BTreeSet<String>>,
    /// The incarnation of each mob members created, when not the first.
    incarnations: Mutex<BTreeMap<String, MobIncarnation>>,
    /// Each mob's event log: `(cursor, kind)`, kinds `completed`, `reset` or
    /// anything else.
    mob_events: Mutex<BTreeMap<String, Vec<(u64, &'static str)>>>,
    /// The cursors member mob logs were read after, in order.
    log_reads: Mutex<Vec<u64>>,
    /// What each member's roster records: its role and its labels. A member
    /// missing here has no roster facts.
    roster_facts: Mutex<BTreeMap<String, RosterRow>>,
    /// Members whose roster facts cannot be read now.
    unreadable_facts: Mutex<BTreeSet<String>>,
    /// How an input reads by its id, before anything below.
    input_ids: Mutex<BTreeMap<String, serde_json::Value>>,
    /// How each batch's admitted input reads, by batch id; a batch missing
    /// here reads as `input_default`, or as consumed with no receipt.
    inputs: Mutex<BTreeMap<String, serde_json::Value>>,
    pub(super) input_default: Mutex<Option<serde_json::Value>>,
    /// How many times the members of a role were read.
    role_reads: AtomicU64,
}

/// A member's role and labels, as a scripted roster records them.
type RosterRow = (String, BTreeMap<String, String>);

impl ScriptedNative {
    pub(super) fn new() -> Self {
        Self {
            ledger: Mutex::new(Ledger::default()),
            own: Some(mob(OWN_MOB)),
            governed: false,
            known: ["agent:a".to_string()].into(),
            unusable: BTreeSet::new(),
            peers: ["mob-b".to_string()].into(),
            destroyed: Mutex::new(BTreeSet::new()),
            waiting: Mutex::new(BTreeMap::new()),
            prepared: Mutex::new(Vec::new()),
            submit_gone: Mutex::new(None),
            apply_on_commit: true,
            origins: Mutex::new(BTreeMap::new()),
            unreadable_origins: Mutex::new(BTreeSet::new()),
            restored: std::sync::atomic::AtomicBool::new(true),
            child_roster: Mutex::new(BTreeSet::new()),
            held: Mutex::new(BTreeSet::new()),
            completed: Mutex::new(BTreeSet::new()),
            incarnations: Mutex::new(BTreeMap::new()),
            mob_events: Mutex::new(BTreeMap::new()),
            log_reads: Mutex::new(Vec::new()),
            roster_facts: Mutex::new(BTreeMap::new()),
            unreadable_facts: Mutex::new(BTreeSet::new()),
            input_ids: Mutex::new(BTreeMap::new()),
            inputs: Mutex::new(BTreeMap::new()),
            input_default: Mutex::new(None),
            role_reads: AtomicU64::new(0),
        }
    }

    /// How many times the members of a role were read.
    fn role_reads(&self) -> u64 {
        self.role_reads.load(Ordering::SeqCst)
    }

    /// Record `identity`'s role and labels on this mob's roster.
    fn set_roster_facts(&self, identity: &str, role: &str, labels: &[(&str, &str)]) {
        self.roster_facts.lock().unwrap().insert(
            identity.to_string(),
            (
                role.to_string(),
                labels
                    .iter()
                    .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                    .collect(),
            ),
        );
    }

    /// Put `identity` on the roster of the mob `mob_id` a member created.
    fn add_child_member(&self, mob_id: &str, identity: &str) {
        self.child_roster
            .lock()
            .unwrap()
            .insert((mob_id.to_string(), identity.to_string()));
    }

    /// Append an event to the log of the mob `mob_id`.
    fn log_event(&self, mob_id: &str, kind: &'static str) -> u64 {
        let mut logs = self.mob_events.lock().unwrap();
        let log = logs.entry(mob_id.to_string()).or_default();
        let cursor = log.last().map_or(1, |(cursor, _)| cursor + 1);
        log.push((cursor, kind));
        cursor
    }

    fn incarnation(&self, mob_id: &str) -> MobIncarnation {
        self.incarnations
            .lock()
            .unwrap()
            .get(mob_id)
            .cloned()
            .unwrap_or(MobIncarnation {
                owner_session: Some("owner-1".into()),
                first_event_at_ms: 1,
            })
    }

    fn held(&self, mob_id: &MobId) -> bool {
        self.held.lock().unwrap().contains(mob_id.as_str())
    }

    /// Script the origin Meerkat's records give the mob `mob_id`.
    fn set_origin(&self, mob_id: &str, incarnation: MobIncarnation, origin: MobOrigin) {
        self.origins.lock().unwrap().insert(
            mob_id.to_string(),
            MobOriginFacts {
                incarnation,
                origin,
            },
        );
    }

    /// That a member of `parent` created a mob, under the parent's scripted
    /// incarnation, or of this gateway's own mob, which has none.
    fn created_by(&self, parent: &str, creator: &str) -> MobOrigin {
        let parent_incarnation = (parent != OWN_MOB).then(|| {
            self.origins
                .lock()
                .unwrap()
                .get(parent)
                .map(|facts| facts.incarnation.clone())
                .expect("the parent's origin is scripted first")
        });
        MobOrigin::Member {
            parent: mob(parent),
            creator: creator.to_string(),
            parent_incarnation,
        }
    }

    fn hosted(&self, mob_id: &MobId) -> bool {
        TEST_MOBS.contains(&mob_id.as_str())
            && !self.destroyed.lock().unwrap().contains(mob_id.as_str())
    }

    pub(super) fn submits(&self) -> u64 {
        self.ledger.lock().unwrap().submits
    }

    fn bound(&self, key: &str) -> Option<String> {
        self.ledger
            .lock()
            .unwrap()
            .bindings
            .get(key)
            .map(|(delivery, _, _)| delivery.clone())
    }
}

#[async_trait]
impl NativeDelivery for ScriptedNative {
    async fn member_facts(
        &self,
        member: &Member,
    ) -> std::result::Result<Option<super::sources::MemberFacts>, NativeUnavailable> {
        if member.mob_id != mob(OWN_MOB) {
            return Ok(None);
        }
        if self
            .unreadable_facts
            .lock()
            .unwrap()
            .contains(member.identity.as_str())
        {
            return Err(NativeUnavailable("the roster cannot be read now".into()));
        }
        Ok(self
            .roster_facts
            .lock()
            .unwrap()
            .get(member.identity.as_str())
            .map(|(role, labels)| super::sources::MemberFacts::new(role.as_str(), labels.clone())))
    }

    async fn members_with_role(
        &self,
        mob_id: &MobId,
        role: &str,
    ) -> std::result::Result<Vec<Member>, NativeUnavailable> {
        self.role_reads.fetch_add(1, Ordering::SeqCst);
        if *mob_id != mob(OWN_MOB) {
            return Ok(Vec::new());
        }
        Ok(self
            .roster_facts
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, (held, _))| held == role)
            .map(|(identity, _)| member(identity))
            .collect())
    }

    fn governs_work_authority(&self) -> std::result::Result<bool, NativeUnavailable> {
        Ok(self.governed)
    }

    async fn route_for(
        &self,
        member: &Member,
    ) -> std::result::Result<MemberRouteRecord, MemberProblem> {
        let mob_id = member.mob_id.as_str();
        if mob_id == OWN_MOB {
            return Ok(local_route());
        }
        if !self.hosted(&member.mob_id) {
            return Ok(MemberRouteRecord {
                route: MemberRoute::Peer,
                incarnation: None,
            });
        }
        if self.peers.contains(mob_id) {
            return Err(MemberProblem::Ambiguous(format!(
                "{mob_id} is hosted here and filed as a peer"
            )));
        }
        if self.held(&member.mob_id) {
            return Err(MemberProblem::Unavailable(format!("{mob_id} is held")));
        }
        Ok(MemberRouteRecord {
            route: MemberRoute::Child,
            incarnation: Some(self.incarnation(mob_id)),
        })
    }

    async fn mob_presence(
        &self,
        mobs: &BTreeSet<MobId>,
    ) -> std::result::Result<BTreeMap<MobId, MobPresence>, NativeUnavailable> {
        Ok(mobs
            .iter()
            .map(|mob_id| {
                let presence = if self.hosted(mob_id) {
                    MobPresence::Hosted
                } else {
                    MobPresence::Gone
                };
                (mob_id.clone(), presence)
            })
            .collect())
    }

    async fn hosted_mob_ids(&self) -> std::result::Result<BTreeSet<MobId>, NativeUnavailable> {
        Ok(TEST_MOBS
            .iter()
            .filter(|mob_id| **mob_id != OWN_MOB)
            .map(|mob_id| mob(mob_id))
            .filter(|mob_id| self.hosted(mob_id))
            .collect())
    }

    fn mobs_restored(&self) -> bool {
        self.restored.load(Ordering::SeqCst)
    }

    fn own_mob(&self) -> Option<MobId> {
        self.own.clone()
    }

    async fn mob_origin(
        &self,
        mob_id: &MobId,
    ) -> std::result::Result<Option<MobOriginFacts>, NativeUnavailable> {
        if self
            .unreadable_origins
            .lock()
            .unwrap()
            .contains(mob_id.as_str())
        {
            return Err(NativeUnavailable(
                "the session store is not reachable".into(),
            ));
        }
        if !self.hosted(mob_id) {
            return Ok(None);
        }
        Ok(self.origins.lock().unwrap().get(mob_id.as_str()).cloned())
    }

    async fn check_member(
        &self,
        member: &Member,
        route: MemberRoute,
    ) -> std::result::Result<(), MemberProblem> {
        let identity = member.identity.as_str();
        let mob_id = member.mob_id.as_str();
        match route {
            MemberRoute::Local if self.unusable.contains(identity) => Err(MemberProblem::Unusable(
                format!("{identity} is placed on a host"),
            )),
            MemberRoute::Local if self.known.contains(identity) => Ok(()),
            MemberRoute::Local => Err(MemberProblem::Unknown),
            MemberRoute::Child if self.completed.lock().unwrap().contains(mob_id) => {
                Err(MemberProblem::Unusable(format!("{mob_id} has completed")))
            }
            MemberRoute::Child
                if self
                    .child_roster
                    .lock()
                    .unwrap()
                    .contains(&(mob_id.to_string(), identity.to_string())) =>
            {
                Ok(())
            }
            MemberRoute::Child => Err(MemberProblem::Unknown),
            MemberRoute::Peer if self.peers.contains(mob_id) => Ok(()),
            MemberRoute::Peer => Err(MemberProblem::UnusablePeer(format!(
                "no pinned contact for {mob_id}"
            ))),
        }
    }

    async fn prepare_member(
        &self,
        member: &Member,
        route: MemberRoute,
    ) -> std::result::Result<(), PrepareError> {
        self.prepared
            .lock()
            .unwrap()
            .push(member.identity.to_string());
        if route == MemberRoute::Child {
            if self.held(&member.mob_id) {
                return Err(PrepareError::Unavailable(format!(
                    "{} is held",
                    member.mob_id
                )));
            }
            if !self.hosted(&member.mob_id) {
                return Err(PrepareError::Gone(SubscriptionEnd::MemberMobEnded {
                    why: "destroyed".into(),
                }));
            }
        }
        match self.waiting.lock().unwrap().get(member.identity.as_str()) {
            Some(cause) => Err(PrepareError::Suspended {
                cause: cause.clone(),
                evidence: EvidenceClass::Local,
            }),
            None => Ok(()),
        }
    }

    async fn member_mob_cursor(
        &self,
        mob_id: &MobId,
    ) -> std::result::Result<u64, NativeUnavailable> {
        Ok(self
            .mob_events
            .lock()
            .unwrap()
            .get(mob_id.as_str())
            .and_then(|log| log.last())
            .map_or(0, |(cursor, _)| *cursor))
    }

    async fn member_mob_gone(
        &self,
        mob_id: &MobId,
        incarnation: Option<&MobIncarnation>,
    ) -> std::result::Result<Option<MemberMobEnd>, NativeUnavailable> {
        if self.held(mob_id) {
            return Err(NativeUnavailable(format!("{mob_id} is held")));
        }
        let why = if !self.hosted(mob_id) {
            "destroyed"
        } else if incarnation
            .is_some_and(|subscribed_in| *subscribed_in != self.incarnation(mob_id.as_str()))
        {
            "created again"
        } else {
            return Ok(None);
        };
        Ok(Some(MemberMobEnd {
            cursor: u64::MAX,
            why: why.into(),
        }))
    }

    async fn member_mob_end(
        &self,
        mob_id: &MobId,
        incarnation: Option<&MobIncarnation>,
        after: u64,
    ) -> std::result::Result<MemberMobScan, NativeUnavailable> {
        if let Some(end) = self.member_mob_gone(mob_id, incarnation).await? {
            return Ok(MemberMobScan::Ended(end));
        }
        self.log_reads.lock().unwrap().push(after);
        let logs = self.mob_events.lock().unwrap();
        let mut through = after;
        for (cursor, kind) in logs.get(mob_id.as_str()).into_iter().flatten() {
            if *cursor <= after {
                continue;
            }
            if matches!(*kind, "completed" | "reset") {
                return Ok(MemberMobScan::Ended(MemberMobEnd {
                    cursor: *cursor,
                    why: (*kind).into(),
                }));
            }
            through = *cursor;
        }
        Ok(MemberMobScan::Running { through })
    }

    async fn submit(
        &self,
        batch: &Batch,
        committed_at_ms: u64,
    ) -> std::result::Result<NativeReceipt, NativeSubmitError> {
        let mut ledger = self.ledger.lock().unwrap();
        ledger.submits += 1;
        if let Some(departure) = self.submit_gone.lock().unwrap().take() {
            return Err(NativeSubmitError::MemberGone(departure));
        }
        if ledger.unanswered {
            return Err(NativeSubmitError::OutcomeUnknown {
                cause: SuspendCause::PeerUnanswered {
                    code: Some("unsigned".into()),
                },
                evidence: EvidenceClass::Unverified,
                detail: "an unsigned answer".into(),
            });
        }
        if let Some((cause, evidence)) = ledger.refuse.clone() {
            return Err(NativeSubmitError::NotCommitted { cause, evidence });
        }
        if ledger.refuse_owner_retired || ledger.retired {
            ledger.refuse_owner_retired = false;
            return Err(NativeSubmitError::NotCommitted {
                cause: SuspendCause::MemberNotLive,
                evidence: EvidenceClass::Local,
            });
        }
        let exact = serde_json::to_string(&continuation_for(batch).unwrap()).unwrap();
        let key = batch.id.to_string();
        if let Some((bound, receipt, _)) = ledger.bindings.get(&key) {
            if *bound == exact {
                return Ok(receipt.clone());
            }
            return Err(NativeSubmitError::Blocked {
                cause: BlockCause::Conflict,
                detail: format!("the native key {key} is bound to other content"),
            });
        }
        let receipt = NativeReceipt {
            delivery_id: format!("continuation:{key}"),
            address: "rt:member:test:agent:1".into(),
            submission_digest: format!("digest:{key}"),
            committed_at_ms,
        };
        ledger
            .bindings
            .insert(key, (exact, receipt.clone(), self.apply_on_commit));
        if ledger.lose_next_ack {
            ledger.lose_next_ack = false;
            return Err(NativeSubmitError::OutcomeUnknown {
                cause: SuspendCause::NativeUnavailable,
                evidence: EvidenceClass::Local,
                detail: "the answer was lost".into(),
            });
        }
        Ok(receipt)
    }

    async fn status(&self, batch: &Batch) -> std::result::Result<NativeStatus, NativeStatusError> {
        let mut ledger = self.ledger.lock().unwrap();
        ledger.status_reads += 1;
        if let Some(error) = ledger.status_error.clone() {
            return Err(error);
        }
        let Some((_, receipt, applied)) = ledger.bindings.get(batch.id.as_str()) else {
            return Ok(NativeStatus::NotCommitted);
        };
        if ledger.strand.contains(batch.id.as_str()) {
            return Ok(NativeStatus::Stranded {
                receipt: receipt.clone(),
            });
        }
        Ok(if *applied {
            NativeStatus::Applied {
                receipt: receipt.clone(),
                session_id: "session-1".into(),
                input_id: "input-1".into(),
            }
        } else {
            NativeStatus::Pending {
                receipt: receipt.clone(),
                admitted: None,
            }
        })
    }

    async fn input(
        &self,
        batch: &Batch,
        _: &str,
        input_id: &str,
    ) -> std::result::Result<Option<serde_json::Value>, NativeUnavailable> {
        self.ledger.lock().unwrap().input_reads += 1;
        if let Some(reading) = self.input_ids.lock().unwrap().get(input_id) {
            return Ok(Some(reading.clone()));
        }
        Ok(Some(
            self.inputs
                .lock()
                .unwrap()
                .get(batch.id.as_str())
                .cloned()
                .or_else(|| self.input_default.lock().unwrap().clone())
                .unwrap_or_else(|| serde_json::json!({ "phase": "consumed" })),
        ))
    }
}

pub(super) struct Harness {
    _dir: tempfile::TempDir,
    pub(super) store: Arc<SqliteAnnouncementStore>,
    pub(super) native: Arc<ScriptedNative>,
    pub(super) clock: Arc<AtomicU64>,
    pub(super) service: AnnouncementService,
}

pub(super) fn harness_with(native: ScriptedNative) -> Harness {
    let (dir, store) = temp_store();
    let store = Arc::new(store);
    let native = Arc::new(native);
    let clock = Arc::new(AtomicU64::new(1_000));
    let reader = Arc::clone(&clock);
    let config = AnnouncementConfig {
        policy: SealingPolicy {
            max_count: 256,
            max_bytes: 1_048_576,
            max_delay_ms: 60_000,
            min_wake_interval_ms: 60_000,
        },
        ..AnnouncementConfig::default()
    };
    let service = AnnouncementService::with_clock(
        Arc::clone(&store) as Arc<dyn AnnouncementStore>,
        Arc::clone(&native) as Arc<dyn NativeDelivery>,
        config,
        Arc::new(move || reader.load(Ordering::SeqCst)),
    )
    .unwrap();
    Harness {
        _dir: dir,
        store,
        native,
        clock,
        service,
    }
}

/// A harness whose service runs over `store` with `native`, configured as
/// [`harness_with`]'s. The native owner is shared, so the service of a
/// restarted process can find what the native owner committed before.
pub(super) fn harness_over(store: SqliteAnnouncementStore, native: Arc<ScriptedNative>) -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(store);
    let clock = Arc::new(AtomicU64::new(1_000));
    let reader = Arc::clone(&clock);
    let config = AnnouncementConfig {
        policy: SealingPolicy {
            max_count: 256,
            max_bytes: 1_048_576,
            max_delay_ms: 60_000,
            min_wake_interval_ms: 60_000,
        },
        ..AnnouncementConfig::default()
    };
    let service = AnnouncementService::with_clock(
        Arc::clone(&store) as Arc<dyn AnnouncementStore>,
        Arc::clone(&native) as Arc<dyn NativeDelivery>,
        config,
        Arc::new(move || reader.load(Ordering::SeqCst)),
    )
    .unwrap();
    Harness {
        _dir: dir,
        store,
        native,
        clock,
        service,
    }
}

impl Harness {
    /// The database file of a harness over a store on disk.
    pub(super) fn database_path(&self) -> std::path::PathBuf {
        self._dir.path().join("mob_announcements.sqlite3")
    }

    /// A round once every batch the last one read is due to be read again.
    pub(super) async fn round_after_pause(&self) -> RoundReport {
        self.clock.fetch_add(
            AnnouncementConfig::default().settle_check_max_ms + 1,
            Ordering::SeqCst,
        );
        self.service.run_round().await.unwrap()
    }

    pub(super) async fn subscribe(&self, id: &str) -> Subscription {
        self.service
            .put_subscription(&op(id), put(id, "agent:a", &["ios"], StartFrontier::Now))
            .await
            .unwrap()
    }

    pub(super) async fn announce_burst(&self, count: usize) {
        for index in 0..count {
            self.service
                .announce(announce_request(
                    "payments",
                    &format!("failure-{index}"),
                    &["ios"],
                    &format!("checkout failed for order {index}"),
                ))
                .await
                .unwrap();
        }
    }

    pub(super) async fn only_batch(&self) -> Batch {
        let mut batches = self
            .service
            .deliveries(&BatchQuery {
                member: Some(member("agent:a")),
                limit: 10,
                ..BatchQuery::default()
            })
            .await
            .unwrap();
        assert_eq!(batches.len(), 1, "exactly one batch: {batches:?}");
        batches.remove(0)
    }
}

#[tokio::test]
async fn a_burst_while_paused_is_delivered_once_on_resume() {
    let harness = harness_with(ScriptedNative::new());
    let subscription = harness.subscribe("s1").await;
    let paused = harness
        .service
        .change_subscription(
            &op("p"),
            &sid("s1"),
            subscription.revision,
            SubscriptionChange::Pause,
        )
        .await
        .unwrap();
    harness.announce_burst(200).await;
    harness.clock.store(10_000_000, Ordering::SeqCst);
    let report = harness.service.run_round().await.unwrap();
    assert_eq!(report.sealed, 0, "nothing is delivered while paused");
    assert_eq!(harness.native.submits(), 0);

    harness
        .service
        .change_subscription(
            &op("r"),
            &sid("s1"),
            paused.subscription.revision,
            SubscriptionChange::Resume,
        )
        .await
        .unwrap();
    let report = harness.service.run_round().await.unwrap();
    assert_eq!(report.sealed, 1, "the whole burst is one batch");
    assert_eq!(harness.native.submits(), 1, "one native submission");
    let batch = harness.only_batch().await;
    assert_eq!(batch.items.len(), 200);
    assert_eq!(batch.state.tag(), BatchStateTag::Delivered);
    let observed = harness.service.observe_delivery(&batch.id).await.unwrap();
    assert!(matches!(observed.native, NativeObservation::Applied { .. }));
    assert!(
        observed.input.is_some(),
        "the admitted input's own state is reported"
    );

    let again = harness.service.run_round().await.unwrap();
    assert_eq!(again.sealed, 0);
    assert_eq!(harness.native.submits(), 1, "nothing is delivered twice");
}

#[tokio::test]
async fn a_lost_acknowledgment_is_reconciled_without_a_second_submission() {
    let mut native = ScriptedNative::new();
    native.apply_on_commit = false;
    native.ledger.get_mut().unwrap().lose_next_ack = true;
    let harness = harness_with(native);
    harness.subscribe("s1").await;
    harness.announce_burst(3).await;
    harness.clock.store(10_000_000, Ordering::SeqCst);
    harness.service.run_round().await.unwrap();
    let batch = harness.only_batch().await;
    assert_eq!(
        batch.state,
        BatchState::Submitting { attempt: 1 },
        "an unknown outcome is never recorded as success or failure"
    );
    assert_eq!(harness.native.submits(), 1);

    // The next round reads the native status instead of submitting again.
    harness.round_after_pause().await;
    let batch = harness.only_batch().await;
    assert_eq!(batch.state.tag(), BatchStateTag::Committed);
    assert_eq!(
        harness.native.submits(),
        1,
        "the native commit was found, not repeated"
    );
    let observed = harness.service.observe_delivery(&batch.id).await.unwrap();
    assert!(
        matches!(observed.native, NativeObservation::Pending { .. }),
        "committed but not yet admitted is reported as such: {:?}",
        observed.native
    );
}

/// What a status read of a peer that answers nothing it can verify brings:
/// no status, and no evidence about the delivery.
fn unanswered_status() -> NativeStatusError {
    NativeStatusError::Unavailable {
        cause: SuspendCause::PeerUnanswered {
            code: Some("unsigned".into()),
        },
        evidence: EvidenceClass::Unverified,
        detail: "an unsigned answer".into(),
    }
}

/// The pause before a batch this old is looked at again: the slowest one.
fn settle_pace() -> u64 {
    AnnouncementConfig::default().settle_check_max_ms
}

/// The harness's retention window, and its quiet window after a submission.
fn retention_and_quiet() -> (u64, u64) {
    let config = AnnouncementConfig::default();
    (
        config.retention_ms,
        config.peer_request_window_ms + config.settle_check_max_ms,
    )
}

async fn batches_of(harness: &Harness, member: &Member) -> Vec<Batch> {
    harness
        .service
        .deliveries(&BatchQuery {
            member: Some(member.clone()),
            limit: 10,
            ..BatchQuery::default()
        })
        .await
        .unwrap()
}

/// Announce one more and let the member's backlog fall due.
async fn announce_next(harness: &Harness, key: &str) {
    harness
        .service
        .announce(announce_request(
            "payments",
            key,
            &["ios"],
            "checkout recovered",
        ))
        .await
        .unwrap();
    harness.clock.fetch_add(60_001, Ordering::SeqCst);
    harness.service.run_round().await.unwrap();
}

/// A submission whose outcome no status read ever brings settles as not
/// confirmed once the retention window, and the quiet window since its last
/// submission, have passed. It is never submitted again, its announcements
/// reach the member's next batch as not confirmed, and that batch goes on.
/// A status read later is kept on it and reopens nothing.
#[tokio::test]
async fn an_outcome_never_learned_settles_unverified_and_the_next_batch_goes_on() {
    let native = ScriptedNative::new();
    {
        let mut ledger = native.ledger.lock().unwrap();
        ledger.unanswered = true;
        ledger.status_error = Some(unanswered_status());
    }
    let harness = harness_with(native);
    let remote = peer_member("agent:x", "mob-b");
    harness
        .service
        .put_subscription(&op("o1"), put_for("s1", remote.clone(), &["ios"]))
        .await
        .unwrap();
    harness.announce_burst(2).await;
    harness.clock.store(10_000_000, Ordering::SeqCst);
    harness.service.run_round().await.unwrap();
    let first = batches_of(&harness, &remote).await.remove(0);
    assert_eq!(first.state, BatchState::Submitting { attempt: 1 });
    let (retention, quiet) = retention_and_quiet();
    harness
        .clock
        .store(first.sealed_at_ms + retention - 1, Ordering::SeqCst);
    harness.service.run_round().await.unwrap();
    assert_eq!(
        batches_of(&harness, &remote).await[0].state.tag(),
        BatchStateTag::Submitting,
        "inside the retention window it waits for a status"
    );
    harness
        .clock
        .store(first.sealed_at_ms + retention + quiet + 1, Ordering::SeqCst);
    harness.service.run_round().await.unwrap();
    let settled = batches_of(&harness, &remote).await.remove(0);
    assert_eq!(settled.state, BatchState::ExpiredUnverified { late: None });
    assert_eq!(harness.native.submits(), 1, "it is never submitted again");

    announce_next(&harness, "after").await;
    let all = batches_of(&harness, &remote).await;
    assert_eq!(all.len(), 2, "the member's next batch seals: {all:?}");
    assert_eq!(
        all[1].gaps,
        vec![BatchGap {
            mob_id: mob("payments"),
            count: 2,
            reason: GapReason::DeliveryNotConfirmed,
        }]
    );

    // The peer later answers, signed, that the first batch was applied: the
    // answer is kept on it, which stays settled as it was.
    {
        let mut ledger = harness.native.ledger.lock().unwrap();
        ledger.status_error = None;
        ledger.bindings.insert(
            first.id.to_string(),
            ("late".into(), receipt(first.id.as_str()), true),
        );
    }
    harness.clock.fetch_add(quiet + 1, Ordering::SeqCst);
    harness.service.run_round().await.unwrap();
    let kept = harness
        .service
        .observe_delivery(&first.id)
        .await
        .unwrap()
        .batch;
    assert_eq!(kept.state.tag(), BatchStateTag::ExpiredUnverified);
    let late = kept.state.late().expect("the late status is kept");
    assert!(
        matches!(late.status, NativeStatus::Applied { .. }),
        "{late:?}"
    );
    assert_eq!(
        kept.updated_at_ms, settled.updated_at_ms,
        "it stays settled when it settled"
    );
    let long_after = harness.clock.load(Ordering::SeqCst) + 100 * quiet;
    assert!(
        harness
            .store
            .late_reads_due(long_after, 10)
            .await
            .unwrap()
            .iter()
            .all(|batch| batch.id != first.id),
        "a final late status ends its reads"
    );
}

/// A committed batch whose status can no longer be read settles with its
/// admission not confirmed at its first look once the retention window, and
/// the quiet window after it, have passed since the commit: its receipt is
/// kept, it is never submitted again, and the member's next batch reports it
/// and goes on.
async fn an_unreadable_commit_settles_unconfirmed(to: Member, unreadable: NativeStatusError) {
    let mut native = ScriptedNative::new();
    native.apply_on_commit = false;
    let harness = harness_with(native);
    harness
        .service
        .put_subscription(&op("o1"), put_for("s1", to.clone(), &["ios"]))
        .await
        .unwrap();
    harness.announce_burst(1).await;
    harness.clock.store(10_000_000, Ordering::SeqCst);
    harness.service.run_round().await.unwrap();
    let committed = batches_of(&harness, &to).await.remove(0);
    let BatchState::Committed { receipt } = committed.state.clone() else {
        panic!("expected a committed batch, got {:?}", committed.state);
    };
    harness.native.ledger.lock().unwrap().status_error = Some(unreadable);
    let (retention, quiet) = retention_and_quiet();
    harness.clock.store(
        committed.updated_at_ms + retention + quiet - 1,
        Ordering::SeqCst,
    );
    harness.service.run_round().await.unwrap();
    assert_eq!(
        batches_of(&harness, &to).await[0].state.tag(),
        BatchStateTag::Committed,
        "inside the quiet window after the retention window it waits"
    );
    // Its next look, one settle pace later, is past that window.
    harness.clock.store(
        committed.updated_at_ms + retention + quiet - 1 + settle_pace(),
        Ordering::SeqCst,
    );
    harness.service.run_round().await.unwrap();
    assert_eq!(
        batches_of(&harness, &to).await[0].state,
        BatchState::CommittedUnconfirmed {
            receipt,
            late: None
        }
    );
    assert_eq!(harness.native.submits(), 1, "it is never submitted again");
    announce_next(&harness, "after").await;
    let all = batches_of(&harness, &to).await;
    assert_eq!(all.len(), 2, "the member's next batch seals: {all:?}");
    assert_eq!(
        all[1].gaps,
        vec![BatchGap {
            mob_id: mob("payments"),
            count: 1,
            reason: GapReason::AdmissionNotConfirmed,
        }]
    );
}

/// A peer that committed the batch, then restarted with keyed inject
/// serving turned off, answers every status read with a signed `not_served`
/// (`runtime_disabled`): no status, so the batch settles unconfirmed.
#[tokio::test]
async fn a_commit_whose_peer_stops_serving_settles_unconfirmed() {
    an_unreadable_commit_settles_unconfirmed(
        peer_member("agent:x", "mob-b"),
        NativeStatusError::Unavailable {
            cause: SuspendCause::PeerNotServing {
                cause: crate::runtime::keyed_inject::NotServedCause::RuntimeDisabled,
            },
            evidence: EvidenceClass::Verified,
            detail: "this gateway does not serve keyed injects (runtime_disabled)".into(),
        },
    )
    .await;
}

/// This gateway's own native owner that can no longer read a committed
/// batch's status leaves it unconfirmed the same way.
#[tokio::test]
async fn a_commit_whose_status_cannot_be_read_settles_unconfirmed() {
    an_unreadable_commit_settles_unconfirmed(
        member("agent:a"),
        NativeStatusError::local("the runtime store is not reachable"),
    )
    .await;
}

/// "No status" is any read that leaves the outcome unknown, a peer's signed
/// refusal of every status read included: the batch settles unverified.
#[tokio::test]
async fn a_signed_refusal_of_every_status_read_also_settles_unverified() {
    let native = ScriptedNative::new();
    {
        let mut ledger = native.ledger.lock().unwrap();
        ledger.unanswered = true;
        ledger.status_error = Some(NativeStatusError::Unavailable {
            cause: SuspendCause::PeerComposing,
            evidence: EvidenceClass::Verified,
            detail: "keyed_inject_unavailable".into(),
        });
    }
    let harness = harness_with(native);
    let remote = peer_member("agent:x", "mob-b");
    harness
        .service
        .put_subscription(&op("o1"), put_for("s1", remote.clone(), &["ios"]))
        .await
        .unwrap();
    harness.announce_burst(1).await;
    harness.clock.store(10_000_000, Ordering::SeqCst);
    harness.service.run_round().await.unwrap();
    let first = batches_of(&harness, &remote).await.remove(0);
    assert_eq!(first.state, BatchState::Submitting { attempt: 1 });
    let (retention, quiet) = retention_and_quiet();
    harness
        .clock
        .store(first.sealed_at_ms + retention + quiet + 1, Ordering::SeqCst);
    harness.service.run_round().await.unwrap();
    assert_eq!(
        batches_of(&harness, &remote).await[0].state,
        BatchState::ExpiredUnverified { late: None }
    );
    assert_eq!(harness.native.submits(), 1);
}

/// A committed batch a verified status still reads as pending past the
/// retention window since its commit settles with its admission not
/// confirmed: the member's next batch seals and goes on, and the admission
/// that comes later is kept on the first batch as its late outcome, with
/// that batch delivered once.
#[tokio::test]
async fn a_commit_still_pending_past_retention_settles_unconfirmed_and_keeps_its_admission() {
    let mut native = ScriptedNative::new();
    native.apply_on_commit = false;
    let harness = harness_with(native);
    harness.subscribe("s1").await;
    harness.announce_burst(1).await;
    harness.clock.store(10_000_000, Ordering::SeqCst);
    harness.service.run_round().await.unwrap();
    let committed = harness.only_batch().await;
    let BatchState::Committed { receipt } = committed.state.clone() else {
        panic!("expected a committed batch, got {:?}", committed.state);
    };
    let (retention, quiet) = retention_and_quiet();
    harness
        .clock
        .store(committed.updated_at_ms + retention - 1, Ordering::SeqCst);
    harness.service.run_round().await.unwrap();
    assert_eq!(
        harness.only_batch().await.state.tag(),
        BatchStateTag::Committed,
        "inside the retention window a pending delivery waits"
    );
    // Its next look, one settle pace later, is past the window.
    harness.clock.store(
        committed.updated_at_ms + retention - 1 + settle_pace(),
        Ordering::SeqCst,
    );
    harness.service.run_round().await.unwrap();
    assert_eq!(
        harness.only_batch().await.state,
        BatchState::CommittedUnconfirmed {
            receipt: receipt.clone(),
            late: None
        }
    );
    announce_next(&harness, "after").await;
    let all = batches_of(&harness, &member("agent:a")).await;
    assert_eq!(all.len(), 2, "the member's next batch seals: {all:?}");
    assert_eq!(all[1].gaps[0].reason, GapReason::AdmissionNotConfirmed);

    // The member's runtime admits the first batch later.
    harness
        .native
        .ledger
        .lock()
        .unwrap()
        .bindings
        .get_mut(committed.id.as_str())
        .expect("the first batch's commit")
        .2 = true;
    harness.clock.fetch_add(quiet + 1, Ordering::SeqCst);
    harness.service.run_round().await.unwrap();
    let kept = harness
        .service
        .observe_delivery(&committed.id)
        .await
        .unwrap()
        .batch;
    assert_eq!(kept.state.tag(), BatchStateTag::CommittedUnconfirmed);
    let late = kept.state.late().expect("the admission is kept");
    assert!(
        matches!(&late.status, NativeStatus::Applied { receipt: applied, .. } if *applied == receipt),
        "{late:?}"
    );
    assert_eq!(
        harness.native.submits(),
        2,
        "one submission per batch: the first is delivered once"
    );
}

/// A late status naming another delivery contradicts an unconfirmed batch:
/// it is kept as it was read, and nothing more is read for the batch, though
/// a pending status would otherwise be read again.
#[tokio::test]
async fn a_late_status_naming_another_delivery_is_kept_and_ends_the_reads() {
    let mut native = ScriptedNative::new();
    native.apply_on_commit = false;
    let harness = harness_with(native);
    harness.subscribe("s1").await;
    harness.announce_burst(1).await;
    harness.clock.store(10_000_000, Ordering::SeqCst);
    harness.service.run_round().await.unwrap();
    let committed = harness.only_batch().await;
    let (retention, quiet) = retention_and_quiet();
    harness.clock.store(
        committed.updated_at_ms + retention - 1 + settle_pace(),
        Ordering::SeqCst,
    );
    harness.service.run_round().await.unwrap();
    assert_eq!(
        harness.only_batch().await.state.tag(),
        BatchStateTag::CommittedUnconfirmed
    );
    // The member's native key now holds a pending commit of another delivery.
    let other = receipt("another-delivery");
    harness
        .native
        .ledger
        .lock()
        .unwrap()
        .bindings
        .get_mut(committed.id.as_str())
        .expect("the first batch's commit")
        .1 = other.clone();
    harness.clock.fetch_add(quiet + 1, Ordering::SeqCst);
    harness.service.run_round().await.unwrap();
    let kept = harness.only_batch().await;
    let late = kept.state.late().expect("the contradicting status is kept");
    assert!(
        matches!(&late.status, NativeStatus::Pending { receipt, .. } if *receipt == other),
        "{late:?}"
    );
    let reads = harness.native.ledger.lock().unwrap().status_reads;
    harness.clock.fetch_add(10 * quiet, Ordering::SeqCst);
    harness.service.run_round().await.unwrap();
    assert_eq!(
        harness.native.ledger.lock().unwrap().status_reads,
        reads,
        "nothing more is read for a contradicted batch"
    );
}

/// A batch whose admission was not confirmed is read again once per quiet
/// window, not once per round.
#[tokio::test]
async fn an_unconfirmed_batch_is_read_again_once_per_quiet_window() {
    let mut native = ScriptedNative::new();
    native.apply_on_commit = false;
    let harness = harness_with(native);
    harness.subscribe("s1").await;
    harness.announce_burst(1).await;
    harness.clock.store(10_000_000, Ordering::SeqCst);
    harness.service.run_round().await.unwrap();
    let committed = harness.only_batch().await;
    let (retention, quiet) = retention_and_quiet();
    harness.clock.store(
        committed.updated_at_ms + retention - 1 + settle_pace(),
        Ordering::SeqCst,
    );
    harness.service.run_round().await.unwrap();
    assert_eq!(
        harness.only_batch().await.state.tag(),
        BatchStateTag::CommittedUnconfirmed
    );
    let reads = || harness.native.ledger.lock().unwrap().status_reads;
    let settled = reads();
    for _ in 0..3 {
        harness.clock.fetch_add(quiet / 4, Ordering::SeqCst);
        harness.service.run_round().await.unwrap();
    }
    assert_eq!(
        reads(),
        settled,
        "inside the quiet window nothing is read again"
    );
    harness.clock.fetch_add(quiet / 4 + 1, Ordering::SeqCst);
    harness.service.run_round().await.unwrap();
    assert_eq!(
        reads(),
        settled + 1,
        "one quiet window later it is read once"
    );
}

/// At most 16 batches whose delivery was not confirmed are read again in one
/// round; the rest are read in the next.
#[tokio::test]
async fn late_reads_are_bounded_per_round() {
    let harness = harness_with(ScriptedNative::new());
    harness.subscribe("s1").await;
    let agent = member("agent:a");
    for index in 0..17_u64 {
        harness
            .service
            .announce(announce_request(
                "payments",
                &format!("k{index}"),
                &["ios"],
                "x",
            ))
            .await
            .unwrap();
        let id = batch_id(&format!("late-{index:02}"));
        harness
            .store
            .seal_batch(&agent, &id, &policy(), 2_000 + index)
            .await
            .unwrap()
            .expect("the member's next batch seals");
        harness
            .store
            .transition_batch(&id, BatchStateTag::Sealed, BatchEvent::BeginSubmit, 3_000)
            .await
            .unwrap();
        harness
            .store
            .transition_batch(
                &id,
                BatchStateTag::Submitting,
                BatchEvent::ExpireUnverified,
                4_000,
            )
            .await
            .unwrap();
    }
    harness.clock.store(10_000_000, Ordering::SeqCst);
    let reads = || harness.native.ledger.lock().unwrap().status_reads;
    harness.service.run_round().await.unwrap();
    assert_eq!(reads(), 16, "one round reads at most 16");
    harness.service.run_round().await.unwrap();
    assert_eq!(reads(), 17, "the next round reads the rest");
}

/// A batch every answer to which was a signed "nothing was committed"
/// expires as expired once the retention window has passed, never as not
/// confirmed: nothing native ever held it.
#[tokio::test]
async fn a_batch_every_answer_refused_expires_as_expired() {
    let native = ScriptedNative::new();
    native.ledger.lock().unwrap().refuse = Some((
        SuspendCause::PeerNotServing {
            cause: crate::runtime::keyed_inject::NotServedCause::RuntimeDisabled,
        },
        EvidenceClass::Verified,
    ));
    let harness = harness_with(native);
    harness.subscribe("s1").await;
    harness.announce_burst(1).await;
    harness.clock.store(10_000_000, Ordering::SeqCst);
    harness.service.run_round().await.unwrap();
    let owed = harness.only_batch().await;
    assert_eq!(owed.state, BatchState::Sealed);
    let (retention, _) = retention_and_quiet();
    harness
        .clock
        .store(owed.sealed_at_ms + retention, Ordering::SeqCst);
    harness.service.run_round().await.unwrap();
    assert_eq!(harness.only_batch().await.state, BatchState::Expired);
    harness.native.ledger.lock().unwrap().refuse = None;
    announce_next(&harness, "after").await;
    let all = batches_of(&harness, &member("agent:a")).await;
    assert_eq!(all[1].gaps[0].reason, GapReason::Expired);
}

/// Only what a batch whose delivery was not confirmed does not already
/// state is news: any status for one whose outcome was never learned; for a
/// committed one, not its pending delivery and nothing about another
/// delivery. A status already kept is not news again.
#[test]
fn late_news_is_what_an_unconfirmed_batch_does_not_already_state() {
    let ours = receipt("b1");
    let applied = |receipt: NativeReceipt| NativeStatus::Applied {
        receipt,
        session_id: "session".into(),
        input_id: "input".into(),
    };
    let pending = NativeStatus::Pending {
        receipt: ours.clone(),
        admitted: None,
    };
    let unverified = BatchState::ExpiredUnverified { late: None };
    for status in [
        NativeStatus::NotCommitted,
        pending.clone(),
        applied(ours.clone()),
    ] {
        assert_eq!(unverified.late_news(status.clone()), Some(status));
    }
    let seen = unverified
        .with_late(LateOutcome {
            status: pending.clone(),
            at_ms: 1,
        })
        .unwrap();
    assert_eq!(seen.late_news(pending.clone()), None, "already kept");
    assert!(seen.late_news(applied(ours.clone())).is_some());
    let unconfirmed = BatchState::CommittedUnconfirmed {
        receipt: ours.clone(),
        late: None,
    };
    assert_eq!(unconfirmed.late_news(pending), None, "what it states");
    let elsewhere = applied(receipt("other"));
    assert_eq!(
        unconfirmed.late_news(elsewhere.clone()),
        Some(elsewhere.clone()),
        "a status about another delivery is kept, never dropped"
    );
    assert!(unconfirmed.late_names_other_delivery(&elsewhere));
    assert!(!unconfirmed.late_names_other_delivery(&applied(ours.clone())));
    assert!(!unverified.late_names_other_delivery(&elsewhere));
    assert!(unconfirmed.late_news(applied(ours.clone())).is_some());
    assert!(unconfirmed.late_news(NativeStatus::NotCommitted).is_some());
    let committed = BatchState::Committed { receipt: ours };
    assert_eq!(committed.late_news(NativeStatus::NotCommitted), None);
    assert!(
        committed
            .with_late(LateOutcome {
                status: NativeStatus::NotCommitted,
                at_ms: 1,
            })
            .is_err()
    );
}

/// A late outcome keeps the native status's own wire shape, with the time it
/// was read next to it, and reads back as written.
#[test]
fn a_late_outcome_keeps_the_native_status_shape() {
    let late = LateOutcome {
        status: NativeStatus::Applied {
            receipt: receipt("b1"),
            session_id: "session".into(),
            input_id: "input".into(),
        },
        at_ms: 9,
    };
    let wire = serde_json::to_value(&late).unwrap();
    assert_eq!(wire["status"], "applied");
    assert_eq!(wire["session_id"], "session");
    assert_eq!(wire["at_ms"], 9);
    assert_eq!(serde_json::from_value::<LateOutcome>(wire).unwrap(), late);
    let state = BatchState::CommittedUnconfirmed {
        receipt: receipt("b1"),
        late: Some(late),
    };
    let wire = serde_json::to_value(&state).unwrap();
    assert_eq!(wire["state"], "committed_unconfirmed");
    assert_eq!(wire["late"]["status"], "applied");
    assert_eq!(serde_json::from_value::<BatchState>(wire).unwrap(), state);
}

#[tokio::test]
async fn an_attempt_that_never_reached_the_native_owner_is_resubmitted_identically() {
    let harness = harness_with(ScriptedNative::new());
    harness.subscribe("s1").await;
    harness.announce_burst(2).await;
    let id = batch_id("b-crash");
    harness
        .store
        .seal_batch(
            &member("agent:a"),
            &id,
            &harness.service.config().policy,
            10_000_000,
        )
        .await
        .unwrap()
        .unwrap();
    // The intent was durable when the process died; nothing was submitted.
    harness
        .store
        .transition_batch(
            &id,
            BatchStateTag::Sealed,
            BatchEvent::BeginSubmit,
            10_000_001,
        )
        .await
        .unwrap();
    harness.clock.store(10_000_002, Ordering::SeqCst);
    harness.service.run_round().await.unwrap();
    let batch = harness.store.batch(&id).await.unwrap().unwrap();
    assert_eq!(batch.state.tag(), BatchStateTag::Delivered);
    assert_eq!(harness.native.submits(), 1);
    let expected =
        serde_json::to_string(&super::native::continuation_for(&batch).unwrap()).unwrap();
    assert_eq!(harness.native.bound(id.as_str()), Some(expected));
}

#[tokio::test]
async fn a_stranded_delivery_moves_to_a_successor_with_lineage() {
    let mut native = ScriptedNative::new();
    native.apply_on_commit = false;
    let harness = harness_with(native);
    harness.subscribe("s1").await;
    harness.announce_burst(1).await;
    harness.clock.store(10_000_000, Ordering::SeqCst);
    harness.service.run_round().await.unwrap();
    let first = harness.only_batch().await;
    assert_eq!(first.state.tag(), BatchStateTag::Committed);
    harness
        .native
        .ledger
        .lock()
        .unwrap()
        .strand
        .insert(first.id.to_string());
    harness.round_after_pause().await;
    let stranded = harness.store.batch(&first.id).await.unwrap().unwrap();
    assert_eq!(stranded.state.tag(), BatchStateTag::Stranded);
    let successor_id = stranded
        .successor
        .clone()
        .expect("a successor carries the announcements");
    let successor = harness.store.batch(&successor_id).await.unwrap().unwrap();
    assert_eq!(successor.predecessor, Some(first.id.clone()));
    assert_eq!(successor.items, first.items);
    assert!(
        successor.body.contains(first.id.as_str()),
        "the redelivery names its lineage"
    );
    assert_eq!(successor.state.tag(), BatchStateTag::Committed);
    assert_eq!(
        harness.native.submits(),
        2,
        "the successor is a new native key, never a re-key"
    );
}

#[tokio::test]
async fn a_member_without_an_incarnation_keeps_its_batch_owed() {
    let native = ScriptedNative::new();
    native.ledger.lock().unwrap().refuse_owner_retired = true;
    let harness = harness_with(native);
    harness.subscribe("s1").await;
    harness.announce_burst(1).await;
    harness.clock.store(10_000_000, Ordering::SeqCst);
    let report = harness.service.run_round().await.unwrap();
    assert!(
        report.next_due_ms.is_some(),
        "a refused attempt is looked at again"
    );
    let batch = harness.only_batch().await;
    assert_eq!(
        batch.state,
        BatchState::Sealed,
        "nothing was committed: the batch is owed again"
    );
    harness.clock.store(10_100_000, Ordering::SeqCst);
    harness.service.run_round().await.unwrap();
    let batch = harness.only_batch().await;
    assert_eq!(batch.state.tag(), BatchStateTag::Delivered);
    assert_eq!(harness.native.submits(), 2);
}

#[tokio::test]
async fn contradicted_native_evidence_blocks_a_committed_batch() {
    let mut native = ScriptedNative::new();
    native.apply_on_commit = false;
    let harness = harness_with(native);
    harness.subscribe("s1").await;
    harness.announce_burst(1).await;
    harness.clock.store(10_000_000, Ordering::SeqCst);
    harness.service.run_round().await.unwrap();
    let batch = harness.only_batch().await;
    assert_eq!(batch.state.tag(), BatchStateTag::Committed);
    // The native owner no longer holds the commit the receipt names.
    harness
        .native
        .ledger
        .lock()
        .unwrap()
        .bindings
        .remove(batch.id.as_str());
    harness.round_after_pause().await;
    let batch = harness.only_batch().await;
    assert_eq!(
        batch.state.tag(),
        BatchStateTag::Blocked,
        "contradicted evidence settles the batch for operators, never an error loop"
    );
    assert_eq!(harness.native.submits(), 1, "nothing is resubmitted blind");
}

#[tokio::test]
async fn a_committed_batch_is_blocked_by_evidence_about_another_delivery() {
    for applied in [false, true] {
        let mut native = ScriptedNative::new();
        native.apply_on_commit = false;
        let harness = harness_with(native);
        harness.subscribe("s1").await;
        harness.announce_burst(1).await;
        harness.clock.store(10_000_000, Ordering::SeqCst);
        harness.service.run_round().await.unwrap();
        let batch = harness.only_batch().await;
        assert_eq!(batch.state.tag(), BatchStateTag::Committed);
        // The native owner now reports a delivery other than the one this
        // batch's receipt names, pending or applied.
        {
            let mut ledger = harness.native.ledger.lock().unwrap();
            let binding = ledger.bindings.get_mut(batch.id.as_str()).unwrap();
            binding.1.delivery_id = "continuation:another-delivery".into();
            binding.1.submission_digest = "digest:another-delivery".into();
            binding.2 = applied;
        }
        harness.round_after_pause().await;
        let batch = harness.only_batch().await;
        assert_eq!(
            batch.state.tag(),
            BatchStateTag::Blocked,
            "applied={applied}: another delivery never settles this one"
        );
        assert_eq!(harness.native.submits(), 1, "nothing is resubmitted blind");
    }
}

#[test]
fn a_committed_batch_settles_only_from_its_own_receipt() {
    let committed = NativeReceipt {
        delivery_id: "continuation:mine".into(),
        address: "rt:member:test:agent:1".into(),
        submission_digest: "digest:mine".into(),
        committed_at_ms: 1,
    };
    let state = BatchState::Committed {
        receipt: committed.clone(),
    };
    let other_id = NativeReceipt {
        delivery_id: "continuation:theirs".into(),
        ..committed.clone()
    };
    let other_digest = NativeReceipt {
        submission_digest: "digest:theirs".into(),
        ..committed.clone()
    };
    let events = |receipt: &NativeReceipt| {
        vec![
            BatchEvent::NativeApplied {
                receipt: receipt.clone(),
                session_id: "session-1".into(),
                input_id: "input-1".into(),
            },
            BatchEvent::NativeStranded {
                receipt: receipt.clone(),
                successor: None,
            },
            BatchEvent::NativeRefused {
                reason: "refused".into(),
                receipt: Some(receipt.clone()),
            },
        ]
    };
    for other in [&other_id, &other_digest] {
        assert!(state.other_delivery(other).is_some());
        for event in events(other) {
            assert_eq!(
                state.apply(&event).unwrap().tag(),
                BatchStateTag::Blocked,
                "{event:?}"
            );
        }
    }
    assert!(state.other_delivery(&committed).is_none());
    let settled: Vec<_> = events(&committed)
        .iter()
        .map(|event| state.apply(event).unwrap().tag())
        .collect();
    assert_eq!(
        settled,
        [
            BatchStateTag::Delivered,
            BatchStateTag::Stranded,
            BatchStateTag::Refused
        ]
    );
    // A submitting batch has no receipt yet: native evidence settles it.
    assert!(
        BatchState::Submitting { attempt: 1 }
            .other_delivery(&other_id)
            .is_none()
    );
}

/// A round's worth of members whose batches the native owner commits but
/// does not admit (a long turn), each with newer announcements waiting
/// behind its batch, fill a whole round. One more member's announcement is
/// still sealed, and the dispatcher still comes back to its batch.
#[tokio::test]
async fn a_round_full_of_held_members_does_not_starve_the_next_one() {
    const HELD: usize = super::owner::ROUND_BATCHES;
    // One round seals every held member.
    const { assert!(HELD <= super::owner::ROUND_MEMBERS) };
    let mut native = ScriptedNative::new();
    native.apply_on_commit = false;
    native.known = (0..=HELD)
        .map(|index| format!("agent:r{index:02}"))
        .collect();
    let harness = harness_with(native);
    for index in 0..=HELD {
        let topic = format!("t{index:02}");
        harness
            .service
            .put_subscription(
                &op(&format!("o{index:02}")),
                put(
                    &format!("s{index:02}"),
                    &format!("agent:r{index:02}"),
                    &[topic.as_str()],
                    StartFrontier::Now,
                ),
            )
            .await
            .unwrap();
    }
    for index in 0..HELD {
        let topic = format!("t{index:02}");
        harness
            .service
            .announce(announce_request(
                "events",
                &format!("first-{index:02}"),
                &[topic.as_str()],
                "first",
            ))
            .await
            .unwrap();
    }
    harness.clock.store(100_000, Ordering::SeqCst);
    let report = harness.service.run_round().await.unwrap();
    assert_eq!(
        report.sealed as usize, HELD,
        "each held member has its batch"
    );
    for index in 0..HELD {
        let topic = format!("t{index:02}");
        harness
            .service
            .announce(announce_request(
                "events",
                &format!("second-{index:02}"),
                &[topic.as_str()],
                "second",
            ))
            .await
            .unwrap();
    }
    harness.clock.store(200_000, Ordering::SeqCst);
    harness
        .service
        .announce(announce_request(
            "events",
            "last",
            &[format!("t{HELD:02}").as_str()],
            "for the last member",
        ))
        .await
        .unwrap();
    harness.clock.store(300_000, Ordering::SeqCst);
    harness.service.run_round().await.unwrap();
    let last = BatchQuery {
        member: Some(member(&format!("agent:r{HELD:02}"))),
        limit: 10,
        ..BatchQuery::default()
    };
    let sealed = harness.service.deliveries(&last).await.unwrap();
    assert_eq!(
        sealed.len(),
        1,
        "the last member's announcement is sealed although a full round of held members is \
         ahead of it"
    );

    // The native owner admits the 65th member's batch; a later round
    // reads that evidence.
    harness
        .native
        .ledger
        .lock()
        .unwrap()
        .bindings
        .get_mut(sealed[0].id.as_str())
        .expect("the batch was committed")
        .2 = true;
    for round in 1..=3_u64 {
        harness
            .clock
            .store(300_000 + round * 100_000, Ordering::SeqCst);
        harness.service.run_round().await.unwrap();
    }
    let settled = harness.service.deliveries(&last).await.unwrap();
    assert_eq!(
        settled[0].state.tag(),
        BatchStateTag::Delivered,
        "the dispatcher comes back to the last member's batch"
    );
    assert_eq!(
        harness.native.submits() as usize,
        HELD + 1,
        "each batch is submitted once; the held ones are only read"
    );
}

/// The SQLite store with injected faults: batch transitions fail for one
/// member, as a storage fault would, and the first retention passes
/// report more work, as a long backlog would. It counts what it is asked.
struct FaultyStore {
    inner: SqliteAnnouncementStore,
    failing: Option<Member>,
    failures: AtomicU64,
    /// Retention passes left that report more work.
    more_passes: AtomicU64,
    prune_passes: AtomicU64,
    /// Backlog reads: one per dispatcher round.
    backlog_reads: AtomicU64,
    /// When each retention pass started.
    pass_times: Mutex<Vec<std::time::Instant>>,
    /// Panics the next backlog read, as a fault inside a round would.
    panic_next_round: std::sync::atomic::AtomicBool,
}

impl FaultyStore {
    fn new(inner: SqliteAnnouncementStore, failing: Option<&str>) -> Self {
        Self {
            inner,
            failing: failing.map(member),
            failures: AtomicU64::new(0),
            more_passes: AtomicU64::new(0),
            prune_passes: AtomicU64::new(0),
            backlog_reads: AtomicU64::new(0),
            pass_times: Mutex::new(Vec::new()),
            panic_next_round: std::sync::atomic::AtomicBool::new(false),
        }
    }
}

#[async_trait]
impl AnnouncementStore for FaultyStore {
    async fn announce(&self, request: AnnounceRequest, now_ms: u64) -> Result<AnnounceOutcome> {
        self.inner.announce(request, now_ms).await
    }

    async fn announcement(
        &self,
        mob_id: &MobId,
        source_key: &SourceKey,
    ) -> Result<Option<Announcement>> {
        self.inner.announcement(mob_id, source_key).await
    }

    async fn put_subscription(
        &self,
        operation: &OperationId,
        request: PutSubscription,
        now_ms: u64,
    ) -> Result<Subscription> {
        self.inner
            .put_subscription(operation, request, now_ms)
            .await
    }

    async fn change_subscription(
        &self,
        operation: &OperationId,
        id: &SubscriptionId,
        expected_revision: u64,
        change: SubscriptionChange,
        now_ms: u64,
    ) -> Result<SubscriptionChangeOutcome> {
        self.inner
            .change_subscription(operation, id, expected_revision, change, now_ms)
            .await
    }

    async fn subscription(&self, id: &SubscriptionId) -> Result<Option<Subscription>> {
        self.inner.subscription(id).await
    }

    async fn subscriptions(
        &self,
        member: Option<&Member>,
        after: Option<&SubscriptionId>,
        limit: usize,
    ) -> Result<Vec<Subscription>> {
        self.inner.subscriptions(member, after, limit).await
    }

    async fn end_member(
        &self,
        member: &Member,
        end: SubscriptionEnd,
        now_ms: u64,
    ) -> Result<Vec<SubscriptionId>> {
        self.inner.end_member(member, end, now_ms).await
    }

    async fn known_mobs(&self) -> Result<BTreeSet<MobId>> {
        self.inner.known_mobs().await
    }

    async fn drop_ended_mobs(
        &self,
        ended: &BTreeSet<MobId>,
        now_ms: u64,
    ) -> Result<Vec<SubscriptionId>> {
        self.inner.drop_ended_mobs(ended, now_ms).await
    }

    async fn member_route(&self, member: &Member) -> Result<Option<MemberRouteRecord>> {
        self.inner.member_route(member).await
    }

    async fn unbind_member_route(
        &self,
        member: &Member,
        end: SubscriptionEnd,
        now_ms: u64,
    ) -> Result<Vec<SubscriptionId>> {
        self.inner.unbind_member_route(member, end, now_ms).await
    }

    async fn child_member_routes(&self) -> Result<Vec<(Member, MemberRouteRecord)>> {
        self.inner.child_member_routes().await
    }

    async fn end_member_before(
        &self,
        member: &Member,
        cursor: u64,
        end: SubscriptionEnd,
        now_ms: u64,
    ) -> Result<Vec<SubscriptionId>> {
        self.inner
            .end_member_before(member, cursor, end, now_ms)
            .await
    }

    async fn member_mob_check_from(&self, member: &Member) -> Result<Option<u64>> {
        self.inner.member_mob_check_from(member).await
    }

    async fn record_member_mob_check(&self, member: &Member, through: u64) -> Result<()> {
        self.inner.record_member_mob_check(member, through).await
    }

    async fn lineage(&self) -> Result<Vec<MobLineage>> {
        self.inner.lineage().await
    }

    async fn record_lineage(&self, lineage: &MobLineage, now_ms: u64) -> Result<bool> {
        self.inner.record_lineage(lineage, now_ms).await
    }

    async fn end_lineage(&self, mobs: &BTreeSet<MobId>, now_ms: u64) -> Result<()> {
        self.inner.end_lineage(mobs, now_ms).await
    }

    async fn backlogs(&self, limit: usize) -> Result<Vec<MemberBacklog>> {
        self.inner.backlogs(limit).await
    }

    async fn backlogs_at(&self, limit: usize, now_ms: u64) -> Result<Vec<MemberBacklog>> {
        self.backlog_reads.fetch_add(1, Ordering::SeqCst);
        assert!(
            !self.panic_next_round.swap(false, Ordering::SeqCst),
            "injected fault inside a dispatcher round"
        );
        self.inner.backlogs_at(limit, now_ms).await
    }

    async fn seal_batch(
        &self,
        member: &Member,
        batch_id: &BatchId,
        policy: &SealingPolicy,
        now_ms: u64,
    ) -> Result<Option<Batch>> {
        self.inner
            .seal_batch(member, batch_id, policy, now_ms)
            .await
    }

    async fn batch(&self, id: &BatchId) -> Result<Option<Batch>> {
        self.inner.batch(id).await
    }

    async fn batches(&self, query: &BatchQuery) -> Result<Vec<Batch>> {
        self.inner.batches(query).await
    }

    async fn unsettled_batches(
        &self,
        after: Option<&BatchPosition>,
        limit: usize,
    ) -> Result<Vec<Batch>> {
        self.inner.unsettled_batches(after, limit).await
    }

    async fn newest_unsettled_batch(&self) -> Result<Option<BatchPosition>> {
        self.inner.newest_unsettled_batch().await
    }

    async fn transition_batch(
        &self,
        id: &BatchId,
        expected: BatchStateTag,
        event: BatchEvent,
        now_ms: u64,
    ) -> Result<Batch> {
        if self
            .inner
            .batch(id)
            .await?
            .is_some_and(|batch| self.failing.as_ref() == Some(&batch.member))
        {
            self.failures.fetch_add(1, Ordering::SeqCst);
            return Err(AnnouncementStoreError::Storage(
                "injected transition failure".into(),
            ));
        }
        self.inner
            .transition_batch(id, expected, event, now_ms)
            .await
    }

    async fn late_reads_due(&self, now_ms: u64, limit: usize) -> Result<Vec<Batch>> {
        self.inner.late_reads_due(now_ms, limit).await
    }

    async fn record_late_read(
        &self,
        id: &BatchId,
        late: Option<LateOutcome>,
        next_read_at_ms: Option<u64>,
    ) -> Result<Batch> {
        self.inner.record_late_read(id, late, next_read_at_ms).await
    }

    async fn expire(
        &self,
        now_ms: u64,
        retention_ms: u64,
        quiet_ms: u64,
        limit: usize,
    ) -> Result<ExpiryReport> {
        self.inner
            .expire(now_ms, retention_ms, quiet_ms, limit)
            .await
    }

    async fn prune(&self, now_ms: u64, retention_ms: u64, limit: usize) -> Result<PruneReport> {
        self.pass_times
            .lock()
            .unwrap()
            .push(std::time::Instant::now());
        self.prune_passes.fetch_add(1, Ordering::SeqCst);
        let mut report = self.inner.prune(now_ms, retention_ms, limit).await?;
        if self
            .more_passes
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                left.checked_sub(1)
            })
            .is_ok()
        {
            report.more = true;
        }
        Ok(report)
    }
}

/// A batch the round cannot advance (here, its transitions fail in the
/// store) is reported and skipped; it does not end the round for every
/// other member.
#[tokio::test]
async fn one_failing_batch_does_not_end_the_round() {
    let (dir, inner) = temp_store();
    let store = Arc::new(FaultyStore::new(inner, Some("agent:a")));
    let mut native = ScriptedNative::new();
    native.known = ["agent:a".to_string(), "agent:b".to_string()].into();
    let native = Arc::new(native);
    let clock = Arc::new(AtomicU64::new(1_000));
    let reader = Arc::clone(&clock);
    let service = AnnouncementService::with_clock(
        Arc::clone(&store) as Arc<dyn AnnouncementStore>,
        Arc::clone(&native) as Arc<dyn NativeDelivery>,
        AnnouncementConfig {
            policy: policy(),
            ..AnnouncementConfig::default()
        },
        Arc::new(move || reader.load(Ordering::SeqCst)),
    )
    .unwrap();
    for (index, agent) in ["agent:a", "agent:b"].iter().enumerate() {
        let topic = format!("t{index}");
        service
            .put_subscription(
                &op(&format!("o{index}")),
                put(
                    &format!("s{index}"),
                    agent,
                    &[topic.as_str()],
                    StartFrontier::Now,
                ),
            )
            .await
            .unwrap();
        service
            .announce(announce_request(
                "events",
                &format!("k{index}"),
                &[topic.as_str()],
                "x",
            ))
            .await
            .unwrap();
    }
    clock.store(100_000, Ordering::SeqCst);
    let report = service
        .run_round()
        .await
        .expect("a batch that cannot be advanced does not end the round");
    assert_eq!(report.sealed, 2, "both backlogs are sealed");
    let delivered = service
        .deliveries(&BatchQuery {
            member: Some(member("agent:b")),
            limit: 10,
            ..BatchQuery::default()
        })
        .await
        .unwrap();
    assert_eq!(
        delivered[0].state.tag(),
        BatchStateTag::Delivered,
        "the other member's batch is delivered in the same round"
    );

    // The next round walks the failing batch first; it still seals and
    // delivers the other member's new announcement.
    service
        .announce(announce_request("events", "k1-more", &["t1"], "y"))
        .await
        .unwrap();
    clock.store(200_000, Ordering::SeqCst);
    let report = service
        .run_round()
        .await
        .expect("a batch the walk cannot advance does not end the round");
    assert_eq!(report.sealed, 1, "the other member's new backlog is sealed");
    let delivered = service
        .deliveries(&BatchQuery {
            member: Some(member("agent:b")),
            limit: 10,
            ..BatchQuery::default()
        })
        .await
        .unwrap();
    assert_eq!(delivered.len(), 2);
    assert!(
        delivered
            .iter()
            .all(|batch| batch.state.tag() == BatchStateTag::Delivered),
        "{delivered:?}"
    );
    drop(dir);
}

/// More than one pass of retention work, most of it in the mob_id that
/// sorts first: the dispatcher keeps passing until everything the window
/// allows is gone, in both mobs, without waiting for the next interval.
#[tokio::test]
async fn retention_keeps_passing_while_a_pass_hits_its_limit() {
    let harness = harness_with(ScriptedNative::new());
    for index in 0..1_200 {
        harness
            .service
            .announce(announce_request(
                "a-mob",
                &format!("a-{index}"),
                &["ios"],
                "old",
            ))
            .await
            .unwrap();
    }
    for index in 0..5 {
        harness
            .service
            .announce(announce_request(
                "b-mob",
                &format!("b-{index}"),
                &["ios"],
                "old",
            ))
            .await
            .unwrap();
    }
    harness.clock.store(
        1_000 + super::owner::DEFAULT_RETENTION_MS + 1,
        Ordering::SeqCst,
    );
    let gone = |mob_id: &'static str, key: &'static str| {
        let store = Arc::clone(&harness.store);
        async move {
            store
                .announcement(&mob(mob_id), &SourceKey::new(key).unwrap())
                .await
                .unwrap()
                .is_none()
        }
    };
    let dispatcher = harness.service.start();
    let caught_up = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        while !(gone("a-mob", "a-1199").await && gone("b-mob", "b-4").await) {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await;
    harness.service.stop();
    dispatcher.await.unwrap();
    assert!(
        caught_up.is_ok(),
        "liveness: retention passes repeat while a pass hits its limit, so the \
         backlog in both mobs is gone without waiting an interval per pass"
    );
}

#[test]
fn a_retention_window_below_the_floor_is_refused() {
    let options =
        |retention_ms: u64| serde_json::json!({ "enabled": true, "retention_ms": retention_ms });
    assert!(super::parse_runtime_options(&options(0)).is_err());
    assert!(super::parse_runtime_options(&options(3_599_999)).is_err());
    assert!(super::parse_runtime_options(&options(3_600_000)).is_ok());
    let (_dir, store) = temp_store();
    let refused = AnnouncementService::new(
        Arc::new(store) as Arc<dyn AnnouncementStore>,
        Arc::new(ScriptedNative::new()) as Arc<dyn NativeDelivery>,
        AnnouncementConfig {
            retention_ms: 0,
            ..AnnouncementConfig::default()
        },
    );
    assert!(
        refused.is_err(),
        "a window that prunes receipts a retry still needs is refused"
    );
}

/// A batch the native owner has committed but not admitted is read again
/// only once its retry time has passed, not in every round.
#[tokio::test]
async fn a_batch_is_not_read_again_before_its_retry_time() {
    let mut native = ScriptedNative::new();
    native.apply_on_commit = false;
    let harness = harness_with(native);
    harness.subscribe("s1").await;
    harness.announce_burst(1).await;
    harness.clock.store(100_000, Ordering::SeqCst);
    harness.service.run_round().await.unwrap();
    let reads = || harness.native.ledger.lock().unwrap().status_reads;
    assert_eq!(reads(), 1, "the batch is read once after it commits");
    harness.service.run_round().await.unwrap();
    assert_eq!(
        reads(),
        1,
        "a round before the batch's retry time does not read it again"
    );
    harness.clock.store(
        100_000 + AnnouncementConfig::default().settle_check_max_ms + 1,
        Ordering::SeqCst,
    );
    harness.service.run_round().await.unwrap();
    assert_eq!(reads(), 2, "after its retry time it is read again");
}

/// A batch whose transitions keep failing is retried at the slowest settle
/// pace, not in every round.
#[tokio::test]
async fn a_failing_batch_is_retried_at_the_slow_pace() {
    let (dir, inner) = temp_store();
    let store = Arc::new(FaultyStore::new(inner, Some("agent:a")));
    let clock = Arc::new(AtomicU64::new(1_000));
    let reader = Arc::clone(&clock);
    let config = AnnouncementConfig {
        policy: policy(),
        ..AnnouncementConfig::default()
    };
    let service = AnnouncementService::with_clock(
        Arc::clone(&store) as Arc<dyn AnnouncementStore>,
        Arc::new(ScriptedNative::new()) as Arc<dyn NativeDelivery>,
        config.clone(),
        Arc::new(move || reader.load(Ordering::SeqCst)),
    )
    .unwrap();
    service
        .put_subscription(
            &op("o1"),
            put("s1", "agent:a", &["ios"], StartFrontier::Now),
        )
        .await
        .unwrap();
    service
        .announce(announce_request("payments", "k1", &["ios"], "x"))
        .await
        .unwrap();
    clock.store(100_000, Ordering::SeqCst);
    let first = service.run_round().await.unwrap();
    assert_eq!(store.failures.load(Ordering::SeqCst), 1);
    assert!(
        first
            .next_due_ms
            .is_some_and(|due| due >= 100_000 + config.settle_check_max_ms),
        "a failure is retried at the slowest pace: {first:?}"
    );
    clock.store(100_000 + config.settle_check_min_ms, Ordering::SeqCst);
    service.run_round().await.unwrap();
    assert_eq!(
        store.failures.load(Ordering::SeqCst),
        1,
        "a round before the retry time leaves the failing batch alone"
    );
    clock.store(100_000 + config.settle_check_max_ms + 1, Ordering::SeqCst);
    service.run_round().await.unwrap();
    assert_eq!(store.failures.load(Ordering::SeqCst), 2);
    drop(dir);
}

/// Every round seals 64 new batches that stay unsettled, so the walk's
/// windows stay full and never wrap on their own. A lap is still bounded:
/// the walk comes back to an older batch the native owner has since
/// admitted.
#[tokio::test]
async fn a_lap_of_full_windows_still_comes_back_to_an_older_batch() {
    const ROUNDS: usize = 5;
    const FRESH: usize = 64;
    let mut native = ScriptedNative::new();
    native.apply_on_commit = false;
    native.known =
        std::iter::once("agent:held".to_string())
            .chain((1..=ROUNDS).flat_map(|round| {
                (0..FRESH).map(move |index| format!("agent:f{round}-{index:02}"))
            }))
            .collect();
    let harness = harness_with(native);
    let subscribe_and_announce = |agent: String| {
        let service = harness.service.clone();
        async move {
            let topic = agent.replace(':', "-");
            service
                .put_subscription(
                    &op(&format!("o-{topic}")),
                    put(
                        &format!("s-{topic}"),
                        &agent,
                        &[topic.as_str()],
                        StartFrontier::Now,
                    ),
                )
                .await
                .unwrap();
            service
                .announce(announce_request("events", &topic, &[topic.as_str()], "x"))
                .await
                .unwrap();
        }
    };
    // The held member's batch is sealed first, alone, so it is the oldest
    // unsettled batch: the native owner commits it and does not admit it.
    subscribe_and_announce("agent:held".to_string()).await;
    let mut now = 62_000_u64;
    harness.clock.store(now, Ordering::SeqCst);
    harness.service.run_round().await.unwrap();
    let mut held_batch = None;
    for round in 1..=ROUNDS {
        for index in 0..FRESH {
            subscribe_and_announce(format!("agent:f{round}-{index:02}")).await;
        }
        now += 61_000;
        harness.clock.store(now, Ordering::SeqCst);
        harness.service.run_round().await.unwrap();
        if round == 2 {
            // The native owner admits the held member's batch now.
            let held = harness
                .service
                .deliveries(&BatchQuery {
                    member: Some(member("agent:held")),
                    limit: 10,
                    ..BatchQuery::default()
                })
                .await
                .unwrap()
                .remove(0);
            harness
                .native
                .ledger
                .lock()
                .unwrap()
                .bindings
                .get_mut(held.id.as_str())
                .expect("committed")
                .2 = true;
            held_batch = Some(held.id);
        }
    }
    let held = harness
        .service
        .observe_delivery(&held_batch.expect("round 2 ran"))
        .await
        .unwrap();
    assert_eq!(
        held.batch.state.tag(),
        BatchStateTag::Delivered,
        "a lap is bounded, so the walk comes back to the older batch"
    );
}

/// The walk keeps when to read a batch again, and when it last reported a
/// failing one, only while the batch is unsettled. A batch a reconcile
/// settles leaves at once; one the store settles (its last subscription
/// went) leaves at the next window over it. Churn cannot grow the walk.
#[tokio::test]
async fn the_walk_drops_batches_once_they_settle() {
    let (dir, inner) = temp_store();
    let store = Arc::new(FaultyStore::new(inner, Some("agent:b")));
    let mut native = ScriptedNative::new();
    native.apply_on_commit = false;
    native.known = ["agent:a", "agent:b", "agent:c"].map(String::from).into();
    let native = Arc::new(native);
    let clock = Arc::new(AtomicU64::new(1_000));
    let reader = Arc::clone(&clock);
    let service = AnnouncementService::with_clock(
        Arc::clone(&store) as Arc<dyn AnnouncementStore>,
        Arc::clone(&native) as Arc<dyn NativeDelivery>,
        AnnouncementConfig {
            policy: policy(),
            ..AnnouncementConfig::default()
        },
        Arc::new(move || reader.load(Ordering::SeqCst)),
    )
    .unwrap();
    for name in ["a", "b", "c"] {
        service
            .put_subscription(
                &op(&format!("o-{name}")),
                put(
                    &format!("s-{name}"),
                    &format!("agent:{name}"),
                    &[name],
                    StartFrontier::Now,
                ),
            )
            .await
            .unwrap();
        service
            .announce(announce_request("events", name, &[name], "x"))
            .await
            .unwrap();
    }
    clock.store(100_000, Ordering::SeqCst);
    let report = service.run_round().await.unwrap();
    assert_eq!(report.sealed, 3);
    assert_eq!(
        service.walk_entries().await,
        (3, 1),
        "a and c wait for their retry time; b failed, is retried later and was reported"
    );
    let batch_of = |agent: &'static str| {
        let service = service.clone();
        async move {
            service
                .deliveries(&BatchQuery {
                    member: Some(member(agent)),
                    limit: 10,
                    ..BatchQuery::default()
                })
                .await
                .unwrap()
                .remove(0)
        }
    };

    // The native owner admits a's batch and a reconcile settles it.
    let a = batch_of("agent:a").await;
    native
        .ledger
        .lock()
        .unwrap()
        .bindings
        .get_mut(a.id.as_str())
        .expect("committed")
        .2 = true;
    let reconciled = service.reconcile_delivery(&a.id).await.unwrap();
    assert_eq!(reconciled.batch.state.tag(), BatchStateTag::Delivered);
    assert_eq!(
        service.walk_entries().await,
        (2, 1),
        "a batch a reconcile settles leaves the walk at once"
    );

    // b's only subscription goes, and the store cancels its sealed batch.
    service
        .change_subscription(
            &op("o-b-cancel"),
            &sid("s-b"),
            1,
            SubscriptionChange::Cancel,
        )
        .await
        .unwrap();
    assert_eq!(
        batch_of("agent:b").await.state.tag(),
        BatchStateTag::Cancelled
    );
    // Nothing is due yet; the round's window over the unsettled batches no
    // longer holds b's.
    service.run_round().await.unwrap();
    assert_eq!(
        service.walk_entries().await,
        (1, 0),
        "a batch the store settled leaves the walk at the next window over it"
    );
    drop(dir);
}

/// A long retention backlog runs pass after pass with no pause, but the
/// rounds keep their own pace: a pass that has more work behind it does not
/// bring a round with it.
#[tokio::test]
async fn retention_catch_up_keeps_the_round_pace() {
    const PASSES: u64 = 40;
    let (dir, inner) = temp_store();
    let store = Arc::new(FaultyStore::new(inner, None));
    store.more_passes.store(PASSES, Ordering::SeqCst);
    let service = AnnouncementService::with_clock(
        Arc::clone(&store) as Arc<dyn AnnouncementStore>,
        Arc::new(ScriptedNative::new()) as Arc<dyn NativeDelivery>,
        AnnouncementConfig::default(),
        // Past the first retention interval, so the dispatcher's first
        // iteration runs a pass.
        Arc::new(|| 10_000_000),
    )
    .unwrap();
    let dispatcher = service.start();
    let caught_up = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        while store.prune_passes.load(Ordering::SeqCst) <= PASSES {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await;
    service.stop();
    dispatcher.await.unwrap();
    assert!(caught_up.is_ok(), "the passes run back to back");
    assert_eq!(
        store.backlog_reads.load(Ordering::SeqCst),
        1,
        "cadence: one round over {} catch-up passes, since none came due",
        PASSES + 1
    );
    drop(dir);
}

#[tokio::test]
async fn governed_runtimes_and_unknown_members_are_refused_typed() {
    let mut native = ScriptedNative::new();
    native.governed = true;
    let harness = harness_with(native);
    assert_eq!(
        harness
            .service
            .put_subscription(
                &op("o1"),
                put("s1", "agent:a", &["ios"], StartFrontier::Now)
            )
            .await,
        Err(AnnouncementError::GovernedRuntime)
    );
    let harness = harness_with(ScriptedNative::new());
    assert_eq!(
        harness
            .service
            .put_subscription(
                &op("o1"),
                put("s1", "agent:z", &["ios"], StartFrontier::Now)
            )
            .await,
        Err(AnnouncementError::UnknownMember(member("agent:z")))
    );
}

#[tokio::test]
async fn a_peer_member_needs_a_usable_peer_not_a_local_identity() {
    let mut native = ScriptedNative::new();
    // This gateway's own runtime refuses host continuations; the peer
    // answers for its own.
    native.governed = true;
    let harness = harness_with(native);
    let remote = peer_member("agent:x", "mob-b");
    let subscription = harness
        .service
        .put_subscription(&op("o1"), put_for("s1", remote.clone(), &["ios"]))
        .await
        .unwrap();
    assert_eq!(subscription.member, remote);
    assert!(matches!(
        harness
            .service
            .put_subscription(
                &op("o2"),
                put_for("s2", peer_member("agent:x", "mob-z"), &["ios"])
            )
            .await,
        Err(AnnouncementError::UnusablePeer(_))
    ));
}

#[tokio::test]
async fn a_remote_burst_is_delivered_in_batches_that_fit_the_wire() {
    let harness = harness_with(ScriptedNative::new());
    let remote = peer_member("agent:x", "mob-b");
    harness
        .service
        .put_subscription(&op("o1"), put_for("s1", remote.clone(), &["ios"]))
        .await
        .unwrap();
    let body = "x".repeat(2_000);
    for index in 0..40 {
        harness
            .service
            .announce(announce_request(
                "payments",
                &format!("k{index}"),
                &["ios"],
                &body,
            ))
            .await
            .unwrap();
    }
    let mut delivered = 0;
    let mut batches = 0;
    for round in 0..10_u64 {
        harness
            .clock
            .store(10_000_000 + round * 60_000, Ordering::SeqCst);
        harness.service.run_round().await.unwrap();
        let all = harness
            .service
            .deliveries(&BatchQuery {
                member: Some(remote.clone()),
                limit: 100,
                ..BatchQuery::default()
            })
            .await
            .unwrap();
        delivered = all.iter().map(|batch| batch.items.len()).sum::<usize>();
        batches = all.len();
        for batch in &all {
            assert!(wire_len(&batch.body) <= MAX_REMOTE_BODY_WIRE_BYTES);
            assert_eq!(batch.state.tag(), BatchStateTag::Delivered);
        }
        if delivered == 40 {
            break;
        }
    }
    assert_eq!(
        delivered, 40,
        "every announcement is delivered exactly once"
    );
    assert!(batches >= 2, "one request cannot carry the whole burst");
}

#[tokio::test]
async fn the_rpc_surface_is_strict_and_typed() {
    use super::rpc::{
        ANNOUNCEMENTS_CONFLICT_CODE, ANNOUNCEMENTS_FRONTIER_CODE, ANNOUNCEMENTS_NOT_FOUND_CODE,
        ANNOUNCEMENTS_REFUSED_CODE, ANNOUNCEMENTS_UNAVAILABLE_CODE, handle_announcement_method,
    };
    let own = mob("payments");
    let unavailable = handle_announcement_method(
        None,
        &own,
        "meta/mob_announcements/announce",
        &serde_json::json!({}),
    )
    .await
    .unwrap_err();
    assert_eq!(unavailable.code, ANNOUNCEMENTS_UNAVAILABLE_CODE);

    let harness = harness_with(ScriptedNative::new());
    let service = Some(&harness.service);
    let call = |method: &'static str, params: serde_json::Value| {
        let own = own.clone();
        async move { handle_announcement_method(service, &own, method, &params).await }
    };
    let unknown_field = call(
        "meta/mob_announcements/announce",
        serde_json::json!({ "source_key": "k", "topics": ["t"], "body": "b", "extra": 1 }),
    )
    .await
    .unwrap_err();
    assert_eq!(unknown_field.code, -32602);
    let as_member = call(
        "meta/mob_announcements/announce",
        serde_json::json!({
            "source_key": "k", "topics": ["t"], "body": "b",
            "announced_by": { "kind": "member", "identity": "agent:a" }
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(
        as_member.code, -32602,
        "a host call cannot announce as a member"
    );
    let announced = call(
        "meta/mob_announcements/announce",
        serde_json::json!({ "source_key": "k", "topics": ["t"], "body": "b" }),
    )
    .await
    .unwrap();
    assert_eq!(announced["announcement"]["sequence"], 1);
    assert_eq!(
        announced["announcement"]["mob_id"], "payments",
        "a call that names no mob announces into this host's mob"
    );
    assert_eq!(
        announced["announcement"]["announced_by"],
        serde_json::json!({ "kind": "host" })
    );
    assert_eq!(announced["captured"], 0);
    assert!(
        announced.get("captured_for").is_none(),
        "an announce result counts the subscriptions that captured it, never names them"
    );
    let elsewhere = call(
        "meta/mob_announcements/announce",
        serde_json::json!({ "mob_id": "nowhere", "source_key": "k", "topics": ["t"], "body": "b" }),
    )
    .await
    .unwrap_err();
    assert_eq!(elsewhere.code, ANNOUNCEMENTS_NOT_FOUND_CODE);
    assert_eq!(
        elsewhere.data.unwrap(),
        serde_json::json!({ "kind": "unknown_mob", "mobs": ["nowhere"] })
    );
    let conflict = call(
        "meta/mob_announcements/announce",
        serde_json::json!({ "source_key": "k", "topics": ["t"], "body": "c" }),
    )
    .await
    .unwrap_err();
    assert_eq!(conflict.code, ANNOUNCEMENTS_CONFLICT_CODE);
    assert_eq!(conflict.data.unwrap()["kind"], "announcement_conflict");
    let subscribed = call(
        "meta/mob_announcements/subscriptions/put",
        serde_json::json!({
            "operation_id": "op-1",
            "subscription_id": "sub-1",
            "member": { "mob_id": "payments", "identity": "agent:a" },
            "topics": ["t"]
        }),
    )
    .await
    .unwrap();
    assert_eq!(subscribed["subscription"]["revision"], 1);
    assert_eq!(subscribed["subscription"]["state"], "active");
    assert_eq!(
        subscribed["subscription"]["selector"]["mobs"],
        serde_json::json!(["payments"]),
        "a subscription that names no mob takes this host's mob"
    );
    let remote = serde_json::json!({ "mob_id": "mob-b", "identity": "agent:x" });
    let subscribed = call(
        "meta/mob_announcements/subscriptions/put",
        serde_json::json!({
            "operation_id": "op-2",
            "subscription_id": "sub-2",
            "member": remote,
            "topics": ["t"],
            "mobs": ["payments", "builds"]
        }),
    )
    .await
    .unwrap();
    assert_eq!(subscribed["subscription"]["member"], remote);
    let listed = call(
        "meta/mob_announcements/subscriptions/list",
        serde_json::json!({ "member": remote }),
    )
    .await
    .unwrap();
    assert_eq!(listed["subscriptions"].as_array().map(Vec::len), Some(1));
    let unusable = call(
        "meta/mob_announcements/subscriptions/put",
        serde_json::json!({
            "operation_id": "op-3",
            "subscription_id": "sub-3",
            "member": { "mob_id": "mob-z", "identity": "agent:x" },
            "topics": ["t"]
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(unusable.code, ANNOUNCEMENTS_REFUSED_CODE);
    assert_eq!(unusable.data.unwrap()["kind"], "unusable_peer");
    let child = call(
        "meta/mob_announcements/subscriptions/put",
        serde_json::json!({
            "operation_id": "op-3b",
            "subscription_id": "sub-3b",
            "member": { "mob_id": "builds", "identity": "worker-1" },
            "topics": ["t"]
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(child.code, ANNOUNCEMENTS_REFUSED_CODE);
    assert_eq!(
        child.data.unwrap()["kind"],
        "unknown_member",
        "a member of a mob a member created is checked against that mob's roster"
    );
    let malformed = call(
        "meta/mob_announcements/subscriptions/put",
        serde_json::json!({
            "operation_id": "op-4",
            "subscription_id": "sub-4",
            "member": { "mob_id": "mob-b", "identity": "agent:x", "host": "mob-b" },
            "topics": ["t"]
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(malformed.code, -32602, "unknown member fields are refused");
    let bare = call(
        "meta/mob_announcements/subscriptions/put",
        serde_json::json!({
            "operation_id": "op-4b",
            "subscription_id": "sub-4b",
            "member": "agent:a",
            "topics": ["t"]
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(bare.code, -32602, "a member names its mob");
    let beyond = call(
        "meta/mob_announcements/subscriptions/put",
        serde_json::json!({
            "operation_id": "op-5",
            "subscription_id": "sub-5",
            "member": { "mob_id": "payments", "identity": "agent:a" },
            "topics": ["t"],
            "start": { "kind": "after", "frontier": { "payments": 2 } }
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(beyond.code, ANNOUNCEMENTS_FRONTIER_CODE);
    assert_eq!(
        beyond.data.unwrap(),
        serde_json::json!({ "kind": "frontier_beyond_head", "mob_id": "payments", "head": 1 })
    );
}

/// A member that is not registered here, or not served by this gateway's
/// runtime, may be registered or served again: its deliveries wait, typed,
/// and nothing ends. Once it is served again it is delivered to.
#[tokio::test]
async fn a_registry_miss_or_a_placement_waits_and_ends_nothing() {
    for cause in [
        SuspendCause::MemberUnknown,
        SuspendCause::MemberNotServedHere {
            cause: Some(NotServedHereCause::PlacedOnHost),
        },
        SuspendCause::MemberNotServedHere {
            cause: Some(NotServedHereCause::ExternalBinding),
        },
    ] {
        let harness = harness_with(ScriptedNative::new());
        harness.subscribe("s1").await;
        harness.announce_burst(3).await;
        harness
            .native
            .waiting
            .lock()
            .unwrap()
            .insert("agent:a".into(), cause.clone());
        harness.clock.store(10_000_000, Ordering::SeqCst);
        let report = harness.service.run_round().await.unwrap();
        assert_eq!(report.sealed, 1);
        assert_eq!(
            harness.only_batch().await.state,
            BatchState::Sealed,
            "nothing was submitted, and the batch waits"
        );
        assert_eq!(harness.native.submits(), 0);
        let subscription = harness
            .service
            .subscription(&sid("s1"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(subscription.state, SubscriptionState::Active, "{cause}");
        assert_eq!(subscription.ended, None);
        let marker = harness
            .service
            .suspend_marker(&member("agent:a"))
            .expect("the wait is marked");
        assert_eq!(marker.cause, cause);
        assert_eq!(marker.evidence, EvidenceClass::Local);
        // Served again: the member is delivered to.
        harness.native.waiting.lock().unwrap().clear();
        harness.round_after_pause().await;
        assert_eq!(harness.native.submits(), 1);
        assert!(matches!(
            harness.only_batch().await.state,
            BatchState::Committed { .. } | BatchState::Delivered { .. }
        ));
        assert_eq!(harness.service.suspend_marker(&member("agent:a")), None);
    }
}

/// A submission answered that nothing was committed returns the batch to
/// owed, marked with the typed reason and its evidence, and ends nothing.
/// A later attempt submits it again; past the retention window it expires
/// as an owed batch does, never as an unconfirmed delivery.
#[tokio::test]
async fn a_signed_answer_that_nothing_was_committed_leaves_the_batch_owed() {
    let harness = harness_with(ScriptedNative::new());
    harness.subscribe("s1").await;
    harness.announce_burst(2).await;
    let cause = SuspendCause::PeerNotServing {
        cause: crate::runtime::keyed_inject::NotServedCause::RuntimeDisabled,
    };
    harness.native.ledger.lock().unwrap().refuse = Some((cause.clone(), EvidenceClass::Verified));
    harness.clock.store(10_000_000, Ordering::SeqCst);
    harness.service.run_round().await.unwrap();
    assert_eq!(harness.native.submits(), 1);
    assert_eq!(harness.only_batch().await.state, BatchState::Sealed);
    let marker = harness
        .service
        .suspend_marker(&member("agent:a"))
        .expect("the refusal is marked");
    assert_eq!(marker.cause, cause);
    assert_eq!(marker.evidence, EvidenceClass::Verified);
    let subscription = harness
        .service
        .subscription(&sid("s1"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(subscription.state, SubscriptionState::Active);
    // A later attempt submits it again.
    harness.round_after_pause().await;
    assert_eq!(harness.native.submits(), 2);
    assert_eq!(harness.only_batch().await.state, BatchState::Sealed);
    // Past the retention window, the owed batch expires.
    let retention = harness.service.config().retention_ms;
    harness
        .clock
        .fetch_add(retention.saturating_mul(2), Ordering::SeqCst);
    harness.round_after_pause().await;
    assert_eq!(harness.only_batch().await.state, BatchState::Expired);
}

/// A runtime that governs work authority refuses host continuations; that
/// is configuration that can change, so the batch waits and is delivered
/// once the runtime no longer refuses.
#[tokio::test]
async fn a_governed_local_runtime_waits_and_delivers_once_it_no_longer_refuses() {
    let harness = harness_with(ScriptedNative::new());
    harness.subscribe("s1").await;
    harness.announce_burst(1).await;
    harness.native.ledger.lock().unwrap().refuse =
        Some((SuspendCause::Governed, EvidenceClass::Local));
    harness.clock.store(10_000_000, Ordering::SeqCst);
    harness.service.run_round().await.unwrap();
    assert_eq!(harness.only_batch().await.state, BatchState::Sealed);
    assert_eq!(
        harness
            .service
            .suspend_marker(&member("agent:a"))
            .map(|marker| marker.cause),
        Some(SuspendCause::Governed)
    );
    harness.native.ledger.lock().unwrap().refuse = None;
    harness.round_after_pause().await;
    assert!(matches!(
        harness.only_batch().await.state,
        BatchState::Committed { .. } | BatchState::Delivered { .. }
    ));
}

/// A status read the native owner answers as inconsistent blocks the batch,
/// typed; one it cannot answer now leaves the batch as it was, marked.
#[tokio::test]
async fn a_status_read_blocks_only_on_typed_evidence() {
    let harness = harness_with(ScriptedNative::new());
    harness.subscribe("s1").await;
    harness.announce_burst(1).await;
    harness.native.ledger.lock().unwrap().lose_next_ack = true;
    harness.clock.store(10_000_000, Ordering::SeqCst);
    harness.service.run_round().await.unwrap();
    assert!(matches!(
        harness.only_batch().await.state,
        BatchState::Submitting { .. }
    ));
    harness.native.ledger.lock().unwrap().status_error = Some(NativeStatusError::Unavailable {
        cause: SuspendCause::NativeUnavailable,
        evidence: EvidenceClass::Local,
        detail: "not now".into(),
    });
    harness.round_after_pause().await;
    assert!(
        matches!(
            harness.only_batch().await.state,
            BatchState::Submitting { .. }
        ),
        "an unreadable status implies nothing"
    );
    harness.native.ledger.lock().unwrap().status_error = Some(NativeStatusError::Blocked {
        cause: BlockCause::NativeInconsistent,
        detail: "two bindings".into(),
    });
    harness.round_after_pause().await;
    match harness.only_batch().await.state {
        BatchState::Blocked { cause } => {
            assert!(cause.starts_with("native_inconsistent"), "{cause}");
        }
        other => panic!("expected a typed block, got {other:?}"),
    }
}

#[tokio::test]
async fn a_final_refusal_from_the_member_owner_ends_its_subscriptions() {
    let harness = harness_with(ScriptedNative::new());
    let remote = peer_member("agent:x", "mob-b");
    harness
        .service
        .put_subscription(&op("o1"), put_for("s1", remote.clone(), &["ios"]))
        .await
        .unwrap();
    harness.announce_burst(2).await;
    let departure = MemberDeparture::from_signed_departure(mob("mob-b"));
    *harness.native.submit_gone.lock().unwrap() = Some(departure.clone());
    harness.clock.store(10_000_000, Ordering::SeqCst);
    harness.service.run_round().await.unwrap();
    let batches = harness
        .service
        .deliveries(&BatchQuery {
            member: Some(remote),
            limit: 10,
            ..BatchQuery::default()
        })
        .await
        .unwrap();
    assert_eq!(batches.len(), 1);
    assert!(
        matches!(&batches[0].state, BatchState::Refused { receipt: None, .. }),
        "nothing was committed: {:?}",
        batches[0].state
    );
    let subscription = harness
        .service
        .subscription(&sid("s1"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(subscription.state, SubscriptionState::Ended);
    assert_eq!(
        subscription.ended,
        Some(SubscriptionEnd::MemberDeparted { departure })
    );
    harness.round_after_pause().await;
    assert_eq!(harness.native.submits(), 1, "a final answer is not retried");
}

#[tokio::test]
async fn destroyed_mobs_are_reconciled_from_the_mob_owner() {
    let harness = harness_with(ScriptedNative::new());
    let mut both = put("s1", "agent:a", &["ios"], StartFrontier::Now);
    both.selector.mobs = [mob("payments"), mob("builds")].into();
    harness
        .service
        .put_subscription(&op("o1"), both)
        .await
        .unwrap();
    assert!(
        harness.service.reconcile_mobs().await.unwrap().is_empty(),
        "nothing is destroyed yet"
    );
    harness
        .native
        .destroyed
        .lock()
        .unwrap()
        .insert("builds".into());
    assert_eq!(
        harness.service.reconcile_mobs().await.unwrap(),
        vec![sid("s1")]
    );
    let subscription = harness
        .service
        .subscription(&sid("s1"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(subscription.selector.mobs, [mob("payments")].into());
    assert_eq!(subscription.ended_mobs, [mob("builds")].into());
    assert!(
        harness.service.reconcile_mobs().await.unwrap().is_empty(),
        "a reconcile changes nothing twice"
    );
    assert_eq!(
        harness
            .service
            .announce(announce_request("builds", "k", &["ios"], "x"))
            .await,
        Err(AnnouncementError::UnknownMobs(vec![mob("builds")]))
    );
    let mut gone = put("s2", "agent:a", &["ios"], StartFrontier::Now);
    gone.selector.mobs = [mob("builds")].into();
    assert_eq!(
        harness.service.put_subscription(&op("o2"), gone).await,
        Err(AnnouncementError::UnknownMobs(vec![mob("builds")]))
    );
}

#[tokio::test]
async fn members_this_runtime_cannot_deliver_to_are_refused_at_put() {
    let mut native = ScriptedNative::new();
    native.known.insert("agent:placed".into());
    native.unusable.insert("agent:placed".into());
    let harness = harness_with(native);
    assert!(matches!(
        harness
            .service
            .put_subscription(
                &op("o1"),
                put("s1", "agent:placed", &["ios"], StartFrontier::Now)
            )
            .await,
        Err(AnnouncementError::UnusableMember(_))
    ));
    assert!(
        matches!(
            harness
                .service
                .put_subscription(
                    &op("o2"),
                    put_for("s2", peer_member("worker-1", "builds"), &["ios"])
                )
                .await,
            Err(AnnouncementError::UnknownMember(_))
        ),
        "a member of a mob a member created is checked against that mob's roster"
    );
}

#[test]
fn storage_maintenance_fences_the_announcement_database() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir
        .path()
        .join(crate::storage_layout::MOB_ANNOUNCEMENTS_DB_FILE_NAME);
    std::fs::write(&path, b"").unwrap();
    assert!(
        crate::storage_migrate::enumerate_state_dir_databases(dir.path()).contains(&path),
        "a database only some gateways open is still fenced in every build"
    );
}

fn admit(harness: &Harness, id: &BatchId) {
    harness
        .native
        .ledger
        .lock()
        .unwrap()
        .bindings
        .get_mut(id.as_str())
        .expect("committed")
        .2 = true;
}

/// Two windows of unsettled batches. The first round seals a full window
/// of batches (one round's member limit); the second reads them and seals
/// one more, the newest. The round at 184 s starts a lap that ends at the
/// newest batch: it reads the full window of older batches (due again at
/// 196.2 s) and resumes after them. Returns the harness and the newest
/// batch.
async fn two_window_lap() -> (Harness, BatchId) {
    const MEMBERS: usize = super::owner::ROUND_BATCHES + 1;
    assert_eq!(
        super::owner::ROUND_MEMBERS,
        super::owner::ROUND_BATCHES,
        "the first round seals exactly one window"
    );
    let mut native = ScriptedNative::new();
    native.apply_on_commit = false;
    native.known = (0..MEMBERS)
        .map(|index| format!("agent:r{index:02}"))
        .collect();
    let harness = harness_with(native);
    for index in 0..MEMBERS {
        let agent = format!("agent:r{index:02}");
        let topic = agent.replace(':', "-");
        harness
            .service
            .put_subscription(
                &op(&format!("o-{topic}")),
                put(
                    &format!("s-{topic}"),
                    &agent,
                    &[topic.as_str()],
                    StartFrontier::Now,
                ),
            )
            .await
            .unwrap();
        harness
            .service
            .announce(announce_request("events", &topic, &[topic.as_str()], "x"))
            .await
            .unwrap();
    }
    for now in [62_000_u64, 123_000] {
        harness.clock.store(now, Ordering::SeqCst);
        harness.service.run_round().await.unwrap();
    }
    let unsettled = harness.store.unsettled_batches(None, 100).await.unwrap();
    assert_eq!(unsettled.len(), MEMBERS);
    let newest = unsettled.last().unwrap().clone();
    assert_eq!(newest.sealed_at_ms, 123_000);
    harness.clock.store(184_000, Ordering::SeqCst);
    let report = harness.service.run_round().await.unwrap();
    assert_eq!(
        report.next_due_ms,
        Some(185_000),
        "the lap continues after the full window of older batches: {report:?}"
    );
    (harness, newest.id)
}

/// A reconcile settles the lap's last batch between rounds, so the next
/// round's window over the rest of the lap is empty. The round still names a
/// next look no later than when the batches the walk read at 184 s are due
/// again (196.2 s); with none, the dispatcher would idle five minutes.
#[tokio::test]
async fn a_lap_ending_on_an_empty_window_names_the_next_look() {
    let (harness, newest) = two_window_lap().await;
    admit(&harness, &newest);
    let reconciled = harness.service.reconcile_delivery(&newest).await.unwrap();
    assert_eq!(reconciled.batch.state.tag(), BatchStateTag::Delivered);
    harness.clock.store(185_000, Ordering::SeqCst);
    let report = harness.service.run_round().await.unwrap();
    assert!(
        report.next_due_ms.is_some_and(|due| due <= 196_200),
        "a window of unsettled batches is due again at 196200; the round names no look \
         by then: {report:?}"
    );
}

/// The lap's last window settles everything it reads (the newest batch was
/// admitted meanwhile). Same expectation.
#[tokio::test]
async fn a_lap_ending_on_a_settled_window_names_the_next_look() {
    let (harness, newest) = two_window_lap().await;
    admit(&harness, &newest);
    harness.clock.store(185_000, Ordering::SeqCst);
    let report = harness.service.run_round().await.unwrap();
    assert_eq!(
        harness
            .store
            .batch(&newest)
            .await
            .unwrap()
            .unwrap()
            .state
            .tag(),
        BatchStateTag::Delivered,
        "the round read and settled the newest batch"
    );
    assert!(
        report.next_due_ms.is_some_and(|due| due <= 196_200),
        "a window of unsettled batches is due again at 196200; the round names no look \
         by then: {report:?}"
    );
}

/// The wall clock steps back an hour after a committed batch was read; the
/// native owner then admits it. Rounds over the next 155 s of clock time read
/// it again once a settle pause (at most 30 s) has passed.
#[tokio::test]
async fn a_clock_step_back_does_not_hide_a_remembered_batch() {
    let mut native = ScriptedNative::new();
    native.apply_on_commit = false;
    let harness = harness_with(native);
    harness.subscribe("s1").await;
    harness.announce_burst(1).await;
    harness.clock.store(10_000_000, Ordering::SeqCst);
    harness.service.run_round().await.unwrap();
    let batch = harness.only_batch().await;
    assert_eq!(batch.state.tag(), BatchStateTag::Committed);
    admit(&harness, &batch.id);
    let mut now = 10_000_000 - 3_600_000;
    for _ in 0..5 {
        now += 31_000;
        harness.clock.store(now, Ordering::SeqCst);
        harness.service.run_round().await.unwrap();
    }
    assert_eq!(
        harness.only_batch().await.state.tag(),
        BatchStateTag::Delivered,
        "155 s of clock time after a one-hour step back, the admitted batch is still unread"
    );
}

/// The wall clock steps back an hour while retention catches up. The round
/// it had scheduled now lies more than the longest idle ahead; the
/// dispatcher runs a round anyway instead of waiting for the clock.
#[tokio::test]
async fn a_clock_step_back_during_catch_up_still_runs_rounds() {
    const PASSES: u64 = 300;
    let (dir, inner) = temp_store();
    let store = Arc::new(FaultyStore::new(inner, None));
    store.more_passes.store(PASSES, Ordering::SeqCst);
    let clock = Arc::new(AtomicU64::new(10_000_000));
    let reader = Arc::clone(&clock);
    let service = AnnouncementService::with_clock(
        Arc::clone(&store) as Arc<dyn AnnouncementStore>,
        Arc::new(ScriptedNative::new()) as Arc<dyn NativeDelivery>,
        AnnouncementConfig::default(),
        Arc::new(move || reader.load(Ordering::SeqCst)),
    )
    .unwrap();
    let dispatcher = service.start();
    let first_round = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        while store.backlog_reads.load(Ordering::SeqCst) < 1
            || store.prune_passes.load(Ordering::SeqCst) < 2
        {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await;
    assert!(first_round.is_ok(), "the dispatcher started");
    clock.store(10_000_000 - 3_600_000, Ordering::SeqCst);
    let caught_up = tokio::time::timeout(std::time::Duration::from_mins(1), async {
        while store.prune_passes.load(Ordering::SeqCst) <= PASSES {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await;
    service.stop();
    dispatcher.await.unwrap();
    assert!(caught_up.is_ok(), "the passes ran");
    assert!(
        store.backlog_reads.load(Ordering::SeqCst) >= 2,
        "after the clock stepped back an hour, no round ran for the rest of the catch-up"
    );
    drop(dir);
}

/// Wakes arrive every millisecond while retention catches up. Each wake may
/// bring a round, but the passes keep their own spacing, so commands get the
/// store between them.
#[tokio::test]
async fn catch_up_passes_keep_their_spacing_under_wakes() {
    const PASSES: u64 = 30;
    let (dir, inner) = temp_store();
    let store = Arc::new(FaultyStore::new(inner, None));
    store.more_passes.store(PASSES, Ordering::SeqCst);
    let service = AnnouncementService::with_clock(
        Arc::clone(&store) as Arc<dyn AnnouncementStore>,
        Arc::new(ScriptedNative::new()) as Arc<dyn NativeDelivery>,
        AnnouncementConfig::default(),
        // Past the first retention interval, so the first iteration passes.
        Arc::new(|| 10_000_000),
    )
    .unwrap();
    let dispatcher = service.start();
    let waker = {
        let service = service.clone();
        let store = Arc::clone(&store);
        tokio::spawn(async move {
            while store.prune_passes.load(Ordering::SeqCst) <= PASSES {
                service.wake();
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            }
        })
    };
    let caught_up = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        while store.prune_passes.load(Ordering::SeqCst) <= PASSES {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await;
    waker.await.unwrap();
    service.stop();
    dispatcher.await.unwrap();
    assert!(caught_up.is_ok(), "the passes ran");
    let times = store.pass_times.lock().unwrap().clone();
    let shortest = times
        .windows(2)
        .map(|pair| pair[1].duration_since(pair[0]))
        .min()
        .unwrap();
    assert!(
        shortest >= std::time::Duration::from_millis(10),
        "wakes brought passes {shortest:?} apart"
    );
    drop(dir);
}

/// A fault inside a round ends the dispatcher task. It is reported and
/// started again, so the owed announcement is still delivered.
#[tokio::test]
async fn a_dispatcher_that_panics_is_started_again() {
    let (dir, inner) = temp_store();
    let store = Arc::new(FaultyStore::new(inner, None));
    let native = Arc::new(ScriptedNative::new());
    let clock = Arc::new(AtomicU64::new(1_000));
    let reader = Arc::clone(&clock);
    let service = AnnouncementService::with_clock(
        Arc::clone(&store) as Arc<dyn AnnouncementStore>,
        Arc::clone(&native) as Arc<dyn NativeDelivery>,
        AnnouncementConfig {
            policy: policy(),
            ..AnnouncementConfig::default()
        },
        Arc::new(move || reader.load(Ordering::SeqCst)),
    )
    .unwrap();
    service
        .put_subscription(
            &op("o1"),
            put("s1", "agent:a", &["ios"], StartFrontier::Now),
        )
        .await
        .unwrap();
    service
        .announce(announce_request("payments", "k1", &["ios"], "x"))
        .await
        .unwrap();
    clock.store(100_000, Ordering::SeqCst);
    store.panic_next_round.store(true, Ordering::SeqCst);
    let dispatcher = service.start();
    let delivered = tokio::time::timeout(std::time::Duration::from_secs(20), async {
        loop {
            let batches = service
                .deliveries(&BatchQuery {
                    member: Some(member("agent:a")),
                    limit: 10,
                    ..BatchQuery::default()
                })
                .await
                .unwrap();
            if batches
                .first()
                .is_some_and(|batch| batch.state.tag() == BatchStateTag::Delivered)
            {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await;
    service.stop();
    dispatcher.await.unwrap();
    assert!(
        delivered.is_ok(),
        "after a fault inside a round, no dispatcher delivers the owed announcement"
    );
    drop(dir);
}

/// A batch that keeps failing is reported once per interval. After the
/// clock steps back an hour, its next failure is reported at once, not an
/// hour and an interval later.
#[tokio::test]
async fn a_clock_step_back_does_not_mute_failure_reports() {
    let (dir, inner) = temp_store();
    let store = Arc::new(FaultyStore::new(inner, Some("agent:a")));
    let native = Arc::new(ScriptedNative::new());
    let clock = Arc::new(AtomicU64::new(1_000));
    let reader = Arc::clone(&clock);
    let service = AnnouncementService::with_clock(
        Arc::clone(&store) as Arc<dyn AnnouncementStore>,
        Arc::clone(&native) as Arc<dyn NativeDelivery>,
        AnnouncementConfig {
            policy: policy(),
            ..AnnouncementConfig::default()
        },
        Arc::new(move || reader.load(Ordering::SeqCst)),
    )
    .unwrap();
    service
        .put_subscription(
            &op("o1"),
            put("s1", "agent:a", &["ios"], StartFrontier::Now),
        )
        .await
        .unwrap();
    service
        .announce(announce_request("events", "k1", &["ios"], "x"))
        .await
        .unwrap();
    clock.store(10_000_000, Ordering::SeqCst);
    service.run_round().await.unwrap();
    assert_eq!(service.walk_reported_at().await, vec![10_000_000]);
    let stepped_back = 10_000_000 - 3_600_000;
    clock.store(stepped_back, Ordering::SeqCst);
    service.run_round().await.unwrap();
    assert_eq!(
        service.walk_reported_at().await,
        vec![stepped_back],
        "a report dated after the stepped-back clock is dropped, so the failure is reported \
         again now"
    );
    drop(dir);
}

// ---------------------------------------------------------------------------
// Member tools
// ---------------------------------------------------------------------------

/// Sessions the scripted resolver attributes to members.
struct ScriptedCallers {
    sessions: BTreeMap<String, Member>,
}

#[async_trait]
impl super::tools::CallerResolver for ScriptedCallers {
    async fn caller(
        &self,
        session: &meerkat_core::types::SessionId,
    ) -> std::result::Result<Member, String> {
        self.sessions
            .get(&session.to_string())
            .cloned()
            .ok_or_else(|| "the calling session is not a member of this gateway's mob".to_string())
    }
}

struct ToolHarness {
    harness: Harness,
    tools: super::tools::MobAnnouncementTools,
    sessions: BTreeMap<&'static str, meerkat_core::types::SessionId>,
}

fn tool_harness(limits: super::tools::MemberLimits) -> ToolHarness {
    let mut native = ScriptedNative::new();
    native.known.insert("agent:b".into());
    let harness = harness_with(native);
    let sessions = BTreeMap::from([
        ("agent:a", meerkat_core::types::SessionId::new()),
        ("agent:b", meerkat_core::types::SessionId::new()),
    ]);
    let callers = ScriptedCallers {
        sessions: sessions
            .iter()
            .map(|(identity, session)| (session.to_string(), member(identity)))
            .collect(),
    };
    let slot: super::tools::ToolSlot = Arc::new(std::sync::OnceLock::new());
    let bound = slot
        .set(super::tools::ToolBackend {
            service: harness.service.clone(),
            callers: Arc::new(callers),
            limits,
        })
        .is_ok();
    assert!(bound);
    let clock = Arc::clone(&harness.clock);
    let tools = super::tools::MobAnnouncementTools::with_clock(
        slot,
        Arc::new(move || clock.load(Ordering::SeqCst)),
    );
    ToolHarness {
        harness,
        tools,
        sessions,
    }
}

impl ToolHarness {
    /// Call a tool as `who` (`None`: a call with no session at all), in a
    /// turn of its own.
    async fn call(
        &self,
        who: Option<&str>,
        tool: &str,
        args: serde_json::Value,
    ) -> (bool, serde_json::Value) {
        use meerkat_core::AgentToolDispatcher as _;
        let raw = serde_json::value::RawValue::from_string(args.to_string()).unwrap();
        let call = meerkat_core::types::ToolCallView {
            id: "call-1",
            name: tool,
            args: &raw,
        };
        let mut context = meerkat_core::ToolDispatchContext::default();
        if let Some(who) = who {
            let session = self
                .sessions
                .get(who)
                .cloned()
                .unwrap_or_else(meerkat_core::types::SessionId::new);
            context = context.with_runtime_identity(
                session,
                Some(meerkat_core::interaction::InteractionId::new()),
            );
        }
        let outcome = self
            .tools
            .dispatch_with_context(call, &context)
            .await
            .unwrap();
        let text = meerkat_core::types::text_content(&outcome.result.content);
        let value = serde_json::from_str(&text).unwrap_or(serde_json::Value::String(text));
        (outcome.result.is_error, value)
    }
}

#[tokio::test]
async fn a_member_announces_as_itself_into_its_own_mob() {
    let tools = tool_harness(super::tools::MemberLimits::default());
    tools
        .harness
        .service
        .put_subscription(
            &op("o1"),
            put("s1", "agent:b", &["ios"], StartFrontier::Now),
        )
        .await
        .unwrap();
    let (failed, result) = tools
        .call(
            Some("agent:a"),
            super::tools::ANNOUNCE_TOOL,
            serde_json::json!({ "topics": ["ios"], "body": "checkout fixed", "key": "fix-1" }),
        )
        .await;
    assert!(!failed, "{result}");
    assert_eq!(result["mob_id"], OWN_MOB);
    assert_eq!(result["captured"], 1);
    assert!(
        result.get("captured_for").is_none(),
        "who subscribes is not the announcer's to learn: {result}"
    );
    let key = format!("{}-fix-1", super::tools::member_prefix(&member("agent:a")));
    let stored = tools
        .harness
        .store
        .announcement(&mob(OWN_MOB), &SourceKey::new(key).unwrap())
        .await
        .unwrap()
        .expect("the key lives in the calling member's namespace");
    assert_eq!(
        stored.announced_by,
        Announcer::Member {
            identity: MemberIdentity::new("agent:a").unwrap(),
        },
        "the announcement is attributed to the calling member"
    );
}

#[tokio::test]
async fn a_call_that_cannot_be_attributed_changes_nothing() {
    let tools = tool_harness(super::tools::MemberLimits::default());
    for who in [None, Some("a session of no member")] {
        let (failed, result) = tools
            .call(
                who,
                super::tools::ANNOUNCE_TOOL,
                serde_json::json!({ "topics": ["ios"], "body": "x" }),
            )
            .await;
        assert!(failed, "{who:?}: {result}");
    }
    assert!(
        tools.harness.store.known_mobs().await.unwrap().is_empty(),
        "nothing was announced"
    );
}

/// What attributing a call from `session` decides when the roster binds
/// `bound` to it and the identity runtime records `record` for `agent:a`
/// and knows no other identity.
async fn attributed(
    session: &meerkat_core::types::SessionId,
    bound: &[&str],
    record: super::tools::RecordedBinding,
) -> std::result::Result<Member, String> {
    let bound = bound.iter().map(|id| (*id).to_string()).collect::<Vec<_>>();
    super::tools::attribute(&mob(OWN_MOB), session, &bound, |agent| {
        std::future::ready(if agent.as_str() == "agent:a" {
            Ok(record)
        } else {
            Err(format!(
                "the calling member is not registered here: unknown identity {agent}"
            ))
        })
    })
    .await
}

#[tokio::test]
async fn a_call_is_attributed_only_to_the_one_member_both_records_agree_on() {
    let session = meerkat_core::types::SessionId::new();
    let current = super::tools::RecordedBinding {
        session_id: Some(session.to_string()),
        runtime_id: Some("rt:agent:a:1".to_string()),
    };
    let encoded = crate::member_comms_id::mob_member_id_str("agent:a").into_owned();
    // The roster row in either spelling, bound to the session the identity
    // runtime records: the call is that member's.
    for roster_id in ["agent:a", encoded.as_str()] {
        assert_eq!(
            attributed(&session, &[roster_id], current.clone()).await,
            Ok(member("agent:a")),
            "{roster_id}"
        );
    }

    let another_session = super::tools::RecordedBinding {
        session_id: Some(meerkat_core::types::SessionId::new().to_string()),
        ..current.clone()
    };
    let no_session = super::tools::RecordedBinding {
        session_id: None,
        ..current.clone()
    };
    let cases: [(&str, &[&str], super::tools::RecordedBinding, &str); 7] = [
        (
            "no roster entry is bound to the session",
            &[],
            current.clone(),
            "not a member of this gateway's mob",
        ),
        (
            "two roster entries are bound to the session",
            &["agent:a", "agent:b"],
            current.clone(),
            "more than one member",
        ),
        (
            "the roster id is no durable identity",
            &["not durable"],
            current.clone(),
            "not a durable identity",
        ),
        (
            "the identity runtime does not know the identity",
            &["agent:b"],
            current.clone(),
            "not registered here",
        ),
        (
            "the identity runtime records another session",
            &["agent:a"],
            another_session,
            "does not record the calling session",
        ),
        (
            "the identity runtime records no session",
            &["agent:a"],
            no_session,
            "does not record the calling session",
        ),
        // A generated runtime alias is never decoded to the durable
        // identity, even when it names the current incarnation.
        (
            "the roster id is a runtime alias",
            &["rt:agent:a:1"],
            current.clone(),
            "not registered here",
        ),
    ];
    for (what, bound, record, reason) in cases {
        let refused = attributed(&session, bound, record).await.expect_err(what);
        assert!(refused.contains(reason), "{what}: {refused}");
    }
}

#[tokio::test]
async fn member_limits_bound_announcements_and_subscriptions() {
    let tools = tool_harness(super::tools::MemberLimits {
        announcements_per_minute: 2,
        announcement_bytes_per_minute: 1_000,
        subscriptions: 1,
        topics_per_subscription: 2,
    });
    for (index, expected_failure) in [(0, false), (1, false), (2, true)] {
        let (failed, result) = tools
            .call(
                Some("agent:a"),
                super::tools::ANNOUNCE_TOOL,
                serde_json::json!({ "topics": ["ios"], "body": "x", "key": format!("k{index}") }),
            )
            .await;
        assert_eq!(failed, expected_failure, "announcement {index}: {result}");
    }
    let (failed, result) = tools
        .call(
            Some("agent:b"),
            super::tools::ANNOUNCE_TOOL,
            serde_json::json!({ "topics": ["ios"], "body": "x", "key": "k0" }),
        )
        .await;
    assert!(!failed, "another member's minute is its own: {result}");
    tools.harness.clock.fetch_add(60_000, Ordering::SeqCst);
    let (failed, result) = tools
        .call(
            Some("agent:a"),
            super::tools::ANNOUNCE_TOOL,
            serde_json::json!({ "topics": ["ios"], "body": "x".repeat(1_001) }),
        )
        .await;
    assert!(
        failed,
        "the byte limit holds within a fresh minute: {result}"
    );
    let subscribe = |name: &str, topics: serde_json::Value| serde_json::json!({ "action": "subscribe", "subscription": name, "topics": topics });
    let (failed, result) = tools
        .call(
            Some("agent:a"),
            super::tools::SUBSCRIPTION_TOOL,
            subscribe("wide", serde_json::json!(["a", "b", "c"])),
        )
        .await;
    assert!(failed, "too many topics: {result}");
    let (failed, result) = tools
        .call(
            Some("agent:a"),
            super::tools::SUBSCRIPTION_TOOL,
            subscribe("first", serde_json::json!(["ios"])),
        )
        .await;
    assert!(!failed, "{result}");
    let (failed, result) = tools
        .call(
            Some("agent:a"),
            super::tools::SUBSCRIPTION_TOOL,
            subscribe("second", serde_json::json!(["web"])),
        )
        .await;
    assert!(failed, "one subscription at most: {result}");
}

#[tokio::test]
async fn members_see_and_change_only_their_own_subscriptions() {
    let tools = tool_harness(super::tools::MemberLimits::default());
    let (failed, result) = tools
        .call(
            Some("agent:a"),
            super::tools::SUBSCRIPTION_TOOL,
            serde_json::json!({ "action": "subscribe", "subscription": "news", "topics": ["ios"] }),
        )
        .await;
    assert!(!failed, "{result}");
    assert_eq!(result["subscription"], "news");
    assert_eq!(result["revision"], 1);
    let (failed, result) = tools
        .call(
            Some("agent:b"),
            super::tools::SUBSCRIPTION_TOOL,
            serde_json::json!({ "action": "cancel", "subscription": "news", "expected_revision": 1 }),
        )
        .await;
    assert!(
        failed,
        "another member's subscription is out of reach: {result}"
    );
    let (_, listed) = tools
        .call(
            Some("agent:b"),
            super::tools::SUBSCRIPTION_TOOL,
            serde_json::json!({ "action": "list" }),
        )
        .await;
    assert_eq!(listed["subscriptions"], serde_json::json!([]));
    let (_, listed) = tools
        .call(
            Some("agent:a"),
            super::tools::SUBSCRIPTION_TOOL,
            serde_json::json!({ "action": "list" }),
        )
        .await;
    assert_eq!(listed["subscriptions"][0]["subscription"], "news");
    assert_eq!(listed["subscriptions"][0]["state"], "active", "{listed}");
}

/// Two members naming the same key or the same subscription never meet:
/// each name lives in its member's own namespace.
#[tokio::test]
async fn two_members_never_meet_under_one_name() {
    let tools = tool_harness(super::tools::MemberLimits::default());
    for who in ["agent:a", "agent:b"] {
        let (failed, result) = tools
            .call(
                Some(who),
                super::tools::ANNOUNCE_TOOL,
                serde_json::json!({ "topics": ["ios"], "body": "checkout fixed", "key": "fix-1" }),
            )
            .await;
        assert!(!failed, "{who}: {result}");
        assert_eq!(result["replayed"], false, "{who}: {result}");
        let (failed, result) = tools
            .call(
                Some(who),
                super::tools::SUBSCRIPTION_TOOL,
                serde_json::json!({ "action": "subscribe", "subscription": "news", "topics": ["ios"] }),
            )
            .await;
        assert!(!failed, "{who}: {result}");
        assert_eq!(result["revision"], 1, "{who}: {result}");
    }
    for who in ["agent:a", "agent:b"] {
        let key = format!("{}-fix-1", super::tools::member_prefix(&member(who)));
        let stored = tools
            .harness
            .store
            .announcement(&mob(OWN_MOB), &SourceKey::new(key).unwrap())
            .await
            .unwrap()
            .expect("each member's key is its own");
        assert_eq!(
            stored.announced_by,
            Announcer::Member {
                identity: MemberIdentity::new(who).unwrap(),
            },
            "{who}"
        );
        let (_, listed) = tools
            .call(
                Some(who),
                super::tools::SUBSCRIPTION_TOOL,
                serde_json::json!({ "action": "list" }),
            )
            .await;
        assert_eq!(
            listed["subscriptions"].as_array().map(Vec::len),
            Some(1),
            "{who}: {listed}"
        );
    }
}

// ---------------------------------------------------------------------------
// Lineage and reach
// ---------------------------------------------------------------------------

/// Records for the reach walk: `(mob, parent)`.
/// Live records for `(mob, parent)` pairs, each edge derived under its
/// parent's incarnation; a parent without a record is the root.
fn records(pairs: &[(&str, Option<&str>)]) -> Vec<MobLineage> {
    let incarnation_of = |mob_id: &str| {
        pairs
            .iter()
            .position(|(other, _)| *other == mob_id)
            .map(|index| incarnation("s", index as u64))
    };
    pairs
        .iter()
        .enumerate()
        .map(|(index, (mob_id, parent))| MobLineage {
            parent_incarnation: parent.and_then(incarnation_of),
            ..lineage_record(mob_id, *parent, incarnation("s", index as u64), 1)
        })
        .collect()
}

fn names(reach: &BTreeSet<MobId>) -> Vec<&str> {
    reach.iter().map(MobId::as_str).collect()
}

#[test]
fn reach_is_the_own_mob_its_ancestors_and_its_descendants() {
    use super::owner::reach_from;
    // root
    //  +- team (+- task, +- sibling-task)
    //  +- other-team
    let lineage = records(&[
        ("team", Some("root")),
        ("task", Some("team")),
        ("sibling-task", Some("team")),
        ("deep", Some("task")),
        ("other-team", Some("root")),
        ("unrelated", None),
    ]);
    assert_eq!(
        names(&reach_from(&lineage, &mob("task"))),
        ["deep", "root", "task", "team"],
        "never sideways: sibling-task, other-team and unrelated are out"
    );
    assert_eq!(
        names(&reach_from(&lineage, &mob("root"))),
        ["deep", "other-team", "root", "sibling-task", "task", "team"],
        "the root reaches every mob below it, not one without a member origin"
    );
    assert_eq!(
        names(&reach_from(&lineage, &mob("unrelated"))),
        ["unrelated"]
    );

    // Records that loop (an id reused under a new parent) end the walk.
    let looping = records(&[("x", Some("y")), ("y", Some("x"))]);
    assert_eq!(names(&reach_from(&looping, &mob("x"))), ["x", "y"]);

    // A chain longer than the walk's bound stops at the bound.
    let mut chain = vec![("m0".to_string(), Some("root".to_string()))];
    for level in 1..100 {
        chain.push((format!("m{level}"), Some(format!("m{}", level - 1))));
    }
    let chain = chain
        .iter()
        .map(|(mob_id, parent)| (mob_id.as_str(), parent.as_deref()))
        .collect::<Vec<_>>();
    assert_eq!(reach_from(&records(&chain), &mob("root")).len(), 65);
    assert_eq!(reach_from(&records(&chain), &mob("m99")).len(), 65);

    // A parent created again under the same id adopts no child of its old
    // incarnation: the edge holds only to the incarnation it was derived
    // under.
    let mut adopted = records(&[("team", Some("root")), ("task", Some("team"))]);
    adopted[0].incarnation = incarnation("again", 99);
    assert_eq!(names(&reach_from(&adopted, &mob("root"))), ["root", "team"]);
    assert_eq!(names(&reach_from(&adopted, &mob("task"))), ["task"]);
    // An edge to the root never names an incarnation.
    let mut rooted = records(&[("team", Some("root"))]);
    rooted[0].parent_incarnation = Some(incarnation("root", 1));
    assert_eq!(names(&reach_from(&rooted, &mob("team"))), ["team"]);
}

#[tokio::test]
async fn lineage_is_derived_once_and_only_from_proofs() {
    let native = ScriptedNative::new();
    native.set_origin(
        "events",
        incarnation("s1", 10),
        native.created_by(OWN_MOB, "agent:a"),
    );
    native.set_origin(
        "builds",
        incarnation("s2", 20),
        native.created_by("events", "worker"),
    );
    native.set_origin(
        "refunds",
        incarnation("s3", 30),
        MobOrigin::Unproven("its owner session is bound to no member".into()),
    );
    native.set_origin(
        "a-mob",
        incarnation("s4", 40),
        MobOrigin::ParentNotHosted(mob("gone-mob")),
    );
    native
        .unreadable_origins
        .lock()
        .unwrap()
        .insert("b-mob".into());
    native.set_origin(
        "b-mob",
        incarnation("s5", 50),
        native.created_by(OWN_MOB, "agent:a"),
    );
    native.restored.store(false, Ordering::SeqCst);
    let harness = harness_with(native);
    let lineage = |harness: &Harness| {
        let store = Arc::clone(&harness.store);
        async move {
            store
                .lineage()
                .await
                .unwrap()
                .into_iter()
                .map(|record| {
                    (
                        record.mob_id.to_string(),
                        record.parent.map(|parent| parent.to_string()),
                    )
                })
                .collect::<Vec<_>>()
        }
    };

    // A read that fails writes nothing and is reported; the others are
    // recorded. A missing parent before the restore point waits.
    assert!(harness.service.reconcile_lineage().await.is_err());
    assert_eq!(
        lineage(&harness).await,
        [
            ("builds".to_string(), Some("events".to_string())),
            ("events".to_string(), Some(OWN_MOB.to_string())),
            ("refunds".to_string(), None),
        ]
    );

    // After the restore point a missing parent is final: an orphan.
    harness.native.restored.store(true, Ordering::SeqCst);
    harness.native.unreadable_origins.lock().unwrap().clear();
    harness.service.reconcile_lineage().await.unwrap();
    assert_eq!(
        lineage(&harness).await,
        [
            ("a-mob".to_string(), None),
            ("b-mob".to_string(), Some(OWN_MOB.to_string())),
            ("builds".to_string(), Some("events".to_string())),
            ("events".to_string(), Some(OWN_MOB.to_string())),
            ("refunds".to_string(), None),
        ]
    );

    // Derived once: the same incarnation is never rewritten from what the
    // records say later.
    harness.native.set_origin(
        "events",
        incarnation("s1", 10),
        harness.native.created_by("refunds", "someone-else"),
    );
    harness.service.reconcile_lineage().await.unwrap();
    assert!(
        lineage(&harness)
            .await
            .contains(&("events".to_string(), Some(OWN_MOB.to_string())))
    );

    // A new incarnation (destroyed and created again, even by the same
    // owner session) is derived again.
    harness.native.set_origin(
        "events",
        incarnation("s1", 11),
        harness.native.created_by("refunds", "someone-else"),
    );
    harness.service.reconcile_lineage().await.unwrap();
    assert!(
        lineage(&harness)
            .await
            .contains(&("events".to_string(), Some("refunds".to_string())))
    );

    // A mob no longer hosted leaves the live records.
    harness
        .native
        .destroyed
        .lock()
        .unwrap()
        .insert("builds".into());
    harness.service.reconcile_lineage().await.unwrap();
    assert!(
        !lineage(&harness)
            .await
            .iter()
            .any(|(mob_id, _)| mob_id == "builds")
    );
}

#[tokio::test]
async fn members_subscribe_within_their_reach_only() {
    let tools = tool_harness(super::tools::MemberLimits::default());
    let native = &tools.harness.native;
    native.set_origin(
        "events",
        incarnation("s1", 10),
        native.created_by(OWN_MOB, "agent:a"),
    );
    native.set_origin(
        "builds",
        incarnation("s2", 20),
        native.created_by("events", "worker"),
    );
    native.set_origin(
        "refunds",
        incarnation("s3", 30),
        MobOrigin::Unproven("no member created it".into()),
    );
    let subscribe = |name: &str, mobs: serde_json::Value| {
        serde_json::json!({
            "action": "subscribe",
            "subscription": name,
            "topics": ["ios"],
            "mobs": mobs,
        })
    };
    // Below the caller's mob, derived on first use.
    let (failed, result) = tools
        .call(
            Some("agent:a"),
            super::tools::SUBSCRIPTION_TOOL,
            subscribe("below", serde_json::json!(["events", "builds"])),
        )
        .await;
    assert!(!failed, "{result}");
    assert_eq!(result["mobs"], serde_json::json!(["builds", "events"]));
    // Its own mob by default.
    let (failed, result) = tools
        .call(
            Some("agent:a"),
            super::tools::SUBSCRIPTION_TOOL,
            serde_json::json!({ "action": "subscribe", "subscription": "own", "topics": ["ios"] }),
        )
        .await;
    assert!(!failed, "{result}");
    assert_eq!(result["mobs"], serde_json::json!([OWN_MOB]));
    // A mob no member created is outside every member's reach.
    let (failed, result) = tools
        .call(
            Some("agent:a"),
            super::tools::SUBSCRIPTION_TOOL,
            subscribe("unrelated", serde_json::json!(["refunds"])),
        )
        .await;
    assert!(failed, "{result}");
    assert!(
        result
            .to_string()
            .contains("mob refunds is not the member's own mob"),
        "{result}"
    );
    let (failed, result) = tools
        .call(
            Some("agent:a"),
            super::tools::SUBSCRIPTION_TOOL,
            subscribe("none", serde_json::json!([])),
        )
        .await;
    assert!(failed, "{result}");
    let listed = tools
        .harness
        .store
        .subscriptions(Some(&member("agent:a")), None, 10)
        .await
        .unwrap();
    assert_eq!(listed.len(), 2, "refusals change nothing: {listed:?}");
}

#[test]
fn a_member_created_a_mob_only_when_all_three_records_agree() {
    use super::native::{BindingRecord, OriginRecords, OwnerMetadata, ParentRecord, origin_from};
    let binding = |mob_id: &str, role: &str, member: &str| BindingRecord {
        mob_id: mob_id.to_string(),
        role: role.to_string(),
        member: member.to_string(),
    };
    let owned = |journal: Option<(&str, BindingRecord)>| OriginRecords {
        owner_session: Some("session-1".to_string()),
        owner_metadata: Some(OwnerMetadata::Bound(binding("payments", "lead", "agent:a"))),
        parent: Some(ParentRecord::Running {
            journal: journal.map(|(session, recorded)| (session.to_string(), recorded)),
            incarnation: Some(incarnation("parent", 7)),
        }),
    };
    assert_eq!(
        origin_from(&owned(Some((
            "session-1",
            binding("payments", "lead", "agent:a")
        )))),
        Ok(MobOrigin::Member {
            parent: mob("payments"),
            creator: "agent:a".to_string(),
            parent_incarnation: Some(incarnation("parent", 7)),
        })
    );
    let unproven = [
        ("no owner binding", OriginRecords::default()),
        (
            "no persisted metadata",
            OriginRecords {
                owner_session: Some("session-1".to_string()),
                owner_metadata: Some(OwnerMetadata::Absent),
                parent: None,
            },
        ),
        (
            "no member binding",
            OriginRecords {
                owner_session: Some("session-1".to_string()),
                owner_metadata: Some(OwnerMetadata::Unbound),
                parent: None,
            },
        ),
        ("no journal entry", owned(None)),
        (
            "another session in the journal",
            owned(Some(("session-2", binding("payments", "lead", "agent:a")))),
        ),
        (
            "another mob in the journal",
            owned(Some(("session-1", binding("events", "lead", "agent:a")))),
        ),
        (
            "another role in the journal",
            owned(Some((
                "session-1",
                binding("payments", "worker", "agent:a"),
            ))),
        ),
        (
            "another member in the journal",
            owned(Some(("session-1", binding("payments", "lead", "agent:b")))),
        ),
        (
            "a binding that names no valid mob",
            OriginRecords {
                owner_metadata: Some(OwnerMetadata::Bound(binding("", "lead", "agent:a"))),
                ..owned(None)
            },
        ),
    ];
    for (case, records) in unproven {
        assert!(
            matches!(origin_from(&records), Ok(MobOrigin::Unproven(_))),
            "{case}: {:?}",
            origin_from(&records)
        );
    }
    let parent_at = |parent: ParentRecord| OriginRecords {
        parent: Some(parent),
        ..owned(None)
    };
    assert_eq!(
        origin_from(&parent_at(ParentRecord::NotHosted)),
        Ok(MobOrigin::ParentNotHosted(mob("payments")))
    );
    assert!(
        origin_from(&parent_at(ParentRecord::Held)).is_err(),
        "a held parent may run again: nothing is final yet"
    );
}

// ---------------------------------------------------------------------------
// Expiry, gaps and the retention of native-held bodies
// ---------------------------------------------------------------------------

const WINDOW: u64 = 3_600_000;

fn applied(id: &str) -> BatchEvent {
    BatchEvent::NativeApplied {
        receipt: receipt(id),
        session_id: "session-1".into(),
        input_id: "input-1".into(),
    }
}

fn expired_gap(count: u64) -> BatchGap {
    BatchGap {
        mob_id: mob("payments"),
        count,
        reason: GapReason::Expired,
    }
}

async fn expiry_settles_what_waited_past_the_window(store: &dyn AnnouncementStore) {
    store
        .put_subscription(
            &op("o1"),
            put("s1", "agent:a", &["ios"], StartFrontier::Now),
            1_000,
        )
        .await
        .unwrap();
    // A second subscription of the same member captures the same
    // announcement: it still expires once.
    store
        .put_subscription(
            &op("o2"),
            put("s2", "agent:a", &["ios"], StartFrontier::Now),
            1_000,
        )
        .await
        .unwrap();
    store
        .announce(announce_request("payments", "k1", &["ios"], "old"), 1_000)
        .await
        .unwrap();
    assert_eq!(
        store
            .expire(1_000 + WINDOW - 1, WINDOW, 0, 100)
            .await
            .unwrap(),
        ExpiryReport::default(),
        "inside the window nothing expires"
    );
    let report = store.expire(1_000 + WINDOW, WINDOW, 0, 100).await.unwrap();
    assert_eq!(report.captures, 1, "once per member: {report:?}");

    let now = 1_000 + WINDOW + 10;
    store
        .announce(announce_request("payments", "k2", &["ios"], "new"), now)
        .await
        .unwrap();
    let batch = store
        .seal_batch(&member("agent:a"), &batch_id("b1"), &policy(), now + 1_000)
        .await
        .unwrap()
        .expect("the new announcement is due");
    assert_eq!(batch.gaps, vec![expired_gap(1)]);
    assert_eq!(
        batch
            .items
            .iter()
            .map(|item| item.source_key.as_str())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["k2"]),
        "only what is still owed"
    );
    assert!(
        batch
            .body
            .contains("Not delivered: 1 announcement in mob \"payments\""),
        "{}",
        batch.body
    );
    for (expected, event) in [
        (BatchStateTag::Sealed, BatchEvent::BeginSubmit),
        (
            BatchStateTag::Submitting,
            BatchEvent::NativeCommitted {
                receipt: receipt("b1"),
            },
        ),
        (BatchStateTag::Committed, applied("b1")),
    ] {
        store
            .transition_batch(&batch.id, expected, event, now + 2_000)
            .await
            .unwrap();
    }
    store
        .announce(
            announce_request("payments", "k3", &["ios"], "later"),
            now + 3_000,
        )
        .await
        .unwrap();
    let next = store
        .seal_batch(&member("agent:a"), &batch_id("b2"), &policy(), now + 5_000)
        .await
        .unwrap()
        .unwrap();
    assert!(next.gaps.is_empty(), "a gap is reported once");

    // A sealed batch nothing native ever started for expires once it was
    // sealed before the window, and the next batch reports what it held.
    let later = now + 5_000 + WINDOW;
    let report = store.expire(later - 1, WINDOW, 0, 100).await.unwrap();
    assert_eq!(report.batches, 0);
    let report = store.expire(later, WINDOW, 0, 100).await.unwrap();
    assert_eq!(report.batches, 1);
    assert_eq!(
        store.batch(&next.id).await.unwrap().unwrap().state,
        BatchState::Expired
    );
    store
        .announce(announce_request("payments", "k4", &["ios"], "after"), later)
        .await
        .unwrap();
    let after = store
        .seal_batch(
            &member("agent:a"),
            &batch_id("b3"),
            &policy(),
            later + 1_000,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.gaps, vec![expired_gap(1)]);
}

async fn retention_drops_bodies_a_native_held_batch_carries(store: &dyn AnnouncementStore) {
    store
        .put_subscription(
            &op("o1"),
            put("s1", "agent:a", &["ios"], StartFrontier::Now),
            1_000,
        )
        .await
        .unwrap();
    store
        .put_subscription(
            &op("o2"),
            put("s2", "agent:b", &["web"], StartFrontier::Now),
            1_000,
        )
        .await
        .unwrap();
    store
        .announce(announce_request("payments", "k1", &["ios"], "held"), 1_000)
        .await
        .unwrap();
    store
        .announce(
            announce_request("payments", "k2", &["web"], "sealed"),
            1_500,
        )
        .await
        .unwrap();
    let held = store
        .seal_batch(&member("agent:a"), &batch_id("b1"), &policy(), 3_000)
        .await
        .unwrap()
        .unwrap();
    store
        .transition_batch(
            &held.id,
            BatchStateTag::Sealed,
            BatchEvent::BeginSubmit,
            3_000,
        )
        .await
        .unwrap();
    store
        .transition_batch(
            &held.id,
            BatchStateTag::Submitting,
            BatchEvent::NativeCommitted {
                receipt: receipt("b1"),
            },
            3_000,
        )
        .await
        .unwrap();
    store
        .seal_batch(&member("agent:b"), &batch_id("b2"), &policy(), 3_000)
        .await
        .unwrap()
        .unwrap();

    let report = store.prune(3_000 + WINDOW, WINDOW, 100).await.unwrap();
    assert_eq!(
        report.announcements, 1,
        "a batch the native owner holds carries its own body"
    );
    let k = |key: &str| SourceKey::new(key).unwrap();
    assert!(
        store
            .announcement(&mob("payments"), &k("k1"))
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .announcement(&mob("payments"), &k("k2"))
            .await
            .unwrap()
            .is_some(),
        "a sealed batch nothing native holds still needs its announcement"
    );
    assert_eq!(
        store.batch(&held.id).await.unwrap().unwrap().items,
        held.items,
        "the held batch keeps what it owes"
    );

    // Once it settles, what it owed and then the batch itself go.
    store
        .transition_batch(
            &held.id,
            BatchStateTag::Committed,
            applied("b1"),
            4_000 + WINDOW,
        )
        .await
        .unwrap();
    let report = store.prune(4_000 + 2 * WINDOW, WINDOW, 100).await.unwrap();
    assert_eq!(report.batches, 1, "{report:?}");
}

#[tokio::test]
async fn a_member_without_an_incarnation_does_not_hold_retention_past_the_window() {
    let native = ScriptedNative::new();
    native.ledger.lock().unwrap().retired = true;
    let harness = harness_with(native);
    harness.subscribe("s1").await;
    harness.announce_burst(1).await;
    harness.clock.store(10_000_000, Ordering::SeqCst);
    harness.service.run_round().await.unwrap();
    let batch = harness.only_batch().await;
    assert_eq!(batch.state.tag(), BatchStateTag::Sealed);
    let retention = AnnouncementConfig::default().retention_ms;

    // Still inside the window from its sealing: it stays owed.
    harness
        .clock
        .store(batch.sealed_at_ms + retention - 1, Ordering::SeqCst);
    harness.service.run_round().await.unwrap();
    assert_eq!(
        harness.only_batch().await.state.tag(),
        BatchStateTag::Sealed
    );

    // Past it (and past its next look), nothing native holds it: it expires
    // and the announcement can be pruned.
    let settle = AnnouncementConfig::default().settle_check_max_ms;
    harness
        .clock
        .store(batch.sealed_at_ms + retention + settle, Ordering::SeqCst);
    harness.service.run_round().await.unwrap();
    assert_eq!(harness.only_batch().await.state, BatchState::Expired);
    let now = harness.clock.load(Ordering::SeqCst);
    let report = harness.store.prune(now, retention, 100).await.unwrap();
    assert_eq!(report.announcements, 1, "{report:?}");

    // Back again: its next batch reports the gap.
    harness.native.ledger.lock().unwrap().retired = false;
    harness
        .service
        .announce(announce_request(
            "payments",
            "back",
            &["ios"],
            "after the gap",
        ))
        .await
        .unwrap();
    harness.clock.fetch_add(60_001, Ordering::SeqCst);
    harness.service.run_round().await.unwrap();
    let batches = harness
        .service
        .deliveries(&BatchQuery {
            member: Some(member("agent:a")),
            limit: 10,
            ..BatchQuery::default()
        })
        .await
        .unwrap();
    let delivered = batches
        .iter()
        .find(|batch| batch.state.tag() == BatchStateTag::Delivered)
        .expect("the member got its next batch");
    assert_eq!(delivered.gaps, vec![expired_gap(1)]);
    assert!(
        delivered.body.contains("Not delivered"),
        "{}",
        delivered.body
    );
}

#[tokio::test]
async fn a_peer_batch_expires_only_once_its_last_request_is_stale() {
    let native = ScriptedNative::new();
    native.ledger.lock().unwrap().retired = true;
    let harness = harness_with(native);
    let peer = Member::new(mob("mob-b"), MemberIdentity::new("agent:p").unwrap());
    harness
        .service
        .put_subscription(
            &op("p1"),
            PutSubscription {
                member: peer.clone(),
                ..put("p1", "agent:a", &["ios"], StartFrontier::Now)
            },
        )
        .await
        .unwrap();
    harness.announce_burst(1).await;
    harness.clock.store(10_000_000, Ordering::SeqCst);
    harness.service.run_round().await.unwrap();
    let query = BatchQuery {
        member: Some(peer.clone()),
        limit: 10,
        ..BatchQuery::default()
    };
    let batch = harness.service.deliveries(&query).await.unwrap().remove(0);
    let retention = AnnouncementConfig::default().retention_ms;
    let window = AnnouncementConfig::default().peer_request_window_ms;

    // A retry just before the window ends: a request may be in flight.
    harness
        .clock
        .store(batch.sealed_at_ms + retention - 1_000, Ordering::SeqCst);
    harness.service.run_round().await.unwrap();
    let submits = harness.native.submits();
    let retried = harness.service.deliveries(&query).await.unwrap().remove(0);
    // Past the window (and past its next look) it neither expires nor is
    // submitted again while that request could still land.
    let settle = AnnouncementConfig::default().settle_check_max_ms;
    assert!(retried.updated_at_ms + settle + 1 > batch.sealed_at_ms + retention);
    harness
        .clock
        .store(retried.updated_at_ms + settle + 1, Ordering::SeqCst);
    harness.service.run_round().await.unwrap();
    let waiting = harness.service.deliveries(&query).await.unwrap().remove(0);
    assert_eq!(waiting.state.tag(), BatchStateTag::Sealed);
    assert_eq!(harness.native.submits(), submits, "no further attempt");
    // Just past the peer's window it still waits: a request issued late in
    // a round, or read by a clock a little behind, may still be admitted.
    harness
        .clock
        .store(retried.updated_at_ms + window + 1, Ordering::SeqCst);
    harness.service.run_round().await.unwrap();
    let margin = harness.service.deliveries(&query).await.unwrap().remove(0);
    assert_eq!(margin.state.tag(), BatchStateTag::Sealed);
    // Once the window and the margin past it have run out, the peer would
    // refuse that request as stale, and the batch expires.
    harness.clock.store(
        retried.updated_at_ms + window + settle + 1,
        Ordering::SeqCst,
    );
    harness.service.run_round().await.unwrap();
    let expired = harness.service.deliveries(&query).await.unwrap().remove(0);
    assert_eq!(expired.state, BatchState::Expired);
    assert_eq!(harness.native.submits(), submits);
}

/// A gap line can be longer than the item it stands for: a successor for a
/// member on a peer gateway carries the gap lines its items leave room for
/// in one request, and defers the rest to the member's next batch.
#[test]
fn a_successor_defers_the_gap_lines_one_peer_request_has_no_room_for() {
    let items = vec!["x".repeat(40 * 1024)];
    let gaps = (0..100)
        .map(|index| BatchGap {
            mob_id: mob(&format!("{}-{index:03}", "m".repeat(240))),
            count: 1,
            reason: GapReason::NotRetained,
        })
        .collect::<Vec<_>>();
    let successor = successor_batch(
        batch_id("b2"),
        &batch_id("b1"),
        gaps.clone(),
        &items,
        Some(MAX_REMOTE_BODY_WIRE_BYTES),
    );
    assert!(wire_len(&successor.body) <= MAX_REMOTE_BODY_WIRE_BYTES);
    assert!(!successor.gaps.is_empty() && !successor.deferred.is_empty());
    let mut all = successor.gaps.clone();
    all.extend(successor.deferred);
    assert_eq!(all, gaps);
    // For a member of a mob hosted here every gap line rides along.
    let local = successor_batch(batch_id("b3"), &batch_id("b1"), gaps.clone(), &items, None);
    assert_eq!(local.gaps, gaps);
    assert!(local.deferred.is_empty());
}

#[tokio::test]
async fn a_stranded_batch_whose_announcements_were_pruned_hands_on_a_gap() {
    let mut native = ScriptedNative::new();
    native.apply_on_commit = false;
    let harness = harness_with(native);
    harness.subscribe("s1").await;
    harness.announce_burst(2).await;
    harness.clock.store(10_000_000, Ordering::SeqCst);
    harness.service.run_round().await.unwrap();
    let first = harness.only_batch().await;
    assert_eq!(first.state.tag(), BatchStateTag::Committed);
    let retention = AnnouncementConfig::default().retention_ms;
    let report = harness
        .store
        .prune(10_000_000 + retention, retention, 100)
        .await
        .unwrap();
    assert_eq!(report.announcements, 2, "{report:?}");
    harness
        .native
        .ledger
        .lock()
        .unwrap()
        .strand
        .insert(first.id.to_string());
    harness.round_after_pause().await;
    let stranded = harness.store.batch(&first.id).await.unwrap().unwrap();
    assert_eq!(stranded.state.tag(), BatchStateTag::Stranded);
    let successor = harness
        .store
        .batch(&stranded.successor.expect("a successor"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        successor.gaps,
        vec![BatchGap {
            mob_id: mob("payments"),
            count: 2,
            reason: GapReason::NotRetained,
        }]
    );
    assert!(
        successor.body.contains("no longer retained"),
        "{}",
        successor.body
    );
}

#[tokio::test]
async fn a_backlog_paused_past_the_window_expires_and_the_resume_reports_it() {
    let harness = harness_with(ScriptedNative::new());
    let subscription = harness.subscribe("s1").await;
    harness
        .service
        .change_subscription(
            &op("pause"),
            &sid("s1"),
            subscription.revision,
            SubscriptionChange::Pause,
        )
        .await
        .unwrap();
    harness.announce_burst(3).await;
    let retention = AnnouncementConfig::default().retention_ms;
    let report = harness
        .store
        .expire(1_000 + retention, retention, 0, 100)
        .await
        .unwrap();
    assert_eq!(report.captures, 3);
    harness
        .service
        .change_subscription(
            &op("resume"),
            &sid("s1"),
            subscription.revision + 1,
            SubscriptionChange::Resume,
        )
        .await
        .unwrap();
    harness
        .service
        .announce(announce_request(
            "payments",
            "after",
            &["ios"],
            "after the pause",
        ))
        .await
        .unwrap();
    harness.clock.store(1_000 + retention + 1, Ordering::SeqCst);
    harness.service.run_round().await.unwrap();
    let batch = harness.only_batch().await;
    assert_eq!(batch.gaps, vec![expired_gap(3)]);
    assert_eq!(batch.items.len(), 1);
}

#[tokio::test]
async fn this_gateways_own_mob_never_gets_a_lineage_record() {
    let native = ScriptedNative::new();
    native.set_origin(
        OWN_MOB,
        incarnation("own", 1),
        MobOrigin::Unproven("no member created it".into()),
    );
    native.set_origin(
        "refunds",
        incarnation("s3", 30),
        MobOrigin::Unproven("no member created it".into()),
    );
    let harness = harness_with(native);
    // A member of this gateway's own mob names a mob outside its reach: the
    // check derives what it names and the member's own mob, and refuses.
    let refused = harness
        .service
        .put_member_subscription(
            &op("r1"),
            PutSubscription {
                selector: Selector {
                    topics: topics(&["ios"]),
                    mobs: BTreeSet::from([mob("refunds")]),
                },
                ..put("r1", "agent:a", &["ios"], StartFrontier::Now)
            },
        )
        .await;
    assert!(
        matches!(refused, Err(AnnouncementError::UnrelatedMob(_))),
        "{refused:?}"
    );
    let recorded = |harness: &Harness| {
        let store = Arc::clone(&harness.store);
        async move {
            store
                .lineage()
                .await
                .unwrap()
                .into_iter()
                .map(|record| record.mob_id.to_string())
                .collect::<Vec<_>>()
        }
    };
    // Read before a reconcile could end a record of the root: there is none
    // to end, so none can leak.
    assert_eq!(recorded(&harness).await, ["refunds"]);
    harness.service.reconcile_lineage().await.unwrap();
    assert_eq!(recorded(&harness).await, ["refunds"]);
}

/// A member of a mob a member created, subscribed through the owner.
async fn subscribe_worker(
    harness: &Harness,
    id: &str,
) -> std::result::Result<Subscription, AnnouncementError> {
    harness
        .service
        .put_subscription(&op(id), put_for(id, worker(), &["ios"]))
        .await
}

fn worker_batches_query() -> BatchQuery {
    BatchQuery {
        member: Some(worker()),
        limit: 10,
        ..BatchQuery::default()
    }
}

fn child_native() -> ScriptedNative {
    let native = ScriptedNative::new();
    native.add_child_member("builds", "worker-1");
    native.log_event("builds", "spawned");
    native.log_event("builds", "spawned");
    native
}

#[tokio::test]
async fn a_member_of_a_mob_a_member_created_is_delivered_on_its_kept_route() {
    let harness = harness_with(child_native());
    let subscription = subscribe_worker(&harness, "w1").await.unwrap();
    assert_eq!(
        subscription.member_mob_cursor,
        Some(2),
        "made at the mob's current cursor"
    );
    let kept = harness
        .store
        .member_route(&worker())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(kept.route, MemberRoute::Child);
    assert_eq!(kept.incarnation, Some(harness.native.incarnation("builds")));
    harness.announce_burst(1).await;
    harness.clock.store(10_000_000, Ordering::SeqCst);
    harness.service.run_round().await.unwrap();
    let batches = harness
        .service
        .deliveries(&worker_batches_query())
        .await
        .unwrap();
    assert_eq!(batches.len(), 1);
    assert_eq!(batches[0].route, MemberRoute::Child);
    assert_eq!(batches[0].state.tag(), BatchStateTag::Delivered);
    assert_eq!(harness.native.submits(), 1);
    // A member not on its mob's roster, and a member of a completed mob,
    // are refused.
    assert!(matches!(
        harness
            .service
            .put_subscription(
                &op("w2"),
                put_for("w2", peer_member("worker-2", "builds"), &["ios"])
            )
            .await,
        Err(AnnouncementError::UnknownMember(_))
    ));
    harness
        .native
        .completed
        .lock()
        .unwrap()
        .insert("builds".into());
    assert!(matches!(
        subscribe_worker(&harness, "w3").await,
        Err(AnnouncementError::UnusableMember(_))
    ));
}

/// A completion or reset of the member's mob after the put ends the
/// subscription before any submission. No public surface completes or
/// resets a mob a member created (the agent mob tools have no lifecycle
/// verb), so these paths are covered here, against the scripted native
/// owner, and not by a process test.
#[tokio::test]
async fn a_completion_or_reset_after_the_put_ends_the_subscription_before_any_submission() {
    for kind in ["completed", "reset"] {
        let harness = harness_with(child_native());
        // A completion and reset before the put end nothing.
        harness.native.log_event("builds", "completed");
        harness.native.log_event("builds", "reset");
        subscribe_worker(&harness, "w1").await.unwrap();
        harness.native.log_event("builds", "spawned");
        let at = harness.native.log_event("builds", kind);
        harness.announce_burst(1).await;
        harness.clock.store(10_000_000, Ordering::SeqCst);
        harness.service.run_round().await.unwrap();
        let ended = harness
            .service
            .subscription(&sid("w1"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(ended.state, SubscriptionState::Ended, "{kind} at {at}");
        assert_eq!(
            ended.ended,
            Some(SubscriptionEnd::MemberMobEnded { why: kind.into() })
        );
        assert_eq!(harness.native.submits(), 0, "nothing reached the member");
        let batches = harness
            .service
            .deliveries(&worker_batches_query())
            .await
            .unwrap();
        assert!(
            batches
                .iter()
                .all(|batch| batch.state.tag() == BatchStateTag::Cancelled),
            "{batches:?}"
        );
        // Subscribed again after it, the member is delivered to.
        subscribe_worker(&harness, "w2").await.unwrap();
        harness
            .service
            .announce(announce_request("payments", "again", &["ios"], "x"))
            .await
            .unwrap();
        harness.round_after_pause().await;
        harness.clock.fetch_add(60_001, Ordering::SeqCst);
        harness.service.run_round().await.unwrap();
        assert_eq!(harness.native.submits(), 1, "{kind}");
    }
}

/// The settlement of every announcement owed to `subscription`, by
/// sequence.
fn owed_settlements(harness: &Harness, subscription: &str) -> Vec<(i64, Option<String>)> {
    let db =
        rusqlite::Connection::open(harness._dir.path().join("mob_announcements.sqlite3")).unwrap();
    let mut statement = db
        .prepare(
            "SELECT sequence, settlement FROM owed_announcements
             WHERE subscription_id = ?1 ORDER BY sequence",
        )
        .unwrap();
    statement
        .query_map([subscription], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .map(std::result::Result::unwrap)
        .collect()
}

/// A reset between a capture and the seal: the member subscribes again after
/// it, and the next announcement is owed to both subscriptions. The seal
/// reads the mob's log first, so the subscription the reset ended carries
/// nothing into the batch and what was captured before the reset never
/// reaches the member.
#[tokio::test]
async fn what_was_captured_before_a_reset_never_reaches_the_member_subscribed_again() {
    let harness = harness_with(child_native());
    subscribe_worker(&harness, "w1").await.unwrap();
    harness
        .service
        .announce(announce_request(
            "payments",
            "before",
            &["ios"],
            "captured before the reset",
        ))
        .await
        .unwrap();
    harness.native.log_event("builds", "reset");
    subscribe_worker(&harness, "w2").await.unwrap();
    harness
        .service
        .announce(announce_request(
            "payments",
            "after",
            &["ios"],
            "captured after the reset",
        ))
        .await
        .unwrap();
    harness.clock.store(10_000_000, Ordering::SeqCst);
    harness.service.run_round().await.unwrap();

    let ended = harness
        .service
        .subscription(&sid("w1"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        ended.ended,
        Some(SubscriptionEnd::MemberMobEnded {
            why: "reset".into()
        })
    );
    let owed = owed_settlements(&harness, "w1");
    assert_eq!(owed.len(), 2, "{owed:?}");
    assert!(
        owed.iter()
            .all(|(_, settlement)| settlement.as_deref() == Some("ended")),
        "{owed:?}"
    );
    assert_eq!(harness.native.submits(), 1);
    let batches = harness
        .service
        .deliveries(&worker_batches_query())
        .await
        .unwrap();
    assert_eq!(batches.len(), 1, "{batches:?}");
    let delivered = &batches[0];
    assert_eq!(delivered.state.tag(), BatchStateTag::Delivered);
    assert!(
        delivered
            .items
            .iter()
            .all(|item| item.subscription_id == sid("w2")),
        "{:?}",
        delivered.items
    );
    assert!(delivered.body.contains("captured after the reset"));
    assert!(!delivered.body.contains("captured before the reset"));
}

/// A mob created again under the same id ends what its members had in the
/// old incarnation at the mob set reconcile and at the read before a
/// delivery, not only at the next put.
#[tokio::test]
async fn a_mob_created_again_ends_its_members_subscriptions_at_reconcile_and_delivery() {
    let created_again = Some(SubscriptionEnd::MemberMobEnded {
        why: "created again".into(),
    });
    let harness = harness_with(child_native());
    subscribe_worker(&harness, "w1").await.unwrap();
    harness
        .native
        .incarnations
        .lock()
        .unwrap()
        .insert("builds".into(), incarnation("owner-2", 20));
    harness.service.reconcile_mobs().await.unwrap();
    let ended = harness
        .service
        .subscription(&sid("w1"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ended.ended, created_again);

    let harness = harness_with(child_native());
    subscribe_worker(&harness, "w1").await.unwrap();
    harness.announce_burst(1).await;
    harness
        .native
        .incarnations
        .lock()
        .unwrap()
        .insert("builds".into(), incarnation("owner-2", 20));
    harness.clock.store(10_000_000, Ordering::SeqCst);
    harness.service.run_round().await.unwrap();
    let ended = harness
        .service
        .subscription(&sid("w1"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ended.ended, created_again);
    assert_eq!(harness.native.submits(), 0, "nothing reached the member");
}

/// A backlog that cannot be sealed, such as a member's of a mob held until
/// its owner resolves, is reported once while it keeps failing, not at
/// every round; once its mob runs again it is delivered.
#[tokio::test]
async fn a_held_members_backlog_is_reported_once_while_it_waits() {
    let harness = harness_with(child_native());
    subscribe_worker(&harness, "w1").await.unwrap();
    harness.announce_burst(1).await;
    harness.native.held.lock().unwrap().insert("builds".into());
    harness.clock.store(10_000_000, Ordering::SeqCst);
    // A second registered dispatcher, so the interest in the warning's
    // callsite is never decided by another test thread's empty default.
    let _anchor = tracing::Dispatch::new(tracing_subscriber::Registry::default());
    let ((), log) = crate::test_diagnostics::capture_warnings(async {
        for _ in 0..3 {
            harness.round_after_pause().await;
        }
    })
    .await;
    assert_eq!(
        log.matches("announcement backlog not sealed this round")
            .count(),
        1,
        "{log}"
    );
    assert_eq!(harness.native.submits(), 0);
    harness.native.held.lock().unwrap().clear();
    harness.round_after_pause().await;
    assert_eq!(harness.native.submits(), 1);
}

#[tokio::test]
async fn the_member_mob_log_is_read_on_from_the_last_read() {
    let harness = harness_with(child_native());
    subscribe_worker(&harness, "w1").await.unwrap();
    harness.native.log_event("builds", "spawned");
    harness.native.log_event("builds", "spawned");
    harness.announce_burst(1).await;
    harness.clock.store(10_000_000, Ordering::SeqCst);
    harness.service.run_round().await.unwrap();
    assert_eq!(harness.native.submits(), 1);
    harness.native.log_event("builds", "spawned");
    harness
        .service
        .announce(announce_request("payments", "again", &["ios"], "x"))
        .await
        .unwrap();
    harness.clock.fetch_add(60_001, Ordering::SeqCst);
    harness.service.run_round().await.unwrap();
    assert_eq!(harness.native.submits(), 2);
    // The log is read at each seal and before each submission; every read
    // starts where the last one ended.
    assert_eq!(
        *harness.native.log_reads.lock().unwrap(),
        vec![2, 4, 4, 5],
        "each read starts where the last one ended"
    );
}

#[tokio::test]
async fn a_mob_created_again_or_destroyed_ends_what_its_members_had() {
    let harness = harness_with(child_native());
    subscribe_worker(&harness, "w1").await.unwrap();
    // Created again under the same id: the next put ends the old
    // incarnation's subscriptions and keeps the new incarnation.
    let second = incarnation("owner-2", 20);
    harness
        .native
        .incarnations
        .lock()
        .unwrap()
        .insert("builds".into(), second.clone());
    subscribe_worker(&harness, "w2").await.unwrap();
    let old = harness
        .service
        .subscription(&sid("w1"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        old.ended,
        Some(SubscriptionEnd::MemberMobEnded {
            why: "created again".into()
        })
    );
    let kept = harness
        .store
        .member_route(&worker())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(kept.incarnation, Some(second));
    // Destroyed: the mob set reconcile ends the member's subscriptions.
    harness
        .native
        .destroyed
        .lock()
        .unwrap()
        .insert("builds".into());
    harness.service.reconcile_mobs().await.unwrap();
    let gone = harness
        .service
        .subscription(&sid("w2"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        gone.ended,
        Some(SubscriptionEnd::MemberMobEnded {
            why: "destroyed".into()
        })
    );
}

#[tokio::test]
async fn a_held_mob_ends_nothing_and_its_members_wait() {
    let harness = harness_with(child_native());
    subscribe_worker(&harness, "w1").await.unwrap();
    harness.native.held.lock().unwrap().insert("builds".into());
    harness.announce_burst(1).await;
    harness.clock.store(10_000_000, Ordering::SeqCst);
    harness.service.run_round().await.unwrap();
    harness.service.reconcile_mobs().await.unwrap();
    let subscription = harness
        .service
        .subscription(&sid("w1"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(subscription.state, SubscriptionState::Active);
    assert_eq!(harness.native.submits(), 0);
    // A put while it is held is refused as unavailable, not as gone.
    assert!(matches!(
        subscribe_worker(&harness, "w2").await,
        Err(AnnouncementError::NativeUnavailable(_))
    ));
    harness.native.held.lock().unwrap().clear();
    harness.round_after_pause().await;
    assert_eq!(harness.native.submits(), 1, "delivered once it runs again");
}

#[tokio::test]
async fn a_committed_delivery_in_a_destroyed_mob_strands_without_a_successor() {
    let mut native = child_native();
    native.apply_on_commit = false;
    let harness = harness_with(native);
    subscribe_worker(&harness, "w1").await.unwrap();
    harness.announce_burst(1).await;
    harness.clock.store(10_000_000, Ordering::SeqCst);
    harness.service.run_round().await.unwrap();
    let batches = harness
        .service
        .deliveries(&worker_batches_query())
        .await
        .unwrap();
    assert_eq!(batches[0].state.tag(), BatchStateTag::Committed);
    // Still pending, as Meerkat reports a delivery to a mob it no longer
    // hosts: Meta knows the mob is gone for good.
    harness
        .native
        .destroyed
        .lock()
        .unwrap()
        .insert("builds".into());
    harness.round_after_pause().await;
    let batch = harness.store.batch(&batches[0].id).await.unwrap().unwrap();
    assert_eq!(batch.state.tag(), BatchStateTag::Stranded);
    assert_eq!(batch.successor, None, "the member's subscriptions ended");
    let ended = harness
        .service
        .subscription(&sid("w1"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        ended.ended,
        Some(SubscriptionEnd::MemberMobEnded {
            why: "destroyed".into()
        })
    );
    assert_eq!(harness.native.submits(), 1);
}

#[tokio::test]
async fn a_mob_id_hosted_here_and_filed_as_a_peer_is_ambiguous() {
    let mut native = child_native();
    native.peers.insert("builds".into());
    let harness = harness_with(native);
    assert!(matches!(
        subscribe_worker(&harness, "w1").await,
        Err(AnnouncementError::AmbiguousMob(mob_id)) if mob_id.as_str() == "builds"
    ));
    assert_eq!(harness.store.member_route(&worker()).await.unwrap(), None);
    // A member kept on the peer route is never rerouted to a mob hosted
    // here under the same id, and what it has stays as it is.
    let harness = harness_with(child_native());
    harness
        .store
        .put_subscription(&op("p1"), put_for("p1", worker(), &["ios"]), 1)
        .await
        .unwrap();
    assert!(matches!(
        subscribe_worker(&harness, "w1").await,
        Err(AnnouncementError::AmbiguousMob(_))
    ));
    let kept = harness
        .store
        .member_route(&worker())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(kept.route, MemberRoute::Peer);
    assert_eq!(
        harness
            .service
            .subscription(&sid("p1"))
            .await
            .unwrap()
            .unwrap()
            .state,
        SubscriptionState::Active
    );
}

#[test]
fn child_members_are_attributed_only_through_their_binding_and_their_roster() {
    let session = meerkat_core::types::SessionId::new();
    let own = mob(OWN_MOB);
    let bound = |mob_id: &str, role: &str| {
        Some(super::tools::SessionMemberBinding {
            mob_id: mob(mob_id),
            role: meerkat_mob::ProfileName::from(role),
            member: MemberIdentity::new("worker-1").unwrap(),
        })
    };
    let entry = |session: Option<&meerkat_core::types::SessionId>, role: &str| {
        Some(super::tools::RosterBinding {
            session: session.cloned(),
            role: meerkat_mob::ProfileName::from(role),
        })
    };
    let records = |binding, entry| super::tools::ChildCallerRecords { binding, entry };
    assert_eq!(
        super::tools::attribute_child(
            &own,
            &session,
            &records(bound("builds", "worker"), entry(Some(&session), "worker"))
        ),
        Ok(peer_member("worker-1", "builds"))
    );
    let refused = [
        ("no binding", records(None, entry(Some(&session), "worker"))),
        (
            "bound to this gateway's own mob",
            records(bound(OWN_MOB, "worker"), entry(Some(&session), "worker")),
        ),
        (
            "not on the roster",
            records(bound("builds", "worker"), None),
        ),
        (
            "another session on the roster",
            records(
                bound("builds", "worker"),
                entry(Some(&meerkat_core::types::SessionId::new()), "worker"),
            ),
        ),
        (
            "no session on the roster",
            records(bound("builds", "worker"), entry(None, "worker")),
        ),
        (
            "another role on the roster",
            records(bound("builds", "worker"), entry(Some(&session), "lead")),
        ),
    ];
    for (case, records) in refused {
        assert!(
            super::tools::attribute_child(&own, &session, &records).is_err(),
            "{case}"
        );
    }
}

#[test]
fn child_member_options_are_read_and_bounded() {
    let parsed = super::parse_runtime_options(&serde_json::json!({
        "enabled": true,
        "child_member_limits": { "subscriptions": 10, "per_mob": 4 }
    }))
    .unwrap()
    .unwrap();
    assert!(!parsed.member_tools_in_child_mobs);
    assert_eq!(
        parsed.child_limits,
        ChildMemberLimits {
            subscriptions: 10,
            per_mob: 4
        }
    );
    let defaults = super::parse_runtime_options(&serde_json::json!({ "enabled": true }))
        .unwrap()
        .unwrap();
    assert!(
        !defaults.member_tools_in_child_mobs,
        "the member tools are host-only by default"
    );
    assert_eq!(defaults.child_limits, ChildMemberLimits::default());
    let ceiling = super::parse_runtime_options(&serde_json::json!({
        "enabled": true,
        "member_tools_in_child_mobs": true
    }))
    .unwrap()
    .unwrap();
    assert!(
        ceiling.member_tools_in_child_mobs,
        "the option sets the ceiling of the child profiles' opt-in"
    );
    for limits in [
        serde_json::json!({ "subscriptions": 0 }),
        serde_json::json!({ "per_mob": 0 }),
        serde_json::json!({ "fan_out": 3 }),
    ] {
        assert!(
            super::parse_runtime_options(&serde_json::json!({
                "enabled": true,
                "child_member_limits": limits
            }))
            .is_err(),
            "{limits}"
        );
    }
}

#[tokio::test]
async fn a_put_past_a_child_member_limit_is_refused_typed() {
    let native = child_native();
    native.add_child_member("builds", "worker-2");
    let (dir, store) = temp_store();
    let clock = Arc::new(AtomicU64::new(1_000));
    let reader = Arc::clone(&clock);
    let service = AnnouncementService::with_clock(
        Arc::new(store) as Arc<dyn AnnouncementStore>,
        Arc::new(native) as Arc<dyn NativeDelivery>,
        AnnouncementConfig {
            child_limits: ChildMemberLimits {
                subscriptions: 1,
                per_mob: 1,
            },
            ..AnnouncementConfig::default()
        },
        Arc::new(move || reader.load(Ordering::SeqCst)),
    )
    .unwrap();
    service
        .put_subscription(&op("w1"), put_for("w1", worker(), &["ios"]))
        .await
        .unwrap();
    let refused = service
        .put_subscription(
            &op("w2"),
            put_for("w2", peer_member("worker-2", "builds"), &["ios"]),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(
            refused,
            AnnouncementError::Store(AnnouncementStoreError::ChildMemberLimit { limit: 1, .. })
        ),
        "{refused:?}"
    );
    drop(dir);
}

// ---------------------------------------------------------------------------
// Hosted listings
// ---------------------------------------------------------------------------

/// The agent mob tool state's listings as Meerkat answers them
/// (meerkat-mob-mcp 0.9.2, the same at 0.9.3; the lines are 0.9.3's
/// lib.rs):
/// - `held_child_mobs` restores best effort and answers empty when the
///   restore fails (3637-3646);
/// - `mob_list` and `mob_handles_snapshot` fail until the restore is done,
///   through `ensure_restored` (4496-4531);
/// - activation registers a held mob as managed before it drops the hold
///   (resume 3770-3776, teardown 3620-3625).
///
/// This gateway's own mob `root` runs, `running` runs, `gone` is destroyed
/// but still managed, and `child` is held after a restart. Calls are
/// numbered from 1: `child` activates right after the call numbered
/// `activate_after_call`, and the first `failed_restores` restore attempts
/// fail.
struct CoreListings(Mutex<CoreListingState>);

struct CoreListingState {
    calls: usize,
    activate_after_call: usize,
    failed_restores: usize,
    restored: bool,
    managed: BTreeMap<String, meerkat_mob::MobState>,
    held: BTreeSet<String>,
}

impl CoreListings {
    fn new(activate_after_call: usize, failed_restores: usize) -> Self {
        Self(Mutex::new(CoreListingState {
            calls: 0,
            activate_after_call,
            failed_restores,
            restored: false,
            managed: BTreeMap::from([
                ("root".to_string(), meerkat_mob::MobState::Running),
                ("running".to_string(), meerkat_mob::MobState::Running),
                ("gone".to_string(), meerkat_mob::MobState::Destroyed),
            ]),
            held: BTreeSet::from(["child".to_string()]),
        }))
    }

    /// One listing call: it tries the restore until one attempt is done,
    /// then reads with `answer`; `None` when its restore attempt failed.
    fn call<T>(&self, answer: impl FnOnce(&CoreListingState) -> T) -> Option<T> {
        let mut state = self.0.lock().unwrap();
        if !state.restored {
            if state.failed_restores > 0 {
                state.failed_restores -= 1;
            } else {
                state.restored = true;
            }
        }
        let answered = state.restored.then(|| answer(&state));
        state.calls += 1;
        if state.calls == state.activate_after_call {
            let activated = state.held.clone();
            // Managed first; only then is the hold dropped.
            for mob_id in &activated {
                state
                    .managed
                    .insert(mob_id.clone(), meerkat_mob::MobState::Running);
            }
            for mob_id in &activated {
                state.held.remove(mob_id);
            }
        }
        answered
    }

    fn calls(&self) -> usize {
        self.0.lock().unwrap().calls
    }
}

fn restore_failed() -> meerkat_mob::MobError {
    meerkat_mob::MobError::Internal("persistent mob state restore failed".to_string())
}

#[async_trait]
impl super::native::HostedListings for CoreListings {
    type Handle = ();

    async fn mob_list(
        &self,
    ) -> std::result::Result<Vec<(meerkat_mob::MobId, meerkat_mob::MobState)>, meerkat_mob::MobError>
    {
        self.call(|state| {
            state
                .managed
                .iter()
                .map(|(mob_id, phase)| (meerkat_mob::MobId::from(mob_id.as_str()), *phase))
                .collect()
        })
        .ok_or_else(restore_failed)
    }

    async fn held_child_mobs(&self) -> Vec<(meerkat_mob::MobId, meerkat_core::types::SessionId)> {
        self.call(|state| {
            state
                .held
                .iter()
                .map(|mob_id| {
                    (
                        meerkat_mob::MobId::from(mob_id.as_str()),
                        meerkat_core::types::SessionId::new(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
    }

    async fn mob_handles_snapshot(
        &self,
    ) -> std::result::Result<Vec<(meerkat_mob::MobId, ())>, meerkat_mob::MobError> {
        self.call(|state| {
            state
                .managed
                .iter()
                .filter(|(_, phase)| **phase != meerkat_mob::MobState::Destroyed)
                .map(|(mob_id, _)| (meerkat_mob::MobId::from(mob_id.as_str()), ()))
                .collect()
        })
        .ok_or_else(restore_failed)
    }
}

fn listed(values: &[&str]) -> BTreeSet<String> {
    values.iter().map(ToString::to_string).collect()
}

/// The first call is the managed listing that finishes the restore; the
/// held mob activates right after the second. Read held first, it is held
/// in the one listing and managed in the next. Read the other way round,
/// it would be in neither.
#[tokio::test]
async fn a_held_mob_that_activates_between_the_listings_stays_hosted() {
    let listings = CoreListings::new(2, 0);
    assert_eq!(
        super::native::hosted_set(&listings).await.unwrap(),
        listed(&["child", "root", "running"])
    );
    assert_eq!(listings.calls(), 3);

    // The order the origin of a mob and a member's own mob are read in.
    let listings = CoreListings::new(2, 0);
    let (held, running) = super::native::hosted_handles(&listings).await.unwrap();
    let running = running
        .into_iter()
        .map(|(mob_id, ())| mob_id.as_str().to_string())
        .collect::<BTreeSet<_>>();
    assert!(
        held.contains("child") || running.contains("child"),
        "held {held:?}, running {running:?}"
    );
    assert_eq!(listings.calls(), 3);
}

/// A restore that fails fails the read. The held listing alone answers
/// empty then, and a later listing whose own attempt restores would list
/// the managed mobs without the held one.
#[tokio::test]
async fn a_failed_restore_fails_the_hosted_listings() {
    let listings = CoreListings::new(usize::MAX, 1);
    super::native::hosted_set(&listings).await.unwrap_err();
    assert!(
        super::native::hosted_set(&listings)
            .await
            .unwrap()
            .contains("child")
    );

    let listings = CoreListings::new(usize::MAX, 1);
    super::native::hosted_handles(&listings).await.unwrap_err();
    let (held, _) = super::native::hosted_handles(&listings).await.unwrap();
    assert!(held.contains("child"), "{held:?}");
}

/// A hosted mob id this store cannot hold is skipped, so the other mobs
/// stay readable, and it is warned about once, not at every read.
#[tokio::test]
async fn a_hosted_mob_id_this_store_cannot_hold_is_skipped_and_warned_once() {
    let unusable = "m".repeat(MAX_ID_BYTES + 1);
    let listings = CoreListings::new(usize::MAX, 0);
    listings
        .0
        .lock()
        .unwrap()
        .managed
        .insert(unusable, meerkat_mob::MobState::Running);
    let warned = Mutex::new(BTreeSet::new());
    // A second registered dispatcher, so the interest in the warning's
    // callsite is never decided by another test thread's empty default.
    let _anchor = tracing::Dispatch::new(tracing_subscriber::Registry::default());
    let ((first, second), log) = crate::test_diagnostics::capture_warnings(async {
        let first = super::native::hosted_mob_ids_in(&listings, &mob("root"), &warned).await;
        let second = super::native::hosted_mob_ids_in(&listings, &mob("root"), &warned).await;
        (first, second)
    })
    .await;
    let readable = BTreeSet::from([mob("child"), mob("running")]);
    assert_eq!(first.unwrap(), readable);
    assert_eq!(second.unwrap(), readable);
    assert_eq!(
        log.matches("hosted mob id not usable here").count(),
        1,
        "{log}"
    );
}

/// The name after each `pub mod ` in a module root: the identifier, whatever
/// follows it, so a module declared with a trailing comment or inline is
/// still a module to walk.
fn declared_modules(module_root: &str) -> BTreeSet<&str> {
    module_root
        .lines()
        .filter_map(|line| line.strip_prefix("pub mod "))
        .map(|rest| {
            let end = rest
                .find(|c: char| !(c.is_alphanumeric() || c == '_'))
                .unwrap_or(rest.len());
            &rest[..end]
        })
        .collect()
}

/// A line with the attributes in front of it removed.
fn strip_attributes(mut line: &str) -> &str {
    while let Some(rest) = line.strip_prefix("#[") {
        let Some(end) = rest.find(']') else {
            return line;
        };
        line = rest[end + 1..].trim_start();
    }
    line
}

/// The crate modules a surface re-exports from with `pub use crate::...`.
fn reexported_modules<'a>(sources: impl IntoIterator<Item = &'a str>) -> BTreeSet<&'a str> {
    sources
        .into_iter()
        .flat_map(str::lines)
        .filter_map(|line| line.trim_start().strip_prefix("pub use crate::"))
        .map(|path| {
            let path = &path[..path.find('{').unwrap_or(path.len())];
            path.rsplit_once("::").map_or(path, |(module, _)| module)
        })
        .collect()
}

#[test]
fn the_walk_parses_every_declared_module_and_reexport() {
    assert_eq!(
        declared_modules("pub mod a;\npub mod b; // a note\npub mod c {\nmod d;"),
        BTreeSet::from(["a", "b", "c"])
    );
    assert_eq!(
        reexported_modules([
            "pub use crate::x::y::{A, B};\n    pub use crate::z::C;\npub use super::w::D;"
        ]),
        BTreeSet::from(["x::y", "z"])
    );
    assert_eq!(
        strip_attributes("#[cfg(test)] #[allow(x)] pub mod a;"),
        "pub mod a;"
    );
    assert_eq!(strip_attributes("pub enum A {"), "pub enum A {");
}

/// Every public enum of the announcement surface is `#[non_exhaustive]`, so a
/// later variant (a typed suspend cause, a typed end cause, a batch state) is
/// never a breaking change for a caller that matches on it, and no mirror of
/// one stays exhaustive. The walk reads each public module of the surface and
/// each crate module the surface re-exports from, and fails when either kind
/// joins the surface without being walked. It is a source test rather than a
/// compile_fail doc-test: CI runs no doc-tests.
#[test]
fn every_public_announcement_enum_is_non_exhaustive() {
    let public_modules = [
        ("availability", include_str!("availability.rs")),
        ("declarations", include_str!("declarations.rs")),
        ("escalations", include_str!("escalations.rs")),
        ("handling", include_str!("handling.rs")),
        ("native", include_str!("native.rs")),
        ("owner", include_str!("owner.rs")),
        ("remote", include_str!("remote.rs")),
        ("rpc", include_str!("rpc.rs")),
        ("sources", include_str!("sources.rs")),
        ("sqlite", include_str!("sqlite.rs")),
        ("store", include_str!("store.rs")),
        ("tools", include_str!("tools.rs")),
        ("types", include_str!("types.rs")),
    ];
    let module_root = include_str!("mod.rs");
    // Crate modules outside the surface whose items it re-exports, each with
    // the reason its enums are the surface's too.
    let reexported = [(
        "runtime::keyed_inject",
        include_str!("../runtime/keyed_inject.rs"),
        "native re-exports NativeStatus from it, and its keyed inject calls are \
         new with this surface",
    )];
    // Public enums deliberately left closed, each with the reason. None.
    let closed: [(&str, &str); 0] = [];
    let walked: BTreeSet<&str> = public_modules.iter().map(|(name, _)| *name).collect();
    assert_eq!(
        declared_modules(module_root),
        walked,
        "every public module of the surface is walked"
    );
    assert_eq!(
        reexported_modules(
            std::iter::once(module_root).chain(public_modules.iter().map(|(_, source)| *source))
        ),
        reexported
            .iter()
            .map(|(name, _, _)| *name)
            .collect::<BTreeSet<_>>(),
        "every crate module the surface re-exports from is walked"
    );
    // mod.rs is a child of the crate root: a `super::` re-export there
    // leaves the surface as a `crate::` one does, so it has none.
    assert!(
        !module_root
            .lines()
            .any(|line| line.trim_start().starts_with("pub use super::")),
        "mod.rs re-exports through super::; walk that module instead"
    );
    // A declaration either parser would skip, indented, inline or behind an
    // attribute on its own line, fails here rather than escaping both sides.
    let unparsed: Vec<String> = std::iter::once(("mod", module_root))
        .chain(public_modules)
        .chain(reexported.map(|(name, source, _)| (name, source)))
        .flat_map(|(module, source)| {
            source.lines().filter_map(move |line| {
                let bare = strip_attributes(line.trim_start());
                ((bare.starts_with("pub mod ") || bare.starts_with("pub enum "))
                    && bare.len() != line.len())
                .then(|| format!("{module}: {line}"))
            })
        })
        .collect();
    assert_eq!(
        unparsed,
        Vec::<String>::new(),
        "public declarations the walk cannot parse"
    );
    let mut open = Vec::new();
    for (module, source) in std::iter::once(("mod", module_root))
        .chain(public_modules)
        .chain(reexported.map(|(name, source, _)| (name, source)))
    {
        let lines: Vec<&str> = source.lines().collect();
        for (index, line) in lines.iter().enumerate() {
            let Some(rest) = line.strip_prefix("pub enum ") else {
                continue;
            };
            let name: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            let marked = lines[..index]
                .iter()
                .rev()
                .take_while(|line| line.starts_with("#["))
                .any(|line| *line == "#[non_exhaustive]");
            if !marked && !closed.iter().any(|(exempt, _)| *exempt == name) {
                open.push(format!("{module}::{name}"));
            }
        }
    }
    assert_eq!(
        open,
        Vec::<String>::new(),
        "public enums without #[non_exhaustive]"
    );
}

/// A stored batch record without its route does not decode: no route is
/// assumed for a batch, so it is never delivered as a local one by default.
#[tokio::test]
async fn a_batch_record_without_a_route_does_not_decode() {
    let (_dir, store) = temp_store();
    store
        .put_subscription(
            &op("s1"),
            put("s1", "agent:a", &["ios"], StartFrontier::Now),
            1,
        )
        .await
        .unwrap();
    store
        .announce(announce_request("payments", "k1", &["ios"], "one"), 2)
        .await
        .unwrap();
    let batch = store
        .seal_batch(&member("agent:a"), &batch_id("b1"), &policy(), 1_002)
        .await
        .unwrap()
        .expect("due after the delay");
    let mut record = serde_json::to_value(&batch).unwrap();
    let decoded =
        serde_json::from_value::<Batch>(record.clone()).map_err(|error| error.to_string());
    assert_eq!(decoded, Ok(batch));
    record.as_object_mut().unwrap().remove("route");
    let decoded = serde_json::from_value::<Batch>(record).map_err(|error| error.to_string());
    assert_eq!(
        decoded.map(|_| ()),
        Err("missing field `route`".to_string())
    );
}

/// A subscription reaches the store with the route the owner resolved for
/// its member. A request without one is refused and keeps nothing: the
/// store never assumes a route.
#[tokio::test]
async fn a_subscription_without_its_route_is_refused_by_the_store() {
    let (_dir, store) = temp_store();
    let refused = store
        .put_subscription(
            &op("s1"),
            PutSubscription {
                route: None,
                ..put("s1", "agent:a", &["ios"], StartFrontier::Now)
            },
            1,
        )
        .await;
    assert_eq!(
        refused.map(|_| ()),
        Err(AnnouncementStoreError::RouteRequired {
            member: member("agent:a")
        })
    );
    assert_eq!(store.member_route(&member("agent:a")).await.unwrap(), None);
    assert_eq!(store.subscription(&sid("s1")).await.unwrap(), None);
}

/// A local route names the member's own mob. A local route on a member of
/// another mob is refused, never delivered to this gateway's member with the
/// same identity; a member of a mob a member created keeps its plain id.
#[test]
fn a_local_route_names_the_members_own_mob_and_refuses_another() {
    use super::native::{OwnerRefusal, continuation_owner};
    let own = mob(OWN_MOB);
    let here = member("agent:a");
    let elsewhere = peer_member("agent:a", "builds");
    assert_eq!(
        continuation_owner(&own, &here, MemberRoute::Local),
        Ok(meerkat::ContinuationOwner::Member {
            mob_id: OWN_MOB.to_string(),
            identity: crate::member_comms_id::roster_member_id_for_identity("agent:a").to_string(),
        })
    );
    assert_eq!(
        continuation_owner(&own, &elsewhere, MemberRoute::Local),
        Err(OwnerRefusal::LocalRouteNamesAnotherMob {
            member: elsewhere.clone(),
            own: own.clone(),
        })
    );
    assert_eq!(
        continuation_owner(&own, &elsewhere, MemberRoute::Child),
        Ok(meerkat::ContinuationOwner::Member {
            mob_id: "builds".to_string(),
            identity: "agent:a".to_string(),
        })
    );
    assert_eq!(
        continuation_owner(&own, &elsewhere, MemberRoute::Peer),
        Err(OwnerRefusal::NoPeerPath)
    );
}

/// A kept local route for a member of another mob contradicts itself. The
/// batch is blocked, typed, before anything is prepared or ended: this
/// gateway's member of the same name is neither prepared nor ended, and the
/// subscription stays.
#[tokio::test]
async fn a_local_route_for_another_mobs_member_is_blocked_before_anything_is_touched() {
    let harness = harness_with(ScriptedNative::new());
    // This gateway's member of that name cannot take a delivery now:
    // preparing it would suspend deliveries for the wrong member.
    harness
        .native
        .waiting
        .lock()
        .unwrap()
        .insert("agent:a".into(), SuspendCause::MemberUnknown);
    let other = Member::new(mob("checkout"), MemberIdentity::new("agent:a").unwrap());
    let mut request = put_for("s1", other.clone(), &["ios"]);
    request.route = Some(local_route());
    harness
        .store
        .put_subscription(&op("o1"), request, 1)
        .await
        .unwrap();
    harness
        .service
        .announce(announce_request("payments", "k1", &["ios"], "one"))
        .await
        .unwrap();
    harness.clock.store(10_000_000, Ordering::SeqCst);
    harness.service.run_round().await.unwrap();
    let batches = harness
        .service
        .deliveries(&BatchQuery {
            member: Some(other.clone()),
            limit: 10,
            ..BatchQuery::default()
        })
        .await
        .unwrap();
    assert_eq!(batches.len(), 1, "{batches:?}");
    assert_eq!(
        batches[0].state,
        BatchState::Blocked {
            cause: super::native::OwnerRefusal::LocalRouteNamesAnotherMob {
                member: other,
                own: mob(OWN_MOB),
            }
            .to_string()
        }
    );
    assert!(
        harness.native.prepared.lock().unwrap().is_empty(),
        "nothing is prepared for the same-named member"
    );
    assert_eq!(harness.native.submits(), 0);
    let subscription = harness
        .service
        .subscription(&sid("s1"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        subscription.state,
        SubscriptionState::Active,
        "nothing is ended"
    );
}

/// A batch for another mob's member with a kept local route, left by
/// earlier code in `state`, submitting or committed, as a store holds it.
/// Returns the batch id and the typed state the guard must leave it in.
async fn contradicting_batch_left_in(state: BatchStateTag) -> (Harness, BatchId, BatchState) {
    let harness = harness_with(ScriptedNative::new());
    let other = Member::new(mob("checkout"), MemberIdentity::new("agent:a").unwrap());
    let mut request = put_for("s1", other.clone(), &["ios"]);
    request.route = Some(local_route());
    harness
        .store
        .put_subscription(&op("o1"), request, 1)
        .await
        .unwrap();
    harness
        .service
        .announce(announce_request("payments", "k1", &["ios"], "one"))
        .await
        .unwrap();
    let id = batch_id("b-earlier");
    harness
        .store
        .seal_batch(&other, &id, &harness.service.config().policy, 10_000_000)
        .await
        .unwrap()
        .expect("a backlog to seal");
    harness
        .store
        .transition_batch(
            &id,
            BatchStateTag::Sealed,
            BatchEvent::BeginSubmit,
            10_000_001,
        )
        .await
        .unwrap();
    if state == BatchStateTag::Committed {
        harness
            .store
            .transition_batch(
                &id,
                BatchStateTag::Submitting,
                BatchEvent::NativeCommitted {
                    receipt: receipt("b-earlier"),
                },
                10_000_002,
            )
            .await
            .unwrap();
    }
    assert_eq!(
        harness.store.batch(&id).await.unwrap().unwrap().state.tag(),
        state
    );
    harness.clock.store(10_000_003, Ordering::SeqCst);
    let blocked = BatchState::Blocked {
        cause: super::native::OwnerRefusal::LocalRouteNamesAnotherMob {
            member: other,
            own: mob(OWN_MOB),
        }
        .to_string(),
    };
    (harness, id, blocked)
}

/// What the native seam was asked for: status reads, submissions and input
/// reads.
fn native_calls(harness: &Harness) -> (u64, u64, u64) {
    let ledger = harness.native.ledger.lock().unwrap();
    (ledger.status_reads, ledger.submits, ledger.input_reads)
}

/// A batch earlier code left submitting or committed, with a route that
/// contradicts its member, is never polled: its next round and a reconcile
/// each block it, typed, with no status read, submission, input read or
/// preparation.
async fn a_contradicting_batch_left_in_is_blocked_unread(state: BatchStateTag) {
    let (harness, id, blocked) = contradicting_batch_left_in(state).await;
    harness.service.run_round().await.unwrap();
    assert_eq!(
        harness.store.batch(&id).await.unwrap().unwrap().state,
        blocked,
        "at the next round"
    );
    assert_eq!(
        native_calls(&harness),
        (0, 0, 0),
        "nothing is read or submitted"
    );
    assert!(harness.native.prepared.lock().unwrap().is_empty());

    let (harness, id, blocked) = contradicting_batch_left_in(state).await;
    let observed = harness.service.reconcile_delivery(&id).await.unwrap();
    assert_eq!(observed.batch.state, blocked, "at reconcile");
    assert!(
        matches!(observed.native, NativeObservation::Unavailable { .. }),
        "{:?}",
        observed.native
    );
    assert_eq!(
        native_calls(&harness),
        (0, 0, 0),
        "nothing is read or submitted"
    );
    assert!(harness.native.prepared.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_submitting_batch_with_a_contradicting_route_is_blocked_unread() {
    a_contradicting_batch_left_in_is_blocked_unread(BatchStateTag::Submitting).await;
}

#[tokio::test]
async fn a_committed_batch_with_a_contradicting_route_is_blocked_unread() {
    a_contradicting_batch_left_in_is_blocked_unread(BatchStateTag::Committed).await;
}

/// A member whose kept route is gone under an active subscription is not
/// sealed: the store, which keeps the route and seals on it, refuses typed,
/// and no route is assumed for it; another member's backlog still seals.
#[tokio::test]
async fn a_member_without_its_kept_route_is_not_sealed_and_others_still_are() {
    let mut native = ScriptedNative::new();
    native.known = ["agent:a".to_string(), "agent:b".to_string()].into();
    let harness = harness_with(native);
    harness.subscribe("s1").await;
    harness
        .service
        .put_subscription(
            &op("s2"),
            put("s2", "agent:b", &["ios"], StartFrontier::Now),
        )
        .await
        .unwrap();
    harness.announce_burst(2).await;
    let db =
        rusqlite::Connection::open(harness._dir.path().join("mob_announcements.sqlite3")).unwrap();
    db.execute(
        "DELETE FROM member_routes WHERE member = ?1",
        [member("agent:a").key()],
    )
    .unwrap();
    harness.clock.store(10_000_000, Ordering::SeqCst);
    assert_eq!(
        harness
            .store
            .seal_batch(&member("agent:a"), &batch_id("b9"), &policy(), 10_000_000)
            .await
            .map(|_| ()),
        Err(AnnouncementStoreError::RouteNotKept {
            member: member("agent:a")
        }),
        "the store assumes no route either"
    );
    assert_eq!(
        harness
            .service
            .seal_member(&member("agent:a"))
            .await
            .map(|_| ()),
        Err(AnnouncementError::Store(
            AnnouncementStoreError::RouteNotKept {
                member: member("agent:a")
            }
        ))
    );
    harness.service.run_round().await.unwrap();
    let batches_of = |who: &'static str| {
        let service = harness.service.clone();
        async move {
            service
                .deliveries(&BatchQuery {
                    member: Some(member(who)),
                    limit: 10,
                    ..BatchQuery::default()
                })
                .await
                .unwrap()
                .len()
        }
    };
    assert_eq!(
        batches_of("agent:a").await,
        0,
        "nothing is sealed without a route"
    );
    assert_eq!(batches_of("agent:b").await, 1, "another member still seals");
}

/// A self-contradicting route is never deliverable: a submission and a
/// status read both meet it as blocked and a put as an unusable member, by
/// the one classifier, and a member on a peer as not reachable from here now,
/// with nothing committed. A gateway that names no mob of its own holds no
/// local route.
#[test]
fn a_self_contradicting_route_is_invalid_to_a_submission() {
    let refusal = super::native::route_contradiction(
        Some(&mob(OWN_MOB)),
        &Member::new(mob("checkout"), MemberIdentity::new("agent:a").unwrap()),
        MemberRoute::Local,
    )
    .expect("a local route for another mob's member contradicts itself");
    assert!(matches!(
        refusal.submit_error(),
        NativeSubmitError::Blocked {
            cause: BlockCause::NativeInvalid,
            ..
        }
    ));
    assert!(matches!(
        refusal.status_error(),
        super::native::NativeStatusError::Blocked {
            cause: BlockCause::NativeInvalid,
            ..
        }
    ));
    assert!(matches!(
        super::native::OwnerRefusal::NoPeerPath.submit_error(),
        NativeSubmitError::NotCommitted { .. }
    ));
    assert!(matches!(
        super::native::OwnerRefusal::NoPeerPath.status_error(),
        super::native::NativeStatusError::Unavailable { .. }
    ));
    assert_eq!(
        super::native::route_contradiction(
            Some(&mob(OWN_MOB)),
            &member("agent:a"),
            MemberRoute::Local
        ),
        None
    );
    assert_eq!(
        refusal.member_problem(),
        MemberProblem::Unusable(refusal.to_string())
    );
    let without_own =
        super::native::route_contradiction(None, &member("agent:a"), MemberRoute::Local)
            .expect("no local route holds without an own mob");
    assert_eq!(
        without_own,
        super::native::OwnerRefusal::LocalRouteWithoutOwnMob {
            member: member("agent:a")
        }
    );
    assert!(matches!(
        without_own.submit_error(),
        NativeSubmitError::Blocked {
            cause: BlockCause::NativeInvalid,
            ..
        }
    ));
    assert!(matches!(
        without_own.status_error(),
        super::native::NativeStatusError::Blocked {
            cause: BlockCause::NativeInvalid,
            ..
        }
    ));
    assert_eq!(
        without_own.member_problem(),
        MemberProblem::Unusable(without_own.to_string())
    );
    assert_eq!(
        super::native::route_contradiction(None, &member("agent:a"), MemberRoute::Child),
        None
    );
}

/// A gateway whose native seam names no mob of its own cannot show that a
/// local route names the member's mob: the guard fails closed, and the batch
/// is blocked, typed, before anything is prepared, submitted or read.
#[tokio::test]
async fn a_local_route_on_a_gateway_without_its_own_mob_is_blocked_unread() {
    let mut native = ScriptedNative::new();
    native.own = None;
    let harness = harness_with(native);
    harness
        .store
        .put_subscription(
            &op("o1"),
            put("s1", "agent:a", &["ios"], StartFrontier::Now),
            1,
        )
        .await
        .unwrap();
    harness
        .service
        .announce(announce_request("payments", "k1", &["ios"], "one"))
        .await
        .unwrap();
    harness.clock.store(10_000_000, Ordering::SeqCst);
    harness.service.run_round().await.unwrap();
    let batch = harness.only_batch().await;
    assert_eq!(
        batch.state,
        BatchState::Blocked {
            cause: super::native::OwnerRefusal::LocalRouteWithoutOwnMob {
                member: member("agent:a")
            }
            .to_string()
        }
    );
    assert_eq!(
        native_calls(&harness),
        (0, 0, 0),
        "nothing is read or submitted"
    );
    assert!(harness.native.prepared.lock().unwrap().is_empty());
}

/// The ledger owns every table and index the schema creates, so drift or
/// absence in any of them is caught when the store opens.
#[test]
fn the_ledger_owns_every_object_the_schema_creates() {
    let mut connection = rusqlite::Connection::open_in_memory().unwrap();
    let tx = connection.transaction().unwrap();
    (super::storage::ANNOUNCEMENT_SCHEMA_DOMAIN.initialize_current)(&tx).unwrap();
    tx.commit().unwrap();
    let mut statement = connection
        .prepare("SELECT type, name FROM sqlite_master WHERE name NOT LIKE 'sqlite_%'")
        .unwrap();
    let created: BTreeSet<(String, String)> = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<std::result::Result<_, _>>()
        .unwrap();
    let owned: BTreeSet<(String, String)> = super::storage::ANNOUNCEMENT_SCHEMA_DOMAIN
        .owned_objects
        .iter()
        .map(|object| {
            let kind = match object.kind {
                meerkat_sqlite::SchemaObjectKind::Table => "table",
                meerkat_sqlite::SchemaObjectKind::Index => "index",
                meerkat_sqlite::SchemaObjectKind::Trigger => "trigger",
                meerkat_sqlite::SchemaObjectKind::View => "view",
            };
            (kind.to_string(), object.name.to_string())
        })
        .collect();
    assert_eq!(created, owned);
}

/// A member's announcement window is kept only while it holds an
/// announcement of the last minute: an emptied window is evicted, and a
/// refused announcement leaves no window behind, so the windows stay bounded
/// by the members announcing now.
#[tokio::test]
async fn announce_windows_are_evicted_once_empty() {
    let tools = tool_harness(super::tools::MemberLimits {
        announcements_per_minute: 5,
        announcement_bytes_per_minute: 100,
        subscriptions: 1,
        topics_per_subscription: 2,
    });
    let announce = |key: &str, body: String| serde_json::json!({ "topics": ["ios"], "body": body, "key": key });
    for who in ["agent:a", "agent:b"] {
        let (failed, result) = tools
            .call(
                Some(who),
                super::tools::ANNOUNCE_TOOL,
                announce(&format!("k-{who}"), "x".to_string()),
            )
            .await;
        assert!(!failed, "{who}: {result}");
    }
    assert_eq!(tools.tools.tracked_members(), 2);
    tools.harness.clock.fetch_add(60_000, Ordering::SeqCst);
    let (failed, result) = tools
        .call(
            Some("agent:a"),
            super::tools::ANNOUNCE_TOOL,
            announce("k-again", "x".to_string()),
        )
        .await;
    assert!(!failed, "{result}");
    assert_eq!(
        tools.tools.tracked_members(),
        1,
        "agent:b's emptied window is gone"
    );
    let (failed, result) = tools
        .call(
            Some("agent:b"),
            super::tools::ANNOUNCE_TOOL,
            announce("k-big", "x".repeat(101)),
        )
        .await;
    assert!(failed, "past the byte limit: {result}");
    assert_eq!(
        tools.tools.tracked_members(),
        1,
        "a refused announcement leaves no window"
    );
}

/// A seal takes its wire budget from the route it seals with, in the store's
/// own transaction: a member kept on a peer is sealed within one delivery
/// request whatever its caller read before, so a member re-keyed to a peer
/// after that read is never sealed without the peer's budget.
#[tokio::test]
async fn a_seal_takes_its_wire_budget_from_the_route_it_seals_with() {
    let (_dir, store) = temp_store();
    let remote = peer_member("agent:a", "mob-b");
    store
        .put_subscription(&op("o1"), put_for("s1", remote.clone(), &["ios"]), 1)
        .await
        .unwrap();
    let body = "x".repeat(2_000);
    for index in 0..40_u64 {
        store
            .announce(
                announce_request("payments", &format!("k{index}"), &["ios"], &body),
                2 + index,
            )
            .await
            .unwrap();
    }
    let batch = store
        .seal_batch(&remote, &batch_id("b1"), &policy(), 1_000_000)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(batch.route, MemberRoute::Peer);
    assert!(
        wire_len(&batch.body) <= MAX_REMOTE_BODY_WIRE_BYTES,
        "{}",
        wire_len(&batch.body)
    );
}

/// Version 1 of the announcement schema freezes with the release that ships
/// it: after that, a change is a new version with its migration. The pin makes
/// a change to version 1 fail here instead of at a user's store open.
#[test]
fn the_version_one_schema_is_pinned() {
    use sha2::{Digest, Sha256};
    use std::fmt::Write as _;
    let domain = &super::storage::ANNOUNCEMENT_SCHEMA_DOMAIN;
    assert_eq!(
        domain
            .migrations
            .first()
            .map(|migration| (migration.version, migration.name)),
        Some((1, "announcements-subscriptions-batches")),
        "version 1 stays the first migration"
    );
    let digest = Sha256::digest(super::sqlite::SCHEMA.as_bytes())
        .iter()
        .fold(String::new(), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        });
    assert_eq!(
        digest, "df9370bbddd52625335712536c533900998e667d483edc9dfa8f58003cb18695",
        "the version 1 schema changed: before the release that ships it, update this pin; \
         after it, add version 2 with its migration instead"
    );
}

/// The announce windows run on a monotonic clock, so nothing in them lies
/// ahead of now: a clock that steps back clears no member's limit, the
/// caller's or anyone else's.
#[tokio::test]
async fn a_clock_step_back_clears_no_members_limit() {
    let tools = tool_harness(super::tools::MemberLimits {
        announcements_per_minute: 1,
        announcement_bytes_per_minute: 100,
        subscriptions: 1,
        topics_per_subscription: 2,
    });
    let announce = |key: &str| serde_json::json!({ "topics": ["ios"], "body": "x", "key": key });
    tools.harness.clock.store(100_000, Ordering::SeqCst);
    let (failed, result) = tools
        .call(
            Some("agent:b"),
            super::tools::ANNOUNCE_TOOL,
            announce("k-b1"),
        )
        .await;
    assert!(!failed, "{result}");
    tools.harness.clock.store(70_000, Ordering::SeqCst);
    let (failed, result) = tools
        .call(
            Some("agent:a"),
            super::tools::ANNOUNCE_TOOL,
            announce("k-a1"),
        )
        .await;
    assert!(!failed, "{result}");
    let (failed, result) = tools
        .call(
            Some("agent:b"),
            super::tools::ANNOUNCE_TOOL,
            announce("k-b2"),
        )
        .await;
    assert!(failed, "agent:b's limit still holds: {result}");
}

/// The process test of a child mob member subscribing itself is parked while
/// child mob profiles cannot opt in to the member tools. It must run again
/// before the release that ships mob announcements: from that version on,
/// this fails while it is still parked.
#[test]
fn the_parked_self_subscription_process_test_runs_again_before_release() {
    const SELF_SUBSCRIPTION: &str =
        "fn a_member_of_a_mob_a_member_created_subscribes_itself_to_the_mob_above(";
    let source = include_str!("../../tests/mob_announcements_delivery_process.rs");
    let (before, _) = source
        .split_once(SELF_SUBSCRIPTION)
        .expect("the child mob self-subscription process test exists");
    // The attribute and doc lines directly above the test.
    let parked = before
        .lines()
        .rev()
        .map(str::trim_start)
        .take_while(|line| line.starts_with("#[") || line.starts_with("///"))
        .any(|line| line.starts_with("#[ignore"));
    let version: Vec<u64> = env!("CARGO_PKG_VERSION")
        .split(['.', '-', '+'])
        .take(3)
        .map(|part| part.parse().unwrap())
        .collect();
    let releases_announcements = version >= vec![0, 9, 4];
    assert!(
        !(parked && releases_announcements),
        "version {} would ship with the child mob self-subscription process test parked",
        env!("CARGO_PKG_VERSION")
    );
}

/// The identity-first roster of the gateway the direct status read runs on.
struct StatusRoster;

#[async_trait]
impl crate::identity_first::contracts::RosterProvider for StatusRoster {
    async fn roster(
        &self,
        _context: &crate::identity_first::RosterContext,
    ) -> std::result::Result<
        Vec<crate::identity_first::DurableAgentSpec>,
        crate::identity_first::RosterError,
    > {
        Ok(vec![crate::identity_first::DurableAgentSpec {
            identity: crate::identity_first::AgentIdentity::parse("agent:a")
                .expect("parse identity"),
            profile: meerkat_mob::ProfileName::from("personal"),
            addressability: crate::identity_first::AgentAddressability::Addressable,
            display_name: None,
            labels: BTreeMap::new(),
            context: None,
            additional_instructions: Vec::new(),
            initial_message: None,
            runtime_mode_override: None,
            backend: None,
            binding: None,
            placement: None,
        }])
    }
}

/// An identity-first gateway runtime whose own mob is `mob_id`, with
/// agent:a on its roster, for tests of its own native seam. Each test
/// names its own mob: the comms names a runtime registers are
/// process-wide, so two runtimes of one mob in parallel tests collide.
pub(super) async fn identity_first_test_runtime(
    state: &std::path::Path,
    mob_id: &str,
) -> crate::UnifiedRuntime {
    Box::pin(identity_first_test_builder(state, mob_id, "").await.build())
        .await
        .expect("build the identity-first runtime")
}

/// The builder of [`identity_first_test_runtime`], before `build()`.
/// `tools` adds lines to the member profile's tools table.
pub(super) async fn identity_first_test_builder(
    state: &std::path::Path,
    mob_id: &str,
    tools: &str,
) -> crate::UnifiedRuntimeBuilder {
    crate::UnifiedRuntimeBuilder::default()
        .definition(
            meerkat_mob::MobDefinition::from_toml(&format!(
                r#"
[mob]
id = "{mob_id}"

[profiles.personal]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.personal.tools]
comms = true
{tools}
"#
            ))
            .expect("definition"),
        )
        .persistent_state(state)
        .continuity_from_state_dir(state)
        .await
        .expect("open the state-dir identity substrate")
        .roster_provider(Arc::new(StatusRoster))
        .identity_bootstrap_mode(crate::identity_first::IdentityBootstrapMode::LazyMaterialize)
        .identity_runtime_instance_id(mob_id)
        .comms(true)
        .default_llm_client(Arc::new(meerkat_client::TestClient::default()))
}

/// No drive path reads a batch whose route contradicts its member: the
/// owner's guard runs first. A direct status read of one is terminal too:
/// the gateway's own native seam refuses it as blocked before anything is
/// read.
#[tokio::test(flavor = "multi_thread")]
async fn a_direct_status_read_of_a_contradicting_route_is_blocked() {
    let state = tempfile::tempdir().expect("state dir");
    let runtime = identity_first_test_runtime(state.path(), "mob-k-status").await;
    let delivery = super::local_delivery(&runtime).expect("the gateway's native seam");
    // A member of another mob with a kept local route: this gateway's own
    // mob is mob-k-status.
    let id = batch_id("b-contradicting");
    let body = render_batch(
        &id,
        None,
        &[],
        &["- [ios] payments #1 key=k\n  one\n".to_string()],
    );
    let batch = Batch {
        member: Member::new(mob("checkout"), MemberIdentity::new("agent:a").unwrap()),
        items: vec![BatchItem {
            subscription_id: sid("s1"),
            subscription_revision: 1,
            mob_id: mob("payments"),
            sequence: 1,
            source_key: SourceKey::new("k").unwrap(),
        }],
        gaps: Vec::new(),
        route: MemberRoute::Local,
        content_digest: content_digest(&body),
        body,
        id,
        predecessor: None,
        successor: None,
        state: BatchState::Submitting { attempt: 1 },
        sealed_at_ms: 1,
        updated_at_ms: 1,
    };
    let outcome = super::native::NativeDelivery::status(&delivery, &batch).await;
    assert!(
        matches!(
            outcome,
            Err(super::native::NativeStatusError::Blocked {
                cause: BlockCause::NativeInvalid,
                ..
            })
        ),
        "{outcome:?}"
    );
    runtime.shutdown().await;
}

/// A member of a mob a member created whose actor fail-stopped cannot be
/// subscribed: the gateway's own native seam refuses it as unusable ("has
/// stopped"), although the last phase the mob's actor published is live.
#[tokio::test(flavor = "multi_thread")]
async fn a_member_of_a_fail_stopped_child_mob_is_refused_as_unusable() {
    let state = tempfile::tempdir().expect("state dir");
    let runtime = identity_first_test_runtime(state.path(), "mob-k-fail-stopped").await;
    let delivery = super::local_delivery(&runtime).expect("the gateway's native seam");
    let mob_tools = runtime
        .mob_runtime()
        .agent_mob_mcp_state()
        .expect("the agent mob tool state");
    let child = meerkat_mob::MobId::from("mob-k-fail-stopped-child");
    meerkat_mob::fail_stop_next_actor_start_for_test(&child);
    let definition = meerkat_mob::MobDefinition::from_toml(&format!(
        "[mob]\nid = \"{child}\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n\
         external_addressable = true\nruntime_mode = \"turn_driven\"\n"
    ))
    .expect("the child's definition");
    mob_tools
        .mob_create_definition_with_owner_bridge_session(
            definition,
            meerkat_core::types::SessionId::new(),
            true,
            false,
        )
        .await
        .expect("the child mob is created");
    let handle = mob_tools
        .mob_handles_snapshot()
        .await
        .expect("the hosted mobs")
        .into_iter()
        .find(|(mob_id, _)| *mob_id == child)
        .map(|(_, handle)| handle)
        .expect("the child mob is hosted");
    tokio::time::timeout(
        std::time::Duration::from_secs(30),
        handle.actor_task_watch().wait(),
    )
    .await
    .expect("the child's actor stops");
    let observation = handle.lifecycle_observation();
    assert!(
        observation.failure.is_some(),
        "the child's actor fail-stopped"
    );

    let member = Member::new(mob(child.as_str()), MemberIdentity::new("worker").unwrap());
    let outcome = NativeDelivery::check_member(&delivery, &member, MemberRoute::Child).await;
    assert!(
        matches!(&outcome, Err(MemberProblem::Unusable(detail)) if detail.contains("has stopped")),
        "{outcome:?}"
    );
    runtime.shutdown().await;
}

/// A member this gateway's identity registry does not know may be
/// registered later: the gateway's own native seam has it wait, typed and
/// local, and ends nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_registry_miss_on_the_gateways_native_seam_waits_typed() {
    let state = tempfile::tempdir().expect("state dir");
    let runtime = identity_first_test_runtime(state.path(), "mob-k-registry").await;
    let delivery = super::local_delivery(&runtime).expect("the gateway's native seam");
    let outcome = delivery
        .prepare_identity(&MemberIdentity::new("agent:absent").unwrap())
        .await;
    assert!(
        matches!(
            outcome,
            Err(super::native::PrepareError::Suspended {
                cause: SuspendCause::MemberUnknown,
                evidence: EvidenceClass::Local,
            })
        ),
        "{outcome:?}"
    );
    runtime.shutdown().await;
}

/// A delivery that reaches its commit after its deadline commits nothing:
/// the gateway's own receiver refuses it as stale (`deadline_passed`), and
/// the delivery's native key holds no commit. The member's preparation the
/// deadline cut off is not lost: it runs to its end on its own, so the
/// member is ready without another request, and the retry commits once.
#[tokio::test(flavor = "multi_thread")]
async fn a_delivery_reaching_its_commit_after_its_deadline_commits_nothing() {
    use crate::identity_first::{AgentIdentity, IdentityLifecycleState};
    use crate::runtime::keyed_inject::{
        KeyedInject, KeyedInjectCallerScope, KeyedInjectReceiver, KeyedInjectRefusalCause,
        KeyedInjectRefusalCode, StaleRequestCause,
    };
    let state = tempfile::tempdir().expect("state dir");
    let runtime = identity_first_test_runtime(state.path(), "mob-k-deadline").await;
    let identities = runtime
        .identity_runtime_ref()
        .expect("an identity-first runtime");
    let agent = AgentIdentity::parse("agent:a").expect("identity");
    assert_ne!(
        identities.status(&agent).await.expect("a status").state,
        IdentityLifecycleState::Active,
        "the member is not prepared before the first delivery"
    );
    let local = Arc::new(super::local_delivery(&runtime).expect("the gateway's native seam"));
    let now = 1_800_000_000_000_u64;
    let server =
        super::remote::KeyedInjectServer::with_clock(local, 300_000, Arc::new(move || now));
    let scope = KeyedInjectCallerScope::for_grant_label("first-gateway");
    let body = "an announcement";
    let digest = content_digest(body);
    let delivery = KeyedInject {
        member: "agent:a",
        delivery_key: "batch-late",
        content_digest: &digest,
        summary: "1 announcement",
        body,
    };
    let refusal = server
        .deliver(&scope, delivery, now - 1)
        .await
        .expect_err("a passed deadline is refused");
    assert_eq!(refusal.code, KeyedInjectRefusalCode::StaleRequest);
    assert_eq!(
        refusal.cause,
        Some(KeyedInjectRefusalCause::Stale(
            StaleRequestCause::DeadlinePassed
        ))
    );
    let status = server
        .status(&scope, "agent:a", "batch-late", false)
        .await
        .expect("a status");
    assert_eq!(status.status, NativeStatus::NotCommitted);

    // The preparation the deadline cut off finishes without another request.
    let ready_by = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        let state = identities.status(&agent).await.expect("a status").state;
        if state == IdentityLifecycleState::Active {
            break;
        }
        assert!(
            std::time::Instant::now() < ready_by,
            "the cut-off preparation never finished: {state:?}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    // The retry commits once: a replay answers the same receipt.
    let receipt = server
        .deliver(&scope, delivery, now + 300_000)
        .await
        .expect("the retry commits");
    let replay = server
        .deliver(&scope, delivery, now + 300_000)
        .await
        .expect("the replay answers");
    assert_eq!(replay, receipt);
    let status = server
        .status(&scope, "agent:a", "batch-late", false)
        .await
        .expect("a status");
    assert!(
        matches!(
            &status.status,
            NativeStatus::Pending { receipt: held, .. } | NativeStatus::Applied { receipt: held, .. }
                if *held == receipt
        ),
        "{:?}",
        status.status
    );
    runtime.shutdown().await;
}

/// One `meta/mob_announcements/*` call through the handler every surface
/// dispatches to, as this host.
async fn host_call(
    service: &AnnouncementService,
    own_mob: &MobId,
    method: &str,
    params: serde_json::Value,
) -> serde_json::Value {
    super::rpc::handle_announcement_method(Some(service), own_mob, method, &params)
        .await
        .unwrap_or_else(|error| panic!("{method}: {error:?}"))
}

/// A runtime the builder composes mob announcements on gets the owner the
/// gateways get: attached, its dispatcher delivering to a member whose
/// profile names the member tool bundle the builder registered, and stopped
/// by the runtime's shutdown. The builder's control listener serves keyed
/// injects with [`crate::runtime::keyed_inject::DEFAULT_KEYED_INJECT_WINDOW_MS`]
/// unless told otherwise, so that is the window the owner assumes its peers
/// apply, whatever the configuration said.
#[tokio::test(flavor = "multi_thread")]
async fn the_builder_composes_mob_announcements_like_every_gateway() {
    let state = tempfile::tempdir().expect("state dir");
    let config = AnnouncementConfig {
        policy: SealingPolicy {
            max_count: 1,
            max_bytes: 1_048_576,
            max_delay_ms: 50,
            min_wake_interval_ms: 0,
        },
        peer_request_window_ms: 1,
        ..AnnouncementConfig::default()
    };
    let tools = format!("rust_bundles = [\"{}\"]", super::tools::TOOL_BUNDLE);
    let runtime = Box::pin(
        identity_first_test_builder(state.path(), "mob-k-builder", &tools)
            .await
            .mob_announcements(config)
            .build(),
    )
    .await
    .expect("the builder composes mob announcements");
    let service = runtime
        .mob_announcements()
        .expect("the builder attached the owner to its runtime");
    assert_eq!(
        service.config().peer_request_window_ms,
        crate::runtime::keyed_inject::DEFAULT_KEYED_INJECT_WINDOW_MS,
        "the owner assumes the window the runtime's own keyed inject plane serves with"
    );
    let own_mob = mob("mob-k-builder");
    let member = serde_json::json!({ "mob_id": "mob-k-builder", "identity": "agent:a" });
    let subscribed = host_call(
        &service,
        &own_mob,
        "meta/mob_announcements/subscriptions/put",
        serde_json::json!({
            "operation_id": "op-1",
            "subscription_id": "sub-1",
            "member": member,
            "topics": ["t"]
        }),
    )
    .await;
    assert_eq!(
        subscribed["subscription"]["state"], "active",
        "{subscribed}"
    );
    let announced = host_call(
        &service,
        &own_mob,
        "meta/mob_announcements/announce",
        serde_json::json!({ "source_key": "k-1", "topics": ["t"], "body": "composed by the builder" }),
    )
    .await;
    assert_eq!(announced["captured"], 1, "{announced}");
    // The dispatcher the composition started delivers the member's batch
    // through the runtime's own native seam.
    let deadline = std::time::Instant::now() + std::time::Duration::from_mins(1);
    loop {
        let listed = host_call(
            &service,
            &own_mob,
            "meta/mob_announcements/deliveries/list",
            serde_json::json!({ "member": member }),
        )
        .await;
        let state = &listed["deliveries"][0]["state"]["state"];
        if state == "committed" || state == "delivered" {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the member's batch was never committed natively: {listed}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    runtime.shutdown().await;
    assert!(
        service.is_stopped(),
        "the runtime's shutdown stops the dispatcher"
    );
    // A gateway may stop it as well: a second stop changes nothing.
    service.stop();
    assert!(service.is_stopped());
}

/// The builder refuses mob announcements it cannot compose before it builds
/// anything: without the persistent state that holds their database,
/// without an identity-first runtime, with the gateway-wide child mob grant
/// of the member tools, or with the member tool bundle's name taken.
#[tokio::test(flavor = "multi_thread")]
async fn the_builder_refuses_mob_announcements_it_cannot_compose() {
    fn refusal(
        result: std::result::Result<crate::UnifiedRuntime, crate::UnifiedRuntimeBuilderError>,
    ) -> String {
        match result {
            Err(crate::UnifiedRuntimeBuilderError::ConflictingConfiguration(message)) => message,
            Err(other) => panic!("expected a configuration refusal, got {other:?}"),
            Ok(_) => panic!("expected a configuration refusal, got a runtime"),
        }
    }
    let definition = || {
        meerkat_mob::MobDefinition::from_toml("[mob]\nid = \"mob-k-refused\"\n")
            .expect("definition")
    };
    let message = refusal(
        crate::UnifiedRuntimeBuilder::default()
            .definition(definition())
            .mob_announcements(AnnouncementConfig::default())
            .build()
            .await,
    );
    assert!(message.contains("requires persistent_state()"), "{message}");
    // Identity-first on an external substrate, with no state directory to
    // hold the announcement database.
    let scratch = tempfile::tempdir().expect("scratch dir");
    let message = refusal(
        crate::UnifiedRuntimeBuilder::default()
            .definition(definition())
            .continuity_store(Arc::new(
                crate::identity_first::LocalContinuityStore::in_memory()
                    .expect("in-memory continuity store"),
            ))
            .lease_provider(Arc::new(crate::identity_first::LocalLeaseProvider::new()))
            .roster_provider(Arc::new(StatusRoster))
            .scratch_dir(scratch.path())
            .ephemeral_runtime_store(true)
            .mob_announcements(AnnouncementConfig::default())
            .build()
            .await,
    );
    assert!(message.contains("requires persistent_state()"), "{message}");
    let state = tempfile::tempdir().expect("state dir");
    let message = refusal(
        crate::UnifiedRuntimeBuilder::default()
            .definition(definition())
            .persistent_state(state.path())
            .mob_announcements(AnnouncementConfig::default())
            .build()
            .await,
    );
    assert!(message.contains("an identity-first runtime"), "{message}");
    let state = tempfile::tempdir().expect("state dir");
    let message = refusal(
        identity_first_test_builder(state.path(), "mob-k-refused", "")
            .await
            .register_tool_bundle(
                super::tools::TOOL_BUNDLE,
                Arc::new(super::tools::MobAnnouncementTools::new(Arc::new(
                    std::sync::OnceLock::new(),
                ))),
            )
            .mob_announcements(AnnouncementConfig::default())
            .build()
            .await,
    );
    assert!(
        message.contains("is the mob announcement member tools"),
        "{message}"
    );
}

/// Every constructor takes the ceiling of the member tools in child mobs:
/// the options parser carries it, the composition offers the member tool
/// bundle to child mobs under it (never supplies it to every child
/// profile) and keeps it the host's alone without it, and the owner is
/// built either way.
#[test]
fn every_constructor_takes_the_member_tools_ceiling_for_child_mobs() {
    assert!(matches!(
        super::parse_options(
            &serde_json::json!({ "enabled": false, "member_tools_in_child_mobs": true }),
            "mob_announcements",
        ),
        Ok(None)
    ));
    let parsed = super::parse_options(
        &serde_json::json!({ "enabled": true, "member_tools_in_child_mobs": true }),
        "mob_announcements",
    )
    .unwrap()
    .unwrap();
    assert!(parsed.member_tools_in_child_mobs);
    let granted = AnnouncementConfig {
        member_tools_in_child_mobs: true,
        ..AnnouncementConfig::default()
    };
    let (_, _, availability) = super::AnnouncementComposition::new(granted.clone())
        .expect("the ceiling composes")
        .member_tool_bundle();
    assert_eq!(
        availability,
        meerkat_mob_mcp::ChildToolBundleAvailability::ChildOffered
    );
    let (_, _, availability) = super::AnnouncementComposition::new(AnnouncementConfig::default())
        .expect("the default composes")
        .member_tool_bundle();
    assert_eq!(
        availability,
        meerkat_mob_mcp::ChildToolBundleAvailability::HostOnly
    );
    let (_dir, store) = temp_store();
    let service = AnnouncementService::new(
        Arc::new(store) as Arc<dyn AnnouncementStore>,
        Arc::new(ScriptedNative::new()) as Arc<dyn NativeDelivery>,
        granted,
    )
    .expect("the owner is built under the ceiling");
    assert!(service.config().member_tools_in_child_mobs);
}

/// A runtime the builder composes takes the ceiling as every gateway does.
#[tokio::test(flavor = "multi_thread")]
async fn the_builder_composes_mob_announcements_under_the_member_tools_ceiling() {
    let state = tempfile::tempdir().expect("state dir");
    let runtime = Box::pin(
        identity_first_test_builder(state.path(), "mob-k-ceiling", "")
            .await
            .mob_announcements(AnnouncementConfig {
                member_tools_in_child_mobs: true,
                ..AnnouncementConfig::default()
            })
            .build(),
    )
    .await
    .expect("the ceiling composes on a builder runtime");
    assert!(
        runtime
            .mob_announcements()
            .expect("attached")
            .config()
            .member_tools_in_child_mobs
    );
    runtime.shutdown().await;
}

/// A batch owed again after a refused submission waits out the quiet window
/// after its last request before the retention pass expires it: an earlier
/// request for it may still commit until then.
#[tokio::test]
async fn a_batch_owed_again_waits_out_the_quiet_window_before_it_expires() {
    const QUIET: u64 = 330_000;
    let (_dir, store) = temp_store();
    store
        .put_subscription(
            &op("o1"),
            put("s1", "agent:a", &["ios"], StartFrontier::Now),
            1_000,
        )
        .await
        .unwrap();
    store
        .announce(announce_request("payments", "k1", &["ios"], "owed"), 1_000)
        .await
        .unwrap();
    let batch = store
        .seal_batch(&member("agent:a"), &batch_id("b1"), &policy(), 2_000)
        .await
        .unwrap()
        .expect("the announcement is due");
    // Submitted, then refused, signed, just before the retention deadline:
    // owed again, with its last request inside the quiet window.
    let refused_at = 2_000 + WINDOW - 10;
    store
        .transition_batch(
            &batch.id,
            BatchStateTag::Sealed,
            BatchEvent::BeginSubmit,
            refused_at - 5,
        )
        .await
        .unwrap();
    store
        .transition_batch(
            &batch.id,
            BatchStateTag::Submitting,
            BatchEvent::SubmitRefused,
            refused_at,
        )
        .await
        .unwrap();
    let report = store
        .expire(2_000 + WINDOW, WINDOW, QUIET, 100)
        .await
        .unwrap();
    assert_eq!(
        report.batches, 0,
        "sealed before the window, but quiet too recently"
    );
    let report = store
        .expire(refused_at + QUIET - 1, WINDOW, QUIET, 100)
        .await
        .unwrap();
    assert_eq!(report.batches, 0);
    let report = store
        .expire(refused_at + QUIET, WINDOW, QUIET, 100)
        .await
        .unwrap();
    assert_eq!(report.batches, 1, "past the quiet window it expires");
    assert_eq!(
        store.batch(&batch.id).await.unwrap().unwrap().state,
        BatchState::Expired
    );
}

/// A clock skew is one wait whichever request it was read from: the marker
/// keeps when it was first seen and shows the latest request.
#[test]
fn a_clock_skew_read_from_a_later_request_is_the_same_wait() {
    let first = SuspendCause::ClockSkew {
        issued_at_ms: 1_000,
        window_ms: 300_000,
    };
    let later = SuspendCause::ClockSkew {
        issued_at_ms: 31_000,
        window_ms: 300_000,
    };
    assert!(first.same_wait(&later));
    assert!(!first.same_wait(&SuspendCause::ClockSkew {
        issued_at_ms: 1_000,
        window_ms: 60_000,
    }));
    assert!(!first.same_wait(&SuspendCause::MemberNotLive));
}

// ---------------------------------------------------------------------------
// Announcement sources: the record, its generations, its identity and the
// topics it owns, through the one handler every gateway dispatches to.
// ---------------------------------------------------------------------------

pub(super) const SOURCES_PUT: &str = "meta/mob_announcements/sources/put";
pub(super) const SOURCES_GET: &str = "meta/mob_announcements/sources/get";
pub(super) const SOURCES_LIST: &str = "meta/mob_announcements/sources/list";
const SOURCES_END: &str = "meta/mob_announcements/sources/end";
const ANNOUNCE: &str = "meta/mob_announcements/announce";

/// One `meta/mob_announcements/*` call as this host, keeping a refusal.
pub(super) async fn try_host_call(
    service: &AnnouncementService,
    method: &str,
    params: serde_json::Value,
) -> std::result::Result<serde_json::Value, crate::rpc::JsonRpcError> {
    super::rpc::handle_announcement_method(Some(service), &mob(OWN_MOB), method, &params).await
}

/// The typed kind a refused call carries.
fn refusal_kind(error: &crate::rpc::JsonRpcError) -> Option<String> {
    error
        .data
        .as_ref()
        .and_then(|data| data.get("kind"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

/// `sources/put` params for a mailbox source of this gateway's mob, read
/// through a Connect setup whose verified account has `subject`.
pub(super) fn source_params(
    operation: &str,
    source_id: &str,
    subject: &str,
    topic: &[&str],
) -> serde_json::Value {
    serde_json::json!({
        "operation_id": operation,
        "source_id": source_id,
        "identity": {
            "provider": "google.gmail",
            "principal": {
                "kind": "connector_verified_account",
                "issuer": "https://accounts.example",
                "strategy_id": "oauth-code",
                "subject": subject,
            },
            "scope": { "kind": "mailbox_label", "id": "INBOX" },
        },
        "connection": { "kind": "connect_setup", "id": "setup-1" },
        "observation": { "baseline": "delta", "deletions": "explicit" },
        "topics": topic,
        "audience": [
            { "kind": "member", "member": { "mob_id": OWN_MOB, "identity": "agent:a" } }
        ],
    })
}

pub(super) fn with(
    mut params: serde_json::Value,
    key: &str,
    value: serde_json::Value,
) -> serde_json::Value {
    params[key] = value;
    params
}

/// A source is declared, read and listed whole: a first generation at its
/// first revision, active, baselining from no cursor, with the defaults the
/// contract names. A replay of the same operation answers the original, and
/// a list never carries the cursor token.
#[tokio::test]
async fn a_source_is_declared_and_read_back_whole() {
    let harness = harness_with(ScriptedNative::new());
    let params = source_params("o1", "inbox", "subject-1", &["mail.received"]);
    let put = try_host_call(&harness.service, SOURCES_PUT, params.clone())
        .await
        .unwrap();
    let source = &put["source"];
    assert_eq!(source["mob_id"], serde_json::json!(OWN_MOB));
    assert_eq!(source["source_id"], serde_json::json!("inbox"));
    assert_eq!(source["generation"], serde_json::json!(1));
    assert_eq!(source["revision"], serde_json::json!(1));
    assert_eq!(source["state"], serde_json::json!("active"));
    assert_eq!(source["collection"], serde_json::json!("baselining"));
    assert_eq!(
        source["cursor"],
        serde_json::json!({ "version": 0, "token": null, "advanced_at_ms": null })
    );
    assert_eq!(
        source["identity"], params["identity"],
        "the identity is kept as declared"
    );
    assert_eq!(
        source["observation"],
        serde_json::json!({ "baseline": "delta", "deletions": "explicit" })
    );
    assert_eq!(source["routes"], serde_json::json!([{ "kind": "direct" }]));
    assert_eq!(
        source["delivery"],
        serde_json::json!({
            "priority": "normal", "max_per_batch": 20, "on_unhandled": "redeliver"
        })
    );
    assert_eq!(source["start"], serde_json::json!({ "kind": "baseline" }));

    let replay = try_host_call(&harness.service, SOURCES_PUT, params)
        .await
        .unwrap();
    assert_eq!(replay, put, "a replay answers the original");

    let got = try_host_call(
        &harness.service,
        SOURCES_GET,
        serde_json::json!({ "source_id": "inbox" }),
    )
    .await
    .unwrap();
    assert_eq!(got["source"], *source);

    let listed = try_host_call(&harness.service, SOURCES_LIST, serde_json::json!({}))
        .await
        .unwrap();
    let listed = listed["sources"].as_array().cloned().unwrap_or_default();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["source_id"], serde_json::json!("inbox"));
    assert!(
        listed[0]["cursor"].get("token").is_none(),
        "a list never carries the cursor token: {}",
        listed[0]["cursor"]
    );
}

/// Every source declares how its adapter baselines and whether deletions are
/// observable; there is no capability "none".
#[tokio::test]
async fn a_source_declares_its_baseline_and_deletion_capability() {
    let harness = harness_with(ScriptedNative::new());
    let mut missing = source_params("o1", "inbox", "subject-1", &["mail.received"]);
    missing.as_object_mut().unwrap().remove("observation");
    let refused = try_host_call(&harness.service, SOURCES_PUT, missing)
        .await
        .unwrap_err();
    assert_eq!(refused.code, -32602, "{refused:?}");

    let none = with(
        source_params("o2", "inbox", "subject-1", &["mail.received"]),
        "observation",
        serde_json::json!({ "baseline": "none", "deletions": "explicit" }),
    );
    let refused = try_host_call(&harness.service, SOURCES_PUT, none)
        .await
        .unwrap_err();
    assert_eq!(refused.code, -32602, "{refused:?}");

    let snapshot = with(
        source_params("o3", "inbox", "subject-1", &["mail.received"]),
        "observation",
        serde_json::json!({ "baseline": "snapshot", "deletions": "unobservable" }),
    );
    let put = try_host_call(&harness.service, SOURCES_PUT, snapshot)
        .await
        .unwrap();
    assert_eq!(
        put["source"]["observation"],
        serde_json::json!({ "baseline": "snapshot", "deletions": "unobservable" })
    );
}

/// No identity evidence, no source: a principal is required, and only the
/// declared kinds are accepted.
#[tokio::test]
async fn a_source_without_a_principal_is_refused() {
    let harness = harness_with(ScriptedNative::new());
    let mut missing = source_params("o1", "inbox", "subject-1", &["mail.received"]);
    missing["identity"]
        .as_object_mut()
        .unwrap()
        .remove("principal");
    let refused = try_host_call(&harness.service, SOURCES_PUT, missing)
        .await
        .unwrap_err();
    assert_eq!(refused.code, -32602, "{refused:?}");

    let mut slot = source_params("o2", "inbox", "subject-1", &["mail.received"]);
    slot["identity"]["principal"] =
        serde_json::json!({ "kind": "credential_slot", "realm": "r", "account": "shared" });
    let refused = try_host_call(&harness.service, SOURCES_PUT, slot)
        .await
        .unwrap_err();
    assert_eq!(refused.code, -32602, "a credential slot is not a principal");
}

/// A host-attested principal is accepted only from a host connection: it is
/// the host vouching for an account, never a verified connector account.
#[tokio::test]
async fn a_host_attested_principal_needs_a_host_connection() {
    let harness = harness_with(ScriptedNative::new());
    let attested = |operation: &str, connection: serde_json::Value| {
        let mut params = source_params(operation, "inbox", "unused", &["mail.received"]);
        params["identity"]["principal"] = serde_json::json!({
            "kind": "host_attested", "attester": "host-app", "account": "shared-mail"
        });
        params["connection"] = connection;
        params
    };
    let refused = try_host_call(
        &harness.service,
        SOURCES_PUT,
        attested(
            "o1",
            serde_json::json!({ "kind": "connect_setup", "id": "setup-1" }),
        ),
    )
    .await
    .unwrap_err();
    assert_eq!(refused.code, super::rpc::ANNOUNCEMENTS_REFUSED_CODE);
    assert_eq!(
        refusal_kind(&refused).as_deref(),
        Some("principal_not_allowed")
    );
    assert_eq!(
        refused
            .data
            .as_ref()
            .and_then(|data| data.get("principal_kind")),
        Some(&serde_json::json!("host_attested")),
        "the refused principal's kind rides beside the refusal kind, never over it"
    );
    let refused = try_host_call(
        &harness.service,
        SOURCES_PUT,
        attested("o2", serde_json::Value::Null),
    )
    .await
    .unwrap_err();
    assert_eq!(
        refusal_kind(&refused).as_deref(),
        Some("principal_not_allowed")
    );
    let put = try_host_call(
        &harness.service,
        SOURCES_PUT,
        attested(
            "o3",
            serde_json::json!({ "kind": "host", "id": "host-mail" }),
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        put["source"]["identity"]["principal"]["kind"],
        serde_json::json!("host_attested")
    );
}

/// One principal's scope is one live source per mob: a second source for the
/// same account and scope is refused naming the first; another account's
/// same scope is another source; an ended source frees its identity.
#[tokio::test]
async fn one_principal_scope_is_one_source_per_mob() {
    let harness = harness_with(ScriptedNative::new());
    try_host_call(
        &harness.service,
        SOURCES_PUT,
        source_params("o1", "inbox", "subject-1", &["mail.received"]),
    )
    .await
    .unwrap();
    let refused = try_host_call(
        &harness.service,
        SOURCES_PUT,
        source_params("o2", "inbox-again", "subject-1", &["mail.other"]),
    )
    .await
    .unwrap_err();
    assert_eq!(refused.code, super::rpc::ANNOUNCEMENTS_REFUSED_CODE);
    assert_eq!(
        refusal_kind(&refused).as_deref(),
        Some("source_identity_taken")
    );
    assert_eq!(
        refused.data.as_ref().unwrap()["source_id"],
        serde_json::json!("inbox")
    );
    try_host_call(
        &harness.service,
        SOURCES_PUT,
        source_params("o3", "inbox-bob", "subject-2", &["mail.bob"]),
    )
    .await
    .unwrap();
    try_host_call(
        &harness.service,
        SOURCES_END,
        serde_json::json!({ "operation_id": "o4", "source_id": "inbox", "expected_revision": 1 }),
    )
    .await
    .unwrap();
    try_host_call(
        &harness.service,
        SOURCES_PUT,
        source_params("o5", "inbox-again", "subject-1", &["mail.other"]),
    )
    .await
    .unwrap();
}

/// A source owns its topics: another source cannot claim them, and a plain
/// announcement on one is refused, from the host and from a member's tool
/// alike. Other topics are untouched.
#[tokio::test]
async fn a_source_owns_its_topics_and_plain_announces_are_refused_there() {
    let tools = tool_harness(super::tools::MemberLimits::default());
    let service = &tools.harness.service;
    try_host_call(
        service,
        SOURCES_PUT,
        source_params("o1", "inbox", "subject-1", &["mail.received"]),
    )
    .await
    .unwrap();
    let claimed = try_host_call(
        service,
        SOURCES_PUT,
        source_params("o2", "other", "subject-2", &["mail.received"]),
    )
    .await
    .unwrap_err();
    assert_eq!(claimed.code, super::rpc::ANNOUNCEMENTS_REFUSED_CODE);
    assert_eq!(refusal_kind(&claimed).as_deref(), Some("topic_owned"));
    assert_eq!(
        claimed.data.as_ref().unwrap()["source_id"],
        serde_json::json!("inbox")
    );

    let host = try_host_call(
        service,
        ANNOUNCE,
        serde_json::json!({ "source_key": "k1", "topics": ["mail.received"], "body": "b" }),
    )
    .await
    .unwrap_err();
    assert_eq!(host.code, super::rpc::ANNOUNCEMENTS_REFUSED_CODE);
    assert_eq!(
        refusal_kind(&host).as_deref(),
        Some("topic_owned_by_source")
    );
    assert_eq!(
        host.data.as_ref().unwrap()["source_id"],
        serde_json::json!("inbox")
    );

    let (failed, result) = tools
        .call(
            Some("agent:a"),
            super::tools::ANNOUNCE_TOOL,
            serde_json::json!({ "topics": ["mail.received"], "body": "b", "key": "k2" }),
        )
        .await;
    assert!(
        failed,
        "a member cannot announce on a source topic: {result}"
    );

    try_host_call(
        service,
        ANNOUNCE,
        serde_json::json!({ "source_key": "k3", "topics": ["ops"], "body": "b" }),
    )
    .await
    .unwrap();
}

/// A triage route's forward topics are owned by its source: only that
/// route's triage member announces on them; another member and the host are
/// refused.
#[tokio::test]
async fn only_the_triage_member_announces_on_a_forward_topic() {
    let tools = tool_harness(super::tools::MemberLimits::default());
    let service = &tools.harness.service;
    try_host_call(
        service,
        SOURCES_PUT,
        with(
            source_params("o1", "inbox", "subject-1", &["mail.received"]),
            "routes",
            serde_json::json!([{
                "kind": "triage",
                "member": { "mob_id": OWN_MOB, "identity": "agent:a" },
                "forward_topics": ["mail.task"],
            }]),
        ),
    )
    .await
    .unwrap();
    let (failed, result) = tools
        .call(
            Some("agent:a"),
            super::tools::ANNOUNCE_TOOL,
            serde_json::json!({ "topics": ["mail.task"], "body": "handle it", "key": "fwd-1" }),
        )
        .await;
    assert!(!failed, "the triage member forwards: {result}");
    let (failed, result) = tools
        .call(
            Some("agent:b"),
            super::tools::ANNOUNCE_TOOL,
            serde_json::json!({ "topics": ["mail.task"], "body": "spoof", "key": "fwd-2" }),
        )
        .await;
    assert!(failed, "another member cannot: {result}");
    let host = try_host_call(
        service,
        ANNOUNCE,
        serde_json::json!({ "source_key": "k1", "topics": ["mail.task"], "body": "b" }),
    )
    .await
    .unwrap_err();
    assert_eq!(
        refusal_kind(&host).as_deref(),
        Some("topic_owned_by_source")
    );
}

/// An ended source keeps its topics reserved to its source id: no other
/// source and no plain announcement may claim them, and declaring the same
/// source id again takes them back.
#[tokio::test]
async fn an_ended_sources_topics_stay_reserved_to_its_source_id() {
    let harness = harness_with(ScriptedNative::new());
    try_host_call(
        &harness.service,
        SOURCES_PUT,
        source_params("o1", "inbox", "subject-1", &["mail.received"]),
    )
    .await
    .unwrap();
    let ended = try_host_call(
        &harness.service,
        SOURCES_END,
        serde_json::json!({ "operation_id": "o2", "source_id": "inbox", "expected_revision": 1 }),
    )
    .await
    .unwrap();
    assert_eq!(ended["source"]["state"], serde_json::json!("ended"));
    let claimed = try_host_call(
        &harness.service,
        SOURCES_PUT,
        source_params("o3", "other", "subject-2", &["mail.received"]),
    )
    .await
    .unwrap_err();
    assert_eq!(refusal_kind(&claimed).as_deref(), Some("topic_owned"));
    let host = try_host_call(
        &harness.service,
        ANNOUNCE,
        serde_json::json!({ "source_key": "k1", "topics": ["mail.received"], "body": "b" }),
    )
    .await
    .unwrap_err();
    assert_eq!(
        refusal_kind(&host).as_deref(),
        Some("topic_owned_by_source")
    );
    let again = try_host_call(
        &harness.service,
        SOURCES_PUT,
        source_params("o4", "inbox", "subject-1", &["mail.received"]),
    )
    .await
    .unwrap();
    assert_eq!(again["source"]["generation"], serde_json::json!(2));
}

/// A source's identity never changes within a generation. A new generation
/// (asked for, or a declaration after an end) starts from a fresh baseline
/// with no cursor, and the revision keeps rising across generations, so an
/// advance fenced on an older one can never match.
#[tokio::test]
async fn a_redeclared_source_starts_a_new_generation_from_a_fresh_baseline() {
    let harness = harness_with(ScriptedNative::new());
    try_host_call(
        &harness.service,
        SOURCES_PUT,
        source_params("o1", "inbox", "subject-1", &["mail.received"]),
    )
    .await
    .unwrap();
    let switched = with(
        source_params("o2", "inbox", "subject-2", &["mail.received"]),
        "expected_revision",
        serde_json::json!(1),
    );
    let refused = try_host_call(&harness.service, SOURCES_PUT, switched)
        .await
        .unwrap_err();
    assert_eq!(refused.code, super::rpc::ANNOUNCEMENTS_REFUSED_CODE);
    assert_eq!(
        refusal_kind(&refused).as_deref(),
        Some("identity_immutable")
    );

    let renewed = with(
        with(
            source_params("o3", "inbox", "subject-2", &["mail.received"]),
            "expected_revision",
            serde_json::json!(1),
        ),
        "new_generation",
        serde_json::json!(true),
    );
    let renewed = try_host_call(&harness.service, SOURCES_PUT, renewed)
        .await
        .unwrap();
    let source = &renewed["source"];
    assert_eq!(source["generation"], serde_json::json!(2));
    assert_eq!(source["revision"], serde_json::json!(2));
    assert_eq!(source["collection"], serde_json::json!("baselining"));
    assert_eq!(
        source["cursor"],
        serde_json::json!({ "version": 0, "token": null, "advanced_at_ms": null })
    );
    assert_eq!(
        source["identity"]["principal"]["subject"],
        serde_json::json!("subject-2")
    );

    try_host_call(
        &harness.service,
        SOURCES_END,
        serde_json::json!({ "operation_id": "o4", "source_id": "inbox", "expected_revision": 2 }),
    )
    .await
    .unwrap();
    let declared = try_host_call(
        &harness.service,
        SOURCES_PUT,
        source_params("o5", "inbox", "subject-2", &["mail.received"]),
    )
    .await
    .unwrap();
    assert_eq!(declared["source"]["generation"], serde_json::json!(3));
    assert_eq!(declared["source"]["revision"], serde_json::json!(4));
    assert_eq!(declared["source"]["state"], serde_json::json!("active"));

    let stale = try_host_call(
        &harness.service,
        SOURCES_PUT,
        source_params("o6", "inbox", "subject-2", &["mail.received"]),
    )
    .await
    .unwrap_err();
    assert_eq!(stale.code, super::rpc::ANNOUNCEMENTS_CONFLICT_CODE);
    assert_eq!(refusal_kind(&stale).as_deref(), Some("stale_revision"));
}

/// Routes select by topic in this release: a route selecting streams is
/// refused typed, naming the selector.
#[tokio::test]
async fn a_route_selecting_streams_is_refused_typed() {
    let harness = harness_with(ScriptedNative::new());
    let refused = try_host_call(
        &harness.service,
        SOURCES_PUT,
        with(
            source_params("o1", "inbox", "subject-1", &["mail.received"]),
            "routes",
            serde_json::json!([{ "kind": "direct", "streams": ["label-work"] }]),
        ),
    )
    .await
    .unwrap_err();
    assert_eq!(refused.code, super::rpc::ANNOUNCEMENTS_REFUSED_CODE);
    assert_eq!(
        refusal_kind(&refused).as_deref(),
        Some("route_selector_unsupported")
    );
    assert_eq!(
        refused.data.as_ref().unwrap()["selector"],
        serde_json::json!("streams")
    );
}

/// The key prefix "source:" belongs to source items: a host announcement
/// under it is refused before anything is stored.
#[tokio::test]
async fn a_host_announce_under_the_reserved_source_prefix_is_refused() {
    let harness = harness_with(ScriptedNative::new());
    let refused = try_host_call(
        &harness.service,
        ANNOUNCE,
        serde_json::json!({ "source_key": "source:inbox:0123", "topics": ["ops"], "body": "b" }),
    )
    .await
    .unwrap_err();
    assert_eq!(refused.code, -32602, "{refused:?}");
    assert!(
        harness
            .service
            .announcement(&mob(OWN_MOB), &SourceKey::new("source:inbox:0123").unwrap())
            .await
            .unwrap()
            .is_none()
    );
}

/// A source item names its source as the announcer, by id only: the batch
/// line carries nothing of the connection, the principal or the cursor.
#[test]
fn a_source_item_renders_its_source_id_and_nothing_of_its_connection() {
    let announcement: Announcement = serde_json::from_value(serde_json::json!({
        "mob_id": OWN_MOB,
        "sequence": 7,
        "announced_by": { "kind": "source", "source_id": "inbox" },
        "source_key": "source:inbox:0123",
        "topics": ["mail.received"],
        "body": "From: someone, subject: invoice",
        "digest": "sha256:00",
        "announced_at_ms": 1,
    }))
    .unwrap();
    assert_eq!(
        render_item(&announcement),
        "- Announcement #7 in mob \"payments\", by source \"inbox\"; topics \
         [\"mail.received\"]; key \"source:inbox:0123\":\n  > From: someone, subject: invoice\n"
    );
}

/// The objects schema version 2 adds, as SQLite records them: each table
/// with its columns (type, not null, primary key position) and each index
/// with its table and columns.
fn objects_added_by(tx: &rusqlite::Transaction<'_>, before: &BTreeSet<String>) -> Vec<String> {
    let mut statement = tx
        .prepare(
            "SELECT type, name, tbl_name FROM sqlite_master
             WHERE name NOT LIKE 'sqlite_%' ORDER BY name",
        )
        .unwrap();
    let objects: Vec<(String, String, String)> = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .collect::<std::result::Result<_, _>>()
        .unwrap();
    let mut lines = Vec::new();
    for (kind, name, table) in objects {
        if before.contains(&name) {
            continue;
        }
        if kind == "table" {
            let mut columns = tx.prepare(&format!("PRAGMA table_info({name})")).unwrap();
            let described: Vec<String> = columns
                .query_map([], |row| {
                    let column: String = row.get(1)?;
                    let kind: String = row.get(2)?;
                    let not_null: i64 = row.get(3)?;
                    let key: i64 = row.get(5)?;
                    let mut text = format!("{column} {kind}");
                    if not_null != 0 {
                        text.push_str(" notnull");
                    }
                    if key != 0 {
                        text.push_str(&format!(" pk{key}"));
                    }
                    Ok(text)
                })
                .unwrap()
                .collect::<std::result::Result<_, _>>()
                .unwrap();
            lines.push(format!("table {name}: {}", described.join(", ")));
        } else {
            let mut columns = tx.prepare(&format!("PRAGMA index_info({name})")).unwrap();
            let described: Vec<String> = columns
                .query_map([], |row| row.get::<_, String>(2))
                .unwrap()
                .collect::<std::result::Result<_, _>>()
                .unwrap();
            lines.push(format!(
                "{kind} {name} on {table}: {}",
                described.join(", ")
            ));
        }
    }
    lines
}

/// Version 2 of the announcement schema, the sources, froze with the first
/// 0.9.5 builds that created it: a change to it is a new version with its
/// migration. The pin makes a change fail here instead of at a user's open.
#[test]
fn the_version_two_schema_is_pinned() {
    use sha2::{Digest, Sha256};
    use std::fmt::Write as _;
    let domain = &super::storage::ANNOUNCEMENT_SCHEMA_DOMAIN;
    let version_two = &domain.migrations[1];
    assert_eq!(
        (version_two.version, version_two.name),
        (2, "announcement-sources")
    );
    let digest = Sha256::digest(super::sqlite::SCHEMA_V2.as_bytes())
        .iter()
        .fold(String::new(), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        });
    assert_eq!(
        digest, "f65833c5918c1e669d599348125c4dbfcf1e4bd584538e7a691333f15daa51b3",
        "the version 2 schema changed: it is frozen, so a change is a new version with its \
         migration"
    );
    let mut connection = rusqlite::Connection::open_in_memory().unwrap();
    let tx = connection.transaction().unwrap();
    (domain.migrations[0].apply)(&tx).unwrap();
    let before: BTreeSet<String> = {
        let mut statement = tx.prepare("SELECT name FROM sqlite_master").unwrap();
        statement
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap()
    };
    (version_two.apply)(&tx).unwrap();
    assert_eq!(
        objects_added_by(&tx, &before),
        vec![
            "table announcement_audit: id INTEGER pk1, mob_id TEXT notnull, source_id TEXT \
             notnull, generation INTEGER notnull, member TEXT notnull, action TEXT notnull, \
             reason TEXT notnull, count INTEGER notnull, first_at_ms INTEGER notnull, \
             last_at_ms INTEGER notnull, record TEXT notnull"
                .to_string(),
            "index announcement_audit_by_member on announcement_audit: member, id".to_string(),
            "index announcement_audit_by_source on announcement_audit: mob_id, source_id, id"
                .to_string(),
            "index announcement_audit_counted on announcement_audit: mob_id, source_id, \
             generation, member, action, reason"
                .to_string(),
            "table announcement_holds: subscription_id TEXT notnull pk1, mob_id TEXT notnull \
             pk2, sequence INTEGER notnull pk3, member TEXT notnull, source_id TEXT notnull, \
             generation INTEGER notnull, held_at_ms INTEGER notnull, checked_at_ms INTEGER \
             notnull, record TEXT notnull"
                .to_string(),
            "index announcement_holds_by_check on announcement_holds: checked_at_ms".to_string(),
            "index announcement_holds_by_member on announcement_holds: member, checked_at_ms"
                .to_string(),
            "table announcement_source_streams: mob_id TEXT notnull pk1, source_id TEXT \
             notnull pk2, stream TEXT notnull pk3, generation INTEGER notnull, phase TEXT \
             notnull, mode TEXT notnull, max_items INTEGER, backfilled INTEGER notnull, \
             since_ms INTEGER notnull"
                .to_string(),
            "table announcement_source_topics: mob_id TEXT notnull pk1, topic TEXT notnull \
             pk2, source_id TEXT notnull, kind TEXT notnull"
                .to_string(),
            "index announcement_source_topics_by_source on announcement_source_topics: \
             mob_id, source_id"
                .to_string(),
            "table announcement_sources: mob_id TEXT notnull pk1, source_id TEXT notnull pk2, \
             generation INTEGER notnull, state TEXT notnull, identity_key TEXT notnull, \
             updated_at_ms INTEGER notnull, record TEXT notnull"
                .to_string(),
            "index announcement_sources_by_identity on announcement_sources: mob_id, \
             identity_key, state"
                .to_string(),
        ],
        "the version 2 schema changed: it is frozen, so a change is a new version with its \
         migration"
    );
}

/// Version 3 of the announcement schema (each hold's kind and batch, the
/// handling, redelivery, source item, escalation and declaration tables)
/// freezes with the first build that ships it: after that, a change is a new
/// version with its migration. A hold version 2 wrote reads as a coverage
/// hold.
#[test]
fn the_version_three_schema_is_pinned() {
    let domain = &super::storage::ANNOUNCEMENT_SCHEMA_DOMAIN;
    let versions: Vec<i64> = domain
        .migrations
        .iter()
        .map(|migration| migration.version)
        .collect();
    assert_eq!(versions, vec![1, 2, 3]);
    let version_three = &domain.migrations[2];
    assert_eq!(
        version_three.name,
        "announcement-handling-escalations-declarations"
    );
    let mut connection = rusqlite::Connection::open_in_memory().unwrap();
    let tx = connection.transaction().unwrap();
    (domain.migrations[0].apply)(&tx).unwrap();
    (domain.migrations[1].apply)(&tx).unwrap();
    // The holds table is read again: version 3 gives it two columns.
    let before: BTreeSet<String> = {
        let mut statement = tx
            .prepare("SELECT name FROM sqlite_master WHERE name <> 'announcement_holds'")
            .unwrap();
        statement
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap()
    };
    (version_three.apply)(&tx).unwrap();
    assert_eq!(
        objects_added_by(&tx, &before),
        vec![
            "index announcement_batches_reporting_gaps on announcement_batches: sealed_at_ms, \
             member"
                .to_string(),
            "table announcement_declaration_records: kind TEXT notnull pk1, id TEXT notnull pk2, \
             revision INTEGER notnull, digest TEXT notnull, status TEXT notnull, since_ms INTEGER \
             notnull, record TEXT notnull"
                .to_string(),
            "table announcement_declarations: slot INTEGER pk1, revision INTEGER notnull, digest \
             TEXT notnull, applied_at_ms INTEGER notnull, record TEXT notnull"
                .to_string(),
            "table announcement_escalation_events: cursor INTEGER pk1, escalation_id TEXT notnull, \
             at_ms INTEGER notnull, record TEXT notnull"
                .to_string(),
            "table announcement_escalation_passes: id INTEGER notnull pk1, evaluated_at_ms INTEGER \
             notnull"
                .to_string(),
            "table announcement_escalations: escalation_id TEXT notnull pk1, cause TEXT notnull, \
             subject TEXT notnull, mob_id TEXT, open INTEGER notnull, notified_at_ms INTEGER \
             notnull, record TEXT notnull"
                .to_string(),
            "index announcement_escalations_by_subject on announcement_escalations: subject"
                .to_string(),
            "index announcement_escalations_open on announcement_escalations: cause, subject"
                .to_string(),
            "table announcement_handling: batch_id TEXT notnull pk1, member TEXT notnull, state \
             TEXT notnull, next_read_at_ms INTEGER, updated_at_ms INTEGER notnull, record TEXT \
             notnull"
                .to_string(),
            "index announcement_handling_by_member on announcement_handling: member, state, \
             updated_at_ms"
                .to_string(),
            "index announcement_handling_reads on announcement_handling: next_read_at_ms, batch_id"
                .to_string(),
            "table announcement_holds: subscription_id TEXT notnull pk1, mob_id TEXT notnull pk2, \
             sequence INTEGER notnull pk3, member TEXT notnull, source_id TEXT notnull, generation \
             INTEGER notnull, held_at_ms INTEGER notnull, checked_at_ms INTEGER notnull, record \
             TEXT notnull, kind TEXT notnull, batch_id TEXT"
                .to_string(),
            "index announcement_holds_by_batch on announcement_holds: batch_id".to_string(),
            "table announcement_member_markers: member TEXT notnull pk1, written_at_ms INTEGER \
             notnull, record TEXT notnull"
                .to_string(),
            "table announcement_redeliveries: subscription_id TEXT notnull pk1, mob_id TEXT \
             notnull pk2, sequence INTEGER notnull pk3, count INTEGER notnull, record TEXT notnull"
                .to_string(),
            "table announcement_role_children: template_id TEXT notnull pk1, member TEXT notnull \
             pk2, subscription_id TEXT notnull, status TEXT notnull, since_ms INTEGER notnull, \
             record TEXT notnull"
                .to_string(),
            "table announcement_role_templates: id TEXT notnull pk1, mob_id TEXT notnull, role \
             TEXT notnull, state TEXT notnull, updated_at_ms INTEGER notnull, record TEXT notnull"
                .to_string(),
            "index announcement_role_templates_by_mob on announcement_role_templates: mob_id, state"
                .to_string(),
            "table announcement_source_items: mob_id TEXT notnull pk1, sequence INTEGER notnull \
             pk2, source_id TEXT notnull, generation INTEGER notnull, record TEXT notnull"
                .to_string(),
        ],
        "the version 3 schema changed: before the release that ships it, update this pin; \
         after it, add version 4 with its migration instead"
    );
    let kind_default: Option<String> = tx
        .query_row(
            "SELECT dflt_value FROM pragma_table_info('announcement_holds') WHERE name = 'kind'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(kind_default.as_deref(), Some("'coverage'"));
}

/// A store as 0.9.4 left it (the version 1 schema under its ledger row, with
/// an announcement in it) opens migrated to the current version in place:
/// the ledger moves, the later tables exist, and what it held is still there.
#[tokio::test]
async fn a_version_one_store_migrates_to_the_current_version_in_place() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mob_announcements.sqlite3");
    {
        // The announcement goes in as rows of the frozen version 1 tables,
        // as 0.9.4 wrote them: a store of this release announces only over
        // the tables of its own version.
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection.execute_batch(super::sqlite::SCHEMA).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE main.meerkat_schema (
                     domain TEXT PRIMARY KEY,
                     version INTEGER NOT NULL
                 );
                 INSERT INTO main.meerkat_schema (domain, version)
                     VALUES ('meta-mob-announcements', 1);",
            )
            .unwrap();
        let request = announce_request(OWN_MOB, "kept", &["ios"], "from 0.9.4");
        let announcement = Announcement {
            mob_id: request.mob_id.clone(),
            sequence: 1,
            announced_by: Announcer::Host,
            source_key: request.source_key.clone(),
            topics: request.topics.clone(),
            body: request.body.clone(),
            correlation_id: None,
            causes: Vec::new(),
            digest: request.digest(),
            announced_at_ms: 1,
        };
        let rendered_bytes = i64::try_from(render_item(&announcement).len()).unwrap();
        connection
            .execute(
                "INSERT INTO announcement_logs (mob_id, head_sequence, earliest_retained)
                 VALUES (?1, 1, 1)",
                rusqlite::params![OWN_MOB],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO announcements
                 (mob_id, sequence, source_key, announced_at_ms, rendered_bytes, record)
                 VALUES (?1, 1, 'kept', 1, ?2, ?3)",
                rusqlite::params![
                    OWN_MOB,
                    rendered_bytes,
                    serde_json::to_string(&announcement).unwrap()
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO announcement_topics (topic, mob_id, sequence) VALUES ('ios', ?1, 1)",
                rusqlite::params![OWN_MOB],
            )
            .unwrap();
    }
    let store = super::storage::open_disk_store(path.clone()).unwrap();
    let kept = store
        .announcement(&mob(OWN_MOB), &SourceKey::new("kept").unwrap())
        .await
        .unwrap();
    assert_eq!(
        kept.map(|announcement| announcement.body),
        Some("from 0.9.4".to_string())
    );
    drop(store);
    let connection = rusqlite::Connection::open(&path).unwrap();
    let version: i64 = connection
        .query_row(
            "SELECT version FROM meerkat_schema WHERE domain = 'meta-mob-announcements'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(version, 3);
    let tables: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table'
             AND name IN ('announcement_sources', 'announcement_source_topics',
                          'announcement_source_streams', 'announcement_audit',
                          'announcement_holds', 'announcement_handling',
                          'announcement_redeliveries', 'announcement_source_items',
                          'announcement_escalations', 'announcement_escalation_events',
                          'announcement_member_markers',
                          'announcement_escalation_passes',
                          'announcement_declarations', 'announcement_declaration_records',
                          'announcement_role_templates', 'announcement_role_children')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(tables, 16);
}

/// The objects of the announcement database at `path` as SQLite records
/// them, the ledger aside.
fn announcement_catalog(path: &std::path::Path) -> Vec<(String, String, Option<String>)> {
    let connection = rusqlite::Connection::open(path).unwrap();
    let mut statement = connection
        .prepare(
            "SELECT type, name, sql FROM sqlite_master
             WHERE name NOT LIKE 'sqlite_%' AND name <> 'meerkat_schema'
             ORDER BY type, name",
        )
        .unwrap();
    statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .collect::<std::result::Result<_, _>>()
        .unwrap()
}

/// A store at version 2 as the first 0.9.5 builds left it (the version 1
/// and 2 schemas under their ledger row, with a hold in it) opens migrated
/// to version 3 in place: the ledger moves, the hold is a coverage hold of
/// no batch, and the catalog is a fresh store's.
#[test]
fn a_version_two_store_migrates_to_version_three_in_place() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mob_announcements.sqlite3");
    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection.execute_batch(super::sqlite::SCHEMA).unwrap();
        connection.execute_batch(super::sqlite::SCHEMA_V2).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE main.meerkat_schema (
                     domain TEXT PRIMARY KEY,
                     version INTEGER NOT NULL
                 );
                 INSERT INTO main.meerkat_schema (domain, version)
                     VALUES ('meta-mob-announcements', 2);",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO announcement_holds (subscription_id, mob_id, sequence, member,
                   source_id, generation, held_at_ms, checked_at_ms, record)
                 VALUES ('s1', ?1, 1, 'member', 'inbox', 1, 5, 5, '{}')",
                rusqlite::params![OWN_MOB],
            )
            .unwrap();
    }
    drop(super::storage::open_disk_store(path.clone()).unwrap());
    let connection = rusqlite::Connection::open(&path).unwrap();
    let version: i64 = connection
        .query_row(
            "SELECT version FROM meerkat_schema WHERE domain = 'meta-mob-announcements'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(version, 3);
    let hold: (String, Option<String>) = connection
        .query_row(
            "SELECT kind, batch_id FROM announcement_holds WHERE subscription_id = 's1'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(hold, ("coverage".to_string(), None));
    drop(connection);
    let fresh_dir = tempfile::tempdir().unwrap();
    let fresh = fresh_dir.path().join("mob_announcements.sqlite3");
    drop(super::storage::open_disk_store(fresh.clone()).unwrap());
    assert_eq!(announcement_catalog(&path), announcement_catalog(&fresh));
}

/// A store stamped version 2 whose catalog is not the frozen version 2 (here
/// one that already has a version 3 table, as no build that stamped version
/// 2 and shipped wrote it) is refused before anything changes.
#[test]
fn a_version_two_store_unlike_the_frozen_one_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mob_announcements.sqlite3");
    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection.execute_batch(super::sqlite::SCHEMA).unwrap();
        connection.execute_batch(super::sqlite::SCHEMA_V2).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE announcement_handling (batch_id TEXT NOT NULL PRIMARY KEY);
                 CREATE TABLE main.meerkat_schema (
                     domain TEXT PRIMARY KEY,
                     version INTEGER NOT NULL
                 );
                 INSERT INTO main.meerkat_schema (domain, version)
                     VALUES ('meta-mob-announcements', 2);",
            )
            .unwrap();
    }
    let before = announcement_catalog(&path);
    assert!(super::storage::open_disk_store(path.clone()).is_err());
    assert_eq!(
        announcement_catalog(&path),
        before,
        "the refusal changed nothing"
    );
}

// ---------------------------------------------------------------------------
// Announcement sources through the store and the service.
// ---------------------------------------------------------------------------

/// A source declaration as a store takes it.
fn source_config(subject: &str, topic: &[&str]) -> super::sources::SourceConfig {
    let mut params = source_params("unused", "unused", subject, topic);
    let object = params.as_object_mut().unwrap();
    object.remove("operation_id");
    object.remove("source_id");
    object.insert("routes".into(), serde_json::json!([{ "kind": "direct" }]));
    serde_json::from_value(params).unwrap()
}

fn put_source(source_id: &str, subject: &str, topic: &[&str]) -> super::sources::PutSource {
    super::sources::PutSource {
        mob_id: mob(OWN_MOB),
        source_id: SourceId::new(source_id).unwrap(),
        expected_revision: None,
        new_generation: false,
        config: source_config(subject, topic),
    }
}

fn source_item(source_id: &str, key: &str, topic: &[&str]) -> AnnounceRequest {
    AnnounceRequest {
        mob_id: mob(OWN_MOB),
        announced_by: Announcer::Source {
            source_id: SourceId::new(source_id).unwrap(),
        },
        source_key: SourceKey::new(key).unwrap(),
        topics: topics(topic),
        body: "From: someone".to_string(),
        correlation_id: None,
        causes: Vec::new(),
    }
}

/// A source's own item reaches its own topic, and no other source's does:
/// the announcer and the topic's owner must agree.
#[tokio::test]
async fn a_sources_own_item_reaches_its_own_topic_and_no_other_sources() {
    let (_dir, store) = temp_store();
    store
        .put_source(
            &op("o1"),
            put_source("inbox", "subject-1", &["mail.received"]),
            1,
        )
        .await
        .unwrap();
    let outcome = store
        .announce(
            source_item("inbox", "source:inbox:0001", &["mail.received"]),
            2,
        )
        .await
        .unwrap();
    assert!(!outcome.replayed);
    assert_eq!(
        store
            .announce(
                source_item("calendar", "source:calendar:0001", &["mail.received"]),
                3,
            )
            .await,
        Err(AnnouncementStoreError::TopicOwnedBySource {
            topic: Topic::new("mail.received").unwrap(),
            source_id: SourceId::new("inbox").unwrap(),
        })
    );
}

/// The content digest names a source announcer apart from the host and from
/// every other source.
#[test]
fn a_source_announcer_enters_the_content_digest() {
    let host = announce_request(OWN_MOB, "k", &["t"], "b");
    let by_source = |source_id: &str| {
        let mut request = host.clone();
        request.announced_by = Announcer::Source {
            source_id: SourceId::new(source_id).unwrap(),
        };
        request
    };
    let inbox = by_source("inbox");
    let inbox_again = by_source("inbox");
    let calendar = by_source("calendar");
    assert_ne!(host.digest(), inbox.digest());
    assert_ne!(inbox.digest(), calendar.digest());
    assert_eq!(inbox.digest(), inbox_again.digest());
}

/// Pause and resume move a source at its revision; a repeated pause changes
/// nothing; a resume can rebaseline; an ended source takes no change; and
/// only a resume takes `rebaseline`.
#[tokio::test]
async fn a_source_pauses_resumes_and_ends_at_its_revision() {
    let harness = harness_with(ScriptedNative::new());
    let service = &harness.service;
    try_host_call(
        service,
        SOURCES_PUT,
        source_params("o1", "inbox", "subject-1", &["mail.received"]),
    )
    .await
    .unwrap();
    let change = |operation: &str, method: &str, revision: u64| {
        let params = serde_json::json!({
            "operation_id": operation, "source_id": "inbox", "expected_revision": revision
        });
        let method = format!("meta/mob_announcements/sources/{method}");
        async move { try_host_call(service, &method, params).await }
    };
    let paused = change("o2", "pause", 1).await.unwrap();
    assert_eq!(paused["source"]["state"], serde_json::json!("paused"));
    assert_eq!(paused["source"]["revision"], serde_json::json!(2));
    assert_eq!(
        paused["source"]["health"]["status"],
        serde_json::json!("paused")
    );
    let again = change("o3", "pause", 2).await.unwrap();
    assert_eq!(
        again["source"]["revision"],
        serde_json::json!(2),
        "nothing moved"
    );

    let refused = try_host_call(
        service,
        "meta/mob_announcements/sources/pause",
        serde_json::json!({
            "operation_id": "o4", "source_id": "inbox", "expected_revision": 2,
            "rebaseline": true
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(refused.code, -32602, "{refused:?}");

    let resumed = try_host_call(
        service,
        "meta/mob_announcements/sources/resume",
        serde_json::json!({
            "operation_id": "o5", "source_id": "inbox", "expected_revision": 2,
            "rebaseline": true
        }),
    )
    .await
    .unwrap();
    assert_eq!(resumed["source"]["state"], serde_json::json!("active"));
    assert_eq!(resumed["source"]["revision"], serde_json::json!(3));
    assert_eq!(
        resumed["source"]["collection"],
        serde_json::json!("baselining")
    );
    assert_eq!(
        resumed["source"]["cursor"]["token"],
        serde_json::Value::Null
    );

    let ended = change("o6", "end", 3).await.unwrap();
    assert_eq!(ended["source"]["state"], serde_json::json!("ended"));
    let refused = change("o7", "pause", 4).await.unwrap_err();
    assert_eq!(refusal_kind(&refused).as_deref(), Some("source_ended"));
    let stale = change("o8", "end", 1).await.unwrap_err();
    assert_eq!(refusal_kind(&stale).as_deref(), Some("source_ended"));
}

/// A topic a source stops naming stays reserved to it: ownership is never
/// released by a declaration.
#[tokio::test]
async fn a_topic_a_source_stops_naming_stays_reserved_to_it() {
    let harness = harness_with(ScriptedNative::new());
    try_host_call(
        &harness.service,
        SOURCES_PUT,
        source_params("o1", "inbox", "subject-1", &["mail.received", "mail.old"]),
    )
    .await
    .unwrap();
    try_host_call(
        &harness.service,
        SOURCES_PUT,
        with(
            source_params("o2", "inbox", "subject-1", &["mail.received"]),
            "expected_revision",
            serde_json::json!(1),
        ),
    )
    .await
    .unwrap();
    let claimed = try_host_call(
        &harness.service,
        SOURCES_PUT,
        source_params("o3", "other", "subject-2", &["mail.old"]),
    )
    .await
    .unwrap_err();
    assert_eq!(refusal_kind(&claimed).as_deref(), Some("topic_owned"));
}

/// A source survives reopening its store, whole.
#[tokio::test]
async fn a_source_survives_reopening_the_store() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mob_announcements.sqlite3");
    let first = {
        let store = super::storage::open_disk_store(path.clone()).unwrap();
        store
            .put_source(
                &op("o1"),
                put_source("inbox", "subject-1", &["mail.received"]),
                5,
            )
            .await
            .unwrap()
    };
    let store = super::storage::open_disk_store(path).unwrap();
    assert_eq!(
        store
            .source(&mob(OWN_MOB), &SourceId::new("inbox").unwrap())
            .await
            .unwrap(),
        Some(first)
    );
}

/// A list pages sources in (mob, source) order and filters by state.
#[tokio::test]
async fn a_list_pages_sources_in_order_and_filters_by_state() {
    let harness = harness_with(ScriptedNative::new());
    let service = &harness.service;
    try_host_call(
        service,
        SOURCES_PUT,
        source_params("o1", "b-src", "subject-1", &["mail.b"]),
    )
    .await
    .unwrap();
    try_host_call(
        service,
        SOURCES_PUT,
        source_params("o2", "a-src", "subject-2", &["mail.a"]),
    )
    .await
    .unwrap();
    try_host_call(
        service,
        SOURCES_END,
        serde_json::json!({ "operation_id": "o3", "source_id": "b-src", "expected_revision": 1 }),
    )
    .await
    .unwrap();
    try_host_call(
        service,
        SOURCES_PUT,
        with(
            source_params("o4", "c-src", "subject-3", &["mail.c"]),
            "mob_id",
            serde_json::json!("events"),
        ),
    )
    .await
    .unwrap();
    let ids = |listed: serde_json::Value| -> Vec<String> {
        listed["sources"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|source| source["source_id"].as_str().unwrap_or_default().to_string())
            .collect()
    };
    let list = |params: serde_json::Value| async move {
        ids(try_host_call(service, SOURCES_LIST, params).await.unwrap())
    };
    assert_eq!(
        list(serde_json::json!({})).await,
        vec!["a-src", "b-src"],
        "a list reads the gateway's own mob unless it names another"
    );
    assert_eq!(
        list(serde_json::json!({ "mob_id": "events" })).await,
        vec!["c-src"]
    );
    assert_eq!(list(serde_json::json!({ "limit": 1 })).await, vec!["a-src"]);
    assert_eq!(
        list(serde_json::json!({ "after": "a-src" })).await,
        vec!["b-src"]
    );
    assert_eq!(
        list(serde_json::json!({ "state": "ended" })).await,
        vec!["b-src"]
    );
    assert_eq!(
        list(serde_json::json!({ "state": "active" })).await,
        vec!["a-src"]
    );
}

/// A change at a stale revision names where the source stands: its
/// revision, its generation and its cursor, with the cursor's version beside
/// them, so the caller retries without another read.
#[tokio::test]
async fn a_stale_source_revision_names_where_the_source_stands() {
    let harness = harness_with(ScriptedNative::new());
    let service = &harness.service;
    try_host_call(
        service,
        SOURCES_PUT,
        source_params("o1", "inbox", "subject-1", &["mail.received"]),
    )
    .await
    .unwrap();
    let stale = try_host_call(
        service,
        "meta/mob_announcements/sources/pause",
        serde_json::json!({ "operation_id": "o2", "source_id": "inbox", "expected_revision": 7 }),
    )
    .await
    .unwrap_err();
    assert_eq!(stale.code, super::rpc::ANNOUNCEMENTS_CONFLICT_CODE);
    assert_eq!(
        stale.data,
        Some(serde_json::json!({
            "kind": "stale_revision",
            "current_revision": 1,
            "current_version": 0,
            "generation": 1,
            "cursor": { "version": 0, "token": null, "advanced_at_ms": null },
        }))
    );
}

/// A mob holds a bounded number of live sources: the next one is refused
/// typed with the bound and the mob, and an ended source frees its place.
#[tokio::test]
async fn a_mob_holds_a_bounded_number_of_live_sources() {
    let (_dir, store) = temp_store();
    for index in 0..super::sources::MAX_SOURCES_PER_MOB {
        let topic = format!("mail.{index}");
        store
            .put_source(
                &op(&format!("o{index}")),
                put_source(
                    &format!("s{index}"),
                    &format!("subject-{index}"),
                    &[topic.as_str()],
                ),
                1,
            )
            .await
            .unwrap();
    }
    let over = || put_source("over", "subject-over", &["mail.over"]);
    assert_eq!(
        store.put_source(&op("over-1"), over(), 2).await,
        Err(AnnouncementStoreError::SourceLimit {
            limit: super::sources::MAX_SOURCES_PER_MOB,
            mob_id: Some(mob(OWN_MOB)),
        })
    );
    store
        .change_source(
            &op("end"),
            &mob(OWN_MOB),
            &SourceId::new("s0").unwrap(),
            1,
            super::sources::SourceChange::End,
            3,
        )
        .await
        .unwrap();
    store.put_source(&op("over-2"), over(), 4).await.unwrap();
}

// ---------------------------------------------------------------------------
// Announcement sources: advancing the cursor and recording failures, through
// the one handler every gateway dispatches to.
// ---------------------------------------------------------------------------

pub(super) const SOURCES_ADVANCE: &str = "meta/mob_announcements/sources/advance";
pub(super) const SOURCES_RECORD_FAILURE: &str = "meta/mob_announcements/sources/record_failure";
pub(super) const SOURCES_PAUSE: &str = "meta/mob_announcements/sources/pause";
pub(super) const SOURCES_RESUME: &str = "meta/mob_announcements/sources/resume";
pub(super) const SUBSCRIPTIONS_PUT: &str = "meta/mob_announcements/subscriptions/put";

/// The verified account `source_params` pins for `subject`.
pub(super) fn principal_of(subject: &str) -> serde_json::Value {
    serde_json::json!({
        "kind": "connector_verified_account",
        "issuer": "https://accounts.example",
        "strategy_id": "oauth-code",
        "subject": subject,
    })
}

/// A `sources/advance` of source "inbox" as its collector reading for
/// "subject-1", from `version` at `revision`, moving the cursor to `cursor`.
pub(super) fn advance_params(
    operation: &str,
    revision: u64,
    version: u64,
    cursor: &str,
) -> serde_json::Value {
    serde_json::json!({
        "operation_id": operation,
        "source_id": "inbox",
        "expected_revision": revision,
        "expected_version": version,
        "principal": principal_of("subject-1"),
        "next_cursor": cursor,
    })
}

/// `params` carrying `items`.
pub(super) fn with_items(
    params: serde_json::Value,
    items: &[serde_json::Value],
) -> serde_json::Value {
    with(params, "items", serde_json::Value::Array(items.to_vec()))
}

/// One item on the source topic: `change` of `item_id` at `version`.
fn change_of(item_id: &str, version: &str, change: &str, body: &str) -> serde_json::Value {
    serde_json::json!({
        "item_id": item_id,
        "version": version,
        "change": change,
        "topics": ["mail.received"],
        "body": body,
    })
}

/// An upsert of `item_id` at `version`.
pub(super) fn upsert(item_id: &str, version: &str, body: &str) -> serde_json::Value {
    change_of(item_id, version, "upsert", body)
}

/// An upsert of `item_id` filed in `stream`.
fn upsert_in(stream: &str, item_id: &str, body: &str) -> serde_json::Value {
    let mut item = upsert(item_id, "v1", body);
    item["stream"] = serde_json::json!(stream);
    item
}

/// Declare "inbox" (read for "subject-1", topic "mail.received") and take
/// its baseline in one final page: live at revision 1, version 1, cursor
/// "c1".
pub(super) async fn live_inbox(service: &AnnouncementService) {
    try_host_call(
        service,
        SOURCES_PUT,
        source_params("put-inbox", "inbox", "subject-1", &["mail.received"]),
    )
    .await
    .unwrap();
    let baseline = with(
        advance_params("baseline", 1, 0, "c1"),
        "coverage",
        serde_json::json!("complete"),
    );
    let advanced = try_host_call(service, SOURCES_ADVANCE, baseline)
        .await
        .unwrap();
    assert_eq!(advanced["source"]["collection"], serde_json::json!("live"));
}

/// Subscribe this mob's member `identity` to the source topic from now.
pub(super) async fn subscribe_to_inbox(service: &AnnouncementService, id: &str, identity: &str) {
    try_host_call(
        service,
        SUBSCRIPTIONS_PUT,
        serde_json::json!({
            "operation_id": format!("subscribe-{id}"),
            "subscription_id": id,
            "member": { "mob_id": OWN_MOB, "identity": identity },
            "topics": ["mail.received"],
        }),
    )
    .await
    .unwrap();
}

/// A native owner that serves members "agent:a" and "agent:b".
fn native_serving_a_and_b() -> ScriptedNative {
    let mut native = ScriptedNative::new();
    native.known.insert("agent:b".into());
    native
}

/// "inbox" as `sources/get` returns it.
pub(super) async fn inbox(service: &AnnouncementService) -> serde_json::Value {
    try_host_call(
        service,
        SOURCES_GET,
        serde_json::json!({ "source_id": "inbox" }),
    )
    .await
    .unwrap()["source"]
        .clone()
}

/// What each item of an advance came to, in order.
pub(super) fn outcomes(advanced: &serde_json::Value) -> Vec<String> {
    advanced["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| item["outcome"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Owed announcements per member, as the backlog counts them.
pub(super) async fn owed(harness: &Harness) -> BTreeMap<String, u64> {
    harness
        .store
        .backlogs(16)
        .await
        .unwrap()
        .into_iter()
        .map(|backlog| (backlog.member.identity.to_string(), backlog.eligible_count))
        .collect()
}

/// An advance commits its items, their captures and the cursor in one
/// transaction: a page that fails at its second item leaves neither the
/// first item, its capture nor the cursor behind.
#[tokio::test]
async fn an_advance_commits_its_items_captures_and_cursor_together_or_not_at_all() {
    let harness = harness_with(ScriptedNative::new());
    let service = &harness.service;
    live_inbox(service).await;
    subscribe_to_inbox(service, "sub-a", "agent:a").await;

    // The second item renders past a delivery's bound only when it is
    // rendered, after the first item was written in the same transaction.
    let unrenderable = "\u{1}".repeat(16 * 1024);
    let refused = try_host_call(
        service,
        SOURCES_ADVANCE,
        with_items(
            advance_params("run-1:v1", 1, 1, "c2"),
            &[
                upsert("m-1", "v1", "first"),
                upsert("m-2", "v1", &unrenderable),
            ],
        ),
    )
    .await
    .unwrap_err();
    assert_eq!(refused.code, -32602, "{refused:?}");
    let source = inbox(service).await;
    assert_eq!(source["cursor"]["version"], serde_json::json!(1));
    assert_eq!(source["cursor"]["token"], serde_json::json!("c1"));
    assert!(owed(&harness).await.is_empty(), "nothing was captured");

    let advanced = try_host_call(
        service,
        SOURCES_ADVANCE,
        with_items(
            advance_params("run-2:v1", 1, 1, "c2"),
            &[upsert("m-1", "v1", "first")],
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        outcomes(&advanced),
        vec!["announced"],
        "nothing of the refused page was committed"
    );
    assert_eq!(advanced["items"][0]["captured"], serde_json::json!(1));
    assert_eq!(
        advanced["source"]["cursor"]["version"],
        serde_json::json!(2)
    );
    assert_eq!(
        advanced["source"]["cursor"]["token"],
        serde_json::json!("c2")
    );
    assert_eq!(owed(&harness).await.get("agent:a"), Some(&1));
    let key = SourceKey::new(advanced["items"][0]["source_key"].as_str().unwrap()).unwrap();
    let committed = harness
        .store
        .announcement(&mob(OWN_MOB), &key)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        committed.announced_by,
        Announcer::Source {
            source_id: SourceId::new("inbox").unwrap()
        }
    );
    assert!(key.as_str().starts_with("source:inbox:"), "{key}");
}

/// A step retried with its operation id answers its original result, byte
/// for byte but marked replayed, and the cursor moves once.
#[tokio::test]
async fn an_advance_retried_with_its_operation_id_returns_the_original_result() {
    let harness = harness_with(ScriptedNative::new());
    let service = &harness.service;
    live_inbox(service).await;
    let step = with_items(
        advance_params("schedule:s:occurrence:o:v1", 1, 1, "c2"),
        &[upsert("m-1", "v1", "hello")],
    );
    let first = try_host_call(service, SOURCES_ADVANCE, step.clone())
        .await
        .unwrap();
    assert_eq!(first["replayed"], serde_json::json!(false));
    harness.clock.fetch_add(5_000, Ordering::SeqCst);
    let again = try_host_call(service, SOURCES_ADVANCE, step).await.unwrap();
    let mut original = first.clone();
    original["replayed"] = serde_json::json!(true);
    assert_eq!(again, original);
    assert_eq!(
        inbox(service).await["cursor"]["version"],
        serde_json::json!(2)
    );
}

/// A collector that read the provider and never advanced left nothing
/// durable: the next run reads the same cursor, and what it advances is
/// announced once.
#[tokio::test]
async fn a_collector_that_never_advanced_rereads_the_same_cursor_and_loses_nothing() {
    let harness = harness_with(ScriptedNative::new());
    let service = &harness.service;
    live_inbox(service).await;
    subscribe_to_inbox(service, "sub-a", "agent:a").await;
    // A first run read from "c1" and died before its advance.
    let source = inbox(service).await;
    assert_eq!(source["cursor"]["token"], serde_json::json!("c1"));
    assert_eq!(source["cursor"]["version"], serde_json::json!(1));
    // The next run reads the same cursor and the same changes.
    let advanced = try_host_call(
        service,
        SOURCES_ADVANCE,
        with_items(
            advance_params("run-2:v1", 1, 1, "c2"),
            &[upsert("m-1", "v1", "lost and found")],
        ),
    )
    .await
    .unwrap();
    assert_eq!(outcomes(&advanced), vec!["announced"]);
    assert_eq!(owed(&harness).await.get("agent:a"), Some(&1));
}

/// Two collectors that read one version race: the first advance wins, the
/// second is refused with where the cursor stands and commits none of its
/// items.
#[tokio::test]
async fn overlapping_collectors_are_fenced_by_the_cursor_version() {
    let harness = harness_with(ScriptedNative::new());
    let service = &harness.service;
    live_inbox(service).await;
    let first = try_host_call(
        service,
        SOURCES_ADVANCE,
        with_items(
            advance_params("run-a:v1", 1, 1, "c-a"),
            &[upsert("m-1", "v1", "from a")],
        ),
    )
    .await
    .unwrap();
    assert_eq!(first["source"]["cursor"]["version"], serde_json::json!(2));
    let second = try_host_call(
        service,
        SOURCES_ADVANCE,
        with_items(
            advance_params("run-b:v1", 1, 1, "c-b"),
            &[upsert("m-2", "v1", "from b")],
        ),
    )
    .await
    .unwrap_err();
    assert_eq!(second.code, super::rpc::ANNOUNCEMENTS_CONFLICT_CODE);
    assert_eq!(
        second.data,
        Some(serde_json::json!({
            "kind": "source_version_conflict",
            "current_version": 2,
            "cursor": { "version": 2, "token": "c-a", "advanced_at_ms": 1000 },
            "state": "active",
            "revision": 1,
        }))
    );
    assert!(
        !second.message.contains("c-a"),
        "the token travels in data only: {}",
        second.message
    );
    let retried = try_host_call(
        service,
        SOURCES_ADVANCE,
        with_items(
            advance_params("run-b:v2", 1, 2, "c-b"),
            &[upsert("m-2", "v1", "from b")],
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        outcomes(&retried),
        vec!["announced"],
        "the refused advance committed none of its items"
    );
}

/// A configuration change between a collector's read and its advance moves
/// the revision: the advance loses typed, with the revision, version,
/// generation and cursor it should read again.
#[tokio::test]
async fn an_advance_in_flight_across_a_configuration_change_loses_typed() {
    let harness = harness_with(ScriptedNative::new());
    let service = &harness.service;
    live_inbox(service).await;
    try_host_call(
        service,
        SOURCES_PAUSE,
        serde_json::json!({ "operation_id": "pause", "source_id": "inbox", "expected_revision": 1 }),
    )
    .await
    .unwrap();
    try_host_call(
        service,
        SOURCES_RESUME,
        serde_json::json!({ "operation_id": "resume", "source_id": "inbox", "expected_revision": 2 }),
    )
    .await
    .unwrap();
    let stale = try_host_call(
        service,
        SOURCES_ADVANCE,
        with_items(
            advance_params("run:v1", 1, 1, "c2"),
            &[upsert("m-1", "v1", "late")],
        ),
    )
    .await
    .unwrap_err();
    assert_eq!(stale.code, super::rpc::ANNOUNCEMENTS_CONFLICT_CODE);
    assert_eq!(
        stale.data,
        Some(serde_json::json!({
            "kind": "stale_revision",
            "current_revision": 3,
            "current_version": 1,
            "generation": 1,
            "cursor": { "version": 1, "token": "c1", "advanced_at_ms": 1000 },
        }))
    );
    let advanced = try_host_call(
        service,
        SOURCES_ADVANCE,
        with_items(
            advance_params("run-2:v1", 3, 1, "c2"),
            &[upsert("m-1", "v1", "late")],
        ),
    )
    .await
    .unwrap();
    assert_eq!(outcomes(&advanced), vec!["announced"]);
}

/// `record_failure` is fenced like an advance: a collector that read an
/// older version cannot mark a healthy source failing; a current one can.
#[tokio::test]
async fn a_failure_from_a_stale_collector_does_not_mark_the_source_failing() {
    let harness = harness_with(ScriptedNative::new());
    let service = &harness.service;
    live_inbox(service).await;
    try_host_call(
        service,
        SOURCES_ADVANCE,
        advance_params("run-a:v1", 1, 1, "c2"),
    )
    .await
    .unwrap();
    let failure = |operation: &str, version: u64| {
        serde_json::json!({
            "operation_id": operation,
            "source_id": "inbox",
            "expected_revision": 1,
            "expected_version": version,
            "principal": principal_of("subject-1"),
            "failure": { "class": "provider_unavailable", "retry_after_ms": 60000 },
        })
    };
    let stale = try_host_call(service, SOURCES_RECORD_FAILURE, failure("fail-1", 1))
        .await
        .unwrap_err();
    assert_eq!(
        refusal_kind(&stale).as_deref(),
        Some("source_version_conflict")
    );
    let health = &inbox(service).await["health"];
    assert_eq!(health["consecutive_failures"], serde_json::json!(0));
    assert_eq!(health["status"], serde_json::json!("ok"));
    assert_eq!(health["last_failure"], serde_json::Value::Null);

    let failed = try_host_call(service, SOURCES_RECORD_FAILURE, failure("fail-2", 2))
        .await
        .unwrap();
    let health = &failed["source"]["health"];
    assert_eq!(health["consecutive_failures"], serde_json::json!(1));
    assert_eq!(health["status"], serde_json::json!("failing"));
    assert_eq!(
        health["last_failure"]["class"],
        serde_json::json!("provider_unavailable")
    );
    assert_eq!(
        failed["source"]["cursor"]["version"],
        serde_json::json!(2),
        "a failure moves no cursor"
    );
}

/// Every bound of an advance is enforced and refused as invalid params, and
/// a refusal leaves the cursor where it was.
#[tokio::test]
async fn source_items_are_bounded_and_refused_typed() {
    let harness = harness_with(ScriptedNative::new());
    let service = &harness.service;
    live_inbox(service).await;
    let page_of = |items: Vec<serde_json::Value>| {
        with_items(advance_params("bounded:v1", 1, 1, "c2"), &items)
    };
    let mut cases: Vec<(&str, serde_json::Value)> = Vec::new();
    cases.push((
        "more than 100 items",
        page_of(
            (0..101)
                .map(|index| upsert(&format!("m-{index}"), "v1", "b"))
                .collect(),
        ),
    ));
    cases.push(("an empty item id", page_of(vec![upsert("", "v1", "b")])));
    cases.push((
        "an item id over 256 bytes",
        page_of(vec![upsert(&"i".repeat(257), "v1", "b")]),
    ));
    cases.push((
        "a version over 256 bytes",
        page_of(vec![upsert("m-1", &"v".repeat(257), "b")]),
    ));
    cases.push((
        "a stream over 256 bytes",
        page_of(vec![upsert_in(&"s".repeat(257), "m-1", "b")]),
    ));
    cases.push((
        "a body over 16 KiB",
        page_of(vec![upsert("m-1", "v1", &"b".repeat(16 * 1024 + 1))]),
    ));
    cases.push((
        "bodies over 1 MiB in one page",
        page_of(
            (0..65)
                .map(|index| upsert(&format!("m-{index}"), "v1", &"b".repeat(16 * 1024)))
                .collect(),
        ),
    ));
    let mut foreign = upsert("m-1", "v1", "b");
    foreign["topics"] = serde_json::json!(["mail.sent"]);
    cases.push(("a topic the source does not own", page_of(vec![foreign])));
    let mut untopical = upsert("m-1", "v1", "b");
    untopical["topics"] = serde_json::json!([]);
    cases.push(("an item without a topic", page_of(vec![untopical])));
    cases.push((
        "an unknown change",
        page_of(vec![change_of("m-1", "v1", "modify", "b")]),
    ));
    for (case, params) in cases {
        let refused = try_host_call(service, SOURCES_ADVANCE, params)
            .await
            .unwrap_err();
        assert_eq!(refused.code, -32602, "{case}: {refused:?}");
    }
    let source = inbox(service).await;
    assert_eq!(source["cursor"]["version"], serde_json::json!(1));
    assert_eq!(
        source["health"]["totals"]["announced"],
        serde_json::json!(0)
    );
}

/// A writer that keeps whatever is logged on the test thread.
#[derive(Clone, Default)]
struct CapturedLog(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for CapturedLog {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A collector's cursor token, its principal and a failure's detail never
/// reach a log line, a list, an error message or a delivered body: lists show
/// the principal's kind and digest only, and the token and the detail are
/// read back from `sources/get` alone.
#[tokio::test]
async fn cursor_tokens_principals_and_failure_details_never_reach_logs_lists_status_or_bodies() {
    let log = CapturedLog::default();
    let writer = log.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_writer(move || writer.clone())
        .finish();
    let _logging = tracing::subscriber::set_default(subscriber);

    const SUBJECT: &str = "subject-secret-7f3a";
    const TOKEN: &str = "cursor-secret-91c2";
    const DETAIL: &str = "detail-secret-55e1";
    let harness = harness_with(ScriptedNative::new());
    let service = &harness.service;
    try_host_call(
        service,
        SOURCES_PUT,
        source_params("put", "inbox", SUBJECT, &["mail.received"]),
    )
    .await
    .unwrap();
    subscribe_to_inbox(service, "sub-a", "agent:a").await;
    let mut step = with(
        advance_params("baseline", 1, 0, TOKEN),
        "coverage",
        serde_json::json!("complete"),
    );
    step["principal"] = principal_of(SUBJECT);
    try_host_call(service, SOURCES_ADVANCE, step).await.unwrap();
    let mut step = with_items(
        advance_params("run:v1", 1, 1, TOKEN),
        &[upsert("m-1", "v1", "an invoice arrived")],
    );
    step["principal"] = principal_of(SUBJECT);
    try_host_call(service, SOURCES_ADVANCE, step).await.unwrap();
    let mut stale = advance_params("stale:v1", 1, 1, "other");
    stale["principal"] = principal_of(SUBJECT);
    let refused = try_host_call(service, SOURCES_ADVANCE, stale)
        .await
        .unwrap_err();
    let failed = try_host_call(
        service,
        SOURCES_RECORD_FAILURE,
        serde_json::json!({
            "operation_id": "fail",
            "source_id": "inbox",
            "expected_revision": 1,
            "expected_version": 2,
            "principal": principal_of(SUBJECT),
            "failure": { "class": "adapter_error", "detail": DETAIL },
        }),
    )
    .await
    .unwrap();
    assert_eq!(
        failed["source"]["health"]["last_failure"]["class"],
        serde_json::json!("adapter_error")
    );
    harness.clock.fetch_add(60_001, Ordering::SeqCst);
    harness.service.run_round().await.unwrap();

    let got = inbox(service).await;
    assert_eq!(got["cursor"]["token"], serde_json::json!(TOKEN));
    assert_eq!(
        got["health"]["last_failure"]["detail"],
        serde_json::json!(DETAIL),
        "sources/get keeps the detail for the operator"
    );
    let listed = try_host_call(service, SOURCES_LIST, serde_json::json!({}))
        .await
        .unwrap();
    let principal = &listed["sources"][0]["identity"]["principal"];
    assert_eq!(
        principal["kind"],
        serde_json::json!("connector_verified_account")
    );
    assert!(
        principal["digest"]
            .as_str()
            .is_some_and(|digest| digest.starts_with("sha256:")),
        "{principal}"
    );
    let delivered: Vec<String> = harness
        .native
        .ledger
        .lock()
        .unwrap()
        .bindings
        .values()
        .map(|(delivery, _, _)| delivery.clone())
        .collect();
    assert_eq!(delivered.len(), 1, "the item was delivered");
    let logged = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
    let seen = [
        ("the list", listed.to_string()),
        ("a refusal message", refused.message.clone()),
        ("a delivered body", delivered.join("\n")),
        ("the log", logged),
    ];
    for (place, text) in seen {
        for secret in [SUBJECT, TOKEN, DETAIL] {
            assert!(!text.contains(secret), "{secret} reached {place}: {text}");
        }
    }
}

/// One provider version is announced once however often a collector reads
/// it: every later collection replays it, captures nothing new, and the
/// cursor still moves.
#[tokio::test]
async fn one_provider_version_is_announced_once_however_often_it_is_collected() {
    let harness = harness_with(ScriptedNative::new());
    let service = &harness.service;
    live_inbox(service).await;
    subscribe_to_inbox(service, "sub-a", "agent:a").await;
    let mut sequences = Vec::new();
    for version in 1..=3u64 {
        let advanced = try_host_call(
            service,
            SOURCES_ADVANCE,
            with_items(
                advance_params(&format!("run-{version}:v{version}"), 1, version, "c"),
                &[upsert("m-1", "v1", "the same message")],
            ),
        )
        .await
        .unwrap();
        sequences.push(advanced["items"][0]["sequence"].clone());
        assert_eq!(
            advanced["source"]["cursor"]["version"],
            serde_json::json!(version + 1)
        );
        let expected = if version == 1 {
            "announced"
        } else {
            "replayed"
        };
        assert_eq!(outcomes(&advanced), vec![expected]);
    }
    assert_eq!(sequences[0], sequences[1]);
    assert_eq!(sequences[1], sequences[2]);
    assert_eq!(owed(&harness).await.get("agent:a"), Some(&1));
    let totals = &inbox(service).await["health"]["totals"];
    assert_eq!(totals["announced"], serde_json::json!(1));
    assert_eq!(totals["replayed"], serde_json::json!(2));
}

/// A baseline that spans pages and is interrupted resumes from its interim
/// cursor and stays baselining until its final page; an item offered while
/// it baselines is refused.
#[tokio::test]
async fn a_baseline_interrupted_after_some_pages_resumes_and_stays_baselining() {
    let harness = harness_with(ScriptedNative::new());
    let service = &harness.service;
    try_host_call(
        service,
        SOURCES_PUT,
        source_params("put", "inbox", "subject-1", &["mail.received"]),
    )
    .await
    .unwrap();
    for (index, cursor) in [(0u64, "p1"), (1, "p2")] {
        let page = with(
            advance_params(&format!("baseline:v{index}"), 1, index, cursor),
            "page",
            serde_json::json!({ "index": index, "final": false }),
        );
        let advanced = try_host_call(service, SOURCES_ADVANCE, page).await.unwrap();
        assert_eq!(
            advanced["source"]["collection"],
            serde_json::json!("baselining")
        );
    }
    // The collector stops here; the next run reads where the baseline is.
    let source = inbox(service).await;
    assert_eq!(source["cursor"]["token"], serde_json::json!("p2"));
    assert_eq!(source["cursor"]["version"], serde_json::json!(2));
    assert_eq!(source["collection"], serde_json::json!("baselining"));
    let early = try_host_call(
        service,
        SOURCES_ADVANCE,
        with_items(
            advance_params("early:v2", 1, 2, "p3"),
            &[upsert("m-1", "v1", "too early")],
        ),
    )
    .await
    .unwrap_err();
    assert_eq!(refusal_kind(&early).as_deref(), Some("baseline_required"));
    let resumed = with(
        advance_params("resumed:v2", 1, 2, "p3"),
        "page",
        serde_json::json!({ "index": 2, "final": false }),
    );
    let advanced = try_host_call(service, SOURCES_ADVANCE, resumed)
        .await
        .unwrap();
    assert_eq!(
        advanced["source"]["collection"],
        serde_json::json!("baselining")
    );
    let last = with(
        with(
            advance_params("last:v3", 1, 3, "c1"),
            "page",
            serde_json::json!({ "index": 3, "final": true }),
        ),
        "coverage",
        serde_json::json!("complete"),
    );
    let advanced = try_host_call(service, SOURCES_ADVANCE, last).await.unwrap();
    assert_eq!(advanced["source"]["collection"], serde_json::json!("live"));
}

/// A baseline the collector starts over from its first page announces none
/// of what its pages read: the mob's log stays empty until the first real
/// announcement.
#[tokio::test]
async fn a_restarted_baseline_announces_nothing_of_the_pages_it_read() {
    let harness = harness_with(ScriptedNative::new());
    let service = &harness.service;
    try_host_call(
        service,
        SOURCES_PUT,
        source_params("put", "inbox", "subject-1", &["mail.received"]),
    )
    .await
    .unwrap();
    subscribe_to_inbox(service, "sub-a", "agent:a").await;
    let pages = [
        (0u64, 0u64, false, "p1"),
        (1, 1, false, "p2"),
        // The collector restarted its baseline from the first page.
        (0, 2, false, "q1"),
        (1, 3, true, "q2"),
    ];
    for (index, version, last, cursor) in pages {
        let mut page = with(
            advance_params(&format!("page:v{version}"), 1, version, cursor),
            "page",
            serde_json::json!({ "index": index, "final": last }),
        );
        if last {
            page["coverage"] = serde_json::json!("complete");
        }
        try_host_call(service, SOURCES_ADVANCE, page).await.unwrap();
    }
    assert_eq!(
        inbox(service).await["collection"],
        serde_json::json!("live")
    );
    assert!(
        owed(&harness).await.is_empty(),
        "a baseline captures nothing"
    );
    let first = try_host_call(
        service,
        ANNOUNCE,
        serde_json::json!({ "source_key": "first", "topics": ["other"], "body": "b" }),
    )
    .await
    .unwrap();
    assert_eq!(
        first["announcement"]["sequence"],
        serde_json::json!(1),
        "the baseline announced nothing into the mob's log"
    );
}

/// A source's first collection takes a baseline and announces nothing; a
/// first collection that brings items is refused until the baseline is
/// taken.
#[tokio::test]
async fn a_first_collection_baselines_and_announces_nothing() {
    let harness = harness_with(ScriptedNative::new());
    let service = &harness.service;
    try_host_call(
        service,
        SOURCES_PUT,
        source_params("put", "inbox", "subject-1", &["mail.received"]),
    )
    .await
    .unwrap();
    subscribe_to_inbox(service, "sub-a", "agent:a").await;
    let history = try_host_call(
        service,
        SOURCES_ADVANCE,
        with_items(
            advance_params("history:v0", 1, 0, "c1"),
            &[upsert("m-0", "v1", "old mail")],
        ),
    )
    .await
    .unwrap_err();
    assert_eq!(refusal_kind(&history).as_deref(), Some("baseline_required"));
    let baseline = with(
        advance_params("baseline:v0", 1, 0, "c1"),
        "coverage",
        serde_json::json!("complete"),
    );
    let advanced = try_host_call(service, SOURCES_ADVANCE, baseline)
        .await
        .unwrap();
    assert_eq!(advanced["source"]["collection"], serde_json::json!("live"));
    assert_eq!(advanced["items"], serde_json::json!([]));
    assert!(owed(&harness).await.is_empty());
}

/// A multi-page baseline turns the source live only when its final page
/// commits; each page before it moves the cursor and keeps it baselining.
#[tokio::test]
async fn a_multi_page_baseline_turns_live_only_on_its_final_page() {
    let harness = harness_with(ScriptedNative::new());
    let service = &harness.service;
    try_host_call(
        service,
        SOURCES_PUT,
        source_params("put", "inbox", "subject-1", &["mail.received"]),
    )
    .await
    .unwrap();
    for index in 0..3u64 {
        let page = with(
            advance_params(&format!("page:v{index}"), 1, index, &format!("p{index}")),
            "page",
            serde_json::json!({ "index": index, "final": false }),
        );
        let advanced = try_host_call(service, SOURCES_ADVANCE, page).await.unwrap();
        assert_eq!(
            advanced["source"]["collection"],
            serde_json::json!("baselining")
        );
        assert_eq!(
            advanced["source"]["health"]["status"],
            serde_json::json!("baselining")
        );
        assert_eq!(
            advanced["source"]["cursor"]["version"],
            serde_json::json!(index + 1)
        );
    }
    let last = with(
        with(
            advance_params("page:v3", 1, 3, "c1"),
            "page",
            serde_json::json!({ "index": 3, "final": true }),
        ),
        "coverage",
        serde_json::json!("complete"),
    );
    let advanced = try_host_call(service, SOURCES_ADVANCE, last).await.unwrap();
    assert_eq!(advanced["source"]["collection"], serde_json::json!("live"));
    assert_eq!(
        advanced["source"]["health"]["status"],
        serde_json::json!("ok")
    );
}

/// The first item after a baseline is announced once, captured once, and
/// every later collection of it replays.
#[tokio::test]
async fn the_first_item_after_a_baseline_is_announced_exactly_once() {
    let harness = harness_with(ScriptedNative::new());
    let service = &harness.service;
    live_inbox(service).await;
    subscribe_to_inbox(service, "sub-a", "agent:a").await;
    let step = with_items(
        advance_params("run:v1", 1, 1, "c2"),
        &[upsert("m-1", "v1", "first after baseline")],
    );
    let first = try_host_call(service, SOURCES_ADVANCE, step.clone())
        .await
        .unwrap();
    assert_eq!(outcomes(&first), vec!["announced"]);
    assert_eq!(first["items"][0]["captured"], serde_json::json!(1));
    try_host_call(service, SOURCES_ADVANCE, step).await.unwrap();
    let later = try_host_call(
        service,
        SOURCES_ADVANCE,
        with_items(
            advance_params("run-2:v2", 1, 2, "c3"),
            &[upsert("m-1", "v1", "first after baseline")],
        ),
    )
    .await
    .unwrap();
    assert_eq!(outcomes(&later), vec!["replayed"]);
    assert_eq!(owed(&harness).await.get("agent:a"), Some(&1));
}

/// A backfill announces its history across pages and stops at its total
/// bound: the page that would pass it is refused with what remains, and the
/// final page turns the source live.
#[tokio::test]
async fn a_backfill_spans_pages_and_stops_at_its_total_bound() {
    let harness = harness_with(ScriptedNative::new());
    let service = &harness.service;
    try_host_call(
        service,
        SOURCES_PUT,
        with(
            source_params("put", "inbox", "subject-1", &["mail.received"]),
            "start",
            serde_json::json!({ "kind": "backfill", "max_items": 3 }),
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        inbox(service).await["collection"],
        serde_json::json!("backfilling")
    );
    let first = with(
        with_items(
            advance_params("backfill:v0", 1, 0, "b1"),
            &[upsert("h-1", "v1", "old 1"), upsert("h-2", "v1", "old 2")],
        ),
        "page",
        serde_json::json!({ "index": 0, "final": false }),
    );
    let advanced = try_host_call(service, SOURCES_ADVANCE, first)
        .await
        .unwrap();
    assert_eq!(outcomes(&advanced), vec!["announced", "announced"]);
    assert_eq!(
        advanced["source"]["collection"],
        serde_json::json!("backfilling")
    );
    let past = with(
        with_items(
            advance_params("backfill:v1", 1, 1, "b2"),
            &[upsert("h-3", "v1", "old 3"), upsert("h-4", "v1", "old 4")],
        ),
        "page",
        serde_json::json!({ "index": 1, "final": false }),
    );
    let refused = try_host_call(service, SOURCES_ADVANCE, past)
        .await
        .unwrap_err();
    assert_eq!(
        refused.data,
        Some(serde_json::json!({ "kind": "backfill_bound_exceeded", "remaining": 1 }))
    );
    let last = with(
        with(
            with_items(
                advance_params("backfill-last:v1", 1, 1, "c1"),
                &[upsert("h-3", "v1", "old 3")],
            ),
            "page",
            serde_json::json!({ "index": 1, "final": true }),
        ),
        "coverage",
        serde_json::json!("complete"),
    );
    let advanced = try_host_call(service, SOURCES_ADVANCE, last).await.unwrap();
    assert_eq!(outcomes(&advanced), vec!["announced"]);
    assert_eq!(advanced["source"]["collection"], serde_json::json!("live"));
}

/// A subscription made while a source is busy gets none of its history; one
/// that names an explicit frontier gets the retained window after it.
#[tokio::test]
async fn a_new_subscription_on_a_busy_source_gets_no_history_and_an_explicit_frontier_gets_its_window()
 {
    let harness = harness_with(native_serving_a_and_b());
    let service = &harness.service;
    // The audience covers both members: what they get is the frontier's and
    // the replay's to decide.
    let covering_both = with(
        source_params("put-inbox", "inbox", "subject-1", &["mail.received"]),
        "audience",
        serde_json::json!([member_grant("agent:a"), member_grant("agent:b")]),
    );
    try_host_call(service, SOURCES_PUT, covering_both)
        .await
        .unwrap();
    baseline_inbox(service, 1).await;
    let advanced = try_host_call(
        service,
        SOURCES_ADVANCE,
        with_items(
            advance_params("run:v1", 1, 1, "c2"),
            &[upsert("m-1", "v1", "one"), upsert("m-2", "v1", "two")],
        ),
    )
    .await
    .unwrap();
    let first_sequence = advanced["items"][0]["sequence"].as_u64().unwrap();
    subscribe_to_inbox(service, "now", "agent:a").await;
    let mut frontier = serde_json::Map::new();
    frontier.insert(OWN_MOB.to_string(), serde_json::json!(first_sequence));
    try_host_call(
        service,
        SUBSCRIPTIONS_PUT,
        serde_json::json!({
            "operation_id": "subscribe-window",
            "subscription_id": "window",
            "member": { "mob_id": OWN_MOB, "identity": "agent:b" },
            "topics": ["mail.received"],
            "start": { "kind": "after", "frontier": frontier },
        }),
    )
    .await
    .unwrap();
    let owed_now = owed(&harness).await;
    assert_eq!(
        owed_now.get("agent:a"),
        None,
        "no history for a new subscriber"
    );
    assert_eq!(
        owed_now.get("agent:b"),
        Some(&1),
        "the window after the frontier"
    );
    try_host_call(
        service,
        SOURCES_ADVANCE,
        with_items(
            advance_params("run:v2", 1, 2, "c3"),
            &[upsert("m-3", "v1", "three")],
        ),
    )
    .await
    .unwrap();
    let owed_later = owed(&harness).await;
    assert_eq!(owed_later.get("agent:a"), Some(&1));
    assert_eq!(owed_later.get("agent:b"), Some(&2));
}

/// An item of a stream that has not been baselined refuses the whole
/// advance, naming the stream, and nothing of the advance is announced.
#[tokio::test]
async fn an_item_of_an_unbaselined_stream_refuses_the_advance_and_announces_nothing() {
    let harness = harness_with(ScriptedNative::new());
    let service = &harness.service;
    live_inbox(service).await;
    let refused = try_host_call(
        service,
        SOURCES_ADVANCE,
        with_items(
            advance_params("run:v1", 1, 1, "c2"),
            &[
                upsert("m-1", "v1", "default stream"),
                upsert_in("work", "w-1", "work"),
            ],
        ),
    )
    .await
    .unwrap_err();
    assert_eq!(
        refused.data,
        Some(serde_json::json!({ "kind": "stream_not_baselined", "streams": ["work"] }))
    );
    assert_eq!(
        inbox(service).await["cursor"]["version"],
        serde_json::json!(1)
    );
    let advanced = try_host_call(
        service,
        SOURCES_ADVANCE,
        with_items(
            advance_params("run-2:v1", 1, 1, "c2"),
            &[upsert("m-1", "v1", "default stream")],
        ),
    )
    .await
    .unwrap();
    assert_eq!(outcomes(&advanced), vec!["announced"]);
}

/// A stream onboarded with a baseline announces only the items collected
/// after it.
#[tokio::test]
async fn a_baselined_stream_announces_only_its_later_items() {
    let harness = harness_with(ScriptedNative::new());
    let service = &harness.service;
    live_inbox(service).await;
    let onboard = with(
        with(
            advance_params("onboard:v1", 1, 1, "c2"),
            "streams",
            serde_json::json!([{ "stream": "work", "mode": "baseline", "final": true }]),
        ),
        "coverage",
        serde_json::json!("complete"),
    );
    let onboarded = try_host_call(service, SOURCES_ADVANCE, onboard)
        .await
        .unwrap();
    assert_eq!(
        onboarded["streams"],
        serde_json::json!([{ "stream": "work", "phase": "live" }])
    );
    assert_eq!(
        onboarded["source"]["streams"],
        serde_json::json!({ "live": 1, "onboarding": 0 })
    );
    let advanced = try_host_call(
        service,
        SOURCES_ADVANCE,
        with_items(
            advance_params("run:v2", 1, 2, "c3"),
            &[upsert_in("work", "w-1", "a work mail")],
        ),
    )
    .await
    .unwrap();
    assert_eq!(outcomes(&advanced), vec!["announced"]);
    let streams = try_host_call(
        service,
        SOURCES_GET,
        serde_json::json!({ "source_id": "inbox", "include_streams": true }),
    )
    .await
    .unwrap();
    assert_eq!(streams["streams"][0]["stream"], serde_json::json!("work"));
    assert_eq!(streams["streams"][0]["phase"], serde_json::json!("live"));
}

/// A stream's backfill starts with its onboarding entry, in the advance that
/// carries its first items, and stops at the stream's own bound.
#[tokio::test]
async fn a_stream_backfill_starts_with_its_onboarding_entry() {
    let harness = harness_with(ScriptedNative::new());
    let service = &harness.service;
    live_inbox(service).await;
    let start = with(
        with_items(
            advance_params("backfill:v1", 1, 1, "c2"),
            &[upsert_in("work", "w-1", "old work")],
        ),
        "streams",
        serde_json::json!([
            { "stream": "work", "mode": "backfill", "max_items": 2, "final": false }
        ]),
    );
    let started = try_host_call(service, SOURCES_ADVANCE, start)
        .await
        .unwrap();
    assert_eq!(outcomes(&started), vec!["announced"]);
    assert_eq!(
        started["streams"],
        serde_json::json!([{ "stream": "work", "phase": "backfilling" }])
    );
    let past = try_host_call(
        service,
        SOURCES_ADVANCE,
        with_items(
            advance_params("backfill:v2", 1, 2, "c3"),
            &[
                upsert_in("work", "w-2", "old"),
                upsert_in("work", "w-3", "older"),
            ],
        ),
    )
    .await
    .unwrap_err();
    assert_eq!(
        past.data,
        Some(serde_json::json!({
            "kind": "backfill_bound_exceeded", "remaining": 1, "stream": "work"
        }))
    );
    let last = with(
        with(
            with_items(
                advance_params("backfill-last:v2", 1, 2, "c3"),
                &[upsert_in("work", "w-2", "old")],
            ),
            "streams",
            serde_json::json!([
                { "stream": "work", "mode": "backfill", "max_items": 2, "final": true }
            ]),
        ),
        "coverage",
        serde_json::json!("complete"),
    );
    let finished = try_host_call(service, SOURCES_ADVANCE, last).await.unwrap();
    assert_eq!(outcomes(&finished), vec!["announced"]);
    assert_eq!(
        finished["streams"],
        serde_json::json!([{ "stream": "work", "phase": "live" }])
    );
}

/// Each `stream_not_baselined` refusal counts toward the source's failure
/// streak even when its collector reports nothing; a success ends the
/// streak.
#[tokio::test]
async fn stream_not_baselined_refusals_count_toward_the_failure_streak() {
    let harness = harness_with(ScriptedNative::new());
    let service = &harness.service;
    live_inbox(service).await;
    for attempt in 1..=3u64 {
        let refused = try_host_call(
            service,
            SOURCES_ADVANCE,
            with_items(
                advance_params(&format!("run-{attempt}:v1"), 1, 1, "c2"),
                &[upsert_in("work", "w-1", "work")],
            ),
        )
        .await
        .unwrap_err();
        assert_eq!(
            refusal_kind(&refused).as_deref(),
            Some("stream_not_baselined")
        );
        let health = &inbox(service).await["health"];
        assert_eq!(health["consecutive_failures"], serde_json::json!(attempt));
        assert_eq!(
            health["last_failure"]["class"],
            serde_json::json!("refused:stream_not_baselined")
        );
        assert_eq!(health["totals"]["refusals"], serde_json::json!(attempt));
        assert_eq!(health["status"], serde_json::json!("failing"));
    }
    let advanced = try_host_call(
        service,
        SOURCES_ADVANCE,
        advance_params("run-4:v1", 1, 1, "c2"),
    )
    .await
    .unwrap();
    assert_eq!(
        advanced["source"]["health"]["consecutive_failures"],
        serde_json::json!(0)
    );
    assert_eq!(
        advanced["source"]["health"]["status"],
        serde_json::json!("ok")
    );
}

/// Two versions and a deletion of one item are three announcements under
/// three keys: an edit never collapses into the version before it.
#[tokio::test]
async fn two_versions_and_a_deletion_of_one_item_are_three_announcements() {
    let harness = harness_with(ScriptedNative::new());
    let service = &harness.service;
    live_inbox(service).await;
    subscribe_to_inbox(service, "sub-a", "agent:a").await;
    let changes = [
        upsert("m-1", "v1", "draft"),
        upsert("m-1", "v2", "edited"),
        change_of("m-1", "v3", "delete", "deleted by its sender"),
    ];
    let mut keys = BTreeSet::new();
    for (version, item) in (1u64..).zip(changes) {
        let advanced = try_host_call(
            service,
            SOURCES_ADVANCE,
            with_items(
                advance_params(&format!("run:v{version}"), 1, version, "c"),
                &[item],
            ),
        )
        .await
        .unwrap();
        assert_eq!(outcomes(&advanced), vec!["announced"]);
        keys.insert(
            advanced["items"][0]["source_key"]
                .as_str()
                .unwrap()
                .to_string(),
        );
    }
    assert_eq!(keys.len(), 3);
    assert_eq!(owed(&harness).await.get("agent:a"), Some(&3));
}

/// Each advance records what its adapter observed and when; a spell of
/// empty snapshots is dated from its first one, and a delta ends it.
#[tokio::test]
async fn an_advance_records_its_observation_evidence() {
    let harness = harness_with(ScriptedNative::new());
    let service = &harness.service;
    live_inbox(service).await;
    let observed = |operation: &str, version: u64, evidence: serde_json::Value| {
        with(
            advance_params(operation, 1, version, "c"),
            "observed",
            evidence,
        )
    };
    harness.clock.store(2_000, Ordering::SeqCst);
    let empty = serde_json::json!({ "kind": "snapshot", "records": 0 });
    let advanced = try_host_call(service, SOURCES_ADVANCE, observed("o1", 1, empty.clone()))
        .await
        .unwrap();
    let health = &advanced["source"]["health"];
    assert_eq!(health["last_observed"], empty);
    assert_eq!(health["last_observed_at_ms"], serde_json::json!(2_000));
    assert_eq!(health["empty_snapshot_since_ms"], serde_json::json!(2_000));
    harness.clock.store(3_000, Ordering::SeqCst);
    let advanced = try_host_call(service, SOURCES_ADVANCE, observed("o2", 2, empty))
        .await
        .unwrap();
    assert_eq!(
        advanced["source"]["health"]["empty_snapshot_since_ms"],
        serde_json::json!(2_000),
        "the spell keeps its start"
    );
    harness.clock.store(4_000, Ordering::SeqCst);
    let delta = serde_json::json!({ "kind": "delta" });
    let advanced = try_host_call(service, SOURCES_ADVANCE, observed("o3", 3, delta.clone()))
        .await
        .unwrap();
    let health = &advanced["source"]["health"];
    assert_eq!(health["last_observed"], delta);
    assert_eq!(health["last_observed_at_ms"], serde_json::json!(4_000));
    assert_eq!(health["empty_snapshot_since_ms"], serde_json::Value::Null);
    harness.clock.store(5_000, Ordering::SeqCst);
    let advanced = try_host_call(service, SOURCES_ADVANCE, advance_params("o4", 1, 4, "c"))
        .await
        .unwrap();
    assert_eq!(
        advanced["source"]["health"]["last_observed_at_ms"],
        serde_json::json!(4_000),
        "an advance that says nothing keeps the last evidence"
    );
}

/// A rebaseline takes a fresh baseline and announces one gap notice on the
/// source's topics, and none of the history the provider lost.
#[tokio::test]
async fn a_rebaseline_announces_one_gap_notice_and_no_history() {
    let harness = harness_with(ScriptedNative::new());
    let service = &harness.service;
    live_inbox(service).await;
    subscribe_to_inbox(service, "sub-a", "agent:a").await;
    let rebaseline = with(
        with(
            advance_params("rebaseline:v1", 1, 1, "r1"),
            "rebaseline",
            serde_json::json!({ "reason": "cursor_expired" }),
        ),
        "page",
        serde_json::json!({ "index": 0, "final": false }),
    );
    let advanced = try_host_call(service, SOURCES_ADVANCE, rebaseline)
        .await
        .unwrap();
    assert_eq!(
        advanced["source"]["collection"],
        serde_json::json!("baselining")
    );
    assert_eq!(
        advanced["source"]["health"]["totals"]["gaps"],
        serde_json::json!(1)
    );
    assert_eq!(
        owed(&harness).await.get("agent:a"),
        Some(&1),
        "one gap notice"
    );
    let notice = harness
        .store
        .announcement(
            &mob(OWN_MOB),
            &SourceKey::new("source:inbox:gap:1:2").unwrap(),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        notice.announced_by,
        Announcer::Source {
            source_id: SourceId::new("inbox").unwrap()
        }
    );
    assert_eq!(notice.topics, topics(&["mail.received"]));
    assert!(notice.body.contains("cursor_expired"), "{}", notice.body);
    assert!(notice.body.contains("may be missing"), "{}", notice.body);
    let history = try_host_call(
        service,
        SOURCES_ADVANCE,
        with_items(
            advance_params("history:v2", 1, 2, "r2"),
            &[upsert("m-old", "v1", "lost history")],
        ),
    )
    .await
    .unwrap_err();
    assert_eq!(refusal_kind(&history).as_deref(), Some("baseline_required"));
    let last = with(
        with(
            advance_params("rebaseline-last:v2", 1, 2, "c9"),
            "page",
            serde_json::json!({ "index": 1, "final": true }),
        ),
        "coverage",
        serde_json::json!("complete"),
    );
    let advanced = try_host_call(service, SOURCES_ADVANCE, last).await.unwrap();
    assert_eq!(advanced["source"]["collection"], serde_json::json!("live"));
    assert_eq!(
        owed(&harness).await.get("agent:a"),
        Some(&1),
        "still one notice"
    );
}

/// The same key with other content is a conflict: recorded with both
/// content digests, the original untouched, and the cursor still advances.
#[tokio::test]
async fn a_conflicting_item_is_recorded_with_both_digests_and_the_cursor_still_advances() {
    let harness = harness_with(ScriptedNative::new());
    let service = &harness.service;
    live_inbox(service).await;
    let first = try_host_call(
        service,
        SOURCES_ADVANCE,
        with_items(
            advance_params("run:v1", 1, 1, "c2"),
            &[upsert("m-1", "v1", "the original")],
        ),
    )
    .await
    .unwrap();
    let sequence = first["items"][0]["sequence"].clone();
    let key = first["items"][0]["source_key"].clone();
    let conflicted = try_host_call(
        service,
        SOURCES_ADVANCE,
        with_items(
            advance_params("run:v2", 1, 2, "c3"),
            &[upsert("m-1", "v1", "a read time leaked into the body")],
        ),
    )
    .await
    .unwrap();
    let item = &conflicted["items"][0];
    assert_eq!(item["outcome"], serde_json::json!("conflict"));
    assert_eq!(item["sequence"], sequence);
    let existing = item["existing_digest"].as_str().unwrap();
    let offered = item["offered_digest"].as_str().unwrap();
    assert!(existing.starts_with("sha256:") && offered.starts_with("sha256:"));
    assert_ne!(existing, offered);
    assert_eq!(
        conflicted["source"]["cursor"]["version"],
        serde_json::json!(3)
    );
    let health = &conflicted["source"]["health"];
    assert_eq!(health["totals"]["conflicts"], serde_json::json!(1));
    assert_eq!(health["last_conflict"]["source_key"], key);
    assert_eq!(health["last_conflict"]["existing_sequence"], sequence);
    assert_eq!(
        health["last_conflict"]["existing_digest"],
        serde_json::json!(existing)
    );
    assert_eq!(
        health["last_conflict"]["offered_digest"],
        serde_json::json!(offered)
    );
    let original = harness
        .store
        .announcement(
            &mob(OWN_MOB),
            &SourceKey::new(key.as_str().unwrap()).unwrap(),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(original.body, "the original");
}

/// A source declared again with the same principal and scope replays the
/// item versions still retained instead of announcing them again, and
/// captures them for no subscriber made since.
#[tokio::test]
async fn a_redeclared_source_replays_retained_versions_and_redelivers_no_held_item() {
    let harness = harness_with(native_serving_a_and_b());
    let service = &harness.service;
    // The audience covers both members: what they get is the frontier's and
    // the replay's to decide.
    let covering_both = with(
        source_params("put-inbox", "inbox", "subject-1", &["mail.received"]),
        "audience",
        serde_json::json!([member_grant("agent:a"), member_grant("agent:b")]),
    );
    try_host_call(service, SOURCES_PUT, covering_both.clone())
        .await
        .unwrap();
    baseline_inbox(service, 1).await;
    let first = try_host_call(
        service,
        SOURCES_ADVANCE,
        with_items(
            advance_params("run:v1", 1, 1, "c2"),
            &[upsert("m-1", "v1", "kept")],
        ),
    )
    .await
    .unwrap();
    try_host_call(
        service,
        SOURCES_END,
        serde_json::json!({ "operation_id": "end", "source_id": "inbox", "expected_revision": 1 }),
    )
    .await
    .unwrap();
    subscribe_to_inbox(service, "later", "agent:b").await;
    let declared = try_host_call(
        service,
        SOURCES_PUT,
        with(covering_both, "operation_id", serde_json::json!("again")),
    )
    .await
    .unwrap();
    assert_eq!(declared["source"]["generation"], serde_json::json!(2));
    let revision = declared["source"]["revision"].as_u64().unwrap();
    let baseline = with(
        advance_params("baseline-2:v0", revision, 0, "d1"),
        "coverage",
        serde_json::json!("complete"),
    );
    try_host_call(service, SOURCES_ADVANCE, baseline)
        .await
        .unwrap();
    let again = try_host_call(
        service,
        SOURCES_ADVANCE,
        with_items(
            advance_params("run-2:v1", revision, 1, "d2"),
            &[upsert("m-1", "v1", "kept")],
        ),
    )
    .await
    .unwrap();
    assert_eq!(outcomes(&again), vec!["replayed"]);
    assert_eq!(again["items"][0]["sequence"], first["items"][0]["sequence"]);
    assert_eq!(owed(&harness).await.get("agent:b"), None);
}

/// An advance or a failure under another principal than the generation
/// pins is refused typed, with both principals' digests and neither
/// principal.
#[tokio::test]
async fn an_advance_under_another_principal_is_refused_typed() {
    let harness = harness_with(ScriptedNative::new());
    let service = &harness.service;
    live_inbox(service).await;
    let mut step = with_items(
        advance_params("run:v1", 1, 1, "c2"),
        &[upsert("m-1", "v1", "b")],
    );
    step["principal"] = principal_of("subject-2");
    let refused = try_host_call(service, SOURCES_ADVANCE, step)
        .await
        .unwrap_err();
    assert_eq!(refused.code, super::rpc::ANNOUNCEMENTS_REFUSED_CODE);
    let data = refused.data.clone().unwrap();
    assert_eq!(data["kind"], serde_json::json!("principal_changed"));
    let pinned = data["pinned_digest"].as_str().unwrap();
    let offered = data["offered_digest"].as_str().unwrap();
    assert!(pinned.starts_with("sha256:") && offered.starts_with("sha256:"));
    assert_ne!(pinned, offered);
    assert!(!data.to_string().contains("subject-"), "{data}");
    let failure = try_host_call(
        service,
        SOURCES_RECORD_FAILURE,
        serde_json::json!({
            "operation_id": "fail",
            "source_id": "inbox",
            "expected_revision": 1,
            "expected_version": 1,
            "principal": principal_of("subject-2"),
            "failure": { "class": "auth_required" },
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(refusal_kind(&failure).as_deref(), Some("principal_changed"));
    assert_eq!(
        inbox(service).await["cursor"]["version"],
        serde_json::json!(1)
    );
}

/// A refresh or reconnect under the same verified principal keeps the
/// source's generation, collection and cursor: a new connection reference
/// is a configuration change, not a new baseline.
#[tokio::test]
async fn a_same_principal_refresh_keeps_the_generation_and_cursor() {
    let harness = harness_with(ScriptedNative::new());
    let service = &harness.service;
    live_inbox(service).await;
    try_host_call(
        service,
        SOURCES_ADVANCE,
        advance_params("run:v1", 1, 1, "c2"),
    )
    .await
    .unwrap();
    let reconnected = try_host_call(
        service,
        SOURCES_PUT,
        with(
            with(
                source_params("reconnect", "inbox", "subject-1", &["mail.received"]),
                "connection",
                serde_json::json!({ "kind": "connect_setup", "id": "setup-2" }),
            ),
            "expected_revision",
            serde_json::json!(1),
        ),
    )
    .await
    .unwrap();
    let source = &reconnected["source"];
    assert_eq!(source["generation"], serde_json::json!(1));
    assert_eq!(source["revision"], serde_json::json!(2));
    assert_eq!(source["collection"], serde_json::json!("live"));
    assert_eq!(source["cursor"]["version"], serde_json::json!(2));
    assert_eq!(source["cursor"]["token"], serde_json::json!("c2"));
    let advanced = try_host_call(
        service,
        SOURCES_ADVANCE,
        with_items(
            advance_params("run:v2", 2, 2, "c3"),
            &[upsert("m-1", "v1", "after the refresh")],
        ),
    )
    .await
    .unwrap();
    assert_eq!(outcomes(&advanced), vec!["announced"]);
}

/// A deletion on a source whose provider has no observable deletions is
/// refused typed, and the advance commits nothing.
#[tokio::test]
async fn a_delete_on_a_source_without_observable_deletions_is_refused() {
    let harness = harness_with(ScriptedNative::new());
    let service = &harness.service;
    try_host_call(
        service,
        SOURCES_PUT,
        with(
            source_params("put", "inbox", "subject-1", &["mail.received"]),
            "observation",
            serde_json::json!({ "baseline": "snapshot", "deletions": "unobservable" }),
        ),
    )
    .await
    .unwrap();
    let baseline = with(
        advance_params("baseline", 1, 0, "c1"),
        "coverage",
        serde_json::json!("complete"),
    );
    try_host_call(service, SOURCES_ADVANCE, baseline)
        .await
        .unwrap();
    let refused = try_host_call(
        service,
        SOURCES_ADVANCE,
        with_items(
            advance_params("run:v1", 1, 1, "c2"),
            &[
                upsert("m-1", "v1", "b"),
                change_of("m-2", "v9", "delete", "gone"),
            ],
        ),
    )
    .await
    .unwrap_err();
    assert_eq!(refused.code, super::rpc::ANNOUNCEMENTS_REFUSED_CODE);
    assert_eq!(
        refusal_kind(&refused).as_deref(),
        Some("deletions_unobservable")
    );
    assert_eq!(
        inbox(service).await["cursor"]["version"],
        serde_json::json!(1)
    );
}

/// A step id names one request: the same id with other content is refused
/// typed and commits nothing.
#[tokio::test]
async fn a_changed_request_under_a_used_step_id_is_refused() {
    let harness = harness_with(ScriptedNative::new());
    let service = &harness.service;
    live_inbox(service).await;
    try_host_call(
        service,
        SOURCES_ADVANCE,
        with_items(
            advance_params("run:v1", 1, 1, "c2"),
            &[upsert("m-1", "v1", "b")],
        ),
    )
    .await
    .unwrap();
    let reused = try_host_call(
        service,
        SOURCES_ADVANCE,
        with_items(
            advance_params("run:v1", 1, 1, "c2"),
            &[upsert("m-2", "v1", "rebuilt under a used id")],
        ),
    )
    .await
    .unwrap_err();
    assert_eq!(reused.code, super::rpc::ANNOUNCEMENTS_CONFLICT_CODE);
    assert_eq!(refusal_kind(&reused).as_deref(), Some("operation_reused"));
    assert_eq!(
        inbox(service).await["cursor"]["version"],
        serde_json::json!(2)
    );
}

/// A run that lost its step's answer and its request reads the cursor
/// again: past the step's version, the step committed, and the run goes on
/// from the current cursor under a new step id.
#[tokio::test]
async fn a_resumed_run_after_an_advanced_cursor_steps_with_a_new_id() {
    let harness = harness_with(ScriptedNative::new());
    let service = &harness.service;
    live_inbox(service).await;
    let run = "schedule:s:occurrence:o";
    try_host_call(
        service,
        SOURCES_ADVANCE,
        with_items(
            advance_params(&format!("{run}:v1"), 1, 1, "c2"),
            &[upsert("m-1", "v1", "b")],
        ),
    )
    .await
    .unwrap();
    // The answer and the request were lost; the run reads the source again.
    let version = inbox(service).await["cursor"]["version"].as_u64().unwrap();
    assert_eq!(version, 2, "the step committed");
    let next = try_host_call(
        service,
        SOURCES_ADVANCE,
        with_items(
            advance_params(&format!("{run}:v{version}"), 1, version, "c3"),
            &[upsert("m-2", "v1", "the next page")],
        ),
    )
    .await
    .unwrap();
    assert_eq!(outcomes(&next), vec!["announced"]);
    assert_eq!(next["source"]["cursor"]["version"], serde_json::json!(3));
}

/// A page continuation moves the cursor but settles no checkpoint: the
/// source's last success waits for the read's final page.
#[tokio::test]
async fn a_page_continuation_does_not_settle_a_checkpoint() {
    let harness = harness_with(ScriptedNative::new());
    let service = &harness.service;
    live_inbox(service).await;
    assert_eq!(
        inbox(service).await["health"]["last_success_at_ms"],
        serde_json::json!(1_000)
    );
    harness.clock.store(2_000, Ordering::SeqCst);
    let continuation = with(
        with_items(
            advance_params("read:v1", 1, 1, "page-2"),
            &[upsert("m-1", "v1", "b")],
        ),
        "page",
        serde_json::json!({ "index": 0, "final": false }),
    );
    let advanced = try_host_call(service, SOURCES_ADVANCE, continuation)
        .await
        .unwrap();
    assert_eq!(
        advanced["source"]["cursor"]["version"],
        serde_json::json!(2)
    );
    assert_eq!(
        advanced["source"]["cursor"]["advanced_at_ms"],
        serde_json::json!(2_000)
    );
    assert_eq!(
        advanced["source"]["health"]["last_success_at_ms"],
        serde_json::json!(1_000),
        "a continuation settles nothing"
    );
    harness.clock.store(3_000, Ordering::SeqCst);
    let last = with(
        advance_params("read:v2", 1, 2, "c2"),
        "page",
        serde_json::json!({ "index": 1, "final": true }),
    );
    let advanced = try_host_call(service, SOURCES_ADVANCE, last).await.unwrap();
    assert_eq!(
        advanced["source"]["health"]["last_success_at_ms"],
        serde_json::json!(3_000)
    );
}

/// A baseline whose final page cannot prove complete coverage records a gap
/// and announces one gap notice; a final onboarding page that says nothing
/// about coverage is refused.
#[tokio::test]
async fn a_final_page_with_a_coverage_gap_announces_one_gap_notice() {
    let harness = harness_with(ScriptedNative::new());
    let service = &harness.service;
    try_host_call(
        service,
        SOURCES_PUT,
        source_params("put", "inbox", "subject-1", &["mail.received"]),
    )
    .await
    .unwrap();
    subscribe_to_inbox(service, "sub-a", "agent:a").await;
    let silent = try_host_call(
        service,
        SOURCES_ADVANCE,
        advance_params("baseline:v0", 1, 0, "c1"),
    )
    .await
    .unwrap_err();
    assert_eq!(silent.code, -32602, "{silent:?}");
    let gap = with(
        advance_params("baseline-gap:v0", 1, 0, "c1"),
        "coverage",
        serde_json::json!({ "gap": { "reason": "shared_cursor" } }),
    );
    let advanced = try_host_call(service, SOURCES_ADVANCE, gap).await.unwrap();
    assert_eq!(advanced["source"]["collection"], serde_json::json!("live"));
    assert_eq!(
        advanced["source"]["health"]["totals"]["gaps"],
        serde_json::json!(1)
    );
    assert_eq!(owed(&harness).await.get("agent:a"), Some(&1));
    let notice = harness
        .store
        .announcement(
            &mob(OWN_MOB),
            &SourceKey::new("source:inbox:gap:1:1").unwrap(),
        )
        .await
        .unwrap()
        .unwrap();
    assert!(notice.body.contains("shared_cursor"), "{}", notice.body);
}

/// A cursor token over 8 KiB is refused, never cut; one at the bound is
/// kept byte for byte.
#[tokio::test]
async fn a_cursor_token_over_its_bound_is_refused_never_truncated() {
    let harness = harness_with(ScriptedNative::new());
    let service = &harness.service;
    live_inbox(service).await;
    let over = "t".repeat(8 * 1024 + 1);
    let refused = try_host_call(
        service,
        SOURCES_ADVANCE,
        advance_params("run:v1", 1, 1, &over),
    )
    .await
    .unwrap_err();
    assert_eq!(refused.code, -32602, "{refused:?}");
    assert_eq!(
        inbox(service).await["cursor"]["token"],
        serde_json::json!("c1")
    );
    let at_bound = "t".repeat(8 * 1024);
    try_host_call(
        service,
        SOURCES_ADVANCE,
        advance_params("run-2:v1", 1, 1, &at_bound),
    )
    .await
    .unwrap();
    assert_eq!(
        inbox(service).await["cursor"]["token"],
        serde_json::json!(at_bound)
    );
}

/// 0.9.4 let a host announce under any key, "source:" included: retrying
/// such an announcement after the upgrade answers its original instead of
/// refusing it. Only a new announcement under the prefix is refused.
#[tokio::test]
async fn a_pre_upgrade_host_announcement_under_the_source_prefix_still_replays() {
    let harness = harness_with(ScriptedNative::new());
    let request = announce_request(OWN_MOB, "source:legacy-1", &["t"], "from 0.9.4");
    let announcement = Announcement {
        mob_id: request.mob_id.clone(),
        sequence: 1,
        announced_by: Announcer::Host,
        source_key: request.source_key.clone(),
        topics: request.topics.clone(),
        body: request.body.clone(),
        correlation_id: None,
        causes: Vec::new(),
        digest: request.digest(),
        announced_at_ms: 1,
    };
    {
        let connection =
            rusqlite::Connection::open(harness._dir.path().join("mob_announcements.sqlite3"))
                .unwrap();
        connection
            .execute(
                "INSERT INTO announcement_logs (mob_id, head_sequence, earliest_retained)
                 VALUES (?1, 1, 1)",
                rusqlite::params![OWN_MOB],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO announcements
                 (mob_id, sequence, source_key, announced_at_ms, rendered_bytes, record)
                 VALUES (?1, 1, 'source:legacy-1', 1, ?2, ?3)",
                rusqlite::params![
                    OWN_MOB,
                    i64::try_from(render_item(&announcement).len()).unwrap(),
                    serde_json::to_string(&announcement).unwrap()
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO announcement_topics (topic, mob_id, sequence) VALUES ('t', ?1, 1)",
                rusqlite::params![OWN_MOB],
            )
            .unwrap();
    }
    let replayed = try_host_call(
        &harness.service,
        ANNOUNCE,
        serde_json::json!({ "source_key": "source:legacy-1", "topics": ["t"], "body": "from 0.9.4" }),
    )
    .await
    .unwrap();
    assert_eq!(replayed["replayed"], serde_json::json!(true));
    assert_eq!(replayed["announcement"]["sequence"], serde_json::json!(1));
    let fresh = try_host_call(
        &harness.service,
        ANNOUNCE,
        serde_json::json!({ "source_key": "source:legacy-2", "topics": ["t"], "body": "new" }),
    )
    .await
    .unwrap_err();
    assert_eq!(fresh.code, -32602, "{fresh:?}");
}

/// A gateway holds a bounded number of live sources across all its mobs:
/// with every mob under its own bound, the next source is refused typed
/// with the gateway's bound and no mob, and an ended source frees its place.
#[tokio::test]
async fn a_gateway_holds_a_bounded_number_of_live_sources_across_its_mobs() {
    let (_dir, store) = temp_store();
    let full = super::sources::MAX_SOURCES_PER_GATEWAY / super::sources::MAX_SOURCES_PER_MOB;
    let mobs = ["payments", "events", "builds", "refunds", "a-newer"];
    for (index, mob_name) in (0..).zip(mobs.iter().take(full as usize)) {
        for slot in 0..super::sources::MAX_SOURCES_PER_MOB {
            let topic = format!("mail.{slot}");
            let mut put = put_source(
                &format!("s{slot}"),
                &format!("subject-{index}-{slot}"),
                &[topic.as_str()],
            );
            put.mob_id = mob(mob_name);
            store
                .put_source(&op(&format!("o-{index}-{slot}")), put, 1)
                .await
                .unwrap();
        }
    }
    let mut over = put_source("over", "subject-over", &["mail.over"]);
    over.mob_id = mob("a-newer");
    assert_eq!(
        store.put_source(&op("over-1"), over.clone(), 2).await,
        Err(AnnouncementStoreError::SourceLimit {
            limit: super::sources::MAX_SOURCES_PER_GATEWAY,
            mob_id: None,
        })
    );
    store
        .change_source(
            &op("end"),
            &mob("payments"),
            &SourceId::new("s0").unwrap(),
            1,
            super::sources::SourceChange::End,
            3,
        )
        .await
        .unwrap();
    store.put_source(&op("over-2"), over, 4).await.unwrap();
}

// ---------------------------------------------------------------------------
// Announcement sources: who may receive a source's items. Coverage at a
// subscription's put and at capture, routes, conversation members, and the
// audit of what was refused or withheld.
// ---------------------------------------------------------------------------

const AUDIT_LIST: &str = "meta/mob_announcements/audit/list";

/// A grant of this mob's member `identity`.
fn member_grant(identity: &str) -> serde_json::Value {
    serde_json::json!({
        "kind": "member",
        "member": { "mob_id": OWN_MOB, "identity": identity },
    })
}

/// `sources/put` params for "inbox" (topic "mail.received") with this
/// audience and these routes.
fn inbox_with(
    operation: &str,
    audience: serde_json::Value,
    routes: serde_json::Value,
) -> serde_json::Value {
    with(
        with(
            source_params(operation, "inbox", "subject-1", &["mail.received"]),
            "audience",
            audience,
        ),
        "routes",
        routes,
    )
}

/// A triage route of member `identity` forwarding on "mail.triaged" to
/// `forward_audience`.
fn triage_by(identity: &str, forward_audience: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "kind": "triage",
        "member": { "mob_id": OWN_MOB, "identity": identity },
        "forward_topics": ["mail.triaged"],
        "forward_audience": forward_audience,
    })
}

/// A native owner that serves these members of this mob besides "agent:a".
fn native_serving(identities: &[&str]) -> ScriptedNative {
    let mut native = ScriptedNative::new();
    for identity in identities {
        native.known.insert((*identity).to_string());
    }
    native
}

/// Subscribe this mob's member `identity` to `topics`, keeping a refusal.
async fn try_subscribe(
    service: &AnnouncementService,
    id: &str,
    identity: &str,
    topic: &[&str],
) -> std::result::Result<serde_json::Value, crate::rpc::JsonRpcError> {
    try_host_call(
        service,
        SUBSCRIPTIONS_PUT,
        serde_json::json!({
            "operation_id": format!("subscribe-{id}"),
            "subscription_id": id,
            "member": { "mob_id": OWN_MOB, "identity": identity },
            "topics": topic,
        }),
    )
    .await
}

/// Take "inbox"'s baseline at `revision`: live at version 1.
async fn baseline_inbox(service: &AnnouncementService, revision: u64) {
    let baseline = with(
        advance_params("baseline", revision, 0, "c1"),
        "coverage",
        serde_json::json!("complete"),
    );
    try_host_call(service, SOURCES_ADVANCE, baseline)
        .await
        .unwrap();
}

/// Member "agent:t" announces on `topic` as itself, citing `causes`.
async fn triage_announces(
    service: &AnnouncementService,
    key: &str,
    topic: &[&str],
    causes: Vec<AnnouncementRef>,
) -> AnnounceOutcome {
    service
        .announce(AnnounceRequest {
            mob_id: mob(OWN_MOB),
            announced_by: Announcer::Member {
                identity: MemberIdentity::new("agent:t").unwrap(),
            },
            source_key: SourceKey::new(key).unwrap(),
            topics: topics(topic),
            body: "a summary of the invoice".to_string(),
            correlation_id: None,
            causes,
        })
        .await
        .unwrap()
}

/// The entries `audit/list` answers for `params`.
async fn audit_rows(
    service: &AnnouncementService,
    params: serde_json::Value,
) -> Vec<serde_json::Value> {
    try_host_call(service, AUDIT_LIST, params).await.unwrap()["entries"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// A member a source's audience does not cover cannot subscribe to the
/// source's topics, through the host's RPC or through its own tool; a
/// covered member can, both ways.
#[tokio::test]
async fn an_uncovered_member_is_refused_by_rpc_and_tool_alike() {
    let tools = tool_harness(super::tools::MemberLimits::default());
    let service = &tools.harness.service;
    try_host_call(
        service,
        SOURCES_PUT,
        source_params("put", "inbox", "subject-1", &["mail.received"]),
    )
    .await
    .unwrap();
    let refused = try_subscribe(service, "b-mail", "agent:b", &["mail.received"])
        .await
        .unwrap_err();
    assert_eq!(refused.code, super::rpc::ANNOUNCEMENTS_REFUSED_CODE);
    assert_eq!(
        refused.data,
        Some(serde_json::json!({
            "kind": "audience_not_covered", "source_id": "inbox", "topic": "mail.received"
        }))
    );
    let subscribe = serde_json::json!({
        "action": "subscribe", "subscription": "mail", "topics": ["mail.received"]
    });
    let (failed, result) = tools
        .call(
            Some("agent:b"),
            super::tools::SUBSCRIPTION_TOOL,
            subscribe.clone(),
        )
        .await;
    assert!(failed, "the tool reaches the same check: {result}");
    assert!(result.to_string().contains("audience"), "{result}");

    try_subscribe(service, "a-mail", "agent:a", &["mail.received"])
        .await
        .unwrap();
    let (failed, result) = tools
        .call(Some("agent:a"), super::tools::SUBSCRIPTION_TOOL, subscribe)
        .await;
    assert!(!failed, "{result}");
}

/// Role and label grants match only what the member's roster records: a
/// member whose role or labels differ is refused, and so is a member with
/// no roster facts at all.
#[tokio::test]
async fn a_missing_role_or_label_fails_coverage_closed() {
    let native = native_serving(&["agent:b", "agent:c", "agent:d", "agent:e"]);
    native.set_roster_facts("agent:b", "analyst", &[]);
    native.set_roster_facts("agent:d", "triage", &[]);
    native.set_roster_facts("agent:e", "analyst", &[("team", "finance")]);
    let harness = harness_with(native);
    let service = &harness.service;
    try_host_call(
        service,
        SOURCES_PUT,
        inbox_with(
            "put",
            serde_json::json!([
                { "kind": "role", "mob_id": OWN_MOB, "role": "triage" },
                { "kind": "label", "mob_id": OWN_MOB, "key": "team", "value": "finance" },
            ]),
            serde_json::json!([{ "kind": "direct" }]),
        ),
    )
    .await
    .unwrap();
    for (identity, why) in [
        ("agent:b", "another role and no label"),
        ("agent:c", "no roster facts"),
    ] {
        let refused = try_subscribe(service, identity, identity, &["mail.received"])
            .await
            .unwrap_err();
        assert_eq!(
            refusal_kind(&refused).as_deref(),
            Some("audience_not_covered"),
            "{identity}: {why}"
        );
    }
    try_subscribe(service, "agent:d", "agent:d", &["mail.received"])
        .await
        .unwrap();
    try_subscribe(service, "agent:e", "agent:e", &["mail.received"])
        .await
        .unwrap();
}

/// A forward topic takes the forward audience: a member the source's
/// audience covers but the forward audience does not cannot subscribe to it.
#[tokio::test]
async fn a_member_outside_the_forward_audience_cannot_subscribe_to_a_forward_topic() {
    let harness = harness_with(native_serving(&["agent:b", "agent:t"]));
    let service = &harness.service;
    try_host_call(
        service,
        SOURCES_PUT,
        inbox_with(
            "put",
            serde_json::json!([
                member_grant("agent:a"),
                member_grant("agent:b"),
                member_grant("agent:t")
            ]),
            serde_json::json!([
                { "kind": "direct" },
                triage_by("agent:t", serde_json::json!([member_grant("agent:a")])),
            ]),
        ),
    )
    .await
    .unwrap();
    let refused = try_subscribe(service, "b-triaged", "agent:b", &["mail.triaged"])
        .await
        .unwrap_err();
    assert_eq!(
        refused.data,
        Some(serde_json::json!({
            "kind": "audience_not_covered", "source_id": "inbox", "topic": "mail.triaged"
        }))
    );
    try_subscribe(service, "a-triaged", "agent:a", &["mail.triaged"])
        .await
        .unwrap();
}

/// A forward reaches only the members its forward audience covers when it is
/// captured: a subscriber the audience no longer covers is withheld, with an
/// audit row, and the others capture it.
#[tokio::test]
async fn a_forward_is_withheld_from_a_subscriber_outside_the_forward_audience() {
    let harness = harness_with(native_serving(&["agent:b", "agent:t"]));
    let service = &harness.service;
    let routes = |forward_audience: serde_json::Value| serde_json::json!([{ "kind": "direct" }, triage_by("agent:t", forward_audience)]);
    let audience = serde_json::json!([
        member_grant("agent:a"),
        member_grant("agent:b"),
        member_grant("agent:t")
    ]);
    try_host_call(
        service,
        SOURCES_PUT,
        inbox_with(
            "put",
            audience.clone(),
            routes(serde_json::json!([
                member_grant("agent:a"),
                member_grant("agent:b")
            ])),
        ),
    )
    .await
    .unwrap();
    try_subscribe(service, "a-triaged", "agent:a", &["mail.triaged"])
        .await
        .unwrap();
    try_subscribe(service, "b-triaged", "agent:b", &["mail.triaged"])
        .await
        .unwrap();
    // The forward audience narrows after both subscribed.
    try_host_call(
        service,
        SOURCES_PUT,
        with(
            inbox_with(
                "narrow",
                audience,
                routes(serde_json::json!([member_grant("agent:a")])),
            ),
            "expected_revision",
            serde_json::json!(1),
        ),
    )
    .await
    .unwrap();
    let forwarded = triage_announces(service, "m-forward-1", &["mail.triaged"], Vec::new()).await;
    assert_eq!(forwarded.captured_for, vec![sid("a-triaged")]);
    let owed_now = owed(&harness).await;
    assert_eq!(owed_now.get("agent:a"), Some(&1));
    assert_eq!(owed_now.get("agent:b"), None, "withheld");
    let rows = audit_rows(service, serde_json::json!({ "source_id": "inbox" })).await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["action"], serde_json::json!("capture_withheld"));
    assert_eq!(rows[0]["reason"], serde_json::json!("audience_not_covered"));
    assert_eq!(
        rows[0]["member"],
        serde_json::json!({ "mob_id": OWN_MOB, "identity": "agent:b" })
    );
    assert_eq!(rows[0]["source_id"], serde_json::json!("inbox"));
    assert_eq!(rows[0]["generation"], serde_json::json!(1));
    assert_eq!(rows[0]["count"], serde_json::json!(1));
}

/// One member is never reachable both directly and through a forward of the
/// same source: the second subscription is refused, and so is a route change
/// that would make an existing subscriber reachable both ways.
#[tokio::test]
async fn a_member_reachable_directly_and_through_a_forward_is_refused() {
    let harness = harness_with(native_serving(&["agent:c", "agent:t"]));
    let service = &harness.service;
    let audience = serde_json::json!([
        member_grant("agent:a"),
        member_grant("agent:c"),
        member_grant("agent:t")
    ]);
    let forward_audience = serde_json::json!([member_grant("agent:a"), member_grant("agent:c")]);
    try_host_call(
        service,
        SOURCES_PUT,
        inbox_with(
            "put",
            audience.clone(),
            serde_json::json!([{ "kind": "direct" }, triage_by("agent:t", forward_audience.clone())]),
        ),
    )
    .await
    .unwrap();
    try_subscribe(service, "a-mail", "agent:a", &["mail.received"])
        .await
        .unwrap();
    let refused = try_subscribe(service, "a-triaged", "agent:a", &["mail.triaged"])
        .await
        .unwrap_err();
    assert_eq!(refused.code, super::rpc::ANNOUNCEMENTS_REFUSED_CODE);
    assert_eq!(
        refused.data,
        Some(serde_json::json!({
            "kind": "route_overlap",
            "source_id": "inbox",
            "member": { "mob_id": OWN_MOB, "identity": "agent:a" },
        }))
    );
    // "agent:c" takes the source's items directly and a topic no source
    // owns yet; a route change that makes that topic a forward topic would
    // reach it both ways.
    try_subscribe(service, "c-mail", "agent:c", &["mail.received"])
        .await
        .unwrap();
    try_subscribe(service, "c-digest", "agent:c", &["mail.digest"])
        .await
        .unwrap();
    let mut widened = triage_by("agent:t", forward_audience);
    widened["forward_topics"] = serde_json::json!(["mail.triaged", "mail.digest"]);
    let refused = try_host_call(
        service,
        SOURCES_PUT,
        with(
            inbox_with(
                "widen",
                audience,
                serde_json::json!([{ "kind": "direct" }, widened]),
            ),
            "expected_revision",
            serde_json::json!(1),
        ),
    )
    .await
    .unwrap_err();
    assert_eq!(
        refused.data,
        Some(serde_json::json!({
            "kind": "route_overlap",
            "source_id": "inbox",
            "member": { "mob_id": OWN_MOB, "identity": "agent:c" },
        }))
    );
}

/// A direct recipient and a distinct triage recipient both receive: the
/// direct route's members take the source's items, the triage member takes
/// its topics, and the forward audience takes the forward. A source topic no
/// direct route names is the triage member's alone.
#[tokio::test]
async fn direct_recipient_a_and_distinct_triage_recipient_b_both_receive() {
    let harness = harness_with(native_serving(&["agent:b", "agent:c", "agent:t"]));
    let service = &harness.service;
    let put = with(
        inbox_with(
            "put",
            serde_json::json!([
                member_grant("agent:a"),
                member_grant("agent:b"),
                member_grant("agent:c"),
                member_grant("agent:t"),
            ]),
            serde_json::json!([
                { "kind": "direct", "topics": ["mail.received"] },
                {
                    "kind": "triage",
                    "member": { "mob_id": OWN_MOB, "identity": "agent:t" },
                    "topics": ["mail.received", "mail.urgent"],
                    "forward_topics": ["mail.triaged"],
                    "forward_audience": [member_grant("agent:b")],
                },
            ]),
        ),
        "topics",
        serde_json::json!(["mail.received", "mail.urgent"]),
    );
    try_host_call(service, SOURCES_PUT, put).await.unwrap();
    baseline_inbox(service, 1).await;
    try_subscribe(service, "a-mail", "agent:a", &["mail.received"])
        .await
        .unwrap();
    let refused = try_subscribe(service, "c-urgent", "agent:c", &["mail.urgent"])
        .await
        .unwrap_err();
    assert_eq!(
        refused.data,
        Some(serde_json::json!({
            "kind": "no_direct_route", "source_id": "inbox", "topic": "mail.urgent"
        }))
    );
    try_subscribe(
        service,
        "t-mail",
        "agent:t",
        &["mail.received", "mail.urgent"],
    )
    .await
    .unwrap();
    try_subscribe(service, "b-triaged", "agent:b", &["mail.triaged"])
        .await
        .unwrap();
    let advanced = try_host_call(
        service,
        SOURCES_ADVANCE,
        with_items(
            advance_params("run:v1", 1, 1, "c2"),
            &[upsert("m-1", "v1", "an invoice arrived")],
        ),
    )
    .await
    .unwrap();
    assert_eq!(advanced["items"][0]["captured"], serde_json::json!(2));
    let forwarded = triage_announces(service, "m-forward-1", &["mail.triaged"], Vec::new()).await;
    assert_eq!(forwarded.captured_for, vec![sid("b-triaged")]);
    let owed_now = owed(&harness).await;
    assert_eq!(owed_now.get("agent:a"), Some(&1));
    assert_eq!(owed_now.get("agent:t"), Some(&1));
    assert_eq!(owed_now.get("agent:b"), Some(&1));
}

/// A member that owns a live conversation on the source's scope cannot
/// subscribe to its topics or its forward topics, and a subscription made
/// before it owned one captures nothing.
#[tokio::test]
async fn a_conversation_member_cannot_subscribe_its_own_scope() {
    let harness = harness_with(native_serving(&["agent:c", "agent:t"]));
    let service = &harness.service;
    let audience = serde_json::json!([
        member_grant("agent:a"),
        member_grant("agent:c"),
        member_grant("agent:t")
    ]);
    let routes = serde_json::json!([
        { "kind": "direct" },
        triage_by("agent:t", serde_json::json!([member_grant("agent:a"), member_grant("agent:c")])),
    ]);
    try_subscribe(service, "a-mail", "agent:a", &["mail.received"])
        .await
        .unwrap();
    try_host_call(
        service,
        SOURCES_PUT,
        with(
            inbox_with("put", audience, routes),
            "conversation_members",
            serde_json::json!([
                { "mob_id": OWN_MOB, "identity": "agent:c" },
                { "mob_id": OWN_MOB, "identity": "agent:a" },
            ]),
        ),
    )
    .await
    .unwrap();
    for topic in ["mail.received", "mail.triaged"] {
        let refused = try_subscribe(service, topic, "agent:c", &[topic])
            .await
            .unwrap_err();
        assert_eq!(
            refused.data,
            Some(serde_json::json!({
                "kind": "route_conflict", "conflict": "conversation", "source_id": "inbox"
            })),
            "{topic}"
        );
    }
    baseline_inbox(service, 1).await;
    let advanced = try_host_call(
        service,
        SOURCES_ADVANCE,
        with_items(
            advance_params("run:v1", 1, 1, "c2"),
            &[upsert(
                "m-1",
                "v1",
                "a reply in a conversation agent:a owns",
            )],
        ),
    )
    .await
    .unwrap();
    assert_eq!(advanced["items"][0]["captured"], serde_json::json!(0));
    assert_eq!(owed(&harness).await.get("agent:a"), None);
}

/// Every capture a source's audience withholds is an audit row, read back
/// per source, per member, or both.
#[tokio::test]
async fn withheld_captures_are_listed_per_source_and_per_member() {
    let harness = harness_with(native_serving(&["agent:b"]));
    let service = &harness.service;
    let both = serde_json::json!([member_grant("agent:a"), member_grant("agent:b")]);
    let calendar = |operation: &str, audience: serde_json::Value| {
        with(
            source_params(operation, "calendar", "subject-2", &["cal.events"]),
            "audience",
            audience,
        )
    };
    try_host_call(
        service,
        SOURCES_PUT,
        inbox_with(
            "inbox",
            both.clone(),
            serde_json::json!([{ "kind": "direct" }]),
        ),
    )
    .await
    .unwrap();
    try_host_call(service, SOURCES_PUT, calendar("calendar", both))
        .await
        .unwrap();
    for (identity, topic) in [
        ("agent:a", "mail.received"),
        ("agent:b", "mail.received"),
        ("agent:a", "cal.events"),
        ("agent:b", "cal.events"),
    ] {
        try_subscribe(service, &format!("{identity}-{topic}"), identity, &[topic])
            .await
            .unwrap();
    }
    // Both audiences narrow to "agent:a".
    let only_a = serde_json::json!([member_grant("agent:a")]);
    try_host_call(
        service,
        SOURCES_PUT,
        with(
            inbox_with(
                "inbox-narrow",
                only_a.clone(),
                serde_json::json!([{ "kind": "direct" }]),
            ),
            "expected_revision",
            serde_json::json!(1),
        ),
    )
    .await
    .unwrap();
    try_host_call(
        service,
        SOURCES_PUT,
        with(
            calendar("calendar-narrow", only_a),
            "expected_revision",
            serde_json::json!(1),
        ),
    )
    .await
    .unwrap();
    baseline_inbox(service, 2).await;
    try_host_call(
        service,
        SOURCES_ADVANCE,
        with_items(
            advance_params("inbox:v1", 2, 1, "c2"),
            &[upsert("m-1", "v1", "mail")],
        ),
    )
    .await
    .unwrap();
    let calendar_step = |operation: &str, version: u64, items: serde_json::Value| {
        let mut step = advance_params(operation, 2, version, "k");
        step["source_id"] = serde_json::json!("calendar");
        step["principal"] = principal_of("subject-2");
        step["items"] = items;
        step
    };
    let mut baseline = calendar_step("calendar-baseline", 0, serde_json::json!([]));
    baseline["coverage"] = serde_json::json!("complete");
    try_host_call(service, SOURCES_ADVANCE, baseline)
        .await
        .unwrap();
    let mut event = upsert("e-1", "v1", "a meeting moved");
    event["topics"] = serde_json::json!(["cal.events"]);
    try_host_call(
        service,
        SOURCES_ADVANCE,
        calendar_step("calendar:v1", 1, serde_json::json!([event])),
    )
    .await
    .unwrap();

    let b = serde_json::json!({ "mob_id": OWN_MOB, "identity": "agent:b" });
    let per_source = audit_rows(service, serde_json::json!({ "source_id": "inbox" })).await;
    assert_eq!(per_source.len(), 1, "{per_source:?}");
    assert_eq!(per_source[0]["member"], b);
    assert_eq!(
        per_source[0]["action"],
        serde_json::json!("capture_withheld")
    );
    let per_member = audit_rows(service, serde_json::json!({ "member": b })).await;
    let sources: Vec<_> = per_member
        .iter()
        .map(|row| row["source_id"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(sources, vec!["inbox", "calendar"], "{per_member:?}");
    let both_ways = audit_rows(
        service,
        serde_json::json!({ "source_id": "calendar", "member": b }),
    )
    .await;
    assert_eq!(both_ways.len(), 1, "{both_ways:?}");
    assert_eq!(both_ways[0]["source_id"], serde_json::json!("calendar"));
    assert_eq!(
        both_ways[0]["reason"],
        serde_json::json!("audience_not_covered")
    );
    let after = audit_rows(
        service,
        serde_json::json!({ "member": b, "after": per_member[0]["id"] }),
    )
    .await;
    assert_eq!(after, per_member[1..].to_vec(), "a list pages after an id");
    let withheld = audit_rows(
        service,
        serde_json::json!({ "member": b, "action": "capture_withheld" }),
    )
    .await;
    assert_eq!(
        withheld, per_member,
        "every entry here is a withheld capture"
    );
    let refused = audit_rows(
        service,
        serde_json::json!({ "member": b, "action": "subscription_refused" }),
    )
    .await;
    assert!(
        refused.is_empty(),
        "no subscription was refused: {refused:?}"
    );
}

/// A forward that cites its source item is an announcement of its own: its
/// own key and sequence, captured by the forward audience only, while the
/// item it cites stays with the members its routes gave it to.
#[tokio::test]
async fn a_forward_with_causes_is_its_own_announcement() {
    let harness = harness_with(native_serving(&["agent:b", "agent:t"]));
    let service = &harness.service;
    try_host_call(
        service,
        SOURCES_PUT,
        inbox_with(
            "put",
            serde_json::json!([
                member_grant("agent:a"),
                member_grant("agent:b"),
                member_grant("agent:t")
            ]),
            serde_json::json!([
                { "kind": "direct" },
                triage_by("agent:t", serde_json::json!([member_grant("agent:b")])),
            ]),
        ),
    )
    .await
    .unwrap();
    baseline_inbox(service, 1).await;
    try_subscribe(service, "t-mail", "agent:t", &["mail.received"])
        .await
        .unwrap();
    try_subscribe(service, "b-triaged", "agent:b", &["mail.triaged"])
        .await
        .unwrap();
    let refused = try_subscribe(service, "a-triaged", "agent:a", &["mail.triaged"])
        .await
        .unwrap_err();
    assert_eq!(
        refusal_kind(&refused).as_deref(),
        Some("audience_not_covered")
    );
    let advanced = try_host_call(
        service,
        SOURCES_ADVANCE,
        with_items(
            advance_params("run:v1", 1, 1, "c2"),
            &[upsert("m-1", "v1", "an invoice arrived")],
        ),
    )
    .await
    .unwrap();
    let item_key = SourceKey::new(advanced["items"][0]["source_key"].as_str().unwrap()).unwrap();
    let cause = AnnouncementRef {
        mob_id: mob(OWN_MOB),
        source_key: item_key.clone(),
    };
    let forwarded = triage_announces(
        service,
        "m-forward-1",
        &["mail.triaged"],
        vec![cause.clone()],
    )
    .await;
    assert!(!forwarded.replayed);
    assert_ne!(
        forwarded.announcement.sequence,
        advanced["items"][0]["sequence"].as_u64().unwrap()
    );
    assert_eq!(forwarded.announcement.causes, vec![cause]);
    assert_eq!(forwarded.captured_for, vec![sid("b-triaged")]);
    let owed_now = owed(&harness).await;
    assert_eq!(owed_now.get("agent:t"), Some(&1), "the item");
    assert_eq!(
        owed_now.get("agent:b"),
        Some(&1),
        "the forward, not the item"
    );
    assert_eq!(owed_now.get("agent:a"), None);
}

/// A triage member of another mob, or a role or label grant naming another
/// mob, could never act: such a declaration is refused. A member grant may
/// name a member of another gateway's mob.
#[tokio::test]
async fn cross_mob_triage_members_and_role_or_label_grants_are_refused() {
    let harness = harness_with(native_serving(&["agent:t"]));
    let service = &harness.service;
    let routes = |member_mob: &str| {
        serde_json::json!([
            { "kind": "direct" },
            {
                "kind": "triage",
                "member": { "mob_id": member_mob, "identity": "agent:t" },
                "forward_topics": ["mail.triaged"],
            },
        ])
    };
    let cases = [
        (
            "a triage member of another mob",
            serde_json::json!([member_grant("agent:a")]),
            routes("events"),
        ),
        (
            "a role grant naming another mob",
            serde_json::json!([{ "kind": "role", "mob_id": "events", "role": "triage" }]),
            serde_json::json!([{ "kind": "direct" }]),
        ),
        (
            "a label grant naming another mob",
            serde_json::json!([{ "kind": "label", "mob_id": "events", "key": "team", "value": "x" }]),
            serde_json::json!([{ "kind": "direct" }]),
        ),
    ];
    for (index, (case, audience, routes)) in cases.into_iter().enumerate() {
        let refused = try_host_call(
            service,
            SOURCES_PUT,
            inbox_with(&format!("put-{index}"), audience, routes),
        )
        .await
        .unwrap_err();
        assert_eq!(refused.code, -32602, "{case}: {refused:?}");
    }
    let mut forward_role = triage_by(
        "agent:t",
        serde_json::json!([{ "kind": "role", "mob_id": "events", "role": "triage" }]),
    );
    forward_role["forward_audience"] =
        serde_json::json!([{ "kind": "role", "mob_id": "events", "role": "triage" }]);
    let refused = try_host_call(
        service,
        SOURCES_PUT,
        inbox_with(
            "put-forward",
            serde_json::json!([member_grant("agent:a"), member_grant("agent:t")]),
            serde_json::json!([{ "kind": "direct" }, forward_role]),
        ),
    )
    .await
    .unwrap_err();
    assert_eq!(
        refused.code, -32602,
        "a forward audience role grant of another mob"
    );
    try_host_call(
        service,
        SOURCES_PUT,
        inbox_with(
            "put-peer",
            serde_json::json!([
                member_grant("agent:a"),
                { "kind": "member", "member": { "mob_id": "mob-b", "identity": "agent:x" } },
            ]),
            routes(OWN_MOB),
        ),
    )
    .await
    .unwrap();
}

/// A guard that refuses the store's transactions once its allowance runs
/// out.
#[derive(Default)]
struct AllowanceGuard {
    allowance: Mutex<Option<u32>>,
}

impl AllowanceGuard {
    fn allow(&self, transactions: Option<u32>) {
        *self.allowance.lock().unwrap() = transactions;
    }
}

impl super::sqlite::ConnectionGuard for AllowanceGuard {
    fn acquire(&self) -> Result<Box<dyn Send>> {
        let mut allowance = self.allowance.lock().unwrap();
        match allowance.as_mut() {
            Some(0) => Err(AnnouncementStoreError::Storage(
                "the guard holds the store".to_string(),
            )),
            Some(left) => {
                *left -= 1;
                Ok(Box::new(()))
            }
            None => Ok(Box::new(())),
        }
    }
}

/// An onboarding refusal answers as itself even when counting it toward the
/// failure streak fails: the caller learns the refusal, not the store's
/// trouble.
#[tokio::test]
async fn an_onboarding_refusal_survives_a_failure_to_count_it() {
    let dir = tempfile::tempdir().unwrap();
    let connection =
        rusqlite::Connection::open(dir.path().join("mob_announcements.sqlite3")).unwrap();
    connection.execute_batch(super::sqlite::SCHEMA).unwrap();
    connection.execute_batch(super::sqlite::SCHEMA_V2).unwrap();
    connection.execute_batch(super::sqlite::SCHEMA_V3).unwrap();
    let guard = Arc::new(AllowanceGuard::default());
    let store = SqliteAnnouncementStore::from_connection(
        connection,
        Some(Arc::clone(&guard) as Arc<dyn super::sqlite::ConnectionGuard>),
    );
    store
        .put_source(
            &op("put"),
            put_source("inbox", "subject-1", &["mail.received"]),
            1,
        )
        .await
        .unwrap();
    let step = |version: u64, extra: serde_json::Value| -> super::sources::AdvanceSource {
        let mut request = serde_json::json!({
            "mob_id": OWN_MOB,
            "source_id": "inbox",
            "expected_revision": 1,
            "expected_version": version,
            "principal": principal_of("subject-1"),
            "next_cursor": "c",
            "page": null,
            "coverage": null,
            "items": [],
            "streams": [],
            "rebaseline": null,
            "observed": null,
            "collected_at_ms": null,
        });
        for (key, value) in extra.as_object().cloned().unwrap_or_default() {
            request[key] = value;
        }
        serde_json::from_value(request).unwrap()
    };
    store
        .advance_source(
            &op("baseline"),
            step(0, serde_json::json!({ "coverage": "complete" })),
            2,
        )
        .await
        .unwrap();
    // The advance's own transaction runs; the one that would count its
    // refusal is refused.
    guard.allow(Some(1));
    let refused = store
        .advance_source(
            &op("run"),
            step(
                1,
                serde_json::json!({ "items": [upsert_in("work", "w-1", "b")] }),
            ),
            3,
        )
        .await;
    guard.allow(None);
    assert_eq!(
        refused.map(|_| ()),
        Err(AnnouncementStoreError::StreamNotBaselined {
            streams: vec!["work".to_string()],
        })
    );
    let source = store
        .source(&mob(OWN_MOB), &SourceId::new("inbox").unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        source.health.consecutive_failures, 0,
        "the refusal went uncounted"
    );
}

/// Withheld captures are one counted entry per source, generation, member
/// and reason: a second withheld item counts on the first one's entry,
/// keeping its first time and moving its last, and the source's next
/// generation starts an entry of its own.
#[tokio::test]
async fn withheld_captures_are_one_counted_entry_per_source_generation_and_member() {
    let harness = harness_with(native_serving(&["agent:b"]));
    let service = &harness.service;
    let direct = serde_json::json!([{ "kind": "direct" }]);
    try_host_call(
        service,
        SOURCES_PUT,
        inbox_with(
            "put",
            serde_json::json!([member_grant("agent:a"), member_grant("agent:b")]),
            direct.clone(),
        ),
    )
    .await
    .unwrap();
    try_subscribe(service, "b-direct", "agent:b", &["mail.received"])
        .await
        .unwrap();
    // The audience narrows past "agent:b", which stays subscribed.
    let only_a = serde_json::json!([member_grant("agent:a")]);
    try_host_call(
        service,
        SOURCES_PUT,
        with(
            inbox_with("narrow", only_a.clone(), direct.clone()),
            "expected_revision",
            serde_json::json!(1),
        ),
    )
    .await
    .unwrap();
    baseline_inbox(service, 2).await;
    try_host_call(
        service,
        SOURCES_ADVANCE,
        with_items(
            advance_params("inbox:v1", 2, 1, "c2"),
            &[upsert("m-1", "v1", "first")],
        ),
    )
    .await
    .unwrap();
    harness.clock.store(5_000, Ordering::SeqCst);
    try_host_call(
        service,
        SOURCES_ADVANCE,
        with_items(
            advance_params("inbox:v2", 2, 2, "c3"),
            &[upsert("m-2", "v1", "second")],
        ),
    )
    .await
    .unwrap();
    let b = serde_json::json!({ "mob_id": OWN_MOB, "identity": "agent:b" });
    let entries = audit_rows(service, serde_json::json!({ "member": b })).await;
    assert_eq!(entries.len(), 1, "one counted entry: {entries:?}");
    let entry = &entries[0];
    assert_eq!(entry["action"], serde_json::json!("capture_withheld"));
    assert_eq!(entry["reason"], serde_json::json!("audience_not_covered"));
    assert_eq!(entry["source_id"], serde_json::json!("inbox"));
    assert_eq!(entry["generation"], serde_json::json!(1));
    assert_eq!(entry["member"], b);
    assert_eq!(entry["count"], serde_json::json!(2));
    assert_eq!(entry["first_at_ms"], serde_json::json!(1_000));
    assert_eq!(entry["last_at_ms"], serde_json::json!(5_000));
    assert_eq!(entry["at_ms"], serde_json::json!(5_000));

    // The next generation counts apart.
    let renewed = try_host_call(
        service,
        SOURCES_PUT,
        with(
            with(
                inbox_with("renew", only_a, direct),
                "expected_revision",
                serde_json::json!(2),
            ),
            "new_generation",
            serde_json::json!(true),
        ),
    )
    .await
    .unwrap();
    assert_eq!(renewed["source"]["generation"], serde_json::json!(2));
    try_host_call(
        service,
        SOURCES_ADVANCE,
        with(
            advance_params("baseline-g2", 3, 0, "c1"),
            "coverage",
            serde_json::json!("complete"),
        ),
    )
    .await
    .unwrap();
    try_host_call(
        service,
        SOURCES_ADVANCE,
        with_items(
            advance_params("inbox:g2:v1", 3, 1, "c2"),
            &[upsert("m-3", "v1", "third")],
        ),
    )
    .await
    .unwrap();
    let entries = audit_rows(service, serde_json::json!({ "member": b })).await;
    assert_eq!(entries.len(), 2, "{entries:?}");
    assert_eq!(entries[0]["generation"], serde_json::json!(1));
    assert_eq!(entries[0]["count"], serde_json::json!(2));
    assert_eq!(entries[1]["generation"], serde_json::json!(2));
    assert_eq!(entries[1]["count"], serde_json::json!(1));
    assert_eq!(entries[1]["first_at_ms"], serde_json::json!(5_000));
}

/// A refused subscription is a counted entry too: a member that asks again
/// counts on the entry of its first refusal.
#[tokio::test]
async fn refused_subscriptions_are_counted_entries() {
    let harness = harness_with(native_serving(&["agent:b"]));
    let service = &harness.service;
    try_host_call(
        service,
        SOURCES_PUT,
        source_params("put", "inbox", "subject-1", &["mail.received"]),
    )
    .await
    .unwrap();
    for (attempt, at_ms) in [(1, 1_000), (2, 4_000)] {
        harness.clock.store(at_ms, Ordering::SeqCst);
        let refused = try_subscribe(
            service,
            &format!("b-{attempt}"),
            "agent:b",
            &["mail.received"],
        )
        .await
        .unwrap_err();
        assert_eq!(
            refusal_kind(&refused).as_deref(),
            Some("audience_not_covered")
        );
    }
    let b = serde_json::json!({ "mob_id": OWN_MOB, "identity": "agent:b" });
    let entries = audit_rows(
        service,
        serde_json::json!({ "member": b, "action": "subscription_refused" }),
    )
    .await;
    assert_eq!(entries.len(), 1, "{entries:?}");
    let entry = &entries[0];
    assert_eq!(entry["action"], serde_json::json!("subscription_refused"));
    assert_eq!(entry["reason"], serde_json::json!("audience_not_covered"));
    assert_eq!(entry["source_id"], serde_json::json!("inbox"));
    assert_eq!(entry["generation"], serde_json::json!(1));
    assert_eq!(entry["count"], serde_json::json!(2));
    assert_eq!(entry["first_at_ms"], serde_json::json!(1_000));
    assert_eq!(entry["last_at_ms"], serde_json::json!(4_000));
    let withheld = audit_rows(
        service,
        serde_json::json!({ "member": b, "action": "capture_withheld" }),
    )
    .await;
    assert!(withheld.is_empty(), "{withheld:?}");
}

/// A route change binds the subscribers already there. Once a declaration
/// drops the direct route, an earlier direct subscriber captures none of the
/// source's items (each withheld, counted as `no_direct_route`) while the
/// triage member still captures them. Re-adding the route lets the
/// subscriber, still current, capture what comes next, and nothing withheld
/// meanwhile reaches it.
#[tokio::test]
async fn a_removed_direct_route_stops_an_earlier_direct_subscriber() {
    let harness = harness_with(native_serving(&["agent:t"]));
    let service = &harness.service;
    let audience = serde_json::json!([member_grant("agent:a"), member_grant("agent:t")]);
    let triage = triage_by("agent:t", serde_json::json!([member_grant("agent:a")]));
    let both_routes = serde_json::json!([{ "kind": "direct" }, triage.clone()]);
    try_host_call(
        service,
        SOURCES_PUT,
        inbox_with("put", audience.clone(), both_routes.clone()),
    )
    .await
    .unwrap();
    try_subscribe(service, "a-direct", "agent:a", &["mail.received"])
        .await
        .unwrap();
    try_subscribe(service, "t-takes", "agent:t", &["mail.received"])
        .await
        .unwrap();
    baseline_inbox(service, 1).await;
    try_host_call(
        service,
        SOURCES_ADVANCE,
        with_items(
            advance_params("inbox:v1", 1, 1, "c2"),
            &[upsert("m-1", "v1", "first")],
        ),
    )
    .await
    .unwrap();
    let owed_now = owed(&harness).await;
    assert_eq!(owed_now.get("agent:a"), Some(&1));
    assert_eq!(owed_now.get("agent:t"), Some(&1));

    // The direct route goes: the source is triage-only.
    try_host_call(
        service,
        SOURCES_PUT,
        with(
            inbox_with("triage-only", audience.clone(), serde_json::json!([triage])),
            "expected_revision",
            serde_json::json!(1),
        ),
    )
    .await
    .unwrap();
    try_host_call(
        service,
        SOURCES_ADVANCE,
        with_items(
            advance_params("inbox:v2", 2, 2, "c3"),
            &[upsert("m-2", "v1", "second")],
        ),
    )
    .await
    .unwrap();
    let owed_now = owed(&harness).await;
    assert_eq!(
        owed_now.get("agent:a"),
        Some(&1),
        "the earlier direct subscriber captures nothing more"
    );
    assert_eq!(
        owed_now.get("agent:t"),
        Some(&2),
        "the triage member still captures"
    );
    let a = serde_json::json!({ "mob_id": OWN_MOB, "identity": "agent:a" });
    let entries = audit_rows(service, serde_json::json!({ "member": a })).await;
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(entries[0]["action"], serde_json::json!("capture_withheld"));
    assert_eq!(entries[0]["reason"], serde_json::json!("no_direct_route"));
    assert_eq!(entries[0]["count"], serde_json::json!(1));

    // The direct route comes back: what comes next reaches the subscriber.
    try_host_call(
        service,
        SOURCES_PUT,
        with(
            inbox_with("direct-again", audience, both_routes),
            "expected_revision",
            serde_json::json!(2),
        ),
    )
    .await
    .unwrap();
    try_host_call(
        service,
        SOURCES_ADVANCE,
        with_items(
            advance_params("inbox:v3", 3, 3, "c4"),
            &[upsert("m-3", "v1", "third")],
        ),
    )
    .await
    .unwrap();
    let owed_now = owed(&harness).await;
    assert_eq!(
        owed_now.get("agent:a"),
        Some(&2),
        "the first item and the third: the withheld second never comes"
    );
    assert_eq!(owed_now.get("agent:t"), Some(&3));
}

// ---------------------------------------------------------------------------
// Announcement sources' runners: the collector's host runnable and cadence
// a source declares, and the one durable schedule Meta keeps for it.
// ---------------------------------------------------------------------------

/// `params` with a runner: the host runnable `runnable`, every
/// `every_seconds`.
fn with_runner(params: serde_json::Value, runnable: &str, every_seconds: u64) -> serde_json::Value {
    with(
        params,
        "runner",
        serde_json::json!({ "host_runnable": runnable, "every_seconds": every_seconds }),
    )
}

/// A runner's cadence is one minute to 30 days: outside those bounds
/// `sources/put` refuses the source (-32602) and declares nothing, and at
/// either bound it keeps the runner as declared.
#[tokio::test]
async fn a_runner_cadence_outside_its_bounds_is_refused() {
    let harness = harness_with(ScriptedNative::new());
    for (operation, every_seconds) in [("o-zero", 0), ("o-short", 59), ("o-long", 2_592_001)] {
        let refused = try_host_call(
            &harness.service,
            SOURCES_PUT,
            with_runner(
                source_params(operation, "inbox", "subject-1", &["mail.received"]),
                "inbox-collector",
                every_seconds,
            ),
        )
        .await
        .unwrap_err();
        assert_eq!(refused.code, -32602, "{every_seconds}: {refused:?}");
    }
    let listed = try_host_call(&harness.service, SOURCES_LIST, serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(
        listed["sources"],
        serde_json::json!([]),
        "nothing is declared"
    );
    for (operation, source_id, subject, topic, every_seconds) in [
        ("o-minute", "inbox", "subject-1", "mail.received", 60),
        (
            "o-month",
            "archive",
            "subject-2",
            "mail.archived",
            2_592_000,
        ),
    ] {
        let put = try_host_call(
            &harness.service,
            SOURCES_PUT,
            with_runner(
                source_params(operation, source_id, subject, &[topic]),
                "inbox-collector",
                every_seconds,
            ),
        )
        .await
        .unwrap();
        assert_eq!(
            put["source"]["runner"],
            serde_json::json!({ "host_runnable": "inbox-collector", "every_seconds": every_seconds })
        );
    }
}

/// A runner names the host's collector runnable: a blank name, which Core
/// cannot schedule, and the memory steward's runnable, which is Meta's own,
/// are refused (-32602), and nothing is declared.
#[tokio::test]
async fn a_runner_naming_no_runnable_or_metas_own_is_refused() {
    let harness = harness_with(ScriptedNative::new());
    for (operation, runnable) in [
        ("o-blank", "   "),
        ("o-steward", crate::schedule_wiring::STEWARD_DREAM_RUNNABLE),
    ] {
        let refused = try_host_call(
            &harness.service,
            SOURCES_PUT,
            with_runner(
                source_params(operation, "inbox", "subject-1", &["mail.received"]),
                runnable,
                60,
            ),
        )
        .await
        .unwrap_err();
        assert_eq!(refused.code, -32602, "{runnable:?}: {refused:?}");
    }
    let listed = try_host_call(&harness.service, SOURCES_LIST, serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(
        listed["sources"],
        serde_json::json!([]),
        "nothing is declared"
    );
}

/// Sources' runners kept on a schedule service of their own, installed on
/// `service` the way a gateway installs its schedule authority.
async fn install_runners(service: &AnnouncementService) -> meerkat::ScheduleService {
    let schedules =
        meerkat::ScheduleService::new(Arc::new(meerkat::MemoryScheduleStore::default()));
    service
        .install_source_runners(Arc::new(
            crate::schedule_wiring::ScheduleSourceRunners::new(schedules.clone()),
        ))
        .await;
    schedules
}

/// The host-runnable schedules `schedules` keeps, as (runnable, params,
/// every seconds, schedule id), in params order.
async fn runner_schedules(
    schedules: &meerkat::ScheduleService,
) -> Vec<(String, serde_json::Value, u64, String)> {
    let mut kept: Vec<_> = schedules
        .list()
        .await
        .unwrap()
        .into_iter()
        .filter_map(|schedule| {
            let meerkat::TargetBinding::HostRunnable(binding) = &schedule.target else {
                return None;
            };
            let params =
                serde_json::from_str::<serde_json::Value>(binding.params.as_ref()?.get()).ok()?;
            let meerkat::TriggerSpec::Interval(spec) = &schedule.trigger else {
                return None;
            };
            Some((
                binding.runnable.to_string(),
                params,
                spec.every_seconds,
                schedule.schedule_id.to_string(),
            ))
        })
        .collect();
    kept.sort_by_key(|(_, params, _, _)| params.to_string());
    kept
}

/// A schedule of `runnable` with the params naming `source_id` of this
/// gateway's mob, every minute, carrying `labels`: one a host made itself
/// (no labels), or one an earlier boot of Meta made.
async fn schedule_for(
    schedules: &meerkat::ScheduleService,
    runnable: &str,
    source_id: &str,
    labels: BTreeMap<String, String>,
) -> String {
    schedules
        .create(meerkat::CreateScheduleRequest {
            name: Some(format!("made for {source_id}")),
            description: None,
            trigger: meerkat::TriggerSpec::Interval(meerkat::IntervalTriggerSpec {
                start_at_utc: chrono::Utc::now(),
                every_seconds: 60,
                end_at_utc: None,
            }),
            target: meerkat::TargetBinding::host_runnable(meerkat::HostRunnableTargetBinding {
                runnable: meerkat::HostRunnableName::parse(runnable).unwrap(),
                params: Some(
                    meerkat::HostRunnableParams::try_from(serde_json::json!({
                        "mob_id": OWN_MOB,
                        "source_id": source_id,
                    }))
                    .unwrap(),
                ),
            }),
            misfire_policy: meerkat::MisfirePolicy::default(),
            overlap_policy: meerkat::OverlapPolicy::default(),
            missing_target_policy: meerkat::MissingTargetPolicy::default(),
            labels,
            planning_horizon_days: None,
            planning_horizon_occurrences: None,
        })
        .await
        .unwrap()
        .schedule_id
        .to_string()
}

/// The ids of the schedules `schedules` keeps.
async fn schedule_ids(schedules: &meerkat::ScheduleService) -> BTreeSet<String> {
    schedules
        .list()
        .await
        .unwrap()
        .into_iter()
        .map(|schedule| schedule.schedule_id.to_string())
        .collect()
}

/// A declared source's runner gets one durable schedule: an interval of its
/// cadence from the declaration on, a host-runnable target naming its
/// runnable with the source as params, and Core's default overlap (skip
/// while running) and misfire (skip) policies. A replayed put, a re-put with
/// the same runner and a second install keep that one schedule.
#[tokio::test]
async fn a_declared_source_runner_gets_one_schedule_with_its_target() {
    let harness = harness_with(ScriptedNative::new());
    let schedules = install_runners(&harness.service).await;
    let params = with_runner(
        source_params("o1", "inbox", "subject-1", &["mail.received"]),
        "inbox-collector",
        120,
    );
    let before = chrono::Utc::now();
    try_host_call(&harness.service, SOURCES_PUT, params.clone())
        .await
        .unwrap();
    let after = chrono::Utc::now();
    try_host_call(&harness.service, SOURCES_PUT, params)
        .await
        .unwrap();
    try_host_call(
        &harness.service,
        SOURCES_PUT,
        with(
            with_runner(
                source_params("o2", "inbox", "subject-1", &["mail.received"]),
                "inbox-collector",
                120,
            ),
            "expected_revision",
            serde_json::json!(1),
        ),
    )
    .await
    .unwrap();
    harness
        .service
        .install_source_runners(Arc::new(
            crate::schedule_wiring::ScheduleSourceRunners::new(schedules.clone()),
        ))
        .await;

    let all = schedules.list().await.unwrap();
    assert_eq!(all.len(), 1, "one schedule: {all:#?}");
    let schedule = &all[0];
    let meerkat::TargetBinding::HostRunnable(binding) = &schedule.target else {
        panic!("a host-runnable target: {:?}", schedule.target);
    };
    assert_eq!(binding.runnable.as_str(), "inbox-collector");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(binding.params.as_ref().unwrap().get()).unwrap(),
        serde_json::json!({ "mob_id": OWN_MOB, "source_id": "inbox" })
    );
    let meerkat::TriggerSpec::Interval(spec) = &schedule.trigger else {
        panic!("an interval trigger: {:?}", schedule.trigger);
    };
    assert_eq!(spec.every_seconds, 120);
    assert_eq!(spec.end_at_utc, None);
    assert!(
        spec.start_at_utc >= before - chrono::Duration::seconds(1)
            && spec.start_at_utc <= after + chrono::Duration::seconds(1),
        "the first fire is due at the declaration: {} not in {before}..{after}",
        spec.start_at_utc
    );
    assert_eq!(
        schedule.overlap_policy,
        meerkat::OverlapPolicy::SkipIfRunning
    );
    assert_eq!(schedule.misfire_policy, meerkat::MisfirePolicy::Skip);
    assert_eq!(schedule.phase, meerkat::SchedulePhase::Active);
    assert!(
        schedule
            .config
            .labels
            .contains_key(crate::schedule_wiring::SOURCE_RUNNER_LABEL),
        "Meta's own: {:?}",
        schedule.config.labels
    );
}

/// Two sources sharing one collector runnable keep a schedule each, keyed on
/// the runnable and the source, and ending one leaves the other's.
#[tokio::test]
async fn two_sources_sharing_one_runnable_get_two_schedules_and_end_independently() {
    let harness = harness_with(ScriptedNative::new());
    let schedules = install_runners(&harness.service).await;
    for (operation, source_id, subject, topic) in [
        ("o1", "inbox", "subject-1", "mail.received"),
        ("o2", "archive", "subject-2", "mail.archived"),
    ] {
        try_host_call(
            &harness.service,
            SOURCES_PUT,
            with_runner(
                source_params(operation, source_id, subject, &[topic]),
                "mail-collector",
                60,
            ),
        )
        .await
        .unwrap();
    }
    let kept = runner_schedules(&schedules).await;
    assert_eq!(
        kept.iter()
            .map(|(runnable, params, ..)| (runnable.as_str(), params.clone()))
            .collect::<Vec<_>>(),
        vec![
            (
                "mail-collector",
                serde_json::json!({ "mob_id": OWN_MOB, "source_id": "archive" })
            ),
            (
                "mail-collector",
                serde_json::json!({ "mob_id": OWN_MOB, "source_id": "inbox" })
            ),
        ]
    );
    try_host_call(
        &harness.service,
        SOURCES_END,
        serde_json::json!({ "operation_id": "end-inbox", "source_id": "inbox", "expected_revision": 1 }),
    )
    .await
    .unwrap();
    let left = runner_schedules(&schedules).await;
    assert_eq!(left.len(), 1, "{left:#?}");
    assert_eq!(left[0], kept[0], "the other source keeps its schedule");
}

/// A changed cadence updates the source's schedule in place: the same
/// schedule, the new interval. A changed runnable replaces it.
#[tokio::test]
async fn a_changed_runner_cadence_updates_the_schedule_in_place() {
    let harness = harness_with(ScriptedNative::new());
    let schedules = install_runners(&harness.service).await;
    try_host_call(
        &harness.service,
        SOURCES_PUT,
        with_runner(
            source_params("o1", "inbox", "subject-1", &["mail.received"]),
            "inbox-collector",
            60,
        ),
    )
    .await
    .unwrap();
    let before = runner_schedules(&schedules).await;
    assert_eq!(before.len(), 1, "{before:#?}");
    let changed = try_host_call(
        &harness.service,
        SOURCES_PUT,
        with(
            with_runner(
                source_params("o2", "inbox", "subject-1", &["mail.received"]),
                "inbox-collector",
                900,
            ),
            "expected_revision",
            serde_json::json!(1),
        ),
    )
    .await
    .unwrap();
    let after = runner_schedules(&schedules).await;
    assert_eq!(after.len(), 1, "{after:#?}");
    assert_eq!(after[0].3, before[0].3, "the same schedule");
    assert_eq!(after[0].2, 900, "the new interval");

    let revision = changed["source"]["revision"].clone();
    try_host_call(
        &harness.service,
        SOURCES_PUT,
        with(
            with_runner(
                source_params("o3", "inbox", "subject-1", &["mail.received"]),
                "other-collector",
                900,
            ),
            "expected_revision",
            revision,
        ),
    )
    .await
    .unwrap();
    let replaced = runner_schedules(&schedules).await;
    assert_eq!(replaced.len(), 1, "{replaced:#?}");
    assert_eq!(replaced[0].0, "other-collector");
    assert_ne!(
        replaced[0].3, before[0].3,
        "a new schedule for the new runnable"
    );
}

/// An ended source ends its runner's schedule, and so does a source whose
/// declaration drops its runner. A schedule the host made itself for the
/// same collector and source is never Meta's to end.
#[tokio::test]
async fn an_ended_source_ends_its_runner_schedule() {
    let harness = harness_with(ScriptedNative::new());
    let schedules = install_runners(&harness.service).await;
    let hosts_own = schedule_for(&schedules, "inbox-collector", "inbox", BTreeMap::new()).await;
    try_host_call(
        &harness.service,
        SOURCES_PUT,
        with_runner(
            source_params("o1", "inbox", "subject-1", &["mail.received"]),
            "inbox-collector",
            60,
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        runner_schedules(&schedules).await.len(),
        2,
        "Meta's beside the host's own"
    );
    let ended = try_host_call(
        &harness.service,
        SOURCES_END,
        serde_json::json!({ "operation_id": "end", "source_id": "inbox", "expected_revision": 1 }),
    )
    .await
    .unwrap();
    assert_eq!(ended["source"]["state"], serde_json::json!("ended"));
    assert_eq!(
        schedule_ids(&schedules).await,
        BTreeSet::from([hosts_own.clone()]),
        "the end ends Meta's schedule and leaves the host's"
    );

    try_host_call(
        &harness.service,
        SOURCES_PUT,
        with_runner(
            source_params("o2", "archive", "subject-2", &["mail.archived"]),
            "archive-collector",
            60,
        ),
    )
    .await
    .unwrap();
    assert_eq!(schedule_ids(&schedules).await.len(), 2);
    try_host_call(
        &harness.service,
        SOURCES_PUT,
        with(
            source_params("o3", "archive", "subject-2", &["mail.archived"]),
            "expected_revision",
            serde_json::json!(1),
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        schedule_ids(&schedules).await,
        BTreeSet::from([hosts_own]),
        "a declaration without a runner ends the schedule"
    );
}

/// A paused source keeps its runner's schedule, active: the fires go on and
/// the collector reads the pause and does nothing. The pause has one owner,
/// the source: the collector reads `paused`, and an advance it makes anyway
/// is refused `source_paused`. A resume needs no schedule change.
#[tokio::test]
async fn a_paused_sources_runner_fires_and_the_collector_no_ops() {
    let harness = harness_with(ScriptedNative::new());
    let schedules = install_runners(&harness.service).await;
    try_host_call(
        &harness.service,
        SOURCES_PUT,
        with_runner(
            source_params("o1", "inbox", "subject-1", &["mail.received"]),
            "inbox-collector",
            60,
        ),
    )
    .await
    .unwrap();
    let before = runner_schedules(&schedules).await;
    try_host_call(
        &harness.service,
        SOURCES_PAUSE,
        serde_json::json!({ "operation_id": "pause", "source_id": "inbox", "expected_revision": 1 }),
    )
    .await
    .unwrap();
    let all = schedules.list().await.unwrap();
    assert_eq!(all.len(), 1, "{all:#?}");
    assert_eq!(
        all[0].phase,
        meerkat::SchedulePhase::Active,
        "the schedule keeps firing"
    );
    assert_eq!(runner_schedules(&schedules).await, before);

    let read = try_host_call(
        &harness.service,
        SOURCES_GET,
        serde_json::json!({ "source_id": "inbox" }),
    )
    .await
    .unwrap();
    assert_eq!(
        read["source"]["state"],
        serde_json::json!("paused"),
        "what the collector reads"
    );
    let refused = try_host_call(
        &harness.service,
        SOURCES_ADVANCE,
        with(
            advance_params("run:v1", 2, 0, "c1"),
            "coverage",
            serde_json::json!("complete"),
        ),
    )
    .await
    .unwrap_err();
    assert_eq!(refusal_kind(&refused).as_deref(), Some("source_paused"));

    try_host_call(
        &harness.service,
        SOURCES_RESUME,
        serde_json::json!({ "operation_id": "resume", "source_id": "inbox", "expected_revision": 2 }),
    )
    .await
    .unwrap();
    assert_eq!(
        runner_schedules(&schedules).await,
        before,
        "a resume needs no schedule change"
    );
}

/// Every source with a runner gets its schedule, whenever it was made: one
/// declared before the gateway installed its schedule authority gets it at
/// the install (a boot), and one made at runtime gets it at its put. A
/// runner schedule an earlier boot made for a source ended since ends at
/// the install; a host's own schedule stays.
#[tokio::test]
async fn a_runtime_source_runner_gets_its_schedule_too() {
    let harness = harness_with(ScriptedNative::new());
    try_host_call(
        &harness.service,
        SOURCES_PUT,
        with_runner(
            source_params("o1", "inbox", "subject-1", &["mail.received"]),
            "inbox-collector",
            60,
        ),
    )
    .await
    .unwrap();
    try_host_call(
        &harness.service,
        SOURCES_PUT,
        with_runner(
            source_params("o0", "old", "subject-0", &["mail.old"]),
            "old-collector",
            60,
        ),
    )
    .await
    .unwrap();
    try_host_call(
        &harness.service,
        SOURCES_END,
        serde_json::json!({ "operation_id": "end-old", "source_id": "old", "expected_revision": 1 }),
    )
    .await
    .unwrap();
    let schedules =
        meerkat::ScheduleService::new(Arc::new(meerkat::MemoryScheduleStore::default()));
    let earlier_boot = BTreeMap::from([(
        crate::schedule_wiring::SOURCE_RUNNER_LABEL.to_string(),
        "1".to_string(),
    )]);
    schedule_for(&schedules, "old-collector", "old", earlier_boot).await;
    let hosts_own = schedule_for(&schedules, "old-collector", "old", BTreeMap::new()).await;
    harness
        .service
        .install_source_runners(Arc::new(
            crate::schedule_wiring::ScheduleSourceRunners::new(schedules.clone()),
        ))
        .await;
    let booted = runner_schedules(&schedules).await;
    assert_eq!(booted.len(), 2, "{booted:#?}");
    let inbox = booted
        .iter()
        .find(|(_, params, ..)| params["source_id"] == serde_json::json!("inbox"))
        .expect("the inbox's schedule");
    assert_eq!(inbox.0, "inbox-collector");
    assert!(
        booted.iter().any(|(.., id)| *id == hosts_own),
        "the host's own stays: {booted:#?}"
    );

    try_host_call(
        &harness.service,
        SOURCES_PUT,
        with_runner(
            source_params("o2", "archive", "subject-2", &["mail.archived"]),
            "archive-collector",
            300,
        ),
    )
    .await
    .unwrap();
    let kept = runner_schedules(&schedules).await;
    assert_eq!(
        kept.iter()
            .filter(|(.., id)| *id != hosts_own)
            .map(|(runnable, params, every_seconds, _)| (
                runnable.as_str(),
                params["source_id"].clone(),
                *every_seconds
            ))
            .collect::<Vec<_>>(),
        vec![
            ("archive-collector", serde_json::json!("archive"), 300),
            ("inbox-collector", serde_json::json!("inbox"), 60),
        ]
    );
}

// ---------------------------------------------------------------------------
// Announcement sources: holds. What a member is owed and may no longer
// receive is held at its next seal and at the last check before every
// submission, released only by a fresh check, and expires at retention,
// each counted in the audit.
// ---------------------------------------------------------------------------

/// This mob's member "agent:b", as the wire names it.
fn agent_b() -> serde_json::Value {
    serde_json::json!({ "mob_id": OWN_MOB, "identity": "agent:b" })
}

/// The audit entries of `member` for `action`, read without the filter.
async fn entries_for(
    service: &AnnouncementService,
    member: serde_json::Value,
    action: &str,
) -> Vec<serde_json::Value> {
    audit_rows(service, serde_json::json!({ "member": member }))
        .await
        .into_iter()
        .filter(|entry| entry["action"] == serde_json::json!(action))
        .collect()
}

/// "inbox" covers "agent:a" and "agent:b" on its direct route; "agent:b"
/// subscribes directly and captures one item, "m-1", while covered.
async fn b_owes_an_inbox_item(service: &AnnouncementService) {
    try_host_call(
        service,
        SOURCES_PUT,
        inbox_with(
            "put",
            serde_json::json!([member_grant("agent:a"), member_grant("agent:b")]),
            serde_json::json!([{ "kind": "direct" }]),
        ),
    )
    .await
    .unwrap();
    try_subscribe(service, "b-direct", "agent:b", &["mail.received"])
        .await
        .unwrap();
    baseline_inbox(service, 1).await;
    let advanced = try_host_call(
        service,
        SOURCES_ADVANCE,
        with_items(
            advance_params("inbox:v1", 1, 1, "c2"),
            &[upsert("m-1", "v1", "first")],
        ),
    )
    .await
    .unwrap();
    assert_eq!(advanced["items"][0]["captured"], serde_json::json!(1));
}

/// "inbox"'s audience at `revision` becomes `grants`, on its direct route.
async fn inbox_audience(
    service: &AnnouncementService,
    operation: &str,
    revision: u64,
    grants: serde_json::Value,
) {
    try_host_call(
        service,
        SOURCES_PUT,
        with(
            inbox_with(operation, grants, serde_json::json!([{ "kind": "direct" }])),
            "expected_revision",
            serde_json::json!(revision),
        ),
    )
    .await
    .unwrap();
}

/// The batches sealed for "agent:b".
async fn batches_of_b(service: &AnnouncementService) -> Vec<Batch> {
    service
        .deliveries(&BatchQuery {
            member: Some(member("agent:b")),
            limit: 10,
            ..BatchQuery::default()
        })
        .await
        .unwrap()
}

/// A member that lost coverage after it captured an item has the item held
/// at its next seal: nothing is sealed or submitted for it, and the hold is
/// counted in the audit.
#[tokio::test]
async fn a_member_losing_coverage_has_owed_items_held() {
    let harness = harness_with(native_serving(&["agent:b"]));
    let service = &harness.service;
    b_owes_an_inbox_item(service).await;
    inbox_audience(
        service,
        "narrow",
        1,
        serde_json::json!([member_grant("agent:a")]),
    )
    .await;
    harness.clock.store(100_000, Ordering::SeqCst);
    service.run_round().await.unwrap();
    let batches = batches_of_b(service).await;
    assert!(batches.is_empty(), "nothing is sealed for b: {batches:?}");
    assert_eq!(harness.native.submits(), 0, "nothing reaches b");
    let held = entries_for(service, agent_b(), "items_held").await;
    assert_eq!(held.len(), 1, "{held:?}");
    assert_eq!(held[0]["reason"], serde_json::json!("audience_not_covered"));
    assert_eq!(held[0]["source_id"], serde_json::json!("inbox"));
    assert_eq!(held[0]["generation"], serde_json::json!(1));
    assert_eq!(held[0]["count"], serde_json::json!(1));
}

/// Held items are released only by a fresh check at the member's next seal:
/// regaining coverage moves nothing by itself, and the next seal checks
/// again, releases the held item and seals it with what came since.
#[tokio::test]
async fn regained_coverage_releases_held_items_only_after_a_fresh_check() {
    let harness = harness_with(native_serving(&["agent:b"]));
    let service = &harness.service;
    b_owes_an_inbox_item(service).await;
    inbox_audience(
        service,
        "narrow",
        1,
        serde_json::json!([member_grant("agent:a")]),
    )
    .await;
    harness.clock.store(100_000, Ordering::SeqCst);
    service.run_round().await.unwrap();
    assert!(batches_of_b(service).await.is_empty(), "held at the seal");

    // Coverage comes back: nothing moves without a check.
    inbox_audience(
        service,
        "widen",
        2,
        serde_json::json!([member_grant("agent:a"), member_grant("agent:b")]),
    )
    .await;
    harness.clock.store(200_000, Ordering::SeqCst);
    service.run_round().await.unwrap();
    assert!(
        batches_of_b(service).await.is_empty(),
        "not before its next check: no new item, and the recheck bound not reached"
    );
    assert!(
        entries_for(service, agent_b(), "items_released")
            .await
            .is_empty()
    );

    // The next seal checks afresh: the held item goes with what came since.
    try_host_call(
        service,
        SOURCES_ADVANCE,
        with_items(
            advance_params("inbox:v2", 3, 2, "c3"),
            &[upsert("m-2", "v1", "second")],
        ),
    )
    .await
    .unwrap();
    harness.clock.store(300_000, Ordering::SeqCst);
    service.run_round().await.unwrap();
    let batches = batches_of_b(service).await;
    assert_eq!(batches.len(), 1, "{batches:?}");
    assert_eq!(
        batches[0].items.len(),
        2,
        "the released item and the new one"
    );
    let released = entries_for(service, agent_b(), "items_released").await;
    assert_eq!(released.len(), 1, "{released:?}");
    assert_eq!(released[0]["count"], serde_json::json!(1));
    assert_eq!(
        released[0]["reason"],
        serde_json::json!("audience_not_covered")
    );
}

/// A batch sealed while its member was covered is checked again immediately
/// before its submission: a revocation in between cancels it while nothing
/// native holds it, and its item is held.
#[tokio::test]
async fn a_batch_sealed_before_a_revocation_is_cancelled_at_its_last_check() {
    let harness = harness_with(native_serving(&["agent:b"]));
    let service = &harness.service;
    b_owes_an_inbox_item(service).await;
    let policy = SealingPolicy {
        max_count: 256,
        max_bytes: 1_048_576,
        max_delay_ms: 60_000,
        min_wake_interval_ms: 60_000,
    };
    let sealed = harness
        .store
        .seal_batch(&member("agent:b"), &batch_id("b-sealed"), &policy, 100_000)
        .await
        .unwrap()
        .expect("sealed while b is covered");
    inbox_audience(
        service,
        "narrow",
        1,
        serde_json::json!([member_grant("agent:a")]),
    )
    .await;
    harness.clock.store(100_000, Ordering::SeqCst);
    service.run_round().await.unwrap();
    assert_eq!(harness.native.submits(), 0, "the last check stops it");
    let batches = batches_of_b(service).await;
    assert_eq!(batches.len(), 1, "{batches:?}");
    assert_eq!(batches[0].id, sealed.id);
    assert_eq!(batches[0].state, BatchState::Cancelled);
    let held = entries_for(service, agent_b(), "items_held").await;
    assert_eq!(held.len(), 1, "{held:?}");
    assert_eq!(held[0]["reason"], serde_json::json!("audience_not_covered"));
}

/// A resubmission after an answer that nothing was committed is checked
/// again first: a revocation while the outcome was open cancels the batch
/// instead of submitting it again.
#[tokio::test]
async fn a_resubmission_after_nothing_committed_is_checked_again() {
    let native = native_serving(&["agent:b"]);
    native.ledger.lock().unwrap().unanswered = true;
    let harness = harness_with(native);
    let service = &harness.service;
    b_owes_an_inbox_item(service).await;
    harness.clock.store(100_000, Ordering::SeqCst);
    service.run_round().await.unwrap();
    assert_eq!(harness.native.submits(), 1);
    let batches = batches_of_b(service).await;
    assert_eq!(batches.len(), 1, "{batches:?}");
    assert_eq!(batches[0].state, BatchState::Submitting { attempt: 1 });

    inbox_audience(
        service,
        "narrow",
        1,
        serde_json::json!([member_grant("agent:a")]),
    )
    .await;
    harness.native.ledger.lock().unwrap().unanswered = false;
    harness.round_after_pause().await;
    assert_eq!(harness.native.submits(), 1, "no second submission");
    let batches = batches_of_b(service).await;
    assert_eq!(batches.len(), 1, "{batches:?}");
    assert_eq!(batches[0].state, BatchState::Cancelled);
    let held = entries_for(service, agent_b(), "items_held").await;
    assert_eq!(held.len(), 1, "{held:?}");
}

/// A held item past the retention window expires with a counted audit
/// entry, and no gap line reports it to the member.
#[tokio::test]
async fn items_held_past_retention_expire_with_an_audit_row() {
    let harness = harness_with(native_serving(&["agent:b"]));
    let service = &harness.service;
    b_owes_an_inbox_item(service).await;
    inbox_audience(
        service,
        "narrow",
        1,
        serde_json::json!([member_grant("agent:a")]),
    )
    .await;
    harness.clock.store(100_000, Ordering::SeqCst);
    service.run_round().await.unwrap();
    assert!(batches_of_b(service).await.is_empty(), "held at the seal");

    let (retention_ms, _) = retention_and_quiet();
    let later = 1_000 + retention_ms + 1;
    harness
        .store
        .expire(later, retention_ms, 0, 100)
        .await
        .unwrap();
    let expired = entries_for(service, agent_b(), "expired_while_held").await;
    assert_eq!(expired.len(), 1, "{expired:?}");
    assert_eq!(expired[0]["count"], serde_json::json!(1));
    assert_eq!(
        expired[0]["reason"],
        serde_json::json!("audience_not_covered")
    );

    // Coverage back, the next item reaches b alone: the expired one is gone
    // and no gap line tells of it.
    inbox_audience(
        service,
        "widen",
        2,
        serde_json::json!([member_grant("agent:a"), member_grant("agent:b")]),
    )
    .await;
    harness.clock.store(later, Ordering::SeqCst);
    try_host_call(
        service,
        SOURCES_ADVANCE,
        with_items(
            advance_params("inbox:v2", 3, 2, "c3"),
            &[upsert("m-2", "v1", "second")],
        ),
    )
    .await
    .unwrap();
    harness.clock.store(later + 100_000, Ordering::SeqCst);
    service.run_round().await.unwrap();
    let batches = batches_of_b(service).await;
    assert_eq!(batches.len(), 1, "{batches:?}");
    assert_eq!(batches[0].items.len(), 1);
    assert!(batches[0].gaps.is_empty(), "{:?}", batches[0].gaps);
}

/// Routes bind what is already owed too: an item an earlier direct
/// subscriber captured before the direct route was removed is held at its
/// next seal, counted as `no_direct_route`.
#[tokio::test]
async fn an_item_owed_before_a_route_change_is_not_sealed_for_an_earlier_direct_subscriber() {
    let harness = harness_with(native_serving(&["agent:b", "agent:t"]));
    let service = &harness.service;
    let audience = serde_json::json!([
        member_grant("agent:a"),
        member_grant("agent:b"),
        member_grant("agent:t")
    ]);
    let triage = triage_by("agent:t", serde_json::json!([member_grant("agent:a")]));
    try_host_call(
        service,
        SOURCES_PUT,
        inbox_with(
            "put",
            audience.clone(),
            serde_json::json!([{ "kind": "direct" }, triage.clone()]),
        ),
    )
    .await
    .unwrap();
    try_subscribe(service, "b-direct", "agent:b", &["mail.received"])
        .await
        .unwrap();
    baseline_inbox(service, 1).await;
    try_host_call(
        service,
        SOURCES_ADVANCE,
        with_items(
            advance_params("inbox:v1", 1, 1, "c2"),
            &[upsert("m-1", "v1", "first")],
        ),
    )
    .await
    .unwrap();
    // The direct route goes before b's item is sealed.
    try_host_call(
        service,
        SOURCES_PUT,
        with(
            inbox_with("triage-only", audience, serde_json::json!([triage])),
            "expected_revision",
            serde_json::json!(1),
        ),
    )
    .await
    .unwrap();
    harness.clock.store(100_000, Ordering::SeqCst);
    service.run_round().await.unwrap();
    assert!(
        batches_of_b(service).await.is_empty(),
        "nothing sealed for b"
    );
    let held = entries_for(service, agent_b(), "items_held").await;
    assert_eq!(held.len(), 1, "{held:?}");
    assert_eq!(held[0]["reason"], serde_json::json!("no_direct_route"));
}

/// On a gateway without a governed binder, a batch committed while its
/// member was covered and admitted after a revocation reaches the member:
/// nothing Meta controls stands between the commit and the admission. The
/// delivery is counted as `delivered_across_revocation`.
#[tokio::test]
async fn a_batch_admitted_after_a_revocation_is_audited_as_delivered_across_it() {
    let mut native = native_serving(&["agent:b"]);
    native.apply_on_commit = false;
    let harness = harness_with(native);
    let service = &harness.service;
    b_owes_an_inbox_item(service).await;
    harness.clock.store(100_000, Ordering::SeqCst);
    service.run_round().await.unwrap();
    let batches = batches_of_b(service).await;
    assert_eq!(batches.len(), 1, "{batches:?}");
    assert_eq!(batches[0].state.tag(), BatchStateTag::Committed);

    inbox_audience(
        service,
        "narrow",
        1,
        serde_json::json!([member_grant("agent:a")]),
    )
    .await;
    harness
        .native
        .ledger
        .lock()
        .unwrap()
        .bindings
        .get_mut(batches[0].id.as_str())
        .unwrap()
        .2 = true;
    harness.round_after_pause().await;
    let batches = batches_of_b(service).await;
    assert_eq!(batches[0].state.tag(), BatchStateTag::Delivered);
    let across = entries_for(service, agent_b(), "delivered_across_revocation").await;
    assert_eq!(across.len(), 1, "{across:?}");
    assert_eq!(
        across[0]["reason"],
        serde_json::json!("audience_not_covered")
    );
    assert_eq!(across[0]["count"], serde_json::json!(1));
}

/// The bound on a held-only member's fresh check: a hold is checked again
/// at least this often while its member has nothing else to seal.
const HOLD_RECHECK_BOUND_MS: u64 = 300_000;

/// A held item whose member passes again is released within the bound even
/// when nothing new arrives for the member.
#[tokio::test]
async fn regained_coverage_without_new_traffic_releases_held_items_within_the_bound() {
    let harness = harness_with(native_serving(&["agent:b"]));
    let service = &harness.service;
    b_owes_an_inbox_item(service).await;
    inbox_audience(
        service,
        "narrow",
        1,
        serde_json::json!([member_grant("agent:a")]),
    )
    .await;
    harness.clock.store(100_000, Ordering::SeqCst);
    service.run_round().await.unwrap();
    assert!(batches_of_b(service).await.is_empty(), "held at the seal");
    inbox_audience(
        service,
        "widen",
        2,
        serde_json::json!([member_grant("agent:a"), member_grant("agent:b")]),
    )
    .await;
    harness
        .clock
        .store(100_000 + HOLD_RECHECK_BOUND_MS + 1, Ordering::SeqCst);
    service.run_round().await.unwrap();
    let batches = batches_of_b(service).await;
    assert_eq!(batches.len(), 1, "released and sealed: {batches:?}");
    assert_eq!(batches[0].items.len(), 1);
    let released = entries_for(service, agent_b(), "items_released").await;
    assert_eq!(released.len(), 1, "{released:?}");
    assert_eq!(released[0]["count"], serde_json::json!(1));
}

/// A hold that passes again is released even behind more holds than one
/// seal checks: an earlier generation's hold, which never releases, does
/// not stand in front of it.
#[tokio::test]
async fn a_regained_hold_is_released_behind_more_holds_than_one_seal_checks() {
    let harness = harness_with(native_serving(&["agent:b"]));
    let service = &harness.service;
    let both = serde_json::json!([member_grant("agent:a"), member_grant("agent:b")]);
    let only_a = serde_json::json!([member_grant("agent:a")]);
    // m-1, held under generation 1.
    b_owes_an_inbox_item(service).await;
    inbox_audience(service, "narrow", 1, only_a.clone()).await;
    harness.clock.store(100_000, Ordering::SeqCst);
    service.run_round().await.unwrap();
    // Generation 2 covers b again, and b captures m-2 there.
    try_host_call(
        service,
        SOURCES_PUT,
        with(
            with(
                inbox_with(
                    "renew",
                    both.clone(),
                    serde_json::json!([{ "kind": "direct" }]),
                ),
                "expected_revision",
                serde_json::json!(2),
            ),
            "new_generation",
            serde_json::json!(true),
        ),
    )
    .await
    .unwrap();
    try_host_call(
        service,
        SOURCES_ADVANCE,
        with(
            advance_params("baseline-g2", 3, 0, "c1"),
            "coverage",
            serde_json::json!("complete"),
        ),
    )
    .await
    .unwrap();
    let advanced = try_host_call(
        service,
        SOURCES_ADVANCE,
        with_items(
            advance_params("inbox:g2:v1", 3, 1, "c2"),
            &[upsert("m-2", "v1", "second")],
        ),
    )
    .await
    .unwrap();
    let second = advanced["items"][0]["sequence"].as_u64().unwrap();
    // m-2 is held under generation 2.
    inbox_audience(service, "narrow-g2", 3, only_a).await;
    harness.clock.store(200_000, Ordering::SeqCst);
    service.run_round().await.unwrap();
    assert!(batches_of_b(service).await.is_empty(), "both held");
    // b passes again: a seal that checks one hold releases m-2.
    inbox_audience(service, "widen-g2", 4, both).await;
    let one = SealingPolicy {
        max_count: 1,
        max_bytes: 1_048_576,
        max_delay_ms: 60_000,
        min_wake_interval_ms: 60_000,
    };
    let sealed = harness
        .store
        .seal_batch(&member("agent:b"), &batch_id("b-one"), &one, 10_000_000)
        .await
        .unwrap()
        .expect("m-2 is released and sealed");
    assert_eq!(sealed.items.len(), 1);
    assert_eq!(sealed.items[0].sequence, second);
}

/// Held items past retention beyond one expiry pass still get no gap line:
/// they wait for the next pass, which counts them expired_while_held.
#[tokio::test]
async fn held_items_past_retention_beyond_one_expiry_pass_get_no_gap_line() {
    let harness = harness_with(native_serving(&["agent:b"]));
    let service = &harness.service;
    b_owes_an_inbox_item(service).await;
    try_host_call(
        service,
        SOURCES_ADVANCE,
        with_items(
            advance_params("inbox:v2", 1, 2, "c3"),
            &[upsert("m-2", "v1", "second")],
        ),
    )
    .await
    .unwrap();
    inbox_audience(
        service,
        "narrow",
        1,
        serde_json::json!([member_grant("agent:a")]),
    )
    .await;
    harness.clock.store(100_000, Ordering::SeqCst);
    service.run_round().await.unwrap();
    assert_eq!(
        entries_for(service, agent_b(), "items_held").await[0]["count"],
        serde_json::json!(2),
        "both held"
    );
    let (retention_ms, _) = retention_and_quiet();
    let later = 1_000 + retention_ms + 1;
    for _ in 0..2 {
        harness
            .store
            .expire(later, retention_ms, 0, 1)
            .await
            .unwrap();
    }
    let expired = entries_for(service, agent_b(), "expired_while_held").await;
    assert_eq!(expired.len(), 1, "{expired:?}");
    assert_eq!(expired[0]["count"], serde_json::json!(2), "{expired:?}");
    inbox_audience(
        service,
        "widen",
        2,
        serde_json::json!([member_grant("agent:a"), member_grant("agent:b")]),
    )
    .await;
    harness.clock.store(later, Ordering::SeqCst);
    try_host_call(
        service,
        SOURCES_ADVANCE,
        with_items(
            advance_params("inbox:v3", 3, 3, "c4"),
            &[upsert("m-3", "v1", "third")],
        ),
    )
    .await
    .unwrap();
    harness.clock.store(later + 100_000, Ordering::SeqCst);
    service.run_round().await.unwrap();
    let batches = batches_of_b(service).await;
    assert_eq!(batches.len(), 1, "{batches:?}");
    assert_eq!(batches[0].items.len(), 1);
    assert!(batches[0].gaps.is_empty(), "{:?}", batches[0].gaps);
}

/// On a peer route an earlier request may still commit until the quiet
/// window after it: a batch that fails its last check inside that window is
/// neither submitted again nor cancelled. Past the window its outcome is
/// read first, so a late commit settles as delivered and is counted, and no
/// second batch carries its item.
#[tokio::test]
async fn a_peer_batch_failing_its_last_check_inside_the_quiet_window_is_neither_resubmitted_nor_cancelled()
 {
    let native = ScriptedNative::new();
    native.ledger.lock().unwrap().refuse =
        Some((SuspendCause::MemberNotLive, EvidenceClass::Local));
    let harness = harness_with(native);
    let service = &harness.service;
    let peer = peer_member("agent:p", "mob-b");
    let peer_grant = serde_json::json!({
        "kind": "member",
        "member": { "mob_id": "mob-b", "identity": "agent:p" },
    });
    try_host_call(
        service,
        SOURCES_PUT,
        inbox_with(
            "put",
            serde_json::json!([member_grant("agent:a"), peer_grant]),
            serde_json::json!([{ "kind": "direct" }]),
        ),
    )
    .await
    .unwrap();
    service
        .put_subscription(
            &op("p-direct"),
            put_for("p-direct", peer.clone(), &["mail.received"]),
        )
        .await
        .unwrap();
    baseline_inbox(service, 1).await;
    try_host_call(
        service,
        SOURCES_ADVANCE,
        with_items(
            advance_params("inbox:v1", 1, 1, "c2"),
            &[upsert("m-1", "v1", "first")],
        ),
    )
    .await
    .unwrap();
    harness.clock.store(100_000, Ordering::SeqCst);
    service.run_round().await.unwrap();
    let query = BatchQuery {
        member: Some(peer.clone()),
        limit: 10,
        ..BatchQuery::default()
    };
    let batch = service.deliveries(&query).await.unwrap().remove(0);
    assert_eq!(
        batch.state.tag(),
        BatchStateTag::Sealed,
        "owed again: {batch:?}"
    );
    let submits = harness.native.submits();
    assert!(submits >= 1);

    // The peer stops passing while that request may still land.
    inbox_audience(
        service,
        "narrow",
        1,
        serde_json::json!([member_grant("agent:a")]),
    )
    .await;
    harness.native.ledger.lock().unwrap().refuse = None;
    harness
        .clock
        .store(batch.updated_at_ms + 1_000, Ordering::SeqCst);
    service.reconcile_delivery(&batch.id).await.unwrap();
    let inside = service.deliveries(&query).await.unwrap().remove(0);
    assert_eq!(
        inside.state.tag(),
        BatchStateTag::Sealed,
        "not cancelled inside the window"
    );
    assert_eq!(harness.native.submits(), submits, "not submitted again");

    // The earlier request did land, and was admitted.
    let exact = serde_json::to_string(&continuation_for(&batch).unwrap()).unwrap();
    let receipt = NativeReceipt {
        delivery_id: format!("continuation:{}", batch.id),
        address: "rt:member:test:agent:1".into(),
        submission_digest: format!("digest:{}", batch.id),
        committed_at_ms: batch.updated_at_ms,
    };
    harness
        .native
        .ledger
        .lock()
        .unwrap()
        .bindings
        .insert(batch.id.to_string(), (exact, receipt, true));
    let (_, quiet) = retention_and_quiet();
    harness
        .clock
        .store(batch.updated_at_ms + quiet + 1_000, Ordering::SeqCst);
    service.reconcile_delivery(&batch.id).await.unwrap();
    let after = service.deliveries(&query).await.unwrap();
    assert_eq!(after.len(), 1, "no second batch: {after:?}");
    assert_eq!(after[0].state.tag(), BatchStateTag::Delivered);
    assert_eq!(harness.native.submits(), submits);
    let p = serde_json::json!({ "mob_id": "mob-b", "identity": "agent:p" });
    let across = entries_for(service, p, "delivered_across_revocation").await;
    assert_eq!(across.len(), 1, "{across:?}");
}

/// A roster that cannot be read now defers the decision: the member's items
/// are neither held nor sealed until a read answers, and then they go as
/// their coverage says.
#[tokio::test]
async fn a_roster_read_error_defers_instead_of_holding() {
    let native = native_serving(&["agent:b"]);
    native.set_roster_facts("agent:b", "analyst", &[]);
    let harness = harness_with(native);
    let service = &harness.service;
    try_host_call(
        service,
        SOURCES_PUT,
        inbox_with(
            "put",
            serde_json::json!([
                member_grant("agent:a"),
                { "kind": "role", "mob_id": OWN_MOB, "role": "analyst" }
            ]),
            serde_json::json!([{ "kind": "direct" }]),
        ),
    )
    .await
    .unwrap();
    try_subscribe(service, "b-direct", "agent:b", &["mail.received"])
        .await
        .unwrap();
    baseline_inbox(service, 1).await;
    try_host_call(
        service,
        SOURCES_ADVANCE,
        with_items(
            advance_params("inbox:v1", 1, 1, "c2"),
            &[upsert("m-1", "v1", "first")],
        ),
    )
    .await
    .unwrap();
    harness
        .native
        .unreadable_facts
        .lock()
        .unwrap()
        .insert("agent:b".to_string());
    harness.clock.store(100_000, Ordering::SeqCst);
    service.run_round().await.unwrap();
    assert!(batches_of_b(service).await.is_empty(), "deferred");
    assert!(
        entries_for(service, agent_b(), "items_held")
            .await
            .is_empty(),
        "nothing is held on an unreadable roster"
    );
    harness.native.unreadable_facts.lock().unwrap().clear();
    harness.clock.store(200_000, Ordering::SeqCst);
    service.run_round().await.unwrap();
    let batches = batches_of_b(service).await;
    assert_eq!(
        batches.len(),
        1,
        "sealed once the roster reads: {batches:?}"
    );
}

/// One member's held items that cannot be checked (here a hold record that
/// no longer decodes) do not stop the bounded check of another member's:
/// each member is checked on its own, in the round that finds it due.
#[tokio::test]
async fn a_member_whose_hold_cannot_be_read_does_not_stop_another_members_release() {
    let harness = harness_with(native_serving(&["agent:b"]));
    let service = &harness.service;
    let both = serde_json::json!([member_grant("agent:a"), member_grant("agent:b")]);
    try_host_call(
        service,
        SOURCES_PUT,
        inbox_with(
            "put",
            both.clone(),
            serde_json::json!([{ "kind": "direct" }]),
        ),
    )
    .await
    .unwrap();
    for (id, identity) in [("a-direct", "agent:a"), ("b-direct", "agent:b")] {
        try_subscribe(service, id, identity, &["mail.received"])
            .await
            .unwrap();
    }
    baseline_inbox(service, 1).await;
    let advanced = try_host_call(
        service,
        SOURCES_ADVANCE,
        with_items(
            advance_params("inbox:v1", 1, 1, "c2"),
            &[upsert("m-1", "v1", "first")],
        ),
    )
    .await
    .unwrap();
    assert_eq!(advanced["items"][0]["captured"], serde_json::json!(2));
    // Neither member passes now: each holds m-1 at its seal.
    inbox_audience(
        service,
        "narrow",
        1,
        serde_json::json!([member_grant("agent:c")]),
    )
    .await;
    harness.clock.store(100_000, Ordering::SeqCst);
    service.run_round().await.unwrap();
    assert!(batches_of_b(service).await.is_empty(), "b's item is held");
    {
        let db = rusqlite::Connection::open(harness._dir.path().join("mob_announcements.sqlite3"))
            .unwrap();
        let spoiled = db
            .execute(
                "UPDATE announcement_holds SET record = 'unreadable' WHERE member = ?1",
                [member("agent:a").key()],
            )
            .unwrap();
        assert_eq!(spoiled, 1, "a's hold");
    }
    inbox_audience(service, "widen", 2, both).await;
    harness
        .clock
        .store(100_000 + HOLD_RECHECK_BOUND_MS + 1, Ordering::SeqCst);
    service.run_round().await.unwrap();
    let batches = batches_of_b(service).await;
    assert_eq!(
        batches.len(),
        1,
        "b's hold is released in the round a's fails: {batches:?}"
    );
    let released = entries_for(service, agent_b(), "items_released").await;
    assert_eq!(released.len(), 1, "{released:?}");
}

// ---------------------------------------------------------------------------
// Handling: what became of a delivered batch's turn, read from the member's
// admitted input, and what Meta does when it did not complete.
// ---------------------------------------------------------------------------

const DELIVERIES_OBSERVE: &str = "meta/mob_announcements/deliveries/observe";
const DELIVERIES_LIST: &str = "meta/mob_announcements/deliveries/list";
const DELIVERIES_REDELIVER: &str = "meta/mob_announcements/deliveries/redeliver";
const DELIVERIES_DISMISS: &str = "meta/mob_announcements/deliveries/dismiss";

/// The admitted input of a turn, as the native owner reads it: its phase,
/// its terminal outcome and its receipt's completion type.
fn input_reading(
    phase: &str,
    outcome: serde_json::Value,
    completion: serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({ "phase": phase, "terminal_outcome": outcome, "completion": completion })
}

/// A Consumed input, with this receipt.
fn consumed_with(completion: &str) -> serde_json::Value {
    input_reading(
        "consumed",
        serde_json::json!({ "outcome_type": "consumed" }),
        serde_json::json!(completion),
    )
}

/// An input Abandoned for `reason`, with no receipt.
fn abandoned_for(reason: serde_json::Value) -> serde_json::Value {
    input_reading(
        "abandoned",
        serde_json::json!({ "outcome_type": "abandoned", "reason": reason }),
        serde_json::Value::Null,
    )
}

/// "agent:a"'s batches, oldest first.
pub(super) async fn batches_of_a(service: &AnnouncementService) -> Vec<Batch> {
    service
        .deliveries(&BatchQuery {
            member: Some(member("agent:a")),
            limit: 10,
            ..BatchQuery::default()
        })
        .await
        .unwrap()
}

/// "agent:a" subscribes to the live "inbox", captures one item, "m-1", and
/// its batch is delivered: admitted as one input of a's session. The
/// native owner reads every admitted input as `reading`.
pub(super) async fn a_is_delivered_an_inbox_item(
    harness: &Harness,
    reading: serde_json::Value,
) -> Batch {
    let service = &harness.service;
    *harness.native.input_default.lock().unwrap() = Some(reading);
    live_inbox(service).await;
    subscribe_to_inbox(service, "sub-a", "agent:a").await;
    try_host_call(
        service,
        SOURCES_ADVANCE,
        with_items(
            advance_params("inbox:v1", 1, 1, "c2"),
            &[upsert("m-1", "v1", "first mail")],
        ),
    )
    .await
    .unwrap();
    harness.clock.store(100_000, Ordering::SeqCst);
    service.run_round().await.unwrap();
    harness.round_after_pause().await;
    let batches = batches_of_a(service).await;
    assert!(!batches.is_empty(), "a's batch is sealed");
    assert_eq!(
        batches[0].state.tag(),
        BatchStateTag::Delivered,
        "{batches:?}"
    );
    batches[0].clone()
}

/// What `deliveries/observe` says of the batch's handling.
async fn handling_of(service: &AnnouncementService, batch: &Batch) -> serde_json::Value {
    try_host_call(
        service,
        DELIVERIES_OBSERVE,
        serde_json::json!({ "batch_id": batch.id }),
    )
    .await
    .unwrap()["handling"]
        .clone()
}

/// A turn whose receipt says it completed settles its batch handled.
#[tokio::test]
async fn a_completed_turn_settles_its_batch_handled() {
    let harness = harness_with(ScriptedNative::new());
    let service = &harness.service;
    let batch = a_is_delivered_an_inbox_item(&harness, consumed_with("completed")).await;
    let handling = handling_of(service, &batch).await;
    assert_eq!(
        handling["state"],
        serde_json::json!("handled"),
        "{handling}"
    );
    harness.round_after_pause().await;
    assert_eq!(
        batches_of_a(service).await.len(),
        1,
        "nothing is delivered again"
    );
}

/// An agent-level failure after the turn changed the session ends the input
/// Consumed with an abandoned_with_error receipt: the receipt decides, and
/// the batch is unhandled.
#[tokio::test]
async fn an_agent_level_failure_consumes_the_input_and_the_batch_is_unhandled() {
    let harness = harness_with(ScriptedNative::new());
    let service = &harness.service;
    let batch = a_is_delivered_an_inbox_item(&harness, consumed_with("abandoned_with_error")).await;
    let handling = handling_of(service, &batch).await;
    assert_eq!(
        handling["state"],
        serde_json::json!("unhandled"),
        "{handling}"
    );
    assert_eq!(handling["cause"], serde_json::json!("abandoned_with_error"));
}

/// A source item whose turn ended unhandled is redelivered by default: the
/// member's next batch carries it again, first, under a notice that names
/// the earlier batch and warns that the turn may have started things.
#[tokio::test]
async fn an_input_that_ends_unhandled_is_redelivered_with_notice() {
    let harness = harness_with(ScriptedNative::new());
    let service = &harness.service;
    let first = a_is_delivered_an_inbox_item(&harness, consumed_with("cancelled")).await;
    assert_eq!(
        handling_of(service, &first).await["state"],
        serde_json::json!("unhandled")
    );
    harness.round_after_pause().await;
    let batches = batches_of_a(service).await;
    assert_eq!(batches.len(), 2, "{batches:?}");
    let again = &batches[1];
    assert_eq!(again.items.len(), 1);
    assert_eq!(again.items[0].source_key, first.items[0].source_key);
    assert!(
        again.body.contains(&format!(
            "These redeliver announcements of batch {}: it was admitted, and its turn ended",
            first.id
        )),
        "{}",
        again.body
    );
    assert!(
        again.body.contains(
            "Anything that turn started may already have happened: check before you repeat it."
        ),
        "{}",
        again.body
    );
}

/// The redelivery notice names each redelivered announcement by mob,
/// sequence and source key, and a source item by its item id, stream and
/// version, every name quoted.
#[tokio::test]
async fn a_redelivery_notice_names_each_original_key_and_version() {
    let harness = harness_with(ScriptedNative::new());
    let service = &harness.service;
    let first = a_is_delivered_an_inbox_item(&harness, consumed_with("abandoned")).await;
    harness.round_after_pause().await;
    let batches = batches_of_a(service).await;
    assert_eq!(batches.len(), 2, "{batches:?}");
    let body = &batches[1].body;
    let item = &first.items[0];
    for expected in [
        format!("mob {}", quoted(item.mob_id.as_str())),
        format!("sequence {}", item.sequence),
        format!("key {}", quoted(item.source_key.as_str())),
        "item \"m-1\"".to_string(),
        "stream \"\"".to_string(),
        "version \"v1\"".to_string(),
    ] {
        assert!(body.contains(&expected), "{expected:?} in {body}");
    }
}

/// A plain announcement's batch reports unhandled and is not redelivered:
/// the gateway's on_unhandled defaults to "report".
#[tokio::test]
async fn a_plain_announcement_batch_reports_unhandled_but_is_not_redelivered_by_default() {
    let harness = harness_with(ScriptedNative::new());
    let service = &harness.service;
    *harness.native.input_default.lock().unwrap() = Some(consumed_with("cancelled"));
    harness.subscribe("s1").await;
    harness.announce_burst(1).await;
    harness.clock.store(10_000_000, Ordering::SeqCst);
    service.run_round().await.unwrap();
    harness.round_after_pause().await;
    let batch = harness.only_batch().await;
    let handling = handling_of(service, &batch).await;
    assert_eq!(
        handling["state"],
        serde_json::json!("unhandled"),
        "{handling}"
    );
    harness.round_after_pause().await;
    assert_eq!(batches_of_a(service).await.len(), 1, "no redelivery");
}

/// An input abandoned with no receipt is classified by its abandon reason:
/// a revival's cancel is unhandled, named by that reason.
#[tokio::test]
async fn a_revival_abandon_without_a_receipt_is_unhandled_by_reason() {
    let harness = harness_with(ScriptedNative::new());
    let service = &harness.service;
    let batch =
        a_is_delivered_an_inbox_item(&harness, abandoned_for(serde_json::json!("cancelled"))).await;
    let handling = handling_of(service, &batch).await;
    assert_eq!(
        handling["state"],
        serde_json::json!("unhandled"),
        "{handling}"
    );
    assert_eq!(handling["cause"], serde_json::json!("cancelled"));
}

/// A source item admitted to an incarnation that retired before its turn
/// ran reaches the member's current incarnation with the notice.
#[tokio::test]
async fn a_source_item_admitted_to_a_retired_incarnation_reaches_the_next_one_with_notice() {
    let harness = harness_with(ScriptedNative::new());
    let service = &harness.service;
    let first =
        a_is_delivered_an_inbox_item(&harness, abandoned_for(serde_json::json!("retired"))).await;
    assert_eq!(
        handling_of(service, &first).await["cause"],
        serde_json::json!("retired")
    );
    harness.round_after_pause().await;
    let batches = batches_of_a(service).await;
    assert_eq!(batches.len(), 2, "{batches:?}");
    assert!(
        batches[1].body.contains(&format!(
            "These redeliver announcements of batch {}",
            first.id
        )),
        "{}",
        batches[1].body
    );
}

/// A held cause (a reset, an interrupted tool process) keeps the items owed
/// and held instead of redelivering them, and so does a source whose
/// on_unhandled is "hold".
#[tokio::test]
async fn a_held_cause_or_the_hold_policy_keeps_items_owed_and_escalates() {
    let harness = harness_with(ScriptedNative::new());
    let service = &harness.service;
    let first =
        a_is_delivered_an_inbox_item(&harness, abandoned_for(serde_json::json!("reset"))).await;
    let handling = handling_of(service, &first).await;
    assert_eq!(
        handling["state"],
        serde_json::json!("unhandled"),
        "{handling}"
    );
    assert_eq!(handling["held"], serde_json::json!(1), "{handling}");
    harness.round_after_pause().await;
    assert_eq!(
        batches_of_a(service).await.len(),
        1,
        "held, not redelivered"
    );
    // What the member holds for an operator escalates as unhandled, and the
    // escalation resolves once an operator settles it.
    let unhandled = open_escalations_of(service, "unhandled").await;
    assert_eq!(unhandled.len(), 1, "{unhandled:?}");
    assert_eq!(
        unhandled[0]["subject"],
        serde_json::json!({
            "kind": "member",
            "member": { "mob_id": OWN_MOB, "identity": "agent:a" }
        })
    );
    try_host_call(
        service,
        DELIVERIES_DISMISS,
        serde_json::json!({ "operation_id": "dismiss-held", "batch_id": first.id }),
    )
    .await
    .unwrap();
    harness.round_after_pause().await;
    assert!(
        open_escalations_of(service, "unhandled").await.is_empty(),
        "resolved once nothing is held"
    );

    // A source whose on_unhandled is "hold" holds what a cancel ended.
    let harness = harness_with(ScriptedNative::new());
    let service = &harness.service;
    *harness.native.input_default.lock().unwrap() = Some(consumed_with("cancelled"));
    try_host_call(
        service,
        SOURCES_PUT,
        with(
            source_params("put-inbox", "inbox", "subject-1", &["mail.received"]),
            "delivery",
            serde_json::json!({ "on_unhandled": "hold" }),
        ),
    )
    .await
    .unwrap();
    baseline_inbox(service, 1).await;
    subscribe_to_inbox(service, "sub-a", "agent:a").await;
    try_host_call(
        service,
        SOURCES_ADVANCE,
        with_items(
            advance_params("inbox:v1", 1, 1, "c2"),
            &[upsert("m-1", "v1", "first mail")],
        ),
    )
    .await
    .unwrap();
    harness.clock.store(100_000, Ordering::SeqCst);
    service.run_round().await.unwrap();
    harness.round_after_pause().await;
    let first = batches_of_a(service).await.remove(0);
    let handling = handling_of(service, &first).await;
    assert_eq!(
        handling["cause"],
        serde_json::json!("cancelled"),
        "{handling}"
    );
    assert_eq!(
        handling["held"],
        serde_json::json!(1),
        "the policy holds: {handling}"
    );
    harness.round_after_pause().await;
    harness.round_after_pause().await;
    assert_eq!(
        batches_of_a(service).await.len(),
        1,
        "held, not redelivered"
    );
}

/// Automatic redeliveries stop at their bound: an item whose turn ends
/// unhandled a third time is held for an operator.
#[tokio::test]
async fn redeliveries_stop_at_their_bound_and_hold() {
    let harness = harness_with(ScriptedNative::new());
    let service = &harness.service;
    a_is_delivered_an_inbox_item(&harness, consumed_with("cancelled")).await;
    for _ in 0..8 {
        harness.round_after_pause().await;
    }
    let batches = batches_of_a(service).await;
    assert_eq!(
        batches.len(),
        3,
        "the first and two redeliveries: {batches:?}"
    );
    let handling = handling_of(service, &batches[2]).await;
    assert_eq!(
        handling["state"],
        serde_json::json!("unhandled"),
        "{handling}"
    );
    assert_eq!(
        handling["held"],
        serde_json::json!(1),
        "past the bound: {handling}"
    );
}

/// An operator settles what is held: redeliver checks coverage afresh and
/// reopens the items for the member's next batch; dismiss settles them as
/// handled by an operator. Both are counted in the audit.
#[tokio::test]
async fn held_items_are_redelivered_or_dismissed_by_an_operator() {
    let harness = harness_with(ScriptedNative::new());
    let service = &harness.service;
    let first =
        a_is_delivered_an_inbox_item(&harness, abandoned_for(serde_json::json!("reset"))).await;
    try_host_call(
        service,
        DELIVERIES_REDELIVER,
        serde_json::json!({ "operation_id": "redeliver-1", "batch_id": first.id }),
    )
    .await
    .unwrap();
    harness.round_after_pause().await;
    harness.round_after_pause().await;
    let batches = batches_of_a(service).await;
    assert_eq!(batches.len(), 2, "{batches:?}");
    harness.round_after_pause().await;
    try_host_call(
        service,
        DELIVERIES_DISMISS,
        serde_json::json!({ "operation_id": "dismiss-1", "batch_id": batches[1].id }),
    )
    .await
    .unwrap();
    harness.round_after_pause().await;
    assert_eq!(
        batches_of_a(service).await.len(),
        2,
        "dismissed, not redelivered"
    );
    let a = serde_json::json!({ "mob_id": OWN_MOB, "identity": "agent:a" });
    let actions: Vec<serde_json::Value> = audit_rows(service, serde_json::json!({ "member": a }))
        .await
        .into_iter()
        .map(|entry| entry["action"].clone())
        .collect();
    assert!(
        actions.contains(&serde_json::json!("redelivered")),
        "{actions:?}"
    );
    assert!(
        actions.contains(&serde_json::json!("dismissed")),
        "{actions:?}"
    );
}

/// An input Consumed with no receipt is handled_unverified, its own state:
/// a failed run never disappears as "handled".
#[tokio::test]
async fn a_handled_unverified_batch_is_counted_and_escalated() {
    let harness = harness_with(ScriptedNative::new());
    let service = &harness.service;
    let batch = a_is_delivered_an_inbox_item(
        &harness,
        input_reading(
            "consumed",
            serde_json::json!({ "outcome_type": "consumed" }),
            serde_json::Value::Null,
        ),
    )
    .await;
    let handling = handling_of(service, &batch).await;
    assert_eq!(
        handling["state"],
        serde_json::json!("handled_unverified"),
        "{handling}"
    );
    // Counted on the member, and three in a day escalate.
    for (operation, version, item) in [("inbox:v2", 2, "m-2"), ("inbox:v3", 3, "m-3")] {
        assert!(
            open_escalations_of(service, "handled_unverified")
                .await
                .is_empty(),
            "not before the third"
        );
        try_host_call(
            service,
            SOURCES_ADVANCE,
            with_items(
                advance_params(operation, 1, version, &format!("c{}", version + 1)),
                &[upsert(item, "v1", item)],
            ),
        )
        .await
        .unwrap();
        for _ in 0..4 {
            harness.round_after_pause().await;
        }
    }
    let read = member_read(service, "agent:a").await;
    assert_eq!(
        read["handling"]["handled_unverified"],
        serde_json::json!(3),
        "{read}"
    );
    assert_eq!(read["handling"]["handled"], serde_json::json!(0), "{read}");
    let escalated = open_escalations_of(service, "handled_unverified").await;
    assert_eq!(escalated.len(), 1, "{escalated:?}");
}

/// Every announcement batch, a redelivery's too, is a Queue continuation:
/// never a Steer, whose CompletedWithoutResult would mark a failed turn
/// handled.
#[tokio::test]
async fn announcement_batches_are_queue_handled() {
    let harness = harness_with(ScriptedNative::new());
    let service = &harness.service;
    let first = a_is_delivered_an_inbox_item(&harness, consumed_with("cancelled")).await;
    harness.round_after_pause().await;
    for batch in batches_of_a(service).await {
        assert!(
            matches!(
                continuation_for(&batch).unwrap().handling,
                meerkat::ContinuationHandling::Queue
            ),
            "{}",
            batch.id
        );
    }
    assert!(matches!(
        continuation_for(&first).unwrap().handling,
        meerkat::ContinuationHandling::Queue
    ));
}

/// `deliveries/list` carries each batch's handling beside it: null until
/// the batch is admitted, then what became of its turn.
#[tokio::test]
async fn deliveries_list_carries_each_batchs_handling() {
    let harness = harness_with(ScriptedNative::new());
    let service = &harness.service;
    let batch = a_is_delivered_an_inbox_item(&harness, consumed_with("completed")).await;
    let listed = try_host_call(
        service,
        DELIVERIES_LIST,
        serde_json::json!({ "member": { "mob_id": OWN_MOB, "identity": "agent:a" } }),
    )
    .await
    .unwrap();
    let entry = listed["deliveries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["id"] == serde_json::json!(batch.id))
        .cloned()
        .expect("the batch is listed");
    assert_eq!(
        entry["handling"]["state"],
        serde_json::json!("handled"),
        "{entry}"
    );
}

/// An operator's remedy applies only to a batch whose turn ended unhandled
/// with items held: any other batch is refused `not_held`, and nothing
/// changes.
#[tokio::test]
async fn a_batch_holding_nothing_refuses_an_operator_remedy_as_not_held() {
    let harness = harness_with(ScriptedNative::new());
    let service = &harness.service;
    let batch = a_is_delivered_an_inbox_item(&harness, consumed_with("completed")).await;
    for method in [DELIVERIES_REDELIVER, DELIVERIES_DISMISS] {
        let refused = try_host_call(
            service,
            method,
            serde_json::json!({ "operation_id": format!("{method}-1"), "batch_id": batch.id }),
        )
        .await
        .unwrap_err();
        assert_eq!(
            refusal_kind(&refused).as_deref(),
            Some("not_held"),
            "{method}"
        );
    }
    assert_eq!(
        handling_of(service, &batch).await["state"],
        serde_json::json!("handled")
    );
}

/// A superseded input is classified as the input that absorbed it: the
/// absorbing input's receipt decides.
#[tokio::test]
async fn a_superseded_input_is_classified_as_the_input_that_absorbed_it() {
    let harness = harness_with(ScriptedNative::new());
    let service = &harness.service;
    harness
        .native
        .input_ids
        .lock()
        .unwrap()
        .insert("input-2".to_string(), consumed_with("completed"));
    let batch = a_is_delivered_an_inbox_item(
        &harness,
        input_reading(
            "superseded",
            serde_json::json!({ "outcome_type": "superseded", "superseded_by": "input-2" }),
            serde_json::Value::Null,
        ),
    )
    .await;
    assert_eq!(
        handling_of(service, &batch).await["state"],
        serde_json::json!("handled")
    );
}

/// A gateway whose `on_unhandled` is "redeliver" redelivers a plain
/// announcement whose turn ended unhandled, with the notice, as a source
/// item is by default.
#[tokio::test]
async fn a_plain_announcement_follows_the_gateways_on_unhandled_option() {
    let config = super::parse_options(
        &serde_json::json!({
            "enabled": true,
            "batch": { "max_delay_ms": 60_000, "min_wake_interval_ms": 60_000 },
            "on_unhandled": "redeliver",
        }),
        "mob_announcements",
    )
    .unwrap()
    .expect("enabled");
    let native = ScriptedNative::new();
    *native.input_default.lock().unwrap() = Some(consumed_with("cancelled"));
    let harness = harness_with_config(native, config);
    let service = &harness.service;
    harness.subscribe("s1").await;
    harness.announce_burst(1).await;
    harness.clock.store(10_000_000, Ordering::SeqCst);
    service.run_round().await.unwrap();
    harness.round_after_pause().await;
    harness.round_after_pause().await;
    harness.round_after_pause().await;
    let batches = batches_of_a(service).await;
    assert!(batches.len() >= 2, "redelivered: {batches:?}");
    assert!(
        batches[1].body.contains(&format!(
            "These redeliver announcements of batch {}",
            batches[0].id
        )),
        "{}",
        batches[1].body
    );
}

/// A batch that redelivers and is stranded before admission hands its
/// items to a successor that carries the same notice.
#[tokio::test]
async fn a_successor_of_a_redelivery_keeps_its_notice() {
    let mut native = ScriptedNative::new();
    native.apply_on_commit = false;
    *native.input_default.lock().unwrap() = Some(consumed_with("cancelled"));
    let harness = harness_with(native);
    let service = &harness.service;
    live_inbox(service).await;
    subscribe_to_inbox(service, "sub-a", "agent:a").await;
    try_host_call(
        service,
        SOURCES_ADVANCE,
        with_items(
            advance_params("inbox:v1", 1, 1, "c2"),
            &[upsert("m-1", "v1", "first mail")],
        ),
    )
    .await
    .unwrap();
    harness.clock.store(100_000, Ordering::SeqCst);
    service.run_round().await.unwrap();
    let first = batches_of_a(service).await.remove(0);
    harness
        .native
        .ledger
        .lock()
        .unwrap()
        .bindings
        .get_mut(first.id.as_str())
        .unwrap()
        .2 = true;
    // Admitted, read unhandled and owed again; then redelivered.
    harness.round_after_pause().await;
    harness.round_after_pause().await;
    let batches = batches_of_a(service).await;
    assert_eq!(batches.len(), 2, "{batches:?}");
    let redelivery = batches[1].clone();
    harness
        .native
        .ledger
        .lock()
        .unwrap()
        .strand
        .insert(redelivery.id.to_string());
    harness.round_after_pause().await;
    let stranded = harness.store.batch(&redelivery.id).await.unwrap().unwrap();
    let successor = harness
        .store
        .batch(&stranded.successor.expect("a successor"))
        .await
        .unwrap()
        .unwrap();
    assert!(
        successor.body.contains(&format!(
            "These redeliver announcements of batch {}",
            first.id
        )),
        "{}",
        successor.body
    );
}

/// No subscription interest, no redelivery: an unhandled turn whose
/// subscription was cancelled meanwhile owes nothing again.
#[tokio::test]
async fn an_unhandled_turn_of_a_cancelled_subscription_is_not_redelivered() {
    let mut native = ScriptedNative::new();
    native.apply_on_commit = false;
    *native.input_default.lock().unwrap() = Some(consumed_with("cancelled"));
    let harness = harness_with(native);
    let service = &harness.service;
    live_inbox(service).await;
    subscribe_to_inbox(service, "sub-a", "agent:a").await;
    try_host_call(
        service,
        SOURCES_ADVANCE,
        with_items(
            advance_params("inbox:v1", 1, 1, "c2"),
            &[upsert("m-1", "v1", "first mail")],
        ),
    )
    .await
    .unwrap();
    harness.clock.store(100_000, Ordering::SeqCst);
    service.run_round().await.unwrap();
    let first = batches_of_a(service).await.remove(0);
    service
        .change_subscription(
            &op("cancel-a"),
            &sid("sub-a"),
            1,
            SubscriptionChange::Cancel,
        )
        .await
        .unwrap();
    harness
        .native
        .ledger
        .lock()
        .unwrap()
        .bindings
        .get_mut(first.id.as_str())
        .unwrap()
        .2 = true;
    for _ in 0..3 {
        harness.round_after_pause().await;
    }
    let handling = handling_of(service, &first).await;
    assert_eq!(
        handling["state"],
        serde_json::json!("unhandled"),
        "{handling}"
    );
    assert_eq!(handling["redelivered"], serde_json::json!(0), "{handling}");
    assert_eq!(batches_of_a(service).await.len(), 1, "nothing owed again");
}

/// A harness whose service runs with `config`.
pub(super) fn harness_with_config(native: ScriptedNative, config: AnnouncementConfig) -> Harness {
    let (dir, store) = temp_store();
    let store = Arc::new(store);
    let native = Arc::new(native);
    let clock = Arc::new(AtomicU64::new(1_000));
    let reader = Arc::clone(&clock);
    let service = AnnouncementService::with_clock(
        Arc::clone(&store) as Arc<dyn AnnouncementStore>,
        Arc::clone(&native) as Arc<dyn NativeDelivery>,
        config,
        Arc::new(move || reader.load(Ordering::SeqCst)),
    )
    .unwrap();
    Harness {
        _dir: dir,
        store,
        native,
        clock,
        service,
    }
}

/// A retried operator redelivery answers what the first one recorded,
/// even while the member's roster cannot be read: only a new redelivery
/// needs the roster.
#[tokio::test]
async fn a_retried_redelivery_answers_its_receipt_while_the_roster_is_unreadable() {
    let harness = harness_with(ScriptedNative::new());
    let service = &harness.service;
    let first =
        a_is_delivered_an_inbox_item(&harness, abandoned_for(serde_json::json!("reset"))).await;
    let params = serde_json::json!({ "operation_id": "redeliver-1", "batch_id": first.id });
    let answered = try_host_call(service, DELIVERIES_REDELIVER, params.clone())
        .await
        .unwrap();
    harness
        .native
        .unreadable_facts
        .lock()
        .unwrap()
        .insert("agent:a".to_string());
    let replayed = try_host_call(service, DELIVERIES_REDELIVER, params)
        .await
        .unwrap();
    assert_eq!(replayed, answered, "the receipt answers");
}

// ---------------------------------------------------------------------------
// Seal fairness: one item one delivery, priority and the per-source cap,
// and single-flight for a member that awaits handling.
// ---------------------------------------------------------------------------

/// A policy that seals one item per batch, every minute.
fn one_per_batch() -> SealingPolicy {
    SealingPolicy {
        max_count: 1,
        max_bytes: 1_048_576,
        max_delay_ms: 60_000,
        min_wake_interval_ms: 60_000,
    }
}

/// One item owed through two subscriptions of one member enters one batch
/// with both of its rows, whatever the seal's limit: the other row is never
/// left for a second delivery.
#[tokio::test]
async fn overlapping_subscriptions_of_one_member_deliver_an_item_once() {
    let config = AnnouncementConfig {
        policy: one_per_batch(),
        ..AnnouncementConfig::default()
    };
    let harness = harness_with_config(ScriptedNative::new(), config);
    let service = &harness.service;
    harness.subscribe("s1").await;
    service
        .put_subscription(
            &op("s2"),
            put("s2", "agent:a", &["ios"], StartFrontier::Now),
        )
        .await
        .unwrap();
    harness.announce_burst(1).await;
    harness.clock.store(10_000_000, Ordering::SeqCst);
    service.run_round().await.unwrap();
    for _ in 0..4 {
        harness.round_after_pause().await;
    }
    let batches = batches_of_a(service).await;
    assert_eq!(batches.len(), 1, "one delivery: {batches:?}");
    assert_eq!(
        batches[0]
            .items
            .iter()
            .map(|item| item.subscription_id.as_str())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["s1", "s2"]),
        "both rows of the one item"
    );
}

/// An item owed through two subscriptions of one member, held on one of
/// them, is delivered once: the batch that takes it through the other
/// settles the held row with it, so a later release delivers nothing again.
#[tokio::test]
async fn an_item_owed_through_two_subscriptions_is_delivered_once_when_one_is_held() {
    let harness = harness_with(native_serving(&["agent:b"]));
    let service = &harness.service;
    let audience = serde_json::json!([member_grant("agent:a"), member_grant("agent:b")]);
    let both_topics = |operation: &str, routes: serde_json::Value| {
        with(
            with(
                source_params(
                    operation,
                    "inbox",
                    "subject-1",
                    &["mail.received", "mail.flagged"],
                ),
                "audience",
                audience.clone(),
            ),
            "routes",
            routes,
        )
    };
    try_host_call(
        service,
        SOURCES_PUT,
        both_topics("put", serde_json::json!([{ "kind": "direct" }])),
    )
    .await
    .unwrap();
    try_subscribe(service, "b-received", "agent:b", &["mail.received"])
        .await
        .unwrap();
    try_subscribe(service, "b-flagged", "agent:b", &["mail.flagged"])
        .await
        .unwrap();
    baseline_inbox(service, 1).await;
    let advanced = try_host_call(
        service,
        SOURCES_ADVANCE,
        with_items(
            advance_params("inbox:v1", 1, 1, "c2"),
            &[serde_json::json!({
                "item_id": "m-1",
                "version": "v1",
                "change": "upsert",
                "topics": ["mail.received", "mail.flagged"],
                "body": "first",
            })],
        ),
    )
    .await
    .unwrap();
    assert_eq!(advanced["items"][0]["captured"], serde_json::json!(2));
    // The flagged topic loses its direct route: that row is refused at the
    // seal, and enters the batch with its passing sibling.
    try_host_call(
        service,
        SOURCES_PUT,
        with(
            both_topics(
                "received-only",
                serde_json::json!([{ "kind": "direct", "topics": ["mail.received"] }]),
            ),
            "expected_revision",
            serde_json::json!(1),
        ),
    )
    .await
    .unwrap();
    harness.clock.store(100_000, Ordering::SeqCst);
    service.run_round().await.unwrap();
    harness.round_after_pause().await;
    assert_eq!(batches_of_b(service).await.len(), 1, "delivered once");
    // The route comes back, and the bound passes.
    try_host_call(
        service,
        SOURCES_PUT,
        with(
            both_topics("both-again", serde_json::json!([{ "kind": "direct" }])),
            "expected_revision",
            serde_json::json!(2),
        ),
    )
    .await
    .unwrap();
    harness
        .clock
        .store(200_000 + HOLD_RECHECK_BOUND_MS, Ordering::SeqCst);
    service.run_round().await.unwrap();
    harness.round_after_pause().await;
    let batches = batches_of_b(service).await;
    assert_eq!(batches.len(), 1, "never a second delivery: {batches:?}");
}

/// A burst of one source does not hold another source's urgent item: the
/// urgent item goes first in the member's next batch, and the burst fills at
/// most its source's cap of that batch.
#[tokio::test]
async fn a_burst_on_one_source_does_not_hold_an_urgent_item_of_another_past_one_batch() {
    let harness = harness_with(ScriptedNative::new());
    let service = &harness.service;
    live_inbox(service).await;
    subscribe_to_inbox(service, "sub-a", "agent:a").await;
    try_host_call(
        service,
        SOURCES_PUT,
        with(
            source_params("put-alerts", "alerts", "subject-2", &["alerts.raised"]),
            "delivery",
            serde_json::json!({ "priority": "urgent" }),
        ),
    )
    .await
    .unwrap();
    let alerts = |params: serde_json::Value| {
        with(
            with(params, "source_id", serde_json::json!("alerts")),
            "principal",
            principal_of("subject-2"),
        )
    };
    try_host_call(
        service,
        SOURCES_ADVANCE,
        alerts(with(
            advance_params("alerts-baseline", 1, 0, "c1"),
            "coverage",
            serde_json::json!("complete"),
        )),
    )
    .await
    .unwrap();
    try_host_call(
        service,
        SUBSCRIPTIONS_PUT,
        serde_json::json!({
            "operation_id": "subscribe-alerts",
            "subscription_id": "alerts-a",
            "member": { "mob_id": OWN_MOB, "identity": "agent:a" },
            "topics": ["alerts.raised"],
        }),
    )
    .await
    .unwrap();
    // A burst of 30 mails, then one urgent alert.
    let burst: Vec<serde_json::Value> = (0..30)
        .map(|index| upsert(&format!("mail-{index}"), "v1", &format!("mail {index}")))
        .collect();
    try_host_call(
        service,
        SOURCES_ADVANCE,
        with_items(advance_params("inbox:burst", 1, 1, "c2"), &burst),
    )
    .await
    .unwrap();
    harness.clock.fetch_add(1, Ordering::SeqCst);
    try_host_call(
        service,
        SOURCES_ADVANCE,
        alerts(with_items(
            advance_params("alerts:v1", 1, 1, "c2"),
            &[serde_json::json!({
                "item_id": "alert-1",
                "version": "v1",
                "change": "upsert",
                "topics": ["alerts.raised"],
                "body": "URGENT-ALERT disk full",
            })],
        )),
    )
    .await
    .unwrap();
    harness.clock.store(10_000_000, Ordering::SeqCst);
    service.run_round().await.unwrap();
    let first = batches_of_a(service).await.remove(0);
    let body = &first.body;
    let alert_at = body
        .find("URGENT-ALERT")
        .expect("the urgent item is in the first batch");
    let first_mail_at = body.find("> mail ").unwrap_or(usize::MAX);
    assert!(
        alert_at < first_mail_at,
        "the urgent item goes first: {body}"
    );
    let mails = first
        .items
        .iter()
        .filter(|item| item.subscription_id.as_str() == "sub-a")
        .count();
    assert_eq!(mails, 20, "the burst fills its source's cap and no more");
}

/// The triage member of a triage route awaits handling by default: while
/// its previous batch's turn has not ended, it gets no new batch, until the
/// turn ends or 15 minutes pass. A member that does not await handling gets
/// its next batch meanwhile.
#[tokio::test]
async fn a_triage_member_is_single_flight_by_default() {
    let harness = harness_with(native_serving(&["agent:t"]));
    let service = &harness.service;
    // Every turn is still running.
    *harness.native.input_default.lock().unwrap() = Some(serde_json::json!({ "phase": "applied" }));
    try_host_call(
        service,
        SOURCES_PUT,
        inbox_with(
            "put",
            serde_json::json!([member_grant("agent:a"), member_grant("agent:t")]),
            serde_json::json!([triage_by(
                "agent:t",
                serde_json::json!([member_grant("agent:a")])
            )]),
        ),
    )
    .await
    .unwrap();
    try_subscribe(service, "t-triage", "agent:t", &["mail.received"])
        .await
        .unwrap();
    baseline_inbox(service, 1).await;
    harness.subscribe("s1").await;
    let advance = |operation: &str, version: u64, item: &str| {
        with_items(
            advance_params(operation, 1, version, &format!("c{}", version + 1)),
            &[upsert(item, "v1", item)],
        )
    };
    try_host_call(service, SOURCES_ADVANCE, advance("inbox:v1", 1, "m-1"))
        .await
        .unwrap();
    harness.announce_burst(1).await;
    harness.clock.store(100_000, Ordering::SeqCst);
    service.run_round().await.unwrap();
    harness.round_after_pause().await;
    let batches_of = |identity: &'static str| async move {
        service
            .deliveries(&BatchQuery {
                member: Some(member(identity)),
                limit: 10,
                ..BatchQuery::default()
            })
            .await
            .unwrap()
    };
    assert_eq!(batches_of("agent:t").await.len(), 1);
    assert_eq!(batches_of("agent:a").await.len(), 1);
    // More comes for both while the first turns are still running.
    try_host_call(service, SOURCES_ADVANCE, advance("inbox:v2", 2, "m-2"))
        .await
        .unwrap();
    service
        .announce(announce_request(
            "payments",
            "failure-later",
            &["ios"],
            "later",
        ))
        .await
        .unwrap();
    for _ in 0..4 {
        harness.round_after_pause().await;
    }
    assert_eq!(
        batches_of("agent:t").await.len(),
        1,
        "the triage member awaits its turn"
    );
    assert_eq!(
        batches_of("agent:a").await.len(),
        2,
        "a member that does not await handling gets its next batch"
    );
    // Past 15 minutes the wait ends though the turn has not.
    harness.clock.fetch_add(15 * 60 * 1_000, Ordering::SeqCst);
    service.run_round().await.unwrap();
    assert_eq!(batches_of("agent:t").await.len(), 2, "the wait is bounded");
}

/// A subscription's explicit `delivery.await_handling: false` turns the
/// triage member's default off, and `true` makes any member wait.
#[tokio::test]
async fn await_handling_is_set_per_subscription() {
    let harness = harness_with(native_serving(&["agent:t"]));
    let service = &harness.service;
    *harness.native.input_default.lock().unwrap() = Some(serde_json::json!({ "phase": "applied" }));
    try_host_call(
        service,
        SOURCES_PUT,
        inbox_with(
            "put",
            serde_json::json!([member_grant("agent:a"), member_grant("agent:t")]),
            serde_json::json!([
                { "kind": "direct" },
                triage_by("agent:t", serde_json::json!([member_grant("agent:a")]))
            ]),
        ),
    )
    .await
    .unwrap();
    let subscribed = try_host_call(
        service,
        SUBSCRIPTIONS_PUT,
        serde_json::json!({
            "operation_id": "subscribe-t",
            "subscription_id": "t-triage",
            "member": { "mob_id": OWN_MOB, "identity": "agent:t" },
            "topics": ["mail.received"],
            "delivery": { "await_handling": false },
        }),
    )
    .await
    .unwrap();
    assert_eq!(
        subscribed["subscription"]["delivery"],
        serde_json::json!({ "await_handling": false })
    );
    try_host_call(
        service,
        SUBSCRIPTIONS_PUT,
        serde_json::json!({
            "operation_id": "subscribe-a",
            "subscription_id": "a-direct",
            "member": { "mob_id": OWN_MOB, "identity": "agent:a" },
            "topics": ["mail.received"],
            "delivery": { "await_handling": true },
        }),
    )
    .await
    .unwrap();
    baseline_inbox(service, 1).await;
    for (operation, version, item) in [("inbox:v1", 1, "m-1"), ("inbox:v2", 2, "m-2")] {
        try_host_call(
            service,
            SOURCES_ADVANCE,
            with_items(
                advance_params(operation, 1, version, &format!("c{}", version + 1)),
                &[upsert(item, "v1", item)],
            ),
        )
        .await
        .unwrap();
        if version == 1 {
            harness.clock.store(100_000, Ordering::SeqCst);
            service.run_round().await.unwrap();
            harness.round_after_pause().await;
        }
    }
    for _ in 0..4 {
        harness.round_after_pause().await;
    }
    let count = |identity: &'static str| async move {
        service
            .deliveries(&BatchQuery {
                member: Some(member(identity)),
                limit: 10,
                ..BatchQuery::default()
            })
            .await
            .unwrap()
            .len()
    };
    assert_eq!(count("agent:t").await, 2, "false turns the default off");
    assert_eq!(count("agent:a").await, 1, "true makes a member wait");
}

/// The open escalations of `cause`, as `escalations/list` reads them.
async fn open_escalations_of(service: &AnnouncementService, cause: &str) -> Vec<serde_json::Value> {
    try_host_call(
        service,
        "meta/mob_announcements/escalations/list",
        serde_json::json!({ "open": true }),
    )
    .await
    .unwrap()["escalations"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|escalation| escalation["cause"] == serde_json::json!(cause))
        .collect()
}

/// What `members/get` reads about this mob's member `identity`.
async fn member_read(service: &AnnouncementService, identity: &str) -> serde_json::Value {
    try_host_call(
        service,
        "meta/mob_announcements/members/get",
        serde_json::json!({ "member": { "mob_id": OWN_MOB, "identity": identity } }),
    )
    .await
    .unwrap()
}

/// members/get counts a member's batches by handling, with what it holds
/// for an operator and when its oldest unhandled turn was read, and
/// meta/status sums the handling counts over members.
#[tokio::test]
async fn members_get_and_status_count_handling() {
    let harness = harness_with(ScriptedNative::new());
    let service = &harness.service;
    let first =
        a_is_delivered_an_inbox_item(&harness, abandoned_for(serde_json::json!("reset"))).await;
    let read = member_read(service, "agent:a").await;
    let handling = &read["handling"];
    assert_eq!(handling["pending"], serde_json::json!(0), "{read}");
    assert_eq!(handling["handled"], serde_json::json!(0), "{read}");
    assert_eq!(
        handling["handled_unverified"],
        serde_json::json!(0),
        "{read}"
    );
    assert_eq!(handling["unhandled"], serde_json::json!(1), "{read}");
    assert_eq!(handling["held"], serde_json::json!(1), "{read}");
    assert_eq!(
        handling["oldest_unhandled_at_ms"],
        handling_of(service, &first).await["read_at_ms"],
        "{read}"
    );
    assert_eq!(
        read["backlog"]["held_unhandled"],
        serde_json::json!(1),
        "{read}"
    );
    let status = super::rpc::status_projection(service).await;
    assert_eq!(
        status["handling"],
        serde_json::json!({ "pending": 0, "handled": 0, "handled_unverified": 0, "unhandled": 1 }),
        "{status}"
    );
    assert_eq!(
        status["members"]["unhandled_held"],
        serde_json::json!(1),
        "{status}"
    );
}

/// Members that await handling take no place in a round's backlog page:
/// with as many of them waiting as the page holds, another member's backlog
/// is still sealed.
#[tokio::test]
async fn waiting_members_take_no_place_in_a_rounds_backlog_page() {
    // As many waiting members as a round's backlog page holds.
    let waiting: Vec<String> = (0..64).map(|index| format!("agent:w{index:02}")).collect();
    let mut identities: Vec<&str> = waiting.iter().map(String::as_str).collect();
    identities.push("agent:x");
    let harness = harness_with(native_serving(&identities));
    let service = &harness.service;
    // Every admitted turn is still running.
    *harness.native.input_default.lock().unwrap() = Some(serde_json::json!({ "phase": "applied" }));
    let awaiting = SubscriptionDelivery {
        await_handling: Some(true),
    };
    for (index, identity) in waiting.iter().enumerate() {
        let id = format!("w{index:02}");
        service
            .put_subscription(
                &op(&id),
                PutSubscription {
                    delivery: awaiting,
                    ..put(&id, identity, &["ios"], StartFrontier::Now)
                },
            )
            .await
            .unwrap();
    }
    harness.announce_burst(1).await;
    harness.clock.store(100_000, Ordering::SeqCst);
    service.run_round().await.unwrap();
    harness.round_after_pause().await;
    // More comes for the waiting members, and then the first item for x.
    service
        .announce(announce_request(
            "payments",
            "for-the-waiting",
            &["ios"],
            "later",
        ))
        .await
        .unwrap();
    service
        .put_subscription(&op("x"), put("x", "agent:x", &["ios"], StartFrontier::Now))
        .await
        .unwrap();
    harness.clock.fetch_add(1, Ordering::SeqCst);
    service
        .announce(announce_request("payments", "for-x", &["ios"], "for x"))
        .await
        .unwrap();
    for _ in 0..4 {
        harness.round_after_pause().await;
    }
    let sealed_for_x = service
        .deliveries(&BatchQuery {
            member: Some(member("agent:x")),
            limit: 10,
            ..BatchQuery::default()
        })
        .await
        .unwrap();
    assert_eq!(
        sealed_for_x.len(),
        1,
        "x is sealed though a page of members waits: {sealed_for_x:?}"
    );
}

/// A held row lifted because its item reaches the member through another
/// row of it is counted released, as every release is.
#[tokio::test]
async fn a_sibling_rows_hold_lifted_by_a_delivery_is_counted_released() {
    let harness = harness_with(native_serving(&["agent:b"]));
    let service = &harness.service;
    let with_b = serde_json::json!([member_grant("agent:a"), member_grant("agent:b")]);
    let without_b = serde_json::json!([member_grant("agent:a")]);
    let inbox = |operation: &str, audience: &serde_json::Value, routes: serde_json::Value| {
        with(
            with(
                source_params(
                    operation,
                    "inbox",
                    "subject-1",
                    &["mail.received", "mail.flagged"],
                ),
                "audience",
                audience.clone(),
            ),
            "routes",
            routes,
        )
    };
    try_host_call(
        service,
        SOURCES_PUT,
        inbox("put", &with_b, serde_json::json!([{ "kind": "direct" }])),
    )
    .await
    .unwrap();
    try_subscribe(service, "b-received", "agent:b", &["mail.received"])
        .await
        .unwrap();
    try_subscribe(service, "b-flagged", "agent:b", &["mail.flagged"])
        .await
        .unwrap();
    baseline_inbox(service, 1).await;
    let advanced = try_host_call(
        service,
        SOURCES_ADVANCE,
        with_items(
            advance_params("inbox:v1", 1, 1, "c2"),
            &[serde_json::json!({
                "item_id": "m-1",
                "version": "v1",
                "change": "upsert",
                "topics": ["mail.received", "mail.flagged"],
                "body": "first",
            })],
        ),
    )
    .await
    .unwrap();
    assert_eq!(advanced["items"][0]["captured"], serde_json::json!(2));
    // b loses coverage: both rows are held at its next seal.
    try_host_call(
        service,
        SOURCES_PUT,
        with(
            inbox(
                "narrow",
                &without_b,
                serde_json::json!([{ "kind": "direct" }]),
            ),
            "expected_revision",
            serde_json::json!(1),
        ),
    )
    .await
    .unwrap();
    harness.clock.store(100_000, Ordering::SeqCst);
    service.run_round().await.unwrap();
    assert!(batches_of_b(service).await.is_empty(), "both rows held");
    // Coverage comes back with a direct route for mail.received only: the
    // bounded check releases that row and leaves the flagged one held, and
    // the item then reaches b once, through the released row, with the held
    // sibling.
    try_host_call(
        service,
        SOURCES_PUT,
        with(
            inbox(
                "received-only",
                &with_b,
                serde_json::json!([{ "kind": "direct", "topics": ["mail.received"] }]),
            ),
            "expected_revision",
            serde_json::json!(2),
        ),
    )
    .await
    .unwrap();
    harness
        .clock
        .store(100_000 + HOLD_RECHECK_BOUND_MS + 1, Ordering::SeqCst);
    service.run_round().await.unwrap();
    harness.round_after_pause().await;
    let batches = batches_of_b(service).await;
    assert_eq!(batches.len(), 1, "{batches:?}");
    assert_eq!(
        batches[0]
            .items
            .iter()
            .map(|item| item.subscription_id.as_str())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["b-flagged", "b-received"])
    );
    let released: u64 = entries_for(service, agent_b(), "items_released")
        .await
        .iter()
        .map(|entry| entry["count"].as_u64().unwrap())
        .sum();
    assert_eq!(released, 2, "both holds are counted released");
    // A later route change delivers nothing again.
    try_host_call(
        service,
        SOURCES_PUT,
        with(
            inbox(
                "both-again",
                &with_b,
                serde_json::json!([{ "kind": "direct" }]),
            ),
            "expected_revision",
            serde_json::json!(3),
        ),
    )
    .await
    .unwrap();
    harness
        .clock
        .fetch_add(HOLD_RECHECK_BOUND_MS + 1, Ordering::SeqCst);
    service.run_round().await.unwrap();
    harness.round_after_pause().await;
    assert_eq!(
        batches_of_b(service).await.len(),
        1,
        "never a second delivery"
    );
}

/// The per-member reads of the handling records are served by an index,
/// not a scan of every admitted batch's handling.
#[test]
fn the_per_member_handling_reads_are_served_by_an_index() {
    let connection = rusqlite::Connection::open_in_memory().unwrap();
    connection.execute_batch(super::sqlite::SCHEMA).unwrap();
    connection.execute_batch(super::sqlite::SCHEMA_V2).unwrap();
    connection.execute_batch(super::sqlite::SCHEMA_V3).unwrap();
    for query in [
        "SELECT MAX(b.updated_at_ms) FROM announcement_handling h
         JOIN announcement_batches b ON b.id = h.batch_id
         WHERE h.member = 'm' AND h.state = 'pending'",
        "SELECT COUNT(*), MIN(updated_at_ms) FROM announcement_handling
         WHERE member = 'm' AND state = 'handled_unverified' AND updated_at_ms > 0",
        "SELECT state, COUNT(*), MIN(updated_at_ms) FROM announcement_handling
         WHERE member = 'm' GROUP BY state",
    ] {
        let mut statement = connection
            .prepare(&format!("EXPLAIN QUERY PLAN {query}"))
            .unwrap();
        let plan: Vec<String> = statement
            .query_map([], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap();
        assert!(
            plan.iter()
                .any(|step| step.contains("announcement_handling_by_member")),
            "{query}: {plan:?}"
        );
    }
}

/// A paused subscription's row for an item another subscription of the
/// member delivers enters the same batch, so resuming it later delivers
/// nothing again.
#[tokio::test]
async fn a_paused_subscriptions_row_enters_with_its_item() {
    let harness = harness_with(ScriptedNative::new());
    let service = &harness.service;
    harness.subscribe("s1").await;
    let paused = service
        .put_subscription(
            &op("s2"),
            put("s2", "agent:a", &["ios"], StartFrontier::Now),
        )
        .await
        .unwrap();
    let paused = service
        .change_subscription(
            &op("pause-s2"),
            &sid("s2"),
            paused.revision,
            SubscriptionChange::Pause,
        )
        .await
        .unwrap();
    harness.announce_burst(1).await;
    harness.clock.store(10_000_000, Ordering::SeqCst);
    service.run_round().await.unwrap();
    harness.round_after_pause().await;
    let batches = batches_of_a(service).await;
    assert_eq!(batches.len(), 1, "{batches:?}");
    assert_eq!(
        batches[0]
            .items
            .iter()
            .map(|item| item.subscription_id.as_str())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["s1", "s2"]),
        "the paused row enters with its item"
    );
    service
        .change_subscription(
            &op("resume-s2"),
            &sid("s2"),
            paused.subscription.revision,
            SubscriptionChange::Resume,
        )
        .await
        .unwrap();
    for _ in 0..4 {
        harness.round_after_pause().await;
    }
    assert_eq!(
        batches_of_a(service).await.len(),
        1,
        "resuming delivers nothing again"
    );
}

/// At the last check before a submission, a row of a subscription that is no
/// longer live vouches for no item a live row brings too: an item whose live
/// rows fail cancels the batch, though a cancelled subscription's row of it
/// would pass.
#[tokio::test]
async fn a_cancelled_subscriptions_row_does_not_vouch_for_its_item_at_the_last_check() {
    let harness = harness_with(native_serving(&["agent:b"]));
    let service = &harness.service;
    let audience = serde_json::json!([member_grant("agent:a"), member_grant("agent:b")]);
    let inbox = |operation: &str, routes: serde_json::Value| {
        with(
            with(
                source_params(
                    operation,
                    "inbox",
                    "subject-1",
                    &["mail.received", "mail.flagged"],
                ),
                "audience",
                audience.clone(),
            ),
            "routes",
            routes,
        )
    };
    try_host_call(
        service,
        SOURCES_PUT,
        inbox("put", serde_json::json!([{ "kind": "direct" }])),
    )
    .await
    .unwrap();
    try_subscribe(service, "b-received", "agent:b", &["mail.received"])
        .await
        .unwrap();
    try_subscribe(service, "b-flagged", "agent:b", &["mail.flagged"])
        .await
        .unwrap();
    baseline_inbox(service, 1).await;
    try_host_call(
        service,
        SOURCES_ADVANCE,
        with_items(
            advance_params("inbox:v1", 1, 1, "c2"),
            &[serde_json::json!({
                "item_id": "m-1",
                "version": "v1",
                "change": "upsert",
                "topics": ["mail.received", "mail.flagged"],
                "body": "first",
            })],
        ),
    )
    .await
    .unwrap();
    // Sealed through both rows, and not submitted yet.
    let batch = harness
        .store
        .seal_batch(
            &member("agent:b"),
            &batch_id("b-both"),
            &policy(),
            10_000_000,
        )
        .await
        .unwrap()
        .expect("sealed");
    assert_eq!(batch.items.len(), 2, "{batch:?}");
    // The flagged subscription is cancelled; the batch stays sealed for the
    // received one. Then mail.received loses its direct route.
    service
        .change_subscription(
            &op("cancel-flagged"),
            &sid("b-flagged"),
            1,
            SubscriptionChange::Cancel,
        )
        .await
        .unwrap();
    try_host_call(
        service,
        SOURCES_PUT,
        with(
            inbox(
                "flagged-only",
                serde_json::json!([{ "kind": "direct", "topics": ["mail.flagged"] }]),
            ),
            "expected_revision",
            serde_json::json!(1),
        ),
    )
    .await
    .unwrap();
    let check = harness
        .store
        .check_before_submission(&batch.id, BatchStateTag::Sealed, None, true, 10_000_001)
        .await
        .unwrap();
    assert!(
        matches!(check, super::store::SubmissionCheck::Cancelled(_)),
        "the cancelled row does not vouch: {check:?}"
    );
}

/// An item that only a cancelled subscription's row brings is still checked
/// at the last check before a submission: with no live row to judge it by,
/// its own rows judge it, and one that fails cancels the batch.
#[tokio::test]
async fn an_item_only_a_cancelled_subscription_brings_is_still_checked_at_the_last_check() {
    let harness = harness_with(native_serving(&["agent:b"]));
    let service = &harness.service;
    let audience = serde_json::json!([member_grant("agent:a"), member_grant("agent:b")]);
    let inbox = |operation: &str, routes: serde_json::Value| {
        with(
            with(
                source_params(
                    operation,
                    "inbox",
                    "subject-1",
                    &["mail.received", "mail.flagged"],
                ),
                "audience",
                audience.clone(),
            ),
            "routes",
            routes,
        )
    };
    try_host_call(
        service,
        SOURCES_PUT,
        inbox("put", serde_json::json!([{ "kind": "direct" }])),
    )
    .await
    .unwrap();
    try_subscribe(service, "b-received", "agent:b", &["mail.received"])
        .await
        .unwrap();
    try_subscribe(service, "b-flagged", "agent:b", &["mail.flagged"])
        .await
        .unwrap();
    baseline_inbox(service, 1).await;
    try_host_call(
        service,
        SOURCES_ADVANCE,
        with_items(
            advance_params("inbox:v1", 1, 1, "c2"),
            &[
                serde_json::json!({
                    "item_id": "m-1",
                    "version": "v1",
                    "change": "upsert",
                    "topics": ["mail.received"],
                    "body": "first",
                }),
                serde_json::json!({
                    "item_id": "m-2",
                    "version": "v1",
                    "change": "upsert",
                    "topics": ["mail.flagged"],
                    "body": "second",
                }),
            ],
        ),
    )
    .await
    .unwrap();
    // Sealed, and not submitted yet: m-1 through b-received, m-2 through
    // b-flagged alone.
    let batch = harness
        .store
        .seal_batch(
            &member("agent:b"),
            &batch_id("b-both"),
            &policy(),
            10_000_000,
        )
        .await
        .unwrap()
        .expect("sealed");
    assert_eq!(batch.items.len(), 2, "{batch:?}");
    // The flagged subscription is cancelled; the batch stays sealed for the
    // received one. Then mail.flagged loses its direct route.
    service
        .change_subscription(
            &op("cancel-flagged"),
            &sid("b-flagged"),
            1,
            SubscriptionChange::Cancel,
        )
        .await
        .unwrap();
    try_host_call(
        service,
        SOURCES_PUT,
        with(
            inbox(
                "received-only",
                serde_json::json!([{ "kind": "direct", "topics": ["mail.received"] }]),
            ),
            "expected_revision",
            serde_json::json!(1),
        ),
    )
    .await
    .unwrap();
    let check = harness
        .store
        .check_before_submission(&batch.id, BatchStateTag::Sealed, None, true, 10_000_001)
        .await
        .unwrap();
    assert!(
        matches!(check, super::store::SubmissionCheck::Cancelled(_)),
        "m-2 is checked through its cancelled row: {check:?}"
    );
}

// ---------------------------------------------------------------------------
// A candidate launch's owner: it delivers nothing and changes nothing.
// ---------------------------------------------------------------------------

/// Every row of every table of the announcement database at `path`, read
/// read-only and rendered per table: what any change would have to write.
fn store_rows(path: &std::path::Path) -> BTreeMap<String, Vec<String>> {
    let connection =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let tables: Vec<String> = connection
        .prepare(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' \
             ORDER BY name",
        )
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<std::result::Result<_, _>>()
        .unwrap();
    tables
        .into_iter()
        .map(|table| {
            let mut statement = connection
                .prepare(&format!("SELECT * FROM \"{table}\""))
                .unwrap();
            let columns = statement.column_count();
            let mut rows: Vec<String> = statement
                .query_map([], |row| {
                    (0..columns)
                        .map(|index| {
                            row.get_ref(index)
                                .map(|value| format!("{:?}", rusqlite::types::Value::from(value)))
                        })
                        .collect::<rusqlite::Result<Vec<_>>>()
                        .map(|values| values.join("|"))
                })
                .unwrap()
                .collect::<std::result::Result<_, _>>()
                .unwrap();
            rows.sort();
            (table, rows)
        })
        .collect()
}

/// The announcement database `harness` runs over.
fn harness_db(harness: &Harness) -> std::path::PathBuf {
    harness._dir.path().join("mob_announcements.sqlite3")
}

/// A candidate launch's owner over `harness`'s store, native owner and
/// clock: what a candidate booted against the same state composes.
fn candidate_owner(harness: &Harness) -> AnnouncementService {
    let clock = Arc::clone(&harness.clock);
    let owner = AnnouncementService::with_clock(
        Arc::clone(&harness.store) as Arc<dyn AnnouncementStore>,
        Arc::clone(&harness.native) as Arc<dyn NativeDelivery>,
        harness.service.config().clone(),
        Arc::new(move || clock.load(Ordering::SeqCst)),
    )
    .unwrap();
    owner.quiesce_for_candidate();
    owner
}

/// A candidate launch's owner refuses every change typed, `candidate_launch`
/// through the handler every surface dispatches to, a member tool's refusal
/// and the owner's own verbs alike, before anything is written: the
/// database reads back row for row as before, and nothing reaches the native
/// owner. Every read still answers.
#[tokio::test]
async fn a_candidate_owner_refuses_every_change_typed_and_writes_nothing() {
    let tools = tool_harness(super::tools::MemberLimits::default());
    let harness = &tools.harness;
    let service = &harness.service;
    // What the candidate boots against: a live source, the member subscribed
    // to its topic, and one item the member is owed, not yet sealed.
    live_inbox(service).await;
    subscribe_to_inbox(service, "inbox-mail", "agent:a").await;
    let advanced = try_host_call(
        service,
        SOURCES_ADVANCE,
        with_items(
            advance_params("step-1", 1, 1, "c2"),
            &[upsert("msg-1", "v1", "an invoice")],
        ),
    )
    .await
    .unwrap();
    assert_eq!(outcomes(&advanced), vec!["announced"]);
    assert_eq!(owed(harness).await.get("agent:a"), Some(&1));
    let before = store_rows(&harness_db(harness));

    service.quiesce_for_candidate();
    let held = serde_json::json!({ "operation_id": "held-1", "batch_id": "batch-none" });
    let subscription_change = serde_json::json!({
        "operation_id": "s-change",
        "subscription_id": "inbox-mail",
        "expected_revision": 1,
    });
    let source_change = serde_json::json!({
        "operation_id": "o-change",
        "source_id": "inbox",
        "expected_revision": 1,
    });
    for (method, params) in [
        (
            ANNOUNCE,
            serde_json::json!({ "source_key": "k-1", "topics": ["ios"], "body": "a release" }),
        ),
        (
            SOURCES_PUT,
            source_params(
                "put-calendar",
                "calendar",
                "subject-2",
                &["calendar.changed"],
            ),
        ),
        (SOURCES_PAUSE, source_change.clone()),
        (SOURCES_RESUME, source_change.clone()),
        (SOURCES_END, source_change),
        (
            SOURCES_ADVANCE,
            with_items(
                advance_params("step-2", 1, 2, "c3"),
                &[upsert("msg-2", "v1", "another invoice")],
            ),
        ),
        (
            SOURCES_RECORD_FAILURE,
            serde_json::json!({
                "operation_id": "fail-1",
                "source_id": "inbox",
                "expected_revision": 1,
                "expected_version": 2,
                "principal": principal_of("subject-1"),
                "failure": { "class": "provider_unavailable", "retry_after_ms": 60000 },
            }),
        ),
        (
            SUBSCRIPTIONS_PUT,
            serde_json::json!({
                "operation_id": "subscribe-b",
                "subscription_id": "b-mail",
                "member": { "mob_id": OWN_MOB, "identity": "agent:b" },
                "topics": ["mail.received"],
            }),
        ),
        (
            "meta/mob_announcements/subscriptions/pause",
            subscription_change.clone(),
        ),
        (
            "meta/mob_announcements/subscriptions/resume",
            subscription_change.clone(),
        ),
        (
            "meta/mob_announcements/subscriptions/cancel",
            subscription_change,
        ),
        (
            "meta/mob_announcements/deliveries/reconcile",
            serde_json::json!({ "batch_id": "batch-none" }),
        ),
        ("meta/mob_announcements/deliveries/redeliver", held.clone()),
        ("meta/mob_announcements/deliveries/dismiss", held),
        (
            "meta/mob_announcements/declarations/apply",
            serde_json::json!({ "revision": 1, "declarations": { "revision": 1 } }),
        ),
    ] {
        let refused = try_host_call(service, method, params)
            .await
            .expect_err(method);
        assert_eq!(
            refused.code,
            super::rpc::ANNOUNCEMENTS_REFUSED_CODE,
            "{method}: {refused:?}"
        );
        assert_eq!(
            refusal_kind(&refused).as_deref(),
            Some("candidate_launch"),
            "{method}: {refused:?}"
        );
    }
    // A member's own tools: an announcement and a subscription change.
    for (tool, args) in [
        (
            super::tools::ANNOUNCE_TOOL,
            serde_json::json!({ "topics": ["ios"], "body": "checkout fixed", "key": "fix-1" }),
        ),
        (
            super::tools::SUBSCRIPTION_TOOL,
            serde_json::json!({ "action": "subscribe", "subscription": "mine", "topics": ["ios"] }),
        ),
    ] {
        let (failed, answer) = tools.call(Some("agent:b"), tool, args).await;
        assert!(failed, "{tool}: {answer}");
        assert!(
            answer
                .as_str()
                .is_some_and(|text| text.contains("candidate launch")),
            "{tool}: {answer}"
        );
    }
    // The owner's own verbs, as a collector or a dispatcher in this process
    // would call them.
    assert!(matches!(
        service.run_round().await,
        Err(AnnouncementError::CandidateLaunch)
    ));
    assert!(matches!(
        service.reconcile_mobs().await,
        Err(AnnouncementError::CandidateLaunch)
    ));
    assert!(matches!(
        service.reconcile_lineage().await,
        Err(AnnouncementError::CandidateLaunch)
    ));
    assert!(matches!(
        service.reconcile_declarations().await,
        Err(AnnouncementError::CandidateLaunch)
    ));
    assert_eq!(
        store_rows(&harness_db(harness)),
        before,
        "a refused change writes nothing"
    );
    assert_eq!(
        harness.native.submits(),
        0,
        "nothing reached the native owner"
    );

    // Every read still answers.
    let member_a = serde_json::json!({ "mob_id": OWN_MOB, "identity": "agent:a" });
    for (method, params) in [
        (
            "meta/mob_announcements/get",
            serde_json::json!({ "mob_id": OWN_MOB, "source_key": "k-none" }),
        ),
        (
            "meta/mob_announcements/subscriptions/get",
            serde_json::json!({ "subscription_id": "inbox-mail" }),
        ),
        (
            "meta/mob_announcements/subscriptions/list",
            serde_json::json!({}),
        ),
        (
            "meta/mob_announcements/deliveries/list",
            serde_json::json!({}),
        ),
        (
            SOURCES_GET,
            serde_json::json!({ "source_id": "inbox", "include_streams": true }),
        ),
        (SOURCES_LIST, serde_json::json!({})),
        ("meta/mob_announcements/audit/list", serde_json::json!({})),
        (
            "meta/mob_announcements/declarations/get",
            serde_json::json!({}),
        ),
        (
            "meta/mob_announcements/members/get",
            serde_json::json!({ "member": member_a }),
        ),
        ("meta/mob_announcements/members/list", serde_json::json!({})),
        (
            "meta/mob_announcements/escalations/list",
            serde_json::json!({}),
        ),
    ] {
        try_host_call(service, method, params)
            .await
            .unwrap_or_else(|error| panic!("{method} still answers: {error:?}"));
    }
    let source = inbox(service).await;
    assert_eq!(
        source["cursor"]["version"],
        serde_json::json!(2),
        "{source}"
    );
    let (failed, listed) = tools
        .call(
            Some("agent:a"),
            super::tools::SUBSCRIPTION_TOOL,
            serde_json::json!({ "action": "list" }),
        )
        .await;
    assert!(!failed, "a member still lists its subscriptions: {listed}");
    assert_eq!(store_rows(&harness_db(harness)), before);
}

/// A candidate launch's owner delivers nothing: over a store holding an item
/// its member is owed and due, its dispatcher ends at once, so no batch is
/// sealed and nothing is submitted within the window, and a round run on it
/// is refused. The authoritative owner over the same store then delivers
/// the item exactly once.
#[tokio::test(flavor = "multi_thread")]
async fn a_candidate_owner_delivers_nothing_and_the_authoritative_owner_delivers_once() {
    let harness = harness_with(ScriptedNative::new());
    harness.subscribe("s1").await;
    harness.announce_burst(1).await;
    // Past the item's longest delay: any round would seal and submit it.
    harness.clock.store(10_000_000, Ordering::SeqCst);

    let candidate = candidate_owner(&harness);
    tokio::time::timeout(std::time::Duration::from_secs(10), candidate.start())
        .await
        .expect("a candidate's dispatcher ends at once")
        .expect("and ends cleanly");
    assert!(matches!(
        candidate.run_round().await,
        Err(AnnouncementError::CandidateLaunch)
    ));
    assert_eq!(
        harness.native.submits(),
        0,
        "the candidate submitted a batch"
    );
    let query = BatchQuery {
        member: Some(member("agent:a")),
        limit: 10,
        ..BatchQuery::default()
    };
    assert!(
        candidate.deliveries(&query).await.unwrap().is_empty(),
        "the candidate sealed a batch"
    );
    assert_eq!(
        owed(&harness).await.get("agent:a"),
        Some(&1),
        "the item stays owed for the authoritative owner"
    );

    let report = harness.service.run_round().await.unwrap();
    assert_eq!(report.sealed, 1, "{report:?}");
    assert_eq!(harness.native.submits(), 1);
    let again = harness.service.run_round().await.unwrap();
    assert_eq!(again.sealed, 0, "{again:?}");
    assert_eq!(harness.native.submits(), 1, "delivered exactly once");
}

/// A candidate launch's owner keeps no source runner: installed over a
/// schedule service of its own, it makes no runner schedule for a declared
/// source with a runner, so no collector can run from it, and the source
/// reads as not scheduled here. The authoritative owner over the same store
/// makes the one schedule.
#[tokio::test]
async fn a_candidate_owner_keeps_no_source_runner_schedule() {
    let harness = harness_with(ScriptedNative::new());
    try_host_call(
        &harness.service,
        SOURCES_PUT,
        with_runner(
            source_params("put-inbox", "inbox", "subject-1", &["mail.received"]),
            "inbox-collector",
            60,
        ),
    )
    .await
    .unwrap();

    let candidate = candidate_owner(&harness);
    let schedules = install_runners(&candidate).await;
    assert!(
        runner_schedules(&schedules).await.is_empty(),
        "a candidate keeps no runner schedule"
    );
    assert!(!candidate.runners_installed());
    assert_eq!(
        inbox(&candidate).await["health"]["runner_scheduled_here"],
        serde_json::json!(false)
    );

    let schedules = install_runners(&harness.service).await;
    assert_eq!(runner_schedules(&schedules).await.len(), 1);
    assert!(harness.service.runners_installed());
}
