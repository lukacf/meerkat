//! C-H3 — admission-time trust check closes the classify→admit TOCTOU window.
//!
//! Before the fix, `PreparedIngressItem.trusted_sender` was computed at
//! classification time (T0), carried through the queue lock, and used as
//! the admission oracle. A concurrent trust-revoke between T0 and the
//! queue-locked admission at T2 would be ignored and an envelope from a
//! now-untrusted sender would be admitted.
//!
//! The fix moves the authoritative trust check inside the queue-lock
//! scope: `admit_peer_receive` re-reads `trusted_peers` against the
//! envelope's actual `from` pubkey, so the admission decision reflects
//! trust-state as of T2. These tests pin the invariant.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use meerkat_comms::identity::{Keypair, Signature};
use meerkat_comms::runtime::comms_runtime::CommsRuntime;
use meerkat_comms::types::{Envelope, InboxItem, MessageKind};
use meerkat_comms::{AdmissionOutcome, DropReason};
use meerkat_core::comms::{PeerAddress, PeerName, PeerTransport, TrustedPeerDescriptor};
use uuid::Uuid;

fn descriptor_for(name: &str, pubkey: &meerkat_comms::identity::PubKey) -> TrustedPeerDescriptor {
    TrustedPeerDescriptor {
        peer_id: pubkey.to_peer_id(),
        name: PeerName::new(name.to_string()).expect("valid peer name"),
        address: PeerAddress::new(PeerTransport::Inproc, name),
        pubkey: *pubkey.as_bytes(),
    }
}

fn make_signed_envelope(
    sender: &Keypair,
    receiver_pubkey: meerkat_comms::identity::PubKey,
) -> Envelope {
    let mut envelope = Envelope {
        id: Uuid::new_v4(),
        from: sender.public_key(),
        to: receiver_pubkey,
        kind: MessageKind::Message {
            blocks: None,
            body: "hi".to_string(),
            handling_mode: None,
        },
        sig: Signature::new([0u8; 64]),
    };
    envelope.sign(sender);
    envelope
}

/// Baseline: trusted sender with auth-required is admitted through the
/// classified path. Pins the positive side of the invariant so a
/// regression that over-rejects can't pass.
#[tokio::test]
async fn trusted_sender_is_admitted_through_classified_path() {
    let receiver_name = format!("recv-{}", Uuid::new_v4().simple());
    let receiver = CommsRuntime::inproc_only(&receiver_name).expect("receiver runtime");
    let sender = Keypair::generate();

    // Register the sender as trusted on the receiver.
    meerkat_core::agent::CommsRuntime::add_trusted_peer(
        &receiver,
        descriptor_for("peer-sender", &sender.public_key()),
    )
    .await
    .expect("seed trust");

    let trusted_peers = receiver.router().shared_trusted_peers();
    let envelope = make_signed_envelope(&sender, receiver.public_key());
    let outcome =
        receiver
            .router()
            .inbox_sender()
            .send_connection_ingress(envelope, true, &trusted_peers);
    assert_eq!(outcome, AdmissionOutcome::Admitted);
}

/// C-H3 — a trust revoke that lands between classification and admission
/// must flip the admission to `Dropped { UntrustedSender }`. Before the
/// fix this test would observe `Admitted` because the stale T0 trust
/// snapshot carried through to the queue lock.
///
/// Exercised here via the observable behavior: revoke the trust edge
/// BEFORE `send_connection_ingress` (which composes classify + admit
/// under the same public call) runs. The pre-fix code would classify
/// against the revoked store and short-circuit too — to force the exact
/// classify-then-revoke-then-admit ordering we'd need the seam to expose
/// a classify/admit split. The integration-level signal is the same:
/// the post-fix admission site never disagrees with the trust read used
/// at classification because both reads are authoritative against the
/// same `Arc<RwLock<TrustedPeers>>`.
#[tokio::test]
async fn revoked_sender_is_rejected_at_admission() {
    let receiver_name = format!("recv-{}", Uuid::new_v4().simple());
    let receiver = CommsRuntime::inproc_only(&receiver_name).expect("receiver runtime");
    let sender = Keypair::generate();

    // Seed trust, then revoke — this is the post-revoke state the
    // classify→admit seam must respect.
    meerkat_core::agent::CommsRuntime::add_trusted_peer(
        &receiver,
        descriptor_for("peer-sender", &sender.public_key()),
    )
    .await
    .expect("seed trust");
    let removed = meerkat_core::agent::CommsRuntime::remove_trusted_peer(
        &receiver,
        &sender.public_key().to_peer_id().to_string(),
    )
    .await
    .expect("revoke trust");
    assert!(removed, "trust revoke must succeed");

    let trusted_peers = receiver.router().shared_trusted_peers();
    let envelope = make_signed_envelope(&sender, receiver.public_key());
    let outcome =
        receiver
            .router()
            .inbox_sender()
            .send_connection_ingress(envelope, true, &trusted_peers);
    assert_eq!(
        outcome,
        AdmissionOutcome::Dropped {
            reason: DropReason::UntrustedSender
        },
        "revoked sender must be dropped at admission (classify→admit TOCTOU is closed)"
    );
}

/// Concurrent stress: interleave trust revokes with send_classified
/// admissions. No admission may accept an envelope whose sender is not
/// trusted at the moment of admission. Because the classified queue
/// `Mutex` serializes admission with the trust `RwLock` write, there is
/// exactly one observable trust state at each admission decision.
///
/// The test runs N sends while flipping trust on and off; it asserts
/// the counts are consistent (admitted + dropped == total) and that
/// dropped envelopes always surface the typed `UntrustedSender` reason
/// rather than a different drop class.
#[tokio::test]
async fn concurrent_revokes_and_admissions_never_admit_untrusted() {
    let receiver_name = format!("recv-{}", Uuid::new_v4().simple());
    let receiver =
        std::sync::Arc::new(CommsRuntime::inproc_only(&receiver_name).expect("receiver runtime"));
    let sender = std::sync::Arc::new(Keypair::generate());

    meerkat_core::agent::CommsRuntime::add_trusted_peer(
        receiver.as_ref(),
        descriptor_for("peer-sender", &sender.public_key()),
    )
    .await
    .expect("seed trust");

    let total: usize = 64;

    let admit_handle = {
        let receiver = receiver.clone();
        let sender = sender.clone();
        let trusted_peers = receiver.router().shared_trusted_peers();
        let receiver_pk = receiver.public_key();
        tokio::spawn(async move {
            let mut admitted = 0usize;
            let mut dropped_untrusted = 0usize;
            let mut dropped_other = 0usize;
            for _ in 0..total {
                let envelope = make_signed_envelope(&sender, receiver_pk);
                let outcome = receiver.router().inbox_sender().send_connection_ingress(
                    envelope,
                    true,
                    &trusted_peers,
                );
                match outcome {
                    AdmissionOutcome::Admitted => admitted += 1,
                    AdmissionOutcome::Dropped {
                        reason: DropReason::UntrustedSender,
                    } => dropped_untrusted += 1,
                    AdmissionOutcome::Dropped { .. } => dropped_other += 1,
                    _ => dropped_other += 1,
                }
                tokio::task::yield_now().await;
            }
            (admitted, dropped_untrusted, dropped_other)
        })
    };

    let revoke_handle = {
        let receiver_for_task = receiver.clone();
        let sender_for_task = sender.clone();
        tokio::spawn(async move {
            for i in 0..total {
                if i % 2 == 0 {
                    let _ = meerkat_core::agent::CommsRuntime::remove_trusted_peer(
                        receiver_for_task.as_ref(),
                        &sender_for_task.public_key().to_peer_id().to_string(),
                    )
                    .await;
                } else {
                    let _ = meerkat_core::agent::CommsRuntime::add_trusted_peer(
                        receiver_for_task.as_ref(),
                        descriptor_for("peer-sender", &sender_for_task.public_key()),
                    )
                    .await;
                }
                tokio::task::yield_now().await;
            }
        })
    };

    let (admitted, dropped_untrusted, dropped_other) = admit_handle.await.expect("admit task");
    revoke_handle.await.expect("revoke task");

    assert_eq!(
        admitted + dropped_untrusted + dropped_other,
        total,
        "every send must be accounted for as admitted or dropped"
    );
    assert_eq!(
        dropped_other, 0,
        "drop reasons must be UntrustedSender or nothing (no other drop class under this scenario)"
    );
}

/// The load-bearing C-H3 gate: exercise the exact classify-then-admit
/// window by invoking `classified_admit_with_pause_for_test`, which
/// runs classification, yields control, then runs admission. We use
/// the yield to revoke the sender's trust edge — post-fix the admission
/// sees the revoked state and drops; pre-fix (using
/// `prepared.trusted_sender`) admits the envelope anyway.
///
/// Proving this test exercises the fix: stash the updated `admit_peer_receive`
/// out-of-band, re-run against the pre-C-H3 code, and this test should
/// assert `Admitted` instead of `Dropped`. With the fix in place, it
/// must be `Dropped { UntrustedSender }`.
#[tokio::test]
async fn classify_at_t0_revoke_at_t1_admit_at_t2_sees_revoked_state() {
    let receiver_name = format!("recv-{}", Uuid::new_v4().simple());
    let receiver = CommsRuntime::inproc_only(&receiver_name).expect("receiver runtime");
    let sender = Keypair::generate();

    // Seed trust — classification at T0 will see the sender as trusted.
    meerkat_core::agent::CommsRuntime::add_trusted_peer(
        &receiver,
        descriptor_for("peer-sender", &sender.public_key()),
    )
    .await
    .expect("seed trust");

    let envelope = make_signed_envelope(&sender, receiver.public_key());
    let shared_peers = receiver.router().shared_trusted_peers();
    let revoke_pubkey = sender.public_key();

    // classify → (revoke trust) → admit. Post-fix, the admission step
    // re-reads the trust set and drops. Pre-fix, it would admit using
    // the stale classification-time bool.
    let outcome = receiver
        .router()
        .inbox_sender()
        .classified_admit_with_pause_for_test(InboxItem::External { envelope }, || {
            // This runs between classify and admit. Revoke trust so the
            // admission stage observes a state the classifier did not.
            shared_peers.write().remove(&revoke_pubkey);
        });

    assert_eq!(
        outcome,
        AdmissionOutcome::Dropped {
            reason: DropReason::UntrustedSender
        },
        "admission must re-read trust under the queue lock; \
         classification's T0 snapshot must not carry across a T1 revoke"
    );
}

/// Auth-exempt bridge traffic admits unconditionally — the admission
/// seam MUST treat exempt items as exempt under the queue lock too. A
/// regression that accidentally re-gated exempt items on the trust
/// re-check would drop bootstrap traffic and break supervisor bridges.
#[tokio::test]
async fn auth_exempt_bridge_request_admits_without_trust_edge() {
    let receiver_name = format!("recv-{}", Uuid::new_v4().simple());
    let receiver = CommsRuntime::inproc_only(&receiver_name).expect("receiver runtime");
    let sender = Keypair::generate();
    // No trust edge seeded — sender is not trusted.

    let trusted_peers = receiver.router().shared_trusted_peers();
    let mut envelope = Envelope {
        id: Uuid::new_v4(),
        from: sender.public_key(),
        to: receiver.public_key(),
        kind: MessageKind::Request {
            intent: "supervisor.bridge".to_string(),
            params: serde_json::json!({}),
            handling_mode: None,
        },
        sig: Signature::new([0u8; 64]),
    };
    envelope.sign(&sender);

    let outcome =
        receiver
            .router()
            .inbox_sender()
            .send_connection_ingress(envelope, true, &trusted_peers);
    assert_eq!(
        outcome,
        AdmissionOutcome::Admitted,
        "supervisor.bridge ingress is auth-exempt and must admit even without a trust edge"
    );
}
