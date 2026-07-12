//! Multi-host mobs phase 4 — cross-host wiring install (§10.4 / §6.2): the
//! four §11 runtime rows T-W1..T-W4.
//!
//! Deterministic two-hosts-in-one-process battery (loopback TCP acceptor,
//! TestClient members, no live providers) — ungated, so it rides the
//! `cargo int` GitHub-CI ratchet. Assertions are machine facts first: the
//! roster wiring projection (`RosterEntry.wired_to`), the route-install
//! obligation projection (`MobHandle::route_installs`, ADJ-P4-9a), the
//! member session's MeerkatMachine-owned `direct_peer_endpoints` trust rows
//! (host side), and the comms runtime's public trust projection (controlling
//! side) — plus REAL delivery differentials: a verified transport ack is a
//! delivery-admission PROOF (the receiving io task acks only after inbox
//! admission through the trust gate).
//!
//! No sleeps: readiness is deadline-polled; bounded-non-delivery attempts
//! carry an explicit attempt bound because the router's ack wait is not
//! test-tunable (DEC-P4H-11).

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use meerkat_core::{HandlingMode, SendReceipt};
use meerkat_mob::MobError;
use meerkat_mob::runtime::bridge_protocol::BridgeRejectionCause;
use meerkat_mob::{AgentIdentity, MobEventKind, SpawnMemberSpec};
#[cfg(feature = "test-support")]
use support::spawn_scripted_member_turn_responder;
use support::{
    ControllingMob, HostFixtureOptions, PeerCommsEndpoint, REAL_COMMS_TEST_LOCK, ScriptedHostPeer,
    create_controlling_mob, member_identity_of, send_peer_text, spawn_host_daemon_fixture,
    spawn_peer_comms_endpoint, spawn_scripted_host_peer,
};

const WAIT: Duration = Duration::from_secs(60);

/// Bound on a send attempt whose ack must NOT arrive (receiver-side drop):
/// real member runtimes are factory-built, so the router ack timeout is not
/// test-tunable — the row bounds the attempt itself and asserts non-delivery
/// at the receiver instead (DEC-P4H-11 differential discipline).
const SEND_ATTEMPT_BOUND: Duration = Duration::from_secs(3);

async fn wait_until<F, Fut>(what: &str, mut predicate: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    tokio::time::timeout(WAIT, async {
        loop {
            if predicate().await {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
}

fn identity(name: &str) -> AgentIdentity {
    AgentIdentity::from(name)
}

async fn spawn_local_worker(controlling: &ControllingMob, member: &str) {
    controlling
        .handle
        .spawn_spec(SpawnMemberSpec::new("worker", member))
        .await
        .unwrap_or_else(|error| panic!("spawn local worker {member}: {error}"));
}

/// The route-install obligation set is at rest: projection complete, no
/// outstanding rows (FLAG-4a seam, ADJ-P4-9a).
async fn route_installs_at_rest(controlling: &ControllingMob) -> bool {
    let installs = controlling
        .handle
        .route_installs()
        .await
        .expect("route installs projection");
    installs.complete && installs.outstanding.is_empty()
}

/// Assert one send attempt is NOT admitted by the receiver within the bound.
/// Acceptable outcomes: an immediate typed sender-side refusal (no trust row
/// for the recipient), an unacked receipt, or the attempt out-waiting the
/// bound (envelope dropped at receiver admission ⇒ no transport ack). A
/// verified ack is the one failure shape.
async fn assert_send_not_admitted(
    from: &Arc<dyn meerkat_core::agent::CommsRuntime>,
    to: meerkat_core::comms::PeerId,
    body: &str,
) {
    match tokio::time::timeout(SEND_ATTEMPT_BOUND, send_peer_text(from, to, body)).await {
        Err(_elapsed) => {}
        Ok(Err(_typed_refusal)) => {}
        Ok(Ok(receipt)) => {
            assert!(
                !matches!(
                    receipt,
                    SendReceipt::PeerMessageSent {
                        delivery: meerkat_core::comms::PeerDeliveryOutcome::Acked
                            | meerkat_core::comms::PeerDeliveryOutcome::HandedOff,
                        ..
                    }
                ),
                "send must not be admitted by the receiver, got {receipt:?}"
            );
        }
    }
}

fn edge_pairs(edges: &[meerkat_mob::MemberWireEdge]) -> BTreeSet<(String, String)> {
    edges
        .iter()
        .map(|edge| (edge.a.as_str().to_string(), edge.b.as_str().to_string()))
        .collect()
}

// ==========================================================================
// T-W1 — mixed-placement batch: both transports installed, both usable
// ==========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn batch_wiring_with_mixed_placements_installs_both_transports() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture =
        spawn_host_daemon_fixture(HostFixtureOptions::named("xhw-t1-host-b").with_member_build())
            .await
            .expect("spawn member-build host fixture");
    let controlling = create_controlling_mob("xhw-t1").await;
    let report = controlling.bind_fixture(&fixture).await;
    let mob_id = controlling.mob_id.to_string();

    spawn_local_worker(&controlling, "a1").await;
    spawn_local_worker(&controlling, "a2").await;
    controlling
        .spawn_placed("worker", "b2", &report.host_id)
        .await
        .expect("b2 materializes on host B");

    // The phase-3 local-only rejection is GONE: a mixed local/placed batch
    // returns Ok and admits both edges as NEW.
    let batch = controlling
        .handle
        .wire_members_batch([("a1", "a2"), ("a1", "b2")])
        .await
        .expect("mixed-placement batch wires without the local-only reject");
    assert!(
        batch.already_wired.is_empty(),
        "both edges are new: {batch:?}"
    );
    assert_eq!(
        edge_pairs(&batch.wired),
        BTreeSet::from([
            ("a1".to_string(), "a2".to_string()),
            ("a1".to_string(), "b2".to_string()),
        ]),
        "the batch report names both normalized edges as newly wired"
    );

    // Wiring-graph machine fact through the public roster projection.
    let a1_entry = controlling
        .handle
        .get_member(&identity("a1"))
        .await
        .expect("get a1")
        .expect("a1 present");
    assert_eq!(
        a1_entry.wired_to,
        BTreeSet::from([identity("a2"), identity("b2")]),
        "a1 is wired to both peers in the machine graph projection"
    );

    // Exactly ONE durable batch commit carrying both edges — the
    // runtime-observable twin of the one-epoch-bump-per-new-edge machine
    // pins (`member_session_bindings.rs` stays the epoch authority).
    let batch_commits: Vec<BTreeSet<(String, String)>> = controlling
        .storage_events
        .replay_all()
        .await
        .expect("replay durable mob events")
        .iter()
        .filter_map(|event| match &event.kind {
            MobEventKind::MembersWiredBatch { edges } => Some(edge_pairs(edges)),
            _ => None,
        })
        .collect();
    assert_eq!(
        batch_commits.len(),
        1,
        "exactly one durable batch wiring commit"
    );
    assert_eq!(
        batch_commits[0],
        edge_pairs(&batch.wired),
        "the durable commit carries exactly the admitted edges"
    );

    // Both install directions realized against the live host: the obligation
    // set is EMPTY after return (fail-closed convergence is observable, and
    // here it is already converged).
    assert!(
        route_installs_at_rest(&controlling).await,
        "no outstanding route-install obligations after a live-host batch"
    );

    // Trust facts. Host side: the member session's MeerkatMachine-owned
    // direct peer set (the exact fact InstallPeerTrust realizes).
    let b2_row = fixture
        .host_binding_record(&mob_id)
        .await
        .materialized
        .get("b2")
        .cloned()
        .expect("b2 materialized row on host B");
    let a1_session = controlling.member_session_id(&identity("a1")).await;
    let a1_runtime = controlling.member_comms_runtime(&a1_session).await;
    let a1_peer = a1_runtime.peer_id().expect("a1 peer id");
    assert!(
        fixture
            .member_trusts_peer(&b2_row.session_id, &a1_peer.to_string())
            .await,
        "b2's machine-gated trust seam holds a1's row"
    );
    let b2_runtime = fixture.member_comms_runtime(&b2_row.session_id).await;
    let b2_peer = b2_runtime.peer_id().expect("b2 peer id");
    assert!(
        controlling
            .local_member_trusts_peer(&a1_session, &b2_peer.to_string())
            .await,
        "a1's runtime holds b2's trust row"
    );

    // REAL delivery, both directions and both transports. A verified ack is
    // receiver-admission proof.
    let receipt = controlling
        .handle
        .send_peer_message(
            identity("a1"),
            identity("b2"),
            "t-w1 a1->b2 over loopback tcp",
            HandlingMode::Queue,
        )
        .await
        .expect("a1 -> b2 delivers over the cross-host transport");
    assert!(
        matches!(
            receipt.delivery,
            meerkat_core::comms::PeerDeliveryOutcome::Acked
        ),
        "a1 -> b2 must be receiver-acked, got {receipt:?}"
    );
    let receipt = send_peer_text(&b2_runtime, a1_peer, "t-w1 b2->a1 back-route")
        .await
        .expect("b2 -> a1 delivers");
    assert!(
        matches!(
            receipt,
            SendReceipt::PeerMessageSent {
                delivery: meerkat_core::comms::PeerDeliveryOutcome::Acked
                    | meerkat_core::comms::PeerDeliveryOutcome::HandedOff,
                ..
            }
        ),
        "b2 -> a1 must be receiver-acked, got {receipt:?}"
    );
    let receipt = controlling
        .handle
        .send_peer_message(
            identity("a1"),
            identity("a2"),
            "t-w1 a1->a2 inproc arm",
            HandlingMode::Queue,
        )
        .await
        .expect("a1 -> a2 delivers on the local arm");
    assert!(
        matches!(
            receipt.delivery,
            meerkat_core::comms::PeerDeliveryOutcome::HandedOff
        ),
        "a1 -> a2 must be handed directly to the in-process inbox, got {receipt:?}"
    );

    fixture.shutdown().await;
}

// ==========================================================================
// T-W2 / T-W3 — scripted partial install + retry drain (shared scenario)
// ==========================================================================

struct ScriptedWiringScenario {
    controlling: ControllingMob,
    scripted: ScriptedHostPeer,
    probe: Arc<PeerCommsEndpoint>,
    host_id: String,
}

/// Build the §6.2 partial-install window: b2 is "materialized" on a scripted
/// host as a REAL probe-held endpoint (ADJ-P4-10), the first
/// `InstallPeerTrust` is scripted to reject `Unavailable`, and the wire
/// commits the edge while leaving the obligation outstanding.
async fn scripted_partial_install_scenario(label: &str) -> ScriptedWiringScenario {
    let scripted = spawn_scripted_host_peer(&format!("{label}-host")).await;
    let probe = Arc::new(spawn_peer_comms_endpoint(&format!("{label}-b2"), true, None).await);
    scripted.script_member_identity("b2", member_identity_of(&probe));
    scripted.bind_member_endpoint("b2", Arc::clone(&probe));

    let controlling = create_controlling_mob(label).await;
    let report = controlling.bind_scripted(&scripted).await;
    spawn_local_worker(&controlling, "a1").await;
    controlling
        .spawn_placed("worker", "b2", &report.host_id)
        .await
        .expect("b2 placed on the scripted host");

    scripted.reject_next_install_peer_trust(
        BridgeRejectionCause::Unavailable,
        "scripted install rejection",
    );
    controlling
        .handle
        .wire(identity("a1"), identity("b2"))
        .await
        .expect("wire commits the edge and records the obligation — fail closed, never fail quiet");

    ScriptedWiringScenario {
        controlling,
        scripted,
        probe,
        host_id: report.host_id,
    }
}

async fn scripted_converged_wiring_scenario(label: &str) -> ScriptedWiringScenario {
    let scenario = scripted_partial_install_scenario(label).await;
    scenario
        .controlling
        .handle
        .drive_route_installs()
        .await
        .expect("retry initial scripted install to convergence");
    assert!(route_installs_at_rest(&scenario.controlling).await);
    scenario
}

#[cfg(feature = "test-support")]
fn assert_actor_fail_stopped(error: &MobError) {
    assert!(
        matches!(
            error,
            MobError::ActorCommandChannelClosed | MobError::ActorReplyChannelClosed
        ),
        "follow-up command must observe the fail-stopped actor, got {error}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn partial_install_leaves_obligation_and_unusable_edge_fails_closed() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let scenario = scripted_partial_install_scenario("xhw-t2").await;
    let ScriptedWiringScenario {
        controlling,
        scripted,
        probe,
        host_id,
    } = &scenario;

    // The edge IS committed — wire/unwire stay the only topology mutators; a
    // failed install never unwinds the graph.
    let a1_entry = controlling
        .handle
        .get_member(&identity("a1"))
        .await
        .expect("get a1")
        .expect("a1 present");
    assert!(
        a1_entry.wired_to.contains(&identity("b2")),
        "the failed install must not unwind the committed edge"
    );

    // The obligation is NAMED by the projection — never a silent Ok.
    let installs = controlling
        .handle
        .route_installs()
        .await
        .expect("route installs projection");
    assert!(
        !installs.complete,
        "an outstanding obligation must mark the projection incomplete"
    );
    assert_eq!(installs.outstanding.len(), 1, "exactly one obligation");
    let obligation = &installs.outstanding[0];
    assert_eq!(
        (obligation.edge_a.as_str(), obligation.edge_b.as_str()),
        ("a1", "b2"),
        "the obligation names the normalized edge"
    );
    assert_eq!(
        obligation.host.0, *host_id,
        "the obligation names the owning host"
    );
    // The scripted host observed exactly one install carrying a1's identity
    // material for b2.
    assert_eq!(scripted.install_peer_trust_count(), 1);
    let payloads = scripted.received_install_peer_trust_payloads();
    assert_eq!(payloads[0].agent_identity, "b2");
    let a1_session = controlling.member_session_id(&identity("a1")).await;
    let a1_runtime = controlling.member_comms_runtime(&a1_session).await;
    let a1_peer = a1_runtime.peer_id().expect("a1 peer id");
    assert_eq!(
        payloads[0].peer.peer_id,
        a1_peer.to_string(),
        "the install descriptor carries a1's canonical peer id"
    );

    // Fail-closed at the receiver: the untrusted direction does not deliver.
    // (The positive half of this differential lands in T-W3 after retry.)
    assert_send_not_admitted(
        &a1_runtime,
        probe.self_descriptor().peer_id,
        "t-w2 must not be admitted",
    )
    .await;
    assert!(
        probe.drain_message_bodies().await.is_empty(),
        "the receiver must not admit the untrusted sender's envelope"
    );

    scripted.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn retry_drains_obligations_idempotently() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let scenario = scripted_partial_install_scenario("xhw-t3").await;
    let ScriptedWiringScenario {
        controlling,
        scripted,
        probe,
        ..
    } = &scenario;

    // The scripted rejection is consumed; the default accept path also
    // applies the descriptor to the probe-held endpoint (ADJ-P4-10), so the
    // retry produces a REAL delivery differential.
    controlling
        .handle
        .drive_route_installs()
        .await
        .expect("drive retries the outstanding install");
    assert!(
        route_installs_at_rest(controlling).await,
        "the drive drains the obligation set"
    );
    assert_eq!(
        scripted.install_peer_trust_count(),
        2,
        "the retry re-sends exactly one install"
    );
    let payloads = scripted.received_install_peer_trust_payloads();
    assert_eq!(
        payloads[0].peer.peer_id, payloads[1].peer.peer_id,
        "idempotency is keyed on the peer identity fact, not a fresh tuple"
    );
    assert_eq!(payloads[0].agent_identity, payloads[1].agent_identity);

    // Second drive: nothing outstanding ⇒ nothing sent — the drain is
    // idempotent.
    controlling
        .handle
        .drive_route_installs()
        .await
        .expect("an idle drive is an idempotent no-op");
    assert_eq!(
        scripted.install_peer_trust_count(),
        2,
        "an idle drive must not re-send installs"
    );
    assert!(route_installs_at_rest(controlling).await);

    // The positive half of T-W2's differential: the same direction now
    // delivers, receiver-acked and observed at the probe.
    let a1_session = controlling.member_session_id(&identity("a1")).await;
    let a1_runtime = controlling.member_comms_runtime(&a1_session).await;
    let receipt = send_peer_text(
        &a1_runtime,
        probe.self_descriptor().peer_id,
        "t-w3-differential",
    )
    .await
    .expect("post-retry send is admitted");
    assert!(
        matches!(
            receipt,
            SendReceipt::PeerMessageSent {
                delivery: meerkat_core::comms::PeerDeliveryOutcome::Acked
                    | meerkat_core::comms::PeerDeliveryOutcome::HandedOff,
                ..
            }
        ),
        "post-retry delivery must be receiver-acked, got {receipt:?}"
    );
    probe
        .wait_for_message_body("t-w3-differential", WAIT)
        .await
        .expect("the differential body lands at the probe");

    scripted.shutdown();
}

// ==========================================================================
// T-W4 — unwire removes trust both ends; subsequent delivery rejected
// ==========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn unwire_removes_trust_and_subsequent_delivery_rejected() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture =
        spawn_host_daemon_fixture(HostFixtureOptions::named("xhw-t4-host-b").with_member_build())
            .await
            .expect("spawn member-build host fixture");
    let controlling = create_controlling_mob("xhw-t4").await;
    let report = controlling.bind_fixture(&fixture).await;
    let mob_id = controlling.mob_id.to_string();

    spawn_local_worker(&controlling, "a1").await;
    controlling
        .spawn_placed("worker", "b2", &report.host_id)
        .await
        .expect("b2 materializes on host B");
    controlling
        .handle
        .wire(identity("a1"), identity("b2"))
        .await
        .expect("cross-host wire converges");
    assert!(
        route_installs_at_rest(&controlling).await,
        "no outstanding obligations after a converged wire"
    );

    let b2_row = fixture
        .host_binding_record(&mob_id)
        .await
        .materialized
        .get("b2")
        .cloned()
        .expect("b2 materialized row on host B");
    let a1_session = controlling.member_session_id(&identity("a1")).await;
    let a1_runtime = controlling.member_comms_runtime(&a1_session).await;
    let a1_peer = a1_runtime.peer_id().expect("a1 peer id");
    let b2_runtime = fixture.member_comms_runtime(&b2_row.session_id).await;
    let b2_peer = b2_runtime.peer_id().expect("b2 peer id");

    // Positive control: the edge is usable in both directions before unwire.
    let receipt = controlling
        .handle
        .send_peer_message(
            identity("a1"),
            identity("b2"),
            "t-w4 positive control",
            HandlingMode::Queue,
        )
        .await
        .expect("a1 -> b2 delivers before the unwire");
    assert!(
        matches!(
            receipt.delivery,
            meerkat_core::comms::PeerDeliveryOutcome::Acked
        ),
        "a1 -> b2 positive control must be receiver-acked, got {receipt:?}"
    );
    let receipt = send_peer_text(&b2_runtime, a1_peer, "t-w4 positive control back")
        .await
        .expect("b2 -> a1 delivers before the unwire");
    assert!(matches!(
        receipt,
        SendReceipt::PeerMessageSent {
            delivery: meerkat_core::comms::PeerDeliveryOutcome::Acked
                | meerkat_core::comms::PeerDeliveryOutcome::HandedOff,
            ..
        }
    ));

    controlling
        .handle
        .unwire(identity("a1"), identity("b2"))
        .await
        .expect("unwire the cross-host edge");

    // Graph fact: the edge is gone.
    let a1_entry = controlling
        .handle
        .get_member(&identity("a1"))
        .await
        .expect("get a1")
        .expect("a1 present");
    assert!(
        !a1_entry.wired_to.contains(&identity("b2")),
        "the unwired edge must leave the machine graph projection"
    );

    // Remove was synchronously ACKed before the durable unwire and never
    // entered the pending Install ledger — the machine is at rest.
    wait_until("route-install obligations to drain to rest", || {
        route_installs_at_rest(&controlling)
    })
    .await;

    // RemovePeerTrust realized both ends.
    wait_until("b2's machine trust row for a1 to be removed", || async {
        !fixture
            .member_trusts_peer(&b2_row.session_id, &a1_peer.to_string())
            .await
    })
    .await;
    assert!(
        !controlling
            .local_member_trusts_peer(&a1_session, &b2_peer.to_string())
            .await,
        "a1 no longer trusts b2 after the unwire"
    );

    // Post-unwire delivery rejected, bounded, both directions: the sender's
    // own router refuses the untrusted recipient (trust-before-send).
    let refused = tokio::time::timeout(
        SEND_ATTEMPT_BOUND,
        send_peer_text(&a1_runtime, b2_peer, "t-w4 refused"),
    )
    .await
    .expect("post-unwire refusal must be bounded");
    assert!(
        refused.is_err(),
        "a1's router must refuse the untrusted recipient, got {refused:?}"
    );
    let refused = tokio::time::timeout(
        SEND_ATTEMPT_BOUND,
        send_peer_text(&b2_runtime, a1_peer, "t-w4 refused back"),
    )
    .await
    .expect("post-unwire refusal must be bounded");
    assert!(
        refused.is_err(),
        "b2's router must refuse the untrusted recipient, got {refused:?}"
    );

    // Idempotent second unwire: Ok, and NO second durable unwire commit —
    // the runtime-observable twin of the machine's no-epoch-bump
    // `UnwireMembersAlreadyAbsent` arm (`member_session_bindings.rs` pins
    // stay the epoch authority).
    controlling
        .handle
        .unwire(identity("a1"), identity("b2"))
        .await
        .expect("second unwire is an idempotent no-op");
    let unwire_commits = controlling
        .storage_events
        .replay_all()
        .await
        .expect("replay durable mob events")
        .iter()
        .filter(|event| matches!(&event.kind, MobEventKind::MembersUnwired { .. }))
        .count();
    assert_eq!(
        unwire_commits, 1,
        "the idempotent unwire must not commit a second durable unwire"
    );

    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn remove_rejection_keeps_edge_wired_and_retry_commits_unwire() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let scenario = scripted_converged_wiring_scenario("xhw-remove-reject").await;
    scenario.scripted.reject_next_remove_peer_trust(
        BridgeRejectionCause::Unavailable,
        "scripted pre-unwire remove rejection",
    );

    let error = scenario
        .controlling
        .handle
        .unwire(identity("a1"), identity("b2"))
        .await
        .expect_err("remote Remove rejection must abort before durable unwire");
    assert!(
        error.to_string().contains("Unavailable")
            || error
                .to_string()
                .contains("scripted pre-unwire remove rejection"),
        "typed bridge rejection surfaces: {error}"
    );
    let a1 = scenario
        .controlling
        .handle
        .get_member(&identity("a1"))
        .await
        .expect("query a1")
        .expect("a1 survives");
    assert!(
        a1.wired_to.contains(&identity("b2")),
        "a rejected pre-commit Remove must leave the durable graph wired"
    );
    assert!(
        scenario
            .controlling
            .storage_events
            .replay_all()
            .await
            .expect("events after rejection")
            .iter()
            .all(|event| !matches!(event.kind, MobEventKind::MembersUnwired { .. })),
        "no durable unwire may publish before every remote Remove ACK"
    );

    scenario
        .controlling
        .handle
        .unwire(identity("a1"), identity("b2"))
        .await
        .expect("retry consumes the scripted rejection and commits");
    let a1 = scenario
        .controlling
        .handle
        .get_member(&identity("a1"))
        .await
        .expect("query a1 after retry")
        .expect("a1 survives");
    assert!(!a1.wired_to.contains(&identity("b2")));
    assert_eq!(
        scenario.scripted.remove_peer_trust_count(),
        2,
        "one rejected preflight plus one successful retry"
    );
    scenario.scripted.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn same_host_second_remove_rejection_reinstalls_first_lane_before_return() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let scripted = spawn_scripted_host_peer("xhw-second-remove-host").await;
    let b2 = Arc::new(spawn_peer_comms_endpoint("xhw-second-remove-b2", true, None).await);
    let c3 = Arc::new(spawn_peer_comms_endpoint("xhw-second-remove-c3", true, None).await);
    scripted.script_member_identity("b2", member_identity_of(&b2));
    scripted.script_member_identity("c3", member_identity_of(&c3));
    scripted.bind_member_endpoint("b2", Arc::clone(&b2));
    scripted.bind_member_endpoint("c3", Arc::clone(&c3));
    let controlling = create_controlling_mob("xhw-second-remove").await;
    let report = controlling.bind_scripted(&scripted).await;
    controlling
        .spawn_placed("worker", "b2", &report.host_id)
        .await
        .expect("b2 placed");
    controlling
        .spawn_placed("worker", "c3", &report.host_id)
        .await
        .expect("c3 placed");
    controlling
        .handle
        .wire(identity("b2"), identity("c3"))
        .await
        .expect("same-host edge converges");
    let installs_before = scripted.install_peer_trust_count();

    // Canonical edge order sends Remove to b2 first, then c3. Rejecting c3
    // proves the already-ACKed b2 removal is compensated before Err returns.
    scripted.reject_remove_peer_trust_for(
        "c3",
        BridgeRejectionCause::Unavailable,
        "reject the second same-host remove lane",
    );
    controlling
        .handle
        .unwire(identity("b2"), identity("c3"))
        .await
        .expect_err("second Remove rejection aborts the unwire transaction");
    let b2_member = controlling
        .handle
        .get_member(&identity("b2"))
        .await
        .expect("query b2")
        .expect("b2 survives");
    assert!(
        b2_member.wired_to.contains(&identity("c3")),
        "graph remains wired after the failed pre-commit transaction"
    );
    let remove_targets = scripted
        .received_remove_peer_trust_payloads()
        .into_iter()
        .map(|payload| payload.agent_identity)
        .collect::<Vec<_>>();
    assert_eq!(remove_targets, vec!["b2", "c3"]);
    assert_eq!(
        scripted.install_peer_trust_count(),
        installs_before + 2,
        "compensation reinstalls both host lanes, including the first removed lane"
    );

    let b2_runtime: Arc<dyn meerkat_core::agent::CommsRuntime> = b2.runtime.clone();
    let c3_runtime: Arc<dyn meerkat_core::agent::CommsRuntime> = c3.runtime.clone();
    let forward = send_peer_text(
        &b2_runtime,
        c3.self_descriptor().peer_id,
        "first removed lane was restored",
    )
    .await
    .expect("b2 still trusts c3 after compensation");
    assert!(matches!(
        forward,
        SendReceipt::PeerMessageSent {
            delivery: meerkat_core::comms::PeerDeliveryOutcome::Acked
                | meerkat_core::comms::PeerDeliveryOutcome::HandedOff,
            ..
        }
    ));
    let reverse = send_peer_text(
        &c3_runtime,
        b2.self_descriptor().peer_id,
        "second lane stayed/restored",
    )
    .await
    .expect("c3 still trusts b2 after compensation");
    assert!(matches!(
        reverse,
        SendReceipt::PeerMessageSent {
            delivery: meerkat_core::comms::PeerDeliveryOutcome::Acked
                | meerkat_core::comms::PeerDeliveryOutcome::HandedOff,
            ..
        }
    ));

    controlling
        .handle
        .unwire(identity("b2"), identity("c3"))
        .await
        .expect("retry commits after the scripted rejection is consumed");
    scripted.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
#[cfg(feature = "test-support")]
async fn members_unwired_no_write_transient_latest_cursor_failure_compensates() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let scenario = scripted_converged_wiring_scenario("xhw-unwire-no-write-transient").await;
    let installs_before = scenario.scripted.install_peer_trust_count();
    scenario
        .controlling
        .storage_events_in_memory
        .fail_reconciliation_latest_cursor_reads_after_next_members_unwired_error(1);
    scenario
        .controlling
        .storage_events_in_memory
        .fail_next_members_unwired_before_append();

    let error = scenario
        .controlling
        .handle
        .unwire(identity("a1"), identity("b2"))
        .await
        .expect_err("proved absence returns the original append failure after compensation");
    assert!(error.to_string().contains("failure before append"));
    let a1 = scenario
        .controlling
        .handle
        .get_member(&identity("a1"))
        .await
        .expect("query a1")
        .expect("a1 survives");
    assert!(a1.wired_to.contains(&identity("b2")));
    assert_eq!(
        scenario.scripted.install_peer_trust_count(),
        installs_before + 1,
        "one transient ceiling-read failure must retry, prove no write, and compensate"
    );
    assert!(route_installs_at_rest(&scenario.controlling).await);
    scenario.scripted.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
#[cfg(feature = "test-support")]
async fn members_unwired_lost_ack_transient_poll_failure_keeps_committed_unwire() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let scenario = scripted_converged_wiring_scenario("xhw-unwire-lost-ack-transient").await;
    let installs_before = scenario.scripted.install_peer_trust_count();
    scenario
        .controlling
        .storage_events_in_memory
        .fail_reconciliation_poll_reads_after_next_members_unwired_error(1);
    scenario
        .controlling
        .storage_events_in_memory
        .fail_next_members_unwired_after_append();

    scenario
        .controlling
        .handle
        .unwire(identity("a1"), identity("b2"))
        .await
        .expect("exact post-floor MembersUnwired occurrence reconciles lost ACK");
    let a1 = scenario
        .controlling
        .handle
        .get_member(&identity("a1"))
        .await
        .expect("query a1")
        .expect("a1 survives");
    assert!(!a1.wired_to.contains(&identity("b2")));
    assert_eq!(
        scenario.scripted.install_peer_trust_count(),
        installs_before,
        "wrote-then-error must not compensate a durably committed unwire with Install"
    );
    assert_eq!(scenario.scripted.remove_peer_trust_count(), 1);
    assert_eq!(
        scenario
            .controlling
            .storage_events
            .replay_all()
            .await
            .expect("replay committed unwire")
            .iter()
            .filter(|event| matches!(event.kind, MobEventKind::MembersUnwired { .. }))
            .count(),
        1,
        "lost acknowledgement still stores exactly one structural carrier"
    );
    scenario.scripted.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
#[cfg(feature = "test-support")]
async fn members_unwired_no_write_persistent_ceiling_outage_fail_stops_then_cold_repairs() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let scenario = scripted_converged_wiring_scenario("xhw-unwire-no-write-outage").await;
    let ScriptedWiringScenario {
        controlling,
        scripted,
        probe,
        host_id: _,
    } = scenario;
    let installs_before = scripted.install_peer_trust_count();
    controlling
        .storage_events_in_memory
        .fail_reconciliation_latest_cursor_reads_after_next_members_unwired_error(2);
    controlling
        .storage_events_in_memory
        .fail_next_members_unwired_before_append();

    let error = controlling
        .handle
        .unwire(identity("a1"), identity("b2"))
        .await
        .expect_err("persistent reconciliation outage must fail-stop");
    assert!(error.to_string().contains("cold replay is required"));
    assert_eq!(
        scripted.install_peer_trust_count(),
        installs_before,
        "an uncertain append must never compensate before cold replay"
    );
    let follow_up = controlling
        .handle
        .wire(identity("a1"), identity("b2"))
        .await
        .expect_err("the fail-stop boundary must reject follow-up work");
    assert_actor_fail_stopped(&follow_up);
    assert_eq!(scripted.install_peer_trust_count(), installs_before);
    assert!(
        controlling
            .storage_events
            .replay_all()
            .await
            .expect("replay no-write outcome")
            .iter()
            .all(|event| !matches!(event.kind, MobEventKind::MembersUnwired { .. })),
        "the injected pre-write failure must leave no durable unwire"
    );

    let controlling = controlling.restart_after_actor_fail_stop().await;
    let a1 = controlling
        .handle
        .get_member(&identity("a1"))
        .await
        .expect("query recovered a1")
        .expect("a1 survives cold replay");
    assert!(
        a1.wired_to.contains(&identity("b2")),
        "cold replay of the no-write outcome restores the durable wired graph"
    );
    controlling
        .handle
        .drive_route_installs()
        .await
        .expect("cold-rederived Install repairs remote trust");
    assert_eq!(scripted.install_peer_trust_count(), installs_before + 1);
    assert!(route_installs_at_rest(&controlling).await);
    let a1_session = controlling.member_session_id(&identity("a1")).await;
    let a1_runtime = controlling.member_comms_runtime(&a1_session).await;
    let receipt = send_peer_text(
        &a1_runtime,
        probe.self_descriptor().peer_id,
        "cold replay repaired the no-write edge",
    )
    .await
    .expect("recovered local and remote trust admit delivery");
    assert!(matches!(
        receipt,
        SendReceipt::PeerMessageSent {
            delivery: meerkat_core::comms::PeerDeliveryOutcome::Acked
                | meerkat_core::comms::PeerDeliveryOutcome::HandedOff,
            ..
        }
    ));
    scripted.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
#[cfg(feature = "test-support")]
async fn fail_stop_joins_inflight_placed_delivery_before_publishing_actor_closure() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let scenario = scripted_converged_wiring_scenario("xhw-fail-stop-delivery-barrier").await;
    let ScriptedWiringScenario {
        controlling,
        scripted,
        probe,
        host_id: _,
    } = scenario;
    let responder = spawn_scripted_member_turn_responder(Arc::clone(&probe));
    // The first request reaches the remote endpoint but is held before member
    // admission and before any reply. Without actor task ownership, the
    // provisioner's timeout path would remain alive after fail-stop and issue
    // its exact resend.
    responder.silence_next_deliveries_before_admission(2);
    let placed = controlling
        .handle
        .member(&identity("b2"))
        .await
        .expect("placed member handle");
    let delivery = tokio::spawn(async move {
        placed
            .send("held across fail-stop", HandlingMode::Queue)
            .await
    });
    wait_until("first placed delivery held before admission", || async {
        responder.received_deliveries().len() == 1
    })
    .await;
    assert_eq!(responder.admitted_delivery_count(), 0);

    controlling
        .storage_events_in_memory
        .fail_reconciliation_latest_cursor_reads_after_next_members_unwired_error(2);
    controlling
        .storage_events_in_memory
        .fail_next_members_unwired_before_append();
    let error = controlling
        .handle
        .unwire(identity("a1"), identity("b2"))
        .await
        .expect_err("persistent reconciliation outage must fail-stop");
    assert!(error.to_string().contains("cold replay is required"));

    // Command-channel closure is the externally observable join barrier. The
    // follow-up cannot return until actor I/O, pumps, and listeners are gone.
    let follow_up = controlling
        .handle
        .wire(identity("a1"), identity("b2"))
        .await
        .expect_err("the fail-stop boundary must reject follow-up work");
    assert_actor_fail_stopped(&follow_up);
    let delivery_error = tokio::time::timeout(Duration::from_secs(1), delivery)
        .await
        .expect("placed delivery task must be canceled before actor closure")
        .expect("delivery caller task joins")
        .expect_err("aborted actor-owned delivery cannot report success");
    assert_actor_fail_stopped(&delivery_error);
    tokio::task::yield_now().await;
    assert_eq!(
        responder.received_deliveries().len(),
        1,
        "joined delivery ownership forbids a post-latch resend"
    );
    assert_eq!(
        responder.admitted_delivery_count(),
        0,
        "no remote input may become durable after fail-stop"
    );

    // A cold replacement may bind immediately once closure is visible; it
    // must not race any old listener/task ownership.
    let controlling = controlling.restart_after_actor_fail_stop().await;
    assert!(
        controlling
            .handle
            .get_member(&identity("b2"))
            .await
            .expect("query cold-recovered b2")
            .is_some()
    );
    responder.shutdown();
    scripted.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
#[cfg(feature = "test-support")]
async fn members_unwired_written_persistent_poll_outage_fail_stops_then_cold_commits() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let scenario = scripted_converged_wiring_scenario("xhw-unwire-written-outage").await;
    let ScriptedWiringScenario {
        controlling,
        scripted,
        probe,
        host_id: _,
    } = scenario;
    let installs_before = scripted.install_peer_trust_count();
    controlling
        .storage_events_in_memory
        .fail_reconciliation_poll_reads_after_next_members_unwired_error(2);
    controlling
        .storage_events_in_memory
        .fail_next_members_unwired_after_append();

    let error = controlling
        .handle
        .unwire(identity("a1"), identity("b2"))
        .await
        .expect_err("persistent post-write reconciliation outage must fail-stop");
    assert!(error.to_string().contains("cold replay is required"));
    assert_eq!(
        scripted.install_peer_trust_count(),
        installs_before,
        "an uncertain wrote-then-error outcome must not compensate"
    );
    let follow_up = controlling
        .handle
        .wire(identity("a1"), identity("b2"))
        .await
        .expect_err("the fail-stop boundary must reject follow-up work");
    assert_actor_fail_stopped(&follow_up);
    assert_eq!(scripted.install_peer_trust_count(), installs_before);
    assert_eq!(
        controlling
            .storage_events
            .replay_all()
            .await
            .expect("replay wrote-then-error outcome")
            .iter()
            .filter(|event| matches!(event.kind, MobEventKind::MembersUnwired { .. }))
            .count(),
        1,
        "the lost acknowledgement still left one durable unwire carrier"
    );

    let controlling = controlling.restart_after_actor_fail_stop().await;
    let a1 = controlling
        .handle
        .get_member(&identity("a1"))
        .await
        .expect("query recovered a1")
        .expect("a1 survives cold replay");
    assert!(
        !a1.wired_to.contains(&identity("b2")),
        "cold replay of the written outcome keeps the edge unwired"
    );
    controlling
        .handle
        .drive_route_installs()
        .await
        .expect("unwired recovery has no Install work");
    assert_eq!(
        scripted.install_peer_trust_count(),
        installs_before,
        "cold replay of committed unwire must not compensate"
    );
    assert!(route_installs_at_rest(&controlling).await);
    let a1_session = controlling.member_session_id(&identity("a1")).await;
    let a1_runtime = controlling.member_comms_runtime(&a1_session).await;
    assert_send_not_admitted(
        &a1_runtime,
        probe.self_descriptor().peer_id,
        "committed unwire remains disconnected after cold replay",
    )
    .await;
    scripted.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn placed_retirement_removes_only_surviving_placed_target_trust() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let scripted = spawn_scripted_host_peer("xhw-retire-survivor-host").await;
    let b2 = Arc::new(spawn_peer_comms_endpoint("xhw-retire-b2", true, None).await);
    let c3 = Arc::new(spawn_peer_comms_endpoint("xhw-retire-c3", true, None).await);
    scripted.script_member_identity("b2", member_identity_of(&b2));
    scripted.script_member_identity("c3", member_identity_of(&c3));
    scripted.bind_member_endpoint("b2", Arc::clone(&b2));
    scripted.bind_member_endpoint("c3", Arc::clone(&c3));

    let controlling = create_controlling_mob("xhw-retire-survivor").await;
    let report = controlling.bind_scripted(&scripted).await;
    controlling
        .spawn_placed("worker", "b2", &report.host_id)
        .await
        .expect("b2 placed");
    controlling
        .spawn_placed("worker", "c3", &report.host_id)
        .await
        .expect("c3 placed");
    controlling
        .handle
        .wire(identity("b2"), identity("c3"))
        .await
        .expect("same-host placed edge converges");

    // If retirement incorrectly targets b2's unavailable runtime, this
    // one-shot rejection fires and deadlocks before ReleaseMember. Survivor
    // c3 must be the only Remove target.
    scripted.reject_remove_peer_trust_for(
        "b2",
        BridgeRejectionCause::Unavailable,
        "retiring runtime intentionally unavailable",
    );
    controlling
        .handle
        .retire(identity("b2"))
        .await
        .expect("survivor cleanup succeeds and exact host release retires b2");
    let removals = scripted.received_remove_peer_trust_payloads();
    assert_eq!(removals.len(), 1, "one surviving placed lane gates retire");
    assert_eq!(removals[0].agent_identity, "c3");
    assert!(
        removals
            .iter()
            .all(|payload| payload.agent_identity != "b2"),
        "the retiring placed target's trust store dies with ReleaseMember"
    );
    assert_eq!(scripted.release_count(), 1, "release remains reachable");
    assert!(
        controlling
            .handle
            .get_member(&identity("c3"))
            .await
            .expect("query survivor")
            .is_some(),
        "survivor remains in the roster"
    );
    let b2_runtime: Arc<dyn meerkat_core::agent::CommsRuntime> = b2.runtime.clone();
    assert_send_not_admitted(
        &b2_runtime,
        c3.self_descriptor().peer_id,
        "retired b2 must no longer be admitted by c3",
    )
    .await;
    scripted.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn placed_retirement_unwire_failure_retains_non_routable_retry_anchor() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let scripted = spawn_scripted_host_peer("xhw-retire-unwire-failure-host").await;
    let b2 = Arc::new(spawn_peer_comms_endpoint("xhw-retire-unwire-failure-b2", true, None).await);
    let c3 = Arc::new(spawn_peer_comms_endpoint("xhw-retire-unwire-failure-c3", true, None).await);
    scripted.script_member_identity("b2", member_identity_of(&b2));
    scripted.script_member_identity("c3", member_identity_of(&c3));
    scripted.bind_member_endpoint("b2", Arc::clone(&b2));
    scripted.bind_member_endpoint("c3", Arc::clone(&c3));

    let controlling = create_controlling_mob("xhw-retire-unwire-failure").await;
    let report = controlling.bind_scripted(&scripted).await;
    controlling
        .spawn_placed("worker", "b2", &report.host_id)
        .await
        .expect("b2 placed");
    controlling
        .spawn_placed("worker", "c3", &report.host_id)
        .await
        .expect("c3 placed");
    controlling
        .handle
        .wire(identity("b2"), identity("c3"))
        .await
        .expect("same-host placed edge converges");

    // Retirement must first publish the machine-owned Retiring marker. The
    // surviving c3 lane then fails its one-shot synchronous Remove, leaving a
    // retry anchor rather than a durable-start/live-running split.
    scripted.reject_remove_peer_trust_for(
        "c3",
        BridgeRejectionCause::Unavailable,
        "injected survivor remove failure",
    );
    let unwire_error = controlling
        .handle
        .retire(identity("b2"))
        .await
        .expect_err("placed edge cleanup failure must surface");
    assert!(
        unwire_error
            .to_string()
            .contains("injected survivor remove failure"),
        "fixture must fail in the post-admission survivor unwire path, got: {unwire_error}"
    );
    assert_eq!(
        controlling
            .storage_events
            .replay_all()
            .await
            .expect("replay failed retirement")
            .iter()
            .filter(|event| matches!(
                &event.kind,
                MobEventKind::MemberRetirementStarted { agent_identity, .. }
                    if agent_identity == &identity("b2")
            ))
            .count(),
        1,
        "fallible unwire must run behind the one durable retirement-start anchor"
    );
    assert_eq!(
        scripted.release_count(),
        0,
        "host release must remain behind surviving-edge cleanup"
    );

    let retained = controlling
        .handle
        .list_members_including_retiring()
        .await
        .into_iter()
        .find(|member| member.agent_identity == identity("b2"))
        .expect("failed retirement retains the exact member anchor");
    assert_eq!(
        retained.status,
        meerkat_mob::MobMemberStatus::Retiring,
        "fallible topology cleanup must observe committed Retiring authority"
    );
    assert!(
        controlling
            .handle
            .list_members()
            .await
            .iter()
            .all(|member| member.agent_identity != identity("b2")),
        "the retained retry anchor must not remain active/routable"
    );
    let retiring_member = controlling
        .handle
        .member(&identity("b2"))
        .await
        .expect("retiring member remains inspectable for status and retry");
    let work_error = retiring_member
        .send("must not route through retiring b2", HandlingMode::Queue)
        .await
        .expect_err("MobMachine must reject new work for a Retiring member");
    assert!(
        matches!(work_error, MobError::MemberNotFound(missing) if missing == identity("b2")),
        "work admission must reject the non-active retirement anchor"
    );

    controlling
        .handle
        .retire(identity("b2"))
        .await
        .expect("one-shot unwire failure retries from the Retiring anchor");
    assert_eq!(
        scripted.release_count(),
        1,
        "retry reaches exact host release"
    );
    assert!(
        controlling
            .handle
            .get_member(&identity("b2"))
            .await
            .expect("query retired member")
            .is_none(),
        "retry completes terminal removal"
    );
    scripted.shutdown();
}
