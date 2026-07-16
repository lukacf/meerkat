//! Multi-host mobs phase 2 — deterministic e2e-fast row (design §W4.5):
//! the full two-fixture ceremony walk. Two host-daemon fixtures ("two
//! MeerkatMachines + two acceptors over loopback", plan §11) bind to ONE
//! controlling mob; the test asserts the machine host facts for both hosts
//! and a demux round-trip through each acceptor. Deterministic: TestClient,
//! loopback port-0 listeners, no LLM turns, no members spawned.
//!
//! TDD-first: compiles once Lanes W0..W3 land. The fixture family is shared
//! with the meerkat-mob integration tests via a cross-crate `#[path]`
//! include of `meerkat-mob/tests/support/mod.rs`, composed ONCE at the
//! e2e_fast_lane binary root (single composition, no duplicate — design
//! §W4.2).

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::support;

use std::time::Duration;

use meerkat_core::agent::CommsRuntime as _;
use meerkat_core::comms::TrustedPeerDescriptor;
use meerkat_core::{CommsCommand, HandlingMode, PeerRoute};
use meerkat_mob::runtime::bridge_protocol::BridgeReply;
use meerkat_mob::store::MobHostBindPhaseRecord;
use support::{
    HostFixtureOptions, REAL_COMMS_TEST_LOCK, create_controlling_mob, descriptor_to_bind_request,
    raw_bind_host_command, spawn_host_daemon_fixture, spawn_peer_comms_endpoint,
};

const REPLY_TIMEOUT: Duration = Duration::from_secs(10);

#[tokio::test(flavor = "multi_thread")]
async fn two_hosts_bind_to_one_mob_and_demux_round_trips() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;

    // Host A advertises a live endpoint; host B is live-incapable — the two
    // capability shapes must land as distinct machine facts (DL5: absence is
    // the fact, no boolean shadow).
    let host_a = spawn_host_daemon_fixture(HostFixtureOptions {
        live_endpoint: Some("wss://host-a.test.invalid/live".to_string()),
        ..HostFixtureOptions::named("e2e-host-a")
    })
    .await
    .expect("spawn host A");
    let host_b = spawn_host_daemon_fixture(HostFixtureOptions::named("e2e-host-b"))
        .await
        .expect("spawn host B");

    let controlling = create_controlling_mob("e2e-two-hosts").await;

    // Full ceremony against BOTH hosts (descriptor → BindHost → capabilities
    // → CommitHostBind → durable record).
    let report_a = controlling
        .handle
        .bind_host(descriptor_to_bind_request(&host_a.current_descriptor()))
        .await
        .expect("bind host A");
    let report_b = controlling
        .handle
        .bind_host(descriptor_to_bind_request(&host_b.current_descriptor()))
        .await
        .expect("bind host B");
    assert_ne!(report_a.host_id, report_b.host_id);

    // Committed host facts, asserted through the two PUBLIC carriers of the
    // CommitHostBind facts: the `HostBindReport` capability projection and
    // the FLAG-3 durable `MobHostAuthorityRecord` per bound host. (MobHandle
    // exposes no public machine-state read; record↔machine-map equality is
    // pinned in-crate by
    // meerkat-mob/src/runtime/tests.rs::test_bind_host_commits_machine_facts_and_persists_record.)
    for report in [&report_a, &report_b] {
        assert!(
            !report.capabilities.engine_version.is_empty(),
            "engine_version is a mandatory capability fact for host {}",
            report.host_id
        );
        assert!(report.capabilities.durable_sessions);
    }
    assert_eq!(
        report_a.capabilities.live_endpoint.as_deref(),
        Some("wss://host-a.test.invalid/live"),
        "host A advertised a live endpoint"
    );
    assert_eq!(
        report_b.capabilities.live_endpoint, None,
        "host B is live-incapable — absence is the fact (DL5)"
    );

    // Durable records: one MobHostAuthorityRecord per bound host (FLAG-3).
    let records = controlling
        .storage_metadata
        .list_mob_host_authorities(&controlling.mob_id)
        .await
        .expect("host authority records");
    assert_eq!(records.len(), 2);
    for report in [&report_a, &report_b] {
        let record = records
            .iter()
            .find(|record| record.host_id == report.host_id)
            .unwrap_or_else(|| panic!("missing host authority record for {}", report.host_id));
        assert_eq!(record.bind_phase, MobHostBindPhaseRecord::Bound);
        assert_eq!(record.authority_epoch, report.epoch);
        assert!(
            !record.endpoint.is_empty(),
            "record carries the host endpoint"
        );
        assert!(
            !record.capabilities.engine_version.is_empty(),
            "record carries the declared capability facts"
        );
        assert_eq!(
            record.live_endpoint, report.capabilities.live_endpoint,
            "record mirrors the advertised live-endpoint fact (DL5: absence is the fact)"
        );
    }

    // Demux round-trip through each acceptor: a second raw supervisor binds
    // a probe mob against EACH host with that host's CURRENT descriptor. The
    // request envelope is addressed to the host identity behind the acceptor
    // (acked by that identity's keypair — the sender's router accepts the
    // ack only when `ack.from == sent_to`) and the typed reply names that
    // host's canonical peer id: full per-host request/response demux.
    let sender = spawn_peer_comms_endpoint("e2e-demux-sender", true, None).await;
    for (fixture, report) in [(&host_a, &report_a), (&host_b, &report_b)] {
        let host_descriptor = fixture.host_peer_descriptor();
        sender.trust(host_descriptor.clone()).await;
        let descriptor = fixture.current_descriptor();
        let command = raw_bind_host_command(&sender, "e2e-demux-probe", &descriptor, 1);
        let reply = sender
            .send_bridge_command_raw(&host_descriptor, &command, REPLY_TIMEOUT)
            .await
            .expect("demuxed bind round trip completes");
        let BridgeReply::BindHost(response) = &reply else {
            panic!("expected a BindHost reply from the addressed host, got {reply:?}");
        };
        assert_eq!(
            response.host_peer_id, report.host_id,
            "the reply names the ADDRESSED host's canonical identity"
        );
    }

    // Misaddressed at host A's acceptor: an unregistered identity is
    // rejected without an ack — the sender sees the peer as offline.
    let stranger = meerkat_comms::Keypair::generate();
    let misaddressed = TrustedPeerDescriptor::unsigned_with_pubkey(
        "e2e-demux-stranger",
        stranger.public_key().to_peer_id().to_string(),
        *stranger.public_key().as_bytes(),
        host_a.advertised_address(),
    )
    .expect("stranger descriptor");
    sender.trust(misaddressed.clone()).await;
    let refused = sender
        .runtime
        .send(CommsCommand::PeerMessage {
            content_taint: None,
            to: PeerRoute::with_display_name(misaddressed.peer_id, misaddressed.name.clone()),
            body: "never-delivered".to_string(),
            blocks: None,
            handling_mode: HandlingMode::Queue,
            objective_id: None,
        })
        .await;
    assert!(
        refused.is_err(),
        "misaddressed envelope must not be acked: {refused:?}"
    );

    host_a.shutdown().await;
    host_b.shutdown().await;
}
