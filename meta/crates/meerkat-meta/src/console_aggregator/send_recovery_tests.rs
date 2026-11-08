//! Console send scoping, recovery and boot reconciliation, on an aggregator
//! with no registered runtime (identity-first reservations record under the
//! `identity-first` runtime key) and the admission-evidence test seam.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeMap;

use super::send_recovery::{AdmittedInput, SendAdmissionReader, governed_input_id};
use super::*;

const SESSION: &str = "0193f5d1-0b5a-7c3e-9a1f-1c2d3e4f5a6b";

/// [`SESSION`], parsed.
fn session() -> meerkat_core::types::SessionId {
    meerkat_core::types::SessionId::parse(SESSION).expect("the session id")
}

fn request(key: &str, content: &str) -> ConsoleSendRequest {
    ConsoleSendRequest {
        identity: "agent".to_string(),
        content: json!(content),
        origin: "console".to_string(),
        idempotency_key: key.to_string(),
        handling_mode: Some("queue".to_string()),
        origin_kind: None,
        skill_refs: Vec::new(),
    }
}

fn human(subject: &str) -> ConsoleSendScope {
    ConsoleSendScope::Human {
        issuer: "https://issuer.example".to_string(),
        subject: subject.to_string(),
    }
}

async fn reserve(
    aggregator: &MetaConsoleAggregator,
    key: &str,
    content: &str,
    scope: &ConsoleSendScope,
) -> ScopedReservation {
    aggregator
        .reserve_identity_first_interaction_scoped(
            request(key, content),
            Some(SESSION),
            scope,
            None,
        )
        .await
        .expect("reservation")
}

fn fresh(reservation: ScopedReservation) -> (ConsoleInteractionAccepted, SendDispatchClaim) {
    match reservation {
        ScopedReservation::Fresh(accepted, Some(claim)) => (accepted, claim),
        other => panic!("expected a fresh, claimed reservation: {other:?}"),
    }
}

struct EvidenceReader {
    admitted: BTreeMap<String, String>,
}

#[async_trait::async_trait]
impl SendAdmissionReader for EvidenceReader {
    async fn admitted_input(
        &self,
        _session_id: &meerkat_core::types::SessionId,
        idempotency_key: &str,
    ) -> Result<Option<AdmittedInput>, String> {
        Ok(self
            .admitted
            .get(idempotency_key)
            .map(|input_id| AdmittedInput {
                input_id: input_id.clone(),
                phase: Some("queued".to_string()),
            }))
    }

    async fn admitted_input_by_id(
        &self,
        session_id: &meerkat_core::types::SessionId,
        input_id: &meerkat_core::lifecycle::InputId,
    ) -> Result<Option<AdmittedInput>, String> {
        self.admitted_input(session_id, &input_id.to_string()).await
    }
}

fn admit(aggregator: &MetaConsoleAggregator, admitted: &[(&str, &str)]) {
    *aggregator.inner.send_admission_override.lock().unwrap() = Some(Arc::new(EvidenceReader {
        admitted: admitted
            .iter()
            .map(|(key, input)| ((*key).to_string(), (*input).to_string()))
            .collect(),
    }));
}

/// Admission evidence that answers only after `delay`, as a remote store's
/// does: the read first loads the runtime body of the session the send
/// recorded, one round trip at a time.
struct SlowEvidenceReader {
    evidence: EvidenceReader,
    delay: std::time::Duration,
}

#[async_trait::async_trait]
impl SendAdmissionReader for SlowEvidenceReader {
    async fn admitted_input(
        &self,
        session_id: &meerkat_core::types::SessionId,
        idempotency_key: &str,
    ) -> Result<Option<AdmittedInput>, String> {
        tokio::time::sleep(self.delay).await;
        self.evidence
            .admitted_input(session_id, idempotency_key)
            .await
    }

    async fn admitted_input_by_id(
        &self,
        session_id: &meerkat_core::types::SessionId,
        input_id: &meerkat_core::lifecycle::InputId,
    ) -> Result<Option<AdmittedInput>, String> {
        tokio::time::sleep(self.delay).await;
        self.evidence
            .admitted_input_by_id(session_id, input_id)
            .await
    }
}

fn admit_slowly(
    aggregator: &MetaConsoleAggregator,
    delay: std::time::Duration,
    admitted: &[(&str, &str)],
) {
    *aggregator.inner.send_admission_override.lock().unwrap() =
        Some(Arc::new(SlowEvidenceReader {
            evidence: EvidenceReader {
                admitted: admitted
                    .iter()
                    .map(|(key, input)| ((*key).to_string(), (*input).to_string()))
                    .collect(),
            },
            delay,
        }));
}

async fn status_of(aggregator: &MetaConsoleAggregator, frame_id: &str) -> ConsoleFrameStatus {
    aggregator
        .inner
        .store
        .query_frames(ConsoleTimelineQuery {
            identity: None,
            conversation_id: None,
            after: None,
            limit: 500,
        })
        .await
        .expect("query")
        .frames
        .into_iter()
        .find(|frame| frame.id == frame_id)
        .expect("frame")
        .status
}

#[tokio::test]
async fn one_key_under_two_principals_names_two_sends() {
    let aggregator = MetaConsoleAggregator::in_memory();
    let (first, first_claim) = fresh(reserve(&aggregator, "k1", "hello", &human("u1")).await);
    let (second, _second_claim) = fresh(reserve(&aggregator, "k1", "hello", &human("u2")).await);
    assert_ne!(first.input_frame_id, second.input_frame_id);
    assert_ne!(first.interaction_id, second.interaction_id);

    // The same principal replays its own acceptance while its dispatch is
    // owned, and owns the reconciliation once nothing does.
    match reserve(&aggregator, "k1", "hello", &human("u1")).await {
        ScopedReservation::Replay(replayed) => assert_eq!(replayed, first),
        other => panic!("expected a replay while the dispatch is owned: {other:?}"),
    }
    drop(first_claim);
    match reserve(&aggregator, "k1", "hello", &human("u1")).await {
        ScopedReservation::Unsettled(replayed, claim, frame) => {
            assert_eq!(replayed, first);
            assert_eq!(claim.frame_id(), first.input_frame_id);
            assert_eq!(frame.id, first.input_frame_id);
        }
        other => panic!("expected an unsettled replay: {other:?}"),
    }

    // A different body under a key is a conflict only for the principal who
    // used the key; a third principal's identical key is simply a new send.
    let conflict = aggregator
        .reserve_identity_first_interaction_scoped(
            request("k1", "other body"),
            Some(SESSION),
            &human("u2"),
            None,
        )
        .await
        .expect_err("same principal, same key, other body");
    assert!(matches!(conflict, ConsoleSendError::IdempotencyConflict(_)));
    let (third, _third_claim) = fresh(reserve(&aggregator, "k1", "other body", &human("u3")).await);
    assert_ne!(third.input_frame_id, first.input_frame_id);
    assert_ne!(third.input_frame_id, second.input_frame_id);
}

#[tokio::test]
async fn anonymous_and_authenticated_callers_do_not_share_keys() {
    let aggregator = MetaConsoleAggregator::in_memory();
    let (anonymous, _a) =
        fresh(reserve(&aggregator, "k1", "hello", &ConsoleSendScope::Anonymous).await);
    let service = ConsoleSendScope::Principal {
        principal: "svc:reporter".to_string(),
        provider: "service_identity".to_string(),
        issuer: "https://issuer.example".to_string(),
    };
    let (principal, _p) = fresh(reserve(&aggregator, "k1", "hello", &service).await);
    let (verified, _v) = fresh(reserve(&aggregator, "k1", "hello", &human("u1")).await);
    let ids = [
        &anonymous.input_frame_id,
        &principal.input_frame_id,
        &verified.input_frame_id,
    ];
    assert_ne!(ids[0], ids[1]);
    assert_ne!(ids[0], ids[2]);
    assert_ne!(ids[1], ids[2]);
}

#[tokio::test]
async fn a_record_from_before_scoping_is_never_matched_by_a_scoped_caller() {
    let aggregator = MetaConsoleAggregator::in_memory();
    // The public Rust API keeps the unscoped key shape every pre-upgrade
    // record has.
    let legacy = match aggregator
        .reserve_identity_first_interaction(request("k1", "hello"), Some(SESSION))
        .await
        .expect("unscoped reservation")
    {
        IdentityFirstReservation::Fresh(accepted) => accepted,
        other => panic!("expected a fresh unscoped reservation: {other:?}"),
    };
    let stored = aggregator
        .inner
        .store
        .frame_by_dedupe_key(&send_dedupe_key("identity-first", "agent", "console", "k1"))
        .await
        .expect("lookup")
        .expect("unscoped record");
    assert_eq!(stored.id, legacy.input_frame_id);

    // Not replayed to, recovered by, or bound to any scoped caller.
    for scope in [human("u1"), ConsoleSendScope::Anonymous] {
        assert!(
            aggregator
                .recover_send(&["agent".to_string()], "console", "k1", &scope)
                .await
                .expect("recover")
                .is_none(),
            "{scope:?}"
        );
    }
    let (scoped, _claim) = fresh(reserve(&aggregator, "k1", "hello", &human("u1")).await);
    assert_ne!(scoped.input_frame_id, legacy.input_frame_id);
    assert_ne!(scoped.interaction_id, legacy.interaction_id);
    assert_eq!(
        status_of(&aggregator, &legacy.input_frame_id).await,
        ConsoleFrameStatus::Accepted
    );
    // The unscoped API still replays the record and never reconciles it.
    assert_eq!(
        aggregator
            .reserve_identity_first_interaction(request("k1", "hello"), Some(SESSION))
            .await
            .expect("unscoped replay"),
        IdentityFirstReservation::Existing(legacy)
    );
}

#[tokio::test]
async fn recovery_answers_only_the_callers_own_send_and_settles_from_evidence_only() {
    let aggregator = MetaConsoleAggregator::in_memory();
    let (accepted, claim) = fresh(reserve(&aggregator, "k1", "hello", &human("u1")).await);
    drop(claim);
    let identities = ["agent".to_string()];

    // Without evidence the send is reported as it is; nothing settles it.
    let recovered = aggregator
        .recover_send(&identities, "console", "k1", &human("u1"))
        .await
        .expect("recover")
        .expect("own send");
    assert_eq!(recovered.accepted, accepted);
    assert!(matches!(
        recovered.native,
        NativeSendObservation::Unknown { .. }
    ));

    // Another principal's key, and a key nobody used, answer the same.
    for (scope, key) in [(human("u2"), "k1"), (human("u1"), "k2")] {
        assert!(
            aggregator
                .recover_send(&identities, "console", key, &scope)
                .await
                .expect("recover")
                .is_none()
        );
    }

    admit(&aggregator, &[]);
    let absent = aggregator
        .recover_send(&identities, "console", "k1", &human("u1"))
        .await
        .expect("recover")
        .expect("own send");
    assert_eq!(absent.native, NativeSendObservation::Absent);
    assert_eq!(absent.accepted.status, ConsoleFrameStatus::Accepted);

    admit(
        &aggregator,
        &[(accepted.input_frame_id.as_str(), "input-1")],
    );
    let admitted = aggregator
        .recover_send(&identities, "console", "k1", &human("u1"))
        .await
        .expect("recover")
        .expect("own send");
    assert_eq!(
        admitted.native,
        NativeSendObservation::Admitted {
            input_id: "input-1".to_string(),
            phase: Some("queued".to_string())
        }
    );
    assert_eq!(admitted.accepted.status, ConsoleFrameStatus::Delivered);
    assert_eq!(
        status_of(&aggregator, &accepted.input_frame_id).await,
        ConsoleFrameStatus::Delivered
    );
}

#[tokio::test]
async fn recovery_never_settles_a_send_with_a_live_dispatch_owner() {
    let aggregator = MetaConsoleAggregator::in_memory();
    let (accepted, claim) = fresh(reserve(&aggregator, "k1", "hello", &human("u1")).await);
    admit(
        &aggregator,
        &[(accepted.input_frame_id.as_str(), "input-1")],
    );
    let recovered = aggregator
        .recover_send(&["agent".to_string()], "console", "k1", &human("u1"))
        .await
        .expect("recover")
        .expect("own send");
    assert!(matches!(
        recovered.native,
        NativeSendObservation::Admitted { .. }
    ));
    assert_eq!(recovered.accepted.status, ConsoleFrameStatus::Accepted);
    drop(claim);
}

#[tokio::test]
async fn boot_reconciliation_settles_admitted_sends_only_and_dispatches_nothing() {
    let aggregator = MetaConsoleAggregator::in_memory();
    let (host_human, claim) = fresh(reserve(&aggregator, "k1", "one", &human("u1")).await);
    drop(claim);
    let (absent, claim) = fresh(reserve(&aggregator, "k2", "two", &human("u2")).await);
    drop(claim);
    let (governed, claim) = fresh(reserve(&aggregator, "k3", "three", &human("u1")).await);
    drop(claim);
    let (owned, owned_claim) = fresh(reserve(&aggregator, "k4", "four", &human("u2")).await);
    let governed_key =
        governed_input_id(&governed.interaction_id, &governed.input_frame_id).to_string();
    admit(
        &aggregator,
        &[
            (host_human.input_frame_id.as_str(), "input-1"),
            (governed_key.as_str(), "input-3"),
            (owned.input_frame_id.as_str(), "input-4"),
        ],
    );

    let outcome = aggregator.reconcile_interrupted_sends().await;
    assert_eq!(outcome.examined, 4);
    assert_eq!(outcome.delivered, 2);
    assert_eq!(
        status_of(&aggregator, &host_human.input_frame_id).await,
        ConsoleFrameStatus::Delivered
    );
    assert_eq!(
        status_of(&aggregator, &governed.input_frame_id).await,
        ConsoleFrameStatus::Delivered
    );
    // Absent evidence never fails a send, and a send with a live dispatch
    // owner is left to that owner.
    assert_eq!(
        status_of(&aggregator, &absent.input_frame_id).await,
        ConsoleFrameStatus::Accepted
    );
    assert_eq!(
        status_of(&aggregator, &owned.input_frame_id).await,
        ConsoleFrameStatus::Accepted
    );
    drop(owned_claim);

    // Settled sends are no longer unsettled: a second pass finds only the
    // absent and the formerly owned ones, and settles the admitted one.
    let second = aggregator.reconcile_interrupted_sends().await;
    assert_eq!(second.examined, 2);
    assert_eq!(second.delivered, 1);
}

/// A sender's recovery waits for a slow store's admission evidence. After a
/// restart, the evidence read loads the session the send recorded, which on
/// a remote store takes seconds (here 3 s): the recovery answers what the
/// store holds and settles the send whose input was admitted. Bounded by
/// 2 s, it answered `unknown` on every try, and the delivered send stayed
/// unsettled.
#[tokio::test]
async fn recovery_waits_for_a_slow_store_and_settles_an_admitted_send() {
    let aggregator = MetaConsoleAggregator::in_memory();
    let (accepted, claim) = fresh(reserve(&aggregator, "k1", "hello", &human("u1")).await);
    drop(claim);
    admit_slowly(
        &aggregator,
        std::time::Duration::from_secs(3),
        &[(accepted.input_frame_id.as_str(), "input-1")],
    );

    let recovered = aggregator
        .recover_send(&["agent".to_string()], "console", "k1", &human("u1"))
        .await
        .expect("recover")
        .expect("own send");
    assert_eq!(
        recovered.native,
        NativeSendObservation::Admitted {
            input_id: "input-1".to_string(),
            phase: Some("queued".to_string())
        }
    );
    assert_eq!(recovered.accepted.status, ConsoleFrameStatus::Delivered);
    assert_eq!(
        status_of(&aggregator, &accepted.input_frame_id).await,
        ConsoleFrameStatus::Delivered
    );
}

/// The boot reconciliation keeps its short bound per read. It runs before
/// the gateway serves, so on a slow store it leaves the send unsettled for
/// its sender's recovery instead of holding startup for the read.
#[tokio::test]
async fn boot_reconciliation_keeps_its_short_read_bound_on_a_slow_store() {
    let aggregator = MetaConsoleAggregator::in_memory();
    let (accepted, claim) = fresh(reserve(&aggregator, "k1", "hello", &human("u1")).await);
    drop(claim);
    let delay = std::time::Duration::from_secs(5);
    admit_slowly(
        &aggregator,
        delay,
        &[(accepted.input_frame_id.as_str(), "input-1")],
    );

    let started = std::time::Instant::now();
    let outcome = aggregator.reconcile_interrupted_sends().await;
    assert!(
        started.elapsed() < delay,
        "the boot reconciliation waited for the slow read"
    );
    assert_eq!((outcome.examined, outcome.delivered), (1, 0));
    assert_eq!(
        status_of(&aggregator, &accepted.input_frame_id).await,
        ConsoleFrameStatus::Accepted
    );
}

#[tokio::test]
async fn a_concurrent_identical_send_never_gets_a_second_dispatch_owner() {
    let aggregator = MetaConsoleAggregator::in_memory();
    let scope = human("u1");
    // A request that claimed the frame before it existed owns it; the
    // identical request that then inserts the frame only replays.
    let key = scope.dedupe_key("identity-first", "agent", "console", "k1");
    let early = aggregator
        .inner
        .send_dispatch_claims
        .claim(&stable_frame_id(&key))
        .expect("early claim");
    let replay = reserve(&aggregator, "k1", "hello", &scope).await;
    let ScopedReservation::Replay(accepted) = replay else {
        panic!("the inserting request must not own the dispatch: {replay:?}");
    };
    assert_eq!(accepted.input_frame_id, early.frame_id());
    assert!(
        aggregator
            .inner
            .send_dispatch_claims
            .is_claimed(&accepted.input_frame_id)
    );
    drop(early);
    assert!(matches!(
        reserve(&aggregator, "k1", "hello", &scope).await,
        ScopedReservation::Unsettled(..)
    ));
}

fn held() -> crate::identity_first::GovernedHold {
    crate::identity_first::GovernedHold {
        condition: meerkat_core::authorization::ControllerReadinessFailure::SourceHeld,
        waited: std::time::Duration::from_millis(300),
        binds: 1,
        input_id: meerkat_core::lifecycle::InputId::new(),
        idempotency_key: None,
    }
}

async fn frame_of(aggregator: &MetaConsoleAggregator, frame_id: &str) -> ConsoleFrame {
    aggregator
        .inner
        .store
        .query_frames(ConsoleTimelineQuery {
            identity: None,
            conversation_id: None,
            after: None,
            limit: 500,
        })
        .await
        .expect("query")
        .frames
        .into_iter()
        .find(|frame| frame.id == frame_id)
        .expect("frame")
}

/// `MarkHeld` applies to the send's own state. A hold is recorded
/// only on a send whose dispatch is in flight before any submission
/// (`dispatching`): its frame returns to `accepted` and records the hold.
/// On a send in any other state (accepted with no dispatch in flight,
/// delivered, failed) the mark is refused and the frame is left as it was:
/// its status, its version, no hold.
///
/// RED before the fix: the mark records the hold whatever the send's state,
/// and leaves a dispatching send's status as it was.
#[tokio::test]
async fn a_hold_is_recorded_only_on_a_send_dispatching_before_its_submission() {
    let aggregator = MetaConsoleAggregator::in_memory();
    for (key, status, recorded) in [
        ("dispatching", Some(ConsoleFrameStatus::Dispatching), true),
        ("accepted", None, false),
        ("delivered", Some(ConsoleFrameStatus::Delivered), false),
        ("failed", Some(ConsoleFrameStatus::DeliveryFailed), false),
    ] {
        let (accepted, claim) = fresh(reserve(&aggregator, key, "hello", &human("u1")).await);
        drop(claim);
        if let Some(status) = status {
            aggregator
                .inner
                .store
                .update_frame_status(&accepted.input_frame_id, status)
                .await
                .expect("the status is written")
                .expect("the frame");
        }
        let before = frame_of(&aggregator, &accepted.input_frame_id).await;
        let marked = aggregator
            .mark_interaction_delivery_held(&accepted.input_frame_id, &held())
            .await;
        let after = frame_of(&aggregator, &accepted.input_frame_id).await;
        if recorded {
            assert!(marked.is_ok(), "{key}: {marked:?}");
            assert_eq!(
                after.status,
                ConsoleFrameStatus::Accepted,
                "{key}: back to accepted"
            );
            assert!(after.payload.get("delivery_hold").is_some(), "{key}");
        } else {
            assert!(marked.is_err(), "{key}: the hold is refused: {marked:?}");
            assert_eq!(after.status, before.status, "{key}");
            assert_eq!(after.frame_version, before.frame_version, "{key}");
            assert!(after.payload.get("delivery_hold").is_none(), "{key}");
        }
    }
}

/// A send under `key` whose dispatch was held: dispatching, then held, its
/// frame recording a hold, no dispatch owning it.
async fn held_send_frame(aggregator: &MetaConsoleAggregator, key: &str) -> String {
    let (accepted, claim) = fresh(reserve(aggregator, key, "hello", &human("u1")).await);
    drop(claim);
    aggregator
        .mark_interaction_dispatching(&accepted.input_frame_id)
        .await
        .expect("the dispatch begins");
    aggregator
        .mark_interaction_delivery_held(&accepted.input_frame_id, &held())
        .await
        .expect("the hold is recorded");
    accepted.input_frame_id
}

/// Takes of one frame's hold from `stores`, all at once: how many got it.
async fn concurrent_takes(stores: &[Arc<dyn ConsoleLogStore>], frame_id: &str) -> usize {
    let takers: Vec<_> = stores
        .iter()
        .map(|store| {
            let store = Arc::clone(store);
            let frame_id = frame_id.to_string();
            tokio::spawn(async move { store.take_send_frame_hold(&frame_id).await })
        })
        .collect();
    let mut won = 0;
    for taker in takers {
        let taken = taker.await.expect("the taker").expect("the take");
        if let Some(frame) = taken {
            assert!(frame.payload.get("delivery_hold").is_none(), "{frame:?}");
            won += 1;
        }
    }
    won
}

/// Taking a held send's hold is a compare-and-set on the hold, in
/// both stores. Of concurrent takers exactly one gets the frame, without
/// its hold; the others, and every take after, get none. In SQLite the
/// takers hold two connections to one file, as two processes would: the
/// update changes the row only while it still holds the payload read, hold
/// included.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn taking_a_held_sends_hold_is_a_compare_and_set() {
    let memory: Arc<dyn ConsoleLogStore> = Arc::new(InMemoryConsoleLogStore::new());
    let aggregator = MetaConsoleAggregator::new(Arc::clone(&memory));
    for round in 0..8 {
        let frame_id = held_send_frame(&aggregator, &format!("memory-{round}")).await;
        let stores = vec![Arc::clone(&memory); 4];
        assert_eq!(
            concurrent_takes(&stores, &frame_id).await,
            1,
            "round {round}"
        );
        assert!(
            memory
                .take_send_frame_hold(&frame_id)
                .await
                .unwrap()
                .is_none()
        );
    }

    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("console.sqlite3");
    let first: Arc<dyn ConsoleLogStore> =
        Arc::new(SqliteConsoleLogStore::open(&path).expect("first connection"));
    let second: Arc<dyn ConsoleLogStore> =
        Arc::new(SqliteConsoleLogStore::open(&path).expect("second connection"));
    let aggregator = MetaConsoleAggregator::new(Arc::clone(&first));
    for round in 0..8 {
        let frame_id = held_send_frame(&aggregator, &format!("sqlite-{round}")).await;
        let stores = vec![
            Arc::clone(&first),
            Arc::clone(&second),
            Arc::clone(&first),
            Arc::clone(&second),
        ];
        assert_eq!(
            concurrent_takes(&stores, &frame_id).await,
            1,
            "round {round}"
        );
        assert!(
            second
                .take_send_frame_hold(&frame_id)
                .await
                .unwrap()
                .is_none()
        );
    }
}

/// The send machine's own state decides a transition in both
/// stores. A submitted send (its bind answered bound) records no hold;
/// only a dispatching one is submitted; the SQLite store moves a send only
/// from the state it read, so of two connections moving one send at once
/// exactly one moves it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_submitted_send_records_no_hold_and_moves_once() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("console.sqlite3");
    let stores: [Arc<dyn ConsoleLogStore>; 2] = [
        Arc::new(InMemoryConsoleLogStore::new()),
        Arc::new(SqliteConsoleLogStore::open(&path).expect("sqlite")),
    ];
    for store in stores {
        let aggregator = MetaConsoleAggregator::new(Arc::clone(&store));
        let (accepted, claim) =
            fresh(reserve(&aggregator, "submitted", "hello", &human("u1")).await);
        drop(claim);
        assert!(
            aggregator
                .mark_interaction_submitted(&accepted.input_frame_id, &session())
                .await
                .is_err(),
            "an accepted send is not submitted before its dispatch begins"
        );
        aggregator
            .mark_interaction_dispatching(&accepted.input_frame_id)
            .await
            .expect("the dispatch begins");
        aggregator
            .mark_interaction_submitted(&accepted.input_frame_id, &session())
            .await
            .expect("the bind answered bound");
        let before = frame_of(&aggregator, &accepted.input_frame_id).await;
        assert_eq!(before.status, ConsoleFrameStatus::Submitted);
        let marked = aggregator
            .mark_interaction_delivery_held(&accepted.input_frame_id, &held())
            .await;
        assert!(
            marked.is_err(),
            "a submitted send records no hold: {marked:?}"
        );
        let after = frame_of(&aggregator, &accepted.input_frame_id).await;
        assert_eq!(after.status, ConsoleFrameStatus::Submitted);
        assert_eq!(after.frame_version, before.frame_version);
        assert!(after.payload.get("delivery_hold").is_none());
    }

    let first: Arc<dyn ConsoleLogStore> =
        Arc::new(SqliteConsoleLogStore::open(&path).expect("first connection"));
    let second: Arc<dyn ConsoleLogStore> =
        Arc::new(SqliteConsoleLogStore::open(&path).expect("second connection"));
    let aggregator = MetaConsoleAggregator::new(Arc::clone(&first));
    for round in 0..8 {
        let (accepted, claim) = fresh(
            reserve(
                &aggregator,
                &format!("raced-{round}"),
                "hello",
                &human("u1"),
            )
            .await,
        );
        drop(claim);
        aggregator
            .mark_interaction_dispatching(&accepted.input_frame_id)
            .await
            .expect("the dispatch begins");
        let movers: Vec<_> = [&first, &second, &first, &second]
            .into_iter()
            .map(|store| {
                let store = Arc::clone(store);
                let frame_id = accepted.input_frame_id.clone();
                tokio::spawn(async move {
                    store
                        .transition_send_frame(&frame_id, SendTransition::MarkSubmitted, None)
                        .await
                })
            })
            .collect();
        let mut moved = 0;
        for mover in movers {
            if mover.await.expect("the mover").expect("the move").is_some() {
                moved += 1;
            }
        }
        assert_eq!(moved, 1, "round {round}");
    }
}

/// Admission evidence held in one session only; every read records the
/// session it read.
struct OneSessionEvidence {
    session: meerkat_core::types::SessionId,
    admitted: BTreeMap<String, String>,
    reads: std::sync::Mutex<Vec<meerkat_core::types::SessionId>>,
}

#[async_trait::async_trait]
impl SendAdmissionReader for OneSessionEvidence {
    async fn admitted_input(
        &self,
        session_id: &meerkat_core::types::SessionId,
        idempotency_key: &str,
    ) -> Result<Option<AdmittedInput>, String> {
        self.reads.lock().unwrap().push(session_id.clone());
        Ok((*session_id == self.session)
            .then(|| self.admitted.get(idempotency_key))
            .flatten()
            .map(|input_id| AdmittedInput {
                input_id: input_id.clone(),
                phase: Some("queued".to_string()),
            }))
    }

    async fn admitted_input_by_id(
        &self,
        session_id: &meerkat_core::types::SessionId,
        input_id: &meerkat_core::lifecycle::InputId,
    ) -> Result<Option<AdmittedInput>, String> {
        self.admitted_input(session_id, &input_id.to_string()).await
    }
}

/// A governed send accepted for the member's session (S1, recorded at
/// acceptance) whose identity was reset before its bind binds the new session
/// (S2), and its submission record names S2. The process stops after the
/// submission, its input admitted on S2. The gateway's start reads the
/// runtime's evidence in S2, the session the bind bound, and settles the send
/// delivered; nothing is read in S1.
///
/// RED before the fix: the start reads S1, finds none of the input there and
/// fails the send, its outcome unknown.
#[tokio::test]
async fn a_submitted_send_is_settled_from_the_session_its_bind_bound() {
    let aggregator = MetaConsoleAggregator::in_memory();
    let (accepted, claim) = fresh(reserve(&aggregator, "rebound", "hello", &human("u1")).await);
    drop(claim);
    let bound = meerkat_core::types::SessionId::new();
    assert_ne!(bound, session());
    aggregator
        .mark_interaction_dispatching(&accepted.input_frame_id)
        .await
        .expect("the dispatch begins");
    aggregator
        .mark_interaction_submitted(&accepted.input_frame_id, &bound)
        .await
        .expect("the bind bound the new session");
    let evidence = Arc::new(OneSessionEvidence {
        session: bound.clone(),
        admitted: [(
            governed_input_id(&accepted.interaction_id, &accepted.input_frame_id).to_string(),
            "input-b".to_string(),
        )]
        .into_iter()
        .collect(),
        reads: std::sync::Mutex::new(Vec::new()),
    });
    aggregator.read_send_admission_from(evidence.clone());

    let outcome = aggregator.reconcile_interrupted_sends().await;
    assert_eq!(
        (outcome.delivered, outcome.lost),
        (1, 0),
        "settled delivered from the bound session: {outcome:?}"
    );
    assert_eq!(
        status_of(&aggregator, &accepted.input_frame_id).await,
        ConsoleFrameStatus::Delivered
    );
    let reads = evidence.reads.lock().unwrap().clone();
    assert!(
        !reads.is_empty() && reads.iter().all(|read| *read == bound),
        "only the bound session is read: {reads:?}"
    );
}
