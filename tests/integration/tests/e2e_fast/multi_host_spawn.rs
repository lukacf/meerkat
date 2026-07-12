//! Multi-host mobs phase 3 — deterministic e2e-fast ratchet row
//! (design-fold-ins §3.4; consolidates S E1 + R T-19 + U T-19 + H T25 into
//! ONE condensed walk — the §10.3 exit gate "§7.3 end-to-end including
//! B2→B21 on Host B" in the lane GitHub CI actually runs).
//!
//! Walk: bind Host B (member-build fixture) → spawn B2 with placement →
//! B2 upcall-spawns B21 (defaults to Host B, through B2's REAL forwarding
//! tool seam) → deliver one input to B21 (turn on Host B) → phase-4 wiring
//! beat: wire B2↔B21 (same-remote-host pair ⇒ ADJ-P4-6 inproc
//! substitution), REAL delivery, unwire, delivery rejected → retire B21
//! (`ReleaseMember`, disposal Archived recorded host-side) → host inventory
//! shows exactly B2 → controlling restart → placement-gated resume →
//! post-restart delivery still serves.
//!
//! Deterministic: TestClient/OneShotToolCallClient, loopback port-0
//! listeners, no live providers.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::support;

use std::time::Duration;

use meerkat_core::{HandlingMode, SendReceipt};
use meerkat_mob::AgentIdentity;
use meerkat_mob::MobSessionService as _;
use support::{
    HostFixtureOptions, OneShotToolCallClient, REAL_COMMS_TEST_LOCK, create_controlling_mob,
    send_peer_text, spawn_host_daemon_fixture,
};

const WAIT: Duration = Duration::from_secs(60);

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

#[tokio::test(flavor = "multi_thread")]
async fn multi_host_spawn_lifecycle_walk() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;

    // B2's first turn upcall-spawns B21 with NO placement arg (defaults to
    // B2's own host, DEC-P3F-8).
    let one_shot = OneShotToolCallClient::new(
        "spawn_member",
        serde_json::json!({ "profile": "worker", "member_id": "b21" }),
    );
    let fixture = spawn_host_daemon_fixture(HostFixtureOptions {
        member_llm_client: Some(one_shot.clone()),
        ..HostFixtureOptions::named("e2e-spawn-host-b").with_member_build()
    })
    .await
    .expect("spawn member-build host fixture");
    let controlling = create_controlling_mob("e2e-multi-host-spawn").await;
    let report = controlling.bind_fixture(&fixture).await;
    let mob_id = controlling.mob_id.to_string();

    // --- spawn B2 with placement -----------------------------------------
    controlling
        .spawn_placed("worker", "b2", &report.host_id)
        .await
        .expect("B2 materializes on Host B");
    let b2_row = fixture
        .host_binding_record(&mob_id)
        .await
        .materialized
        .get("b2")
        .cloned()
        .expect("B2 materialized row on Host B");
    assert!(
        fixture.member_session_exists(&b2_row.session_id).await,
        "B2's session lives on Host B"
    );

    // --- B2 upcall-spawns B21 (default placement = Host B) ----------------
    let b2 = controlling
        .handle
        .member(&AgentIdentity::from("b2"))
        .await
        .expect("b2 handle");
    b2.send("spawn your helper", HandlingMode::Queue)
        .await
        .expect("B2 turn delivers");
    wait_until("B21 to commit via the upcall", || async {
        controlling
            .handle
            .list_members()
            .await
            .iter()
            .any(|entry| entry.agent_identity == "b21")
    })
    .await;
    assert!(one_shot.fired(), "the upcall rode B2's tool seam");
    let _b21_row = fixture
        .host_binding_record(&mob_id)
        .await
        .materialized
        .get("b21")
        .cloned()
        .expect("B21 defaulted to Host B");

    // Roster shows both members.
    let members = controlling.handle.list_members().await;
    for identity in ["b2", "b21"] {
        assert!(
            members.iter().any(|entry| entry.agent_identity == identity),
            "{identity} committed, roster {members:?}"
        );
    }

    // --- deliver one input to B21 (turn on Host B) ------------------------
    let b21 = controlling
        .handle
        .member(&AgentIdentity::from("b21"))
        .await
        .expect("b21 handle");
    b21.send("hello b21", HandlingMode::Queue)
        .await
        .expect("B21 turn runs on Host B");

    // --- phase-4 wiring beat: wire B2↔B21 → deliver → unwire → reject -----
    // Both endpoints are placed on Host B, so the host's serve arm
    // substitutes local inproc peer material (ADJ-P4-6) and the edge is
    // immediately usable with no outstanding obligations.
    controlling
        .handle
        .wire(AgentIdentity::from("b2"), AgentIdentity::from("b21"))
        .await
        .expect("wire of two placed members converges");
    let installs = controlling
        .handle
        .route_installs()
        .await
        .expect("route installs projection");
    assert!(
        installs.complete && installs.outstanding.is_empty(),
        "no outstanding route-install obligations after a live-host wire: {installs:?}"
    );
    let b21_row = fixture
        .host_binding_record(&mob_id)
        .await
        .materialized
        .get("b21")
        .cloned()
        .expect("B21 materialized row on Host B");
    let b2_runtime = fixture.member_comms_runtime(&b2_row.session_id).await;
    let b21_runtime = fixture.member_comms_runtime(&b21_row.session_id).await;
    let b2_peer = b2_runtime.peer_id().expect("b2 peer id");
    let b21_peer = b21_runtime.peer_id().expect("b21 peer id");
    assert!(
        fixture
            .member_trusts_peer(&b21_row.session_id, &b2_peer.to_string())
            .await,
        "B21's machine-gated trust seam holds B2's row"
    );
    // REAL delivery over the substituted same-host route: a verified ack is
    // receiver-admission proof.
    let receipt = send_peer_text(&b2_runtime, b21_peer, "phase-4 wiring beat")
        .await
        .expect("B2 -> B21 delivers over the wired edge");
    assert!(
        matches!(
            receipt,
            SendReceipt::PeerMessageSent {
                delivery: meerkat_core::comms::PeerDeliveryOutcome::Acked
                    | meerkat_core::comms::PeerDeliveryOutcome::HandedOff,
                ..
            }
        ),
        "B2 -> B21 must be receiver-acked, got {receipt:?}"
    );
    // Unwire: symmetric trust removal, then delivery is refused at the
    // sender's own router (trust-before-send).
    controlling
        .handle
        .unwire(AgentIdentity::from("b2"), AgentIdentity::from("b21"))
        .await
        .expect("unwire of the placed pair converges");
    wait_until("route-install obligations to drain to rest", || async {
        let installs = controlling
            .handle
            .route_installs()
            .await
            .expect("route installs projection");
        installs.complete && installs.outstanding.is_empty()
    })
    .await;
    wait_until("B21's trust row for B2 to be removed", || async {
        !fixture
            .member_trusts_peer(&b21_row.session_id, &b2_peer.to_string())
            .await
    })
    .await;
    let refused = tokio::time::timeout(
        Duration::from_secs(5),
        send_peer_text(&b2_runtime, b21_peer, "phase-4 post-unwire"),
    )
    .await
    .expect("post-unwire refusal must be bounded");
    assert!(
        refused.is_err(),
        "B2's router must refuse the unwired recipient, got {refused:?}"
    );

    // --- retire B21 via ReleaseMember (Archived recorded) -----------------
    controlling
        .handle
        .retire(AgentIdentity::from("b21"))
        .await
        .expect("remote retire of B21 converges");
    let record = fixture.host_binding_record(&mob_id).await;
    assert!(
        record.released.contains_key("b21"),
        "B21's release-dedup row recorded on Host B"
    );
    assert_eq!(
        record.materialized.keys().collect::<Vec<_>>(),
        vec!["b2"],
        "host inventory shows exactly B2 after the retire"
    );
    assert!(
        !controlling
            .handle
            .list_members()
            .await
            .iter()
            .any(|entry| entry.agent_identity == "b21"),
        "B21 left the roster"
    );

    // --- controlling restart: placement-gated resume (R T-19) -------------
    let controlling = controlling.restart().await;
    let b2_session = meerkat_core::SessionId::parse(&b2_row.session_id).expect("session id");
    assert!(
        controlling
            .service
            .load_persisted_session(&b2_session)
            .await
            .expect("controlling read")
            .is_none(),
        "the resume recreate loop must SKIP the placed member (gotcha 12)"
    );
    assert!(
        controlling
            .handle
            .list_members()
            .await
            .iter()
            .any(|entry| entry.agent_identity == "b2"),
        "B2 survives the controlling restart in the roster"
    );
    // Post-restart delivery still serves through Host B.
    let b2 = controlling
        .handle
        .member(&AgentIdentity::from("b2"))
        .await
        .expect("post-restart b2 handle");
    tokio::time::timeout(WAIT, b2.send("post-restart hello", HandlingMode::Queue))
        .await
        .expect("delivery does not hang")
        .expect("B2 serves after the controlling restart");

    // Host truth unchanged by the restart: same tuple, same session.
    let after = fixture
        .host_binding_record(&mob_id)
        .await
        .materialized
        .get("b2")
        .cloned()
        .expect("B2 row survives");
    assert_eq!(
        (after.generation, after.fence_token, after.session_id),
        (b2_row.generation, b2_row.fence_token, b2_row.session_id),
        "the controlling restart never advances host-recorded tuples"
    );

    fixture.shutdown().await;
}
