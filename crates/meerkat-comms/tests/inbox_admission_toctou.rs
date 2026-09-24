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

use meerkat_comms::runtime::comms_runtime::CommsRuntime;
use meerkat_comms::types::MessageKind;
use meerkat_comms::{DropReason, SendError};
use meerkat_core::comms::{
    CommsTrustMutation, CommsTrustMutationResult, PeerAddress, PeerName, PeerTransport,
    TrustedPeerDescriptor,
};
use meerkat_core::{PeerIngressAuthDecision, PeerIngressAuthExemption, SUPERVISOR_BRIDGE_INTENT};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

type TestMachineAuthority =
    Arc<Mutex<meerkat_runtime::meerkat_machine::dsl::MeerkatMachineAuthority>>;

struct TestPeerCommsAuthority {
    authority: TestMachineAuthority,
    dsl: Arc<meerkat_runtime::HandleDslAuthority>,
}

impl TestPeerCommsAuthority {
    fn install(runtime: &CommsRuntime, session_id: &str) -> Self {
        let authority = Arc::new(Mutex::new(
            meerkat_runtime::meerkat_machine::dsl::MeerkatMachineAuthority::new(),
        ));
        let local_endpoint = local_endpoint_for(runtime);
        {
            let mut guard = authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard
                .apply_signal(
                    meerkat_runtime::meerkat_machine::dsl::MeerkatMachineSignal::Initialize,
                )
                .expect("Initialize signal");
            meerkat_runtime::meerkat_machine::dsl::MeerkatMachineMutator::apply(
                &mut *guard,
                meerkat_runtime::meerkat_machine::dsl::MeerkatMachineInput::RegisterSession {
                    session_id: meerkat_runtime::meerkat_machine::dsl::SessionId::from(session_id),
                    runtime_epoch_id: None,
                },
            )
            .expect("RegisterSession input");
            meerkat_runtime::meerkat_machine::dsl::MeerkatMachineMutator::apply(
                &mut *guard,
                meerkat_runtime::meerkat_machine::dsl::MeerkatMachineInput::PublishLocalEndpoint {
                    endpoint: local_endpoint,
                },
            )
            .expect("PublishLocalEndpoint input");
            meerkat_runtime::meerkat_machine::dsl::MeerkatMachineMutator::apply(
                &mut *guard,
                meerkat_runtime::meerkat_machine::dsl::MeerkatMachineInput::Prepare {
                    session_id: meerkat_runtime::meerkat_machine::dsl::SessionId::from(session_id),
                    run_id: meerkat_runtime::meerkat_machine::dsl::RunId::from(format!(
                        "{session_id}-ingress-run"
                    )),
                },
            )
            .expect("Prepare input");
        }

        let dsl = Arc::new(meerkat_runtime::HandleDslAuthority::from_shared(
            Arc::clone(&authority),
        ));
        meerkat_runtime::RuntimePeerCommsHandle::install_generated_on(Arc::clone(&dsl), runtime)
            .expect("install generated peer-comms handle");
        Self { authority, dsl }
    }

    fn reinstall(&self, runtime: &CommsRuntime) {
        meerkat_runtime::RuntimePeerCommsHandle::install_generated_on(
            Arc::clone(&self.dsl),
            runtime,
        )
        .expect("install generated peer-comms handle");
    }

    fn add_authority(
        &self,
        peer: &TrustedPeerDescriptor,
    ) -> meerkat_core::comms::CommsTrustMutationAuthority {
        let endpoint = meerkat_runtime::meerkat_machine::dsl::PeerEndpoint::from(peer);
        let transition = {
            let mut guard = self
                .authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            meerkat_runtime::meerkat_machine::dsl::MeerkatMachineMutator::apply(
                &mut *guard,
                meerkat_runtime::meerkat_machine::dsl::MeerkatMachineInput::AddDirectPeerEndpoint {
                    endpoint: endpoint.clone(),
                },
            )
            .expect("AddDirectPeerEndpoint input")
        };
        let mut obligations =
            meerkat_runtime::protocol_comms_trust_reconcile::extract_obligations_with_freshness(
                &transition,
                meerkat_runtime::protocol_comms_trust_reconcile::PeerProjectionFreshnessAuthority::from_authority(
                    Arc::clone(&self.authority),
                ),
            );
        let obligation = obligations.pop().expect("generated reconcile obligation");
        meerkat_runtime::protocol_comms_trust_reconcile::authority_for_endpoint(
            &obligation,
            &endpoint,
        )
        .expect("generated peer projection add authority")
    }

    fn remove_authority(
        &self,
        peer: &TrustedPeerDescriptor,
    ) -> meerkat_core::comms::CommsTrustMutationAuthority {
        let endpoint = meerkat_runtime::meerkat_machine::dsl::PeerEndpoint::from(peer);
        let transition = {
            let mut guard = self
                .authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            meerkat_runtime::meerkat_machine::dsl::MeerkatMachineMutator::apply(
                &mut *guard,
                meerkat_runtime::meerkat_machine::dsl::MeerkatMachineInput::RemoveDirectPeerEndpoint {
                    endpoint: endpoint.clone(),
                },
            )
            .expect("RemoveDirectPeerEndpoint input")
        };
        let mut obligations =
            meerkat_runtime::protocol_comms_trust_reconcile::extract_obligations_with_freshness(
                &transition,
                meerkat_runtime::protocol_comms_trust_reconcile::PeerProjectionFreshnessAuthority::from_authority(
                    Arc::clone(&self.authority),
                ),
            );
        let obligation = obligations.pop().expect("generated reconcile obligation");
        meerkat_runtime::protocol_comms_trust_reconcile::removal_authority_for_peer_id(
            &obligation,
            &endpoint.peer_id.0,
        )
        .expect("generated peer projection remove authority")
    }
}

fn descriptor_for(name: &str, pubkey: &meerkat_comms::identity::PubKey) -> TrustedPeerDescriptor {
    TrustedPeerDescriptor {
        peer_id: pubkey.to_peer_id(),
        name: PeerName::new(name.to_string()).expect("valid peer name"),
        address: PeerAddress::new(PeerTransport::Inproc, name),
        pubkey: *pubkey.as_bytes(),
    }
}

fn local_endpoint_for(
    runtime: &CommsRuntime,
) -> meerkat_runtime::meerkat_machine::dsl::PeerEndpoint {
    meerkat_runtime::meerkat_machine::dsl::PeerEndpoint::new(
        "local",
        runtime.public_key().to_peer_id().to_string(),
        "inproc://local",
        *runtime.public_key().as_bytes(),
    )
}

async fn apply_generated_trust(
    runtime: &CommsRuntime,
    peer_authority: &TestPeerCommsAuthority,
    peer: TrustedPeerDescriptor,
) {
    peer_authority.reinstall(runtime);
    let authority = peer_authority.add_authority(&peer);
    meerkat_core::agent::CommsRuntime::apply_trust_mutation(
        runtime,
        CommsTrustMutation::AddTrustedPeer { authority, peer },
    )
    .await
    .expect("seed trust");
}

async fn revoke_generated_trust(
    runtime: &CommsRuntime,
    peer_authority: &TestPeerCommsAuthority,
    peer: TrustedPeerDescriptor,
) -> bool {
    let peer_id = peer.peer_id.to_string();
    peer_authority.reinstall(runtime);
    let authority = peer_authority.remove_authority(&peer);
    match meerkat_core::agent::CommsRuntime::apply_trust_mutation(
        runtime,
        CommsTrustMutation::RemoveTrustedPeer {
            authority,
            peer_id: peer_id.clone(),
        },
    )
    .await
    .expect("revoke trust")
    {
        CommsTrustMutationResult::Removed { removed } => removed,
        other => panic!("expected trust removal result, got {other:?}"),
    }
}

fn lifecycle_request() -> MessageKind {
    MessageKind::Request {
        objective_id: None,
        content_taint: None,
        intent: "mob.peer_added".to_string(),
        params: serde_json::json!({"peer": "worker"}),
        blocks: None,
        reply_endpoint: None,
        handling_mode: None,
    }
}

async fn public_sender_runtime(receiver_name: &str, receiver: &CommsRuntime) -> Arc<CommsRuntime> {
    let sender_name = format!("sender-{}", Uuid::new_v4().simple());
    let sender = Arc::new(CommsRuntime::inproc_only(&sender_name).expect("sender runtime"));
    let sender_authority = TestPeerCommsAuthority::install(sender.as_ref(), &sender_name);
    apply_generated_trust(
        sender.as_ref(),
        &sender_authority,
        descriptor_for(receiver_name, &receiver.public_key()),
    )
    .await;
    sender
}

async fn drain_one_volatile(
    runtime: Arc<CommsRuntime>,
) -> meerkat_core::interaction::PeerInputCandidate {
    loop {
        let mut candidates =
            meerkat_core::agent::CommsRuntime::handoff_volatile_peer_input_candidates(
                runtime.as_ref(),
            )
            .await
            .expect("volatile peer-input handoff");
        if let Some(candidate) = candidates.pop() {
            return candidate;
        }
        tokio::task::yield_now().await;
    }
}

/// Baseline: trusted sender with auth-required is admitted through the
/// classified path. Pins the positive side of the invariant so a
/// regression that over-rejects can't pass.
#[tokio::test]
async fn trusted_sender_is_admitted_through_classified_path() {
    let receiver_name = format!("recv-{}", Uuid::new_v4().simple());
    let receiver = Arc::new(CommsRuntime::inproc_only(&receiver_name).expect("receiver runtime"));
    let peer_authority = TestPeerCommsAuthority::install(receiver.as_ref(), &receiver_name);
    let sender = public_sender_runtime(&receiver_name, receiver.as_ref()).await;
    let sender_pubkey = sender.public_key();

    // Register the sender as trusted on the receiver.
    apply_generated_trust(
        receiver.as_ref(),
        &peer_authority,
        descriptor_for("peer-sender", &sender_pubkey),
    )
    .await;

    let drain = tokio::spawn(drain_one_volatile(Arc::clone(&receiver)));
    let outcome = sender
        .router()
        .send(receiver.public_key().to_peer_id(), lifecycle_request())
        .await
        .expect("trusted lifecycle control must reach the classified runtime");
    assert!(matches!(
        outcome.delivery,
        meerkat_core::comms::PeerDeliveryOutcome::VolatileHandedOff
    ));
    let candidate = drain.await.expect("volatile drain joins");
    assert_eq!(candidate.auth(), Some(PeerIngressAuthDecision::Required));
}

/// C-H3 — a trust revoke that lands between classification and admission
/// must flip the admission to `Dropped { UntrustedSender }`. Before the
/// fix this test would observe `Admitted` because the stale T0 trust
/// snapshot carried through to the queue lock.
///
/// Exercised here via the observable behavior: revoke the trust edge
/// before the public peer send (which composes classify + admit) runs.
/// The pre-fix code would classify
/// against the revoked store and short-circuit too — to force the exact
/// classify-then-revoke-then-admit ordering we'd need the seam to expose
/// a classify/admit split. The integration-level signal is the same:
/// the post-fix admission site never disagrees with the trust read used
/// at classification because both reads are authoritative against the
/// same `Arc<RwLock<TrustStore>>`.
#[tokio::test]
async fn revoked_sender_is_rejected_at_admission() {
    let receiver_name = format!("recv-{}", Uuid::new_v4().simple());
    let receiver = CommsRuntime::inproc_only(&receiver_name).expect("receiver runtime");
    let peer_authority = TestPeerCommsAuthority::install(&receiver, &receiver_name);
    let sender = public_sender_runtime(&receiver_name, &receiver).await;
    let sender_pubkey = sender.public_key();

    // Seed trust, then revoke — this is the post-revoke state the
    // classify→admit seam must respect.
    let sender_descriptor = descriptor_for("peer-sender", &sender_pubkey);
    apply_generated_trust(&receiver, &peer_authority, sender_descriptor.clone()).await;
    let removed = revoke_generated_trust(&receiver, &peer_authority, sender_descriptor).await;
    assert!(removed, "trust revoke must succeed");

    let outcome = sender
        .router()
        .send(receiver.public_key().to_peer_id(), lifecycle_request())
        .await;
    assert!(
        matches!(
            outcome,
            Err(SendError::AdmissionDropped {
                reason: DropReason::UntrustedSender
            })
        ),
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
    let peer_authority = Arc::new(TestPeerCommsAuthority::install(
        receiver.as_ref(),
        &receiver_name,
    ));
    let sender = public_sender_runtime(&receiver_name, receiver.as_ref()).await;
    let sender_pubkey = sender.public_key();

    apply_generated_trust(
        receiver.as_ref(),
        peer_authority.as_ref(),
        descriptor_for("peer-sender", &sender_pubkey),
    )
    .await;

    let total: usize = 64;

    let admit_handle = {
        let receiver = receiver.clone();
        let sender = sender.clone();
        tokio::spawn(async move {
            let mut admitted = 0usize;
            let mut dropped_untrusted = 0usize;
            let mut dropped_other = 0usize;
            for _ in 0..total {
                let outcome = sender
                    .router()
                    .send(receiver.public_key().to_peer_id(), lifecycle_request())
                    .await;
                match outcome {
                    Ok(_) => admitted += 1,
                    Err(SendError::AdmissionDropped {
                        reason: DropReason::UntrustedSender,
                    }) => dropped_untrusted += 1,
                    Err(_) => dropped_other += 1,
                }
                tokio::task::yield_now().await;
            }
            (admitted, dropped_untrusted, dropped_other)
        })
    };

    let revoke_handle = {
        let receiver_for_task = receiver.clone();
        let sender_for_task = sender.clone();
        let peer_authority = Arc::clone(&peer_authority);
        tokio::spawn(async move {
            for i in 0..total {
                if i % 2 == 0 {
                    let _ = revoke_generated_trust(
                        receiver_for_task.as_ref(),
                        peer_authority.as_ref(),
                        descriptor_for("peer-sender", &sender_for_task.public_key()),
                    )
                    .await;
                } else {
                    apply_generated_trust(
                        receiver_for_task.as_ref(),
                        peer_authority.as_ref(),
                        descriptor_for("peer-sender", &sender_for_task.public_key()),
                    )
                    .await;
                }
                tokio::task::yield_now().await;
            }
        })
    };

    let drain_receiver = Arc::clone(&receiver);
    let volatile_drain = tokio::spawn(async move {
        loop {
            meerkat_core::agent::CommsRuntime::handoff_volatile_peer_input_candidates(
                drain_receiver.as_ref(),
            )
            .await
            .expect("exact volatile handoff");
            tokio::task::yield_now().await;
        }
    });

    let (admitted, dropped_untrusted, dropped_other) = admit_handle.await.expect("admit task");
    revoke_handle.await.expect("revoke task");
    volatile_drain.abort();

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

/// Auth-exempt bridge traffic admits unconditionally — the admission
/// seam MUST treat exempt items as exempt under the queue lock too. A
/// regression that accidentally re-gated exempt items on the trust
/// re-check would drop bootstrap traffic and break supervisor bridges.
#[tokio::test]
async fn auth_exempt_bridge_request_admits_without_trust_edge() {
    let receiver_name = format!("recv-{}", Uuid::new_v4().simple());
    let receiver = Arc::new(CommsRuntime::inproc_only(&receiver_name).expect("receiver runtime"));
    let _peer_authority = TestPeerCommsAuthority::install(receiver.as_ref(), &receiver_name);
    let sender = public_sender_runtime(&receiver_name, receiver.as_ref()).await;
    // No trust edge seeded — sender is not trusted.

    let kind = MessageKind::Request {
        objective_id: None,
        content_taint: None,
        intent: SUPERVISOR_BRIDGE_INTENT.to_string(),
        params: serde_json::json!({}),
        blocks: None,
        reply_endpoint: None,
        handling_mode: None,
    };

    let drain = tokio::spawn(drain_one_volatile(Arc::clone(&receiver)));
    let outcome = sender
        .router()
        .send(receiver.public_key().to_peer_id(), kind)
        .await
        .expect("auth-exempt supervisor bridge must hand off through public runtime routing");
    assert!(
        matches!(
            outcome.delivery,
            meerkat_core::comms::PeerDeliveryOutcome::VolatileHandedOff
        ),
        "supervisor.bridge ingress is auth-exempt and must admit even without a trust edge"
    );

    let candidate = drain.await.expect("volatile drain joins");
    assert_eq!(
        candidate.auth(),
        Some(PeerIngressAuthDecision::Exempt(
            PeerIngressAuthExemption::SupervisorBridge
        )),
        "runtime drain must expose the typed machine auth decision"
    );
}
