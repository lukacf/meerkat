//! Console send scoping, recovery and boot reconciliation, on an aggregator
//! with no registered runtime (identity-first reservations record under the
//! `identity-first` runtime key) and the admission-evidence test seam.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeMap;

use super::send_recovery::{AdmittedInput, SendAdmissionReader, governed_delivery_key};
use super::*;

const SESSION: &str = "0193f5d1-0b5a-7c3e-9a1f-1c2d3e4f5a6b";

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
        .reserve_identity_first_interaction_scoped(request(key, content), Some(SESSION), scope)
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
}

fn admit(aggregator: &MetaConsoleAggregator, admitted: &[(&str, &str)]) {
    *aggregator.inner.send_admission_override.lock().unwrap() = Some(Arc::new(EvidenceReader {
        admitted: admitted
            .iter()
            .map(|(key, input)| ((*key).to_string(), (*input).to_string()))
            .collect(),
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
    let governed_key = governed_delivery_key(&governed.interaction_id, &governed.input_frame_id);
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
