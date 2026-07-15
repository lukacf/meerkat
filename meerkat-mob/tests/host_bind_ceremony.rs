//! Multi-host mobs phase 2 — §7.2 host bind ceremony, end-to-end at the
//! harness level (design §W4.1 ceremony rows + §W4.3 failure matrix +
//! the §11 mixed-demux row).
//!
//! TDD-first: written against the Lane W0..W3 shapes; compiles once those
//! lanes land. Covers, over the two-hosts-in-one-process fixture family
//! (`tests/support/mod.rs`):
//!   * full bind ceremony: descriptor → `MobHandle::bind_host` → host-side
//!     `MobHostBindingAuthority` accept → capabilities capture →
//!     `CommitHostBind` machine facts → durable records both sides →
//!     descriptor re-mint (token single-use, DEC-P2-4);
//!   * token single-use across mobs (A14 multi-tenant daemon);
//!   * sender-mismatch / address-mismatch observation assembly, wire-round-
//!     tripped (raw doctored payloads; templates comms_drain.rs:6497/:6600);
//!   * the FLAG-2 rebind ladder: idempotent replay ack at the recorded
//!     epoch, StaleSupervisor below it, accept + capability re-declaration
//!     above it, sender guard, NotBound for unbound mobs;
//!   * the consumed-token crash window: exact bound replay authenticates the
//!     recorded supervisor/epoch/generation/sender/address tuple and does not
//!     require a refreshed descriptor after T0 was consumed into T1;
//!   * `revoke_host` clearing machine facts + durable record, and a fresh
//!     re-bind succeeding after revoke;
//!   * mixed demux on one acceptor (host + two member identities): routing
//!     by `to`, per-identity ack signing (the sender-side router
//!     `ack.from == sent_to` check IS the assertion), misaddressed and
//!     registered-then-removed rejects, owner-gated registry mutation;
//!   * controlling-side ceremony failure matrix against a scripted host:
//!     garbled reply / reply peer-id mismatch ⇒ no `CommitHostBind`, live
//!     fail-stop, then cold exact retry; injected bind failure ⇒ bind window
//!     stays open, live retry idempotent (DEC-P2-10).
//!
//! Controlling-side committed facts are asserted through their PUBLIC
//! carriers — `HostBindReport` and the FLAG-3 durable
//! `MobHostAuthorityRecord` (written/deleted only under transition
//! witnesses). MobHandle exposes no public machine-state read; the
//! machine-map↔record/report equalities are pinned in-crate by the W3 rows
//! in `meerkat-mob/src/runtime/tests.rs`.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::any::Any;
use std::sync::Arc;
use std::time::Duration;

use meerkat_comms::{HostAcceptorConfig, HostAcceptorIdentityRegistry, spawn_host_acceptor};
use meerkat_core::agent::CommsRuntime as _;
use meerkat_core::comms::TrustedPeerDescriptor;
use meerkat_core::{CommsCommand, HandlingMode, PeerRoute, SendReceipt};
use meerkat_mob::machines::mob_host_binding_authority::{
    MobHostBindingAuthorityAuthority, MobHostBindingAuthorityEffect, MobHostBindingAuthorityInput,
    MobHostBindingAuthorityMutator,
};
use meerkat_mob::runtime::bridge_protocol::{
    BridgeBootstrapToken, BridgeCommand, BridgeEventCursor, BridgeHostRebindPayload,
    BridgeHostRevokePayload, BridgeProtocolVersion, BridgeRejectionCause, BridgeReply,
    MaterializeLaunchMode,
};
use meerkat_mob::runtime::host_actor::{MobHostBindingRecord, MobHostRevocationReceipt};
use meerkat_mob::store::MobHostBindPhaseRecord;
use support::{
    HostFixtureOptions, HostPersistenceAckLossFault, REAL_COMMS_TEST_LOCK, bind_then_materialize,
    create_controlling_mob, descriptor_to_bind_request, member_descriptor_from_ack,
    member_incarnation_from_ack, raw_bind_host_command, raw_host_status_command,
    raw_poll_member_events_command, sample_materialize_payload, sample_portable_member_spec,
    spawn_host_daemon_fixture, spawn_peer_comms_endpoint, spawn_scripted_host_peer,
    supervisor_spec_for_endpoint,
};

const REPLY_TIMEOUT: Duration = Duration::from_secs(10);

// ===========================================================================
// Full ceremony walk (§11: "full bind ceremony over the two-host fixture")
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn full_bind_ceremony_records_host_facts_and_reminted_descriptor() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture = spawn_host_daemon_fixture(HostFixtureOptions {
        live_endpoint: Some("wss://host-b.test.invalid/live".to_string()),
        ..HostFixtureOptions::named("ceremony-host-b")
    })
    .await
    .expect("spawn host daemon fixture");
    let controlling = create_controlling_mob("ceremony-full").await;

    let d0 = fixture.current_descriptor();
    let request = descriptor_to_bind_request(&d0);
    let host_peer_id = request.expected_peer_id.to_string();

    let report = controlling
        .handle
        .bind_host(request)
        .await
        .expect("bind_host ceremony must succeed");
    assert_eq!(
        report.host_id, host_peer_id,
        "the host id is the canonical host peer id from the descriptor identity"
    );

    // Controlling-side capability capture (design §W4.1 capability-capture
    // row). `HostBindReport.capabilities` is the domain projection of the
    // CommitHostBind machine facts; the machine-map↔report equality itself is
    // pinned in-crate (runtime/tests.rs
    // test_bind_host_commits_machine_facts_and_persists_record — MobHandle
    // exposes no public machine-state read).
    assert!(
        report.capabilities.durable_sessions,
        "the daemon's durable-sessions fact must land in the captured capabilities"
    );
    assert!(
        report
            .capabilities
            .resolvable_providers
            .contains("anthropic"),
        "the injected presence-probe set must land in the captured capabilities"
    );
    assert!(
        !report.capabilities.engine_version.is_empty(),
        "engine_version is a mandatory capability fact (R8/§15)"
    );
    assert_eq!(
        report.capabilities.live_endpoint.as_deref(),
        Some("wss://host-b.test.invalid/live"),
        "advertised live endpoint captured as a bind fact (DL5)"
    );

    // Host-side durable record (R8: one row per mob in
    // runtime_mob_host_bindings; the record never contains the token).
    let rows = fixture
        .store
        .list_mob_host_bindings()
        .await
        .expect("list host-side binding rows");
    assert_eq!(rows.len(), 1, "exactly one binding row for the bound mob");
    let (row_mob_id, row_bytes) = &rows[0];
    assert_eq!(row_mob_id, &controlling.mob_id.to_string());
    let row_text = String::from_utf8_lossy(row_bytes);
    assert!(
        !row_text.contains(d0.bootstrap_token.as_str()),
        "the bootstrap token must never be persisted (D3/DEC-P2-4)"
    );

    // Controlling-side durable record (FLAG-3(a): MobHostAuthorityRecord
    // written on CommitHostBind under the transition witness — this is the
    // public carrier of the committed machine host facts).
    let records = controlling
        .storage_metadata
        .list_mob_host_authorities(&controlling.mob_id)
        .await
        .expect("list controlling-side host authority records");
    assert_eq!(
        records.len(),
        1,
        "one MobHostAuthorityRecord per bound host"
    );
    assert_eq!(records[0].host_id, host_peer_id);
    assert_eq!(records[0].bind_phase, MobHostBindPhaseRecord::Bound);
    assert_eq!(
        records[0].authority_epoch, report.epoch,
        "the record carries the supervisor authority epoch offered on BindHost (DEC-P2-9)"
    );
    assert_eq!(
        records[0].endpoint, d0.address,
        "the record carries the descriptor-advertised host endpoint"
    );
    assert!(records[0].capabilities.durable_sessions);
    assert!(
        records[0]
            .capabilities
            .resolvable_providers
            .contains("anthropic")
    );
    assert_eq!(
        records[0].live_endpoint.as_deref(),
        Some("wss://host-b.test.invalid/live")
    );

    // Descriptor re-mint: token consumed on HostBindAccepted, slot rewritten
    // with a fresh token at the same address (DEC-P2-4).
    let d1 = fixture.current_descriptor();
    assert_ne!(
        d1.bootstrap_token, d0.bootstrap_token,
        "the one-time token must be consumed + re-minted on successful bind"
    );
    assert_eq!(d1.address, d0.address);

    fixture.shutdown().await;
}

// ===========================================================================
// Token single-use across mobs (A14: one daemon, many mobs)
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn bootstrap_token_is_single_use_and_reminted_descriptor_binds_second_mob() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture = spawn_host_daemon_fixture(HostFixtureOptions::named("single-use-host"))
        .await
        .expect("spawn host daemon fixture");
    let mob_a = create_controlling_mob("single-use-a").await;
    let mob_b = create_controlling_mob("single-use-b").await;

    let d0 = fixture.current_descriptor();
    mob_a
        .handle
        .bind_host(descriptor_to_bind_request(&d0))
        .await
        .expect("first bind consumes the token");

    // REPLAY of the consumed token for a second mob: the host observes
    // token_valid = false on an unbound mob ⇒ InvalidBootstrapToken,
    // wire-round-tripped into the caller's typed error.
    let stale = mob_b
        .handle
        .bind_host(descriptor_to_bind_request(&d0))
        .await
        .expect_err("a consumed bootstrap token must not bind a second mob");
    assert!(
        format!("{stale:?}").contains("InvalidBootstrapToken"),
        "typed cause must surface, got {stale:?}"
    );
    let rows = fixture
        .store
        .list_mob_host_bindings()
        .await
        .expect("list binding rows");
    assert_eq!(rows.len(), 1, "the rejected bind must not persist a row");

    // Fresh re-minted descriptor binds the second mob: the daemon stays
    // bindable without restart (A14), and the controlling machine's bind
    // window retries idempotently after the typed failure (DEC-P2-10).
    let d1 = fixture.current_descriptor();
    assert_ne!(d1.bootstrap_token, d0.bootstrap_token);
    mob_b
        .handle
        .bind_host(descriptor_to_bind_request(&d1))
        .await
        .expect("fresh descriptor binds the second mob");
    let rows = fixture
        .store
        .list_mob_host_bindings()
        .await
        .expect("list binding rows");
    assert_eq!(rows.len(), 2, "one binding row per bound mob (A14)");

    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn exact_bound_replay_accepts_original_consumed_token_without_descriptor_refresh() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture = spawn_host_daemon_fixture(HostFixtureOptions::named("ack-loss-token-host"))
        .await
        .expect("spawn host daemon fixture");
    let supervisor = spawn_peer_comms_endpoint("ack-loss-token-supervisor", true, None).await;
    supervisor.trust(fixture.host_peer_descriptor()).await;

    let d0 = fixture.current_descriptor();
    let bind = raw_bind_host_command(&supervisor, "mob-ack-loss-token", &d0, 1);
    let first = supervisor
        .send_bridge_command_raw(&fixture.host_peer_descriptor(), &bind, REPLY_TIMEOUT)
        .await
        .expect("fresh bind reply");
    assert!(matches!(first, BridgeReply::BindHost(_)));
    let d1 = fixture.current_descriptor();
    assert_ne!(d1.bootstrap_token, d0.bootstrap_token);

    // Simulate loss after the host's durable accept: the controller retries
    // its byte-identical original T0 request, without re-reading D1.
    let replay = supervisor
        .send_bridge_command_raw(&fixture.host_peer_descriptor(), &bind, REPLY_TIMEOUT)
        .await
        .expect("exact consumed-token replay reply");
    assert!(
        matches!(replay, BridgeReply::BindHost(_)),
        "exact recorded authority must replay despite consumed T0, got {replay:?}",
    );
    assert_eq!(
        fixture.current_descriptor().bootstrap_token,
        d1.bootstrap_token,
        "replay must not consume or remint the current token",
    );

    fixture.shutdown().await;
}

// ===========================================================================
// Sender / address mismatch — raw doctored payloads, wire-round-tripped
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn bind_host_rejects_sender_mismatch_without_burning_the_token() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture = spawn_host_daemon_fixture(HostFixtureOptions::named("sender-mismatch-host"))
        .await
        .expect("spawn host daemon fixture");
    let probe = spawn_peer_comms_endpoint("sender-mismatch-probe", true, None).await;
    probe.trust(fixture.host_peer_descriptor()).await;

    let d0 = fixture.current_descriptor();
    let mut command = raw_bind_host_command(&probe, "mob-sender-mismatch", &d0, 1);
    // Same display name, different canonical sender: the payload's supervisor
    // identity names a key that is NOT the envelope signer (the
    // `validate_bind_request_rejects_same_display_name_different_canonical_sender`
    // shape transplanted to the host responder).
    if let BridgeCommand::BindHost(payload) = &mut command {
        let victim = meerkat_comms::Keypair::generate();
        payload.supervisor.pubkey = *victim.public_key().as_bytes();
        payload.supervisor.peer_id = victim.public_key().to_peer_id().to_string();
    } else {
        panic!("raw_bind_host_command must build BindHost");
    }
    let reply = probe
        .send_bridge_command_raw(&fixture.host_peer_descriptor(), &command, REPLY_TIMEOUT)
        .await
        .expect("mismatched bind must produce a typed reply, not a drop");
    assert!(
        matches!(
            reply,
            BridgeReply::Rejected {
                cause: BridgeRejectionCause::SenderMismatch,
                ..
            }
        ),
        "expected SenderMismatch, got {reply:?}"
    );
    assert!(
        fixture
            .store
            .list_mob_host_bindings()
            .await
            .expect("list binding rows")
            .is_empty(),
        "a rejected bind must not persist"
    );

    // The reject consumed nothing: the SAME descriptor still binds honestly.
    let honest = raw_bind_host_command(&probe, "mob-sender-mismatch", &d0, 1);
    let reply = probe
        .send_bridge_command_raw(&fixture.host_peer_descriptor(), &honest, REPLY_TIMEOUT)
        .await
        .expect("honest bind reply");
    assert!(
        matches!(reply, BridgeReply::BindHost(_)),
        "honest bind after a reject must succeed (token unburned), got {reply:?}"
    );

    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn bind_host_rejects_address_mismatch_without_burning_the_token() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture = spawn_host_daemon_fixture(HostFixtureOptions::named("address-mismatch-host"))
        .await
        .expect("spawn host daemon fixture");
    let probe = spawn_peer_comms_endpoint("address-mismatch-probe", true, None).await;
    probe.trust(fixture.host_peer_descriptor()).await;

    let d0 = fixture.current_descriptor();
    let mut command = raw_bind_host_command(&probe, "mob-address-mismatch", &d0, 1);
    if let BridgeCommand::BindHost(payload) = &mut command {
        payload.expected_address = "tcp://127.0.0.1:1".to_string();
    } else {
        panic!("raw_bind_host_command must build BindHost");
    }
    let reply = probe
        .send_bridge_command_raw(&fixture.host_peer_descriptor(), &command, REPLY_TIMEOUT)
        .await
        .expect("mismatched bind must produce a typed reply");
    assert!(
        matches!(
            reply,
            BridgeReply::Rejected {
                cause: BridgeRejectionCause::AddressMismatch,
                ..
            }
        ),
        "expected AddressMismatch, got {reply:?}"
    );

    let honest = raw_bind_host_command(&probe, "mob-address-mismatch", &d0, 1);
    let reply = probe
        .send_bridge_command_raw(&fixture.host_peer_descriptor(), &honest, REPLY_TIMEOUT)
        .await
        .expect("honest bind reply");
    assert!(
        matches!(reply, BridgeReply::BindHost(_)),
        "honest bind after a reject must succeed, got {reply:?}"
    );

    fixture.shutdown().await;
}

// ===========================================================================
// FLAG-2 rebind ladder: idempotent replay ack + strict monotonicity
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn rebind_replays_idempotently_at_recorded_epoch_and_advances_strictly() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture = spawn_host_daemon_fixture(HostFixtureOptions::named("rebind-ladder-host"))
        .await
        .expect("spawn host daemon fixture");
    let supervisor = spawn_peer_comms_endpoint("rebind-supervisor", true, None).await;
    supervisor.trust(fixture.host_peer_descriptor()).await;

    let mob_id = "mob-rebind-ladder";
    let d0 = fixture.current_descriptor();
    let bind = raw_bind_host_command(&supervisor, mob_id, &d0, 3);
    let reply = supervisor
        .send_bridge_command_raw(&fixture.host_peer_descriptor(), &bind, REPLY_TIMEOUT)
        .await
        .expect("bind reply");
    assert!(matches!(reply, BridgeReply::BindHost(_)), "got {reply:?}");

    let rebind = |epoch: u64, sender: &support::PeerCommsEndpoint, mob: &str| {
        BridgeCommand::RebindHost(BridgeHostRebindPayload {
            supervisor: supervisor_spec_for_endpoint(sender, mob),
            epoch,
            binding_generation: 1,
            protocol_version: BridgeProtocolVersion::V4,
            mob_id: mob.to_string(),
            required_capabilities: Default::default(),
        })
    };

    // FLAG-2(ii): rebind at the RECORDED epoch from the recorded supervisor
    // is an idempotent replay ack — the rotation retry probe can converge.
    let replay = supervisor
        .send_bridge_command_raw(
            &fixture.host_peer_descriptor(),
            &rebind(3, &supervisor, mob_id),
            REPLY_TIMEOUT,
        )
        .await
        .expect("replay rebind reply");
    let BridgeReply::HostRebound(rebound) = &replay else {
        panic!("rebind at the recorded epoch must idempotently ack, got {replay:?}");
    };
    assert!(
        !rebound.capabilities.engine_version.is_empty(),
        "rebind re-declares capabilities (restart truthfulness, §14.5/D5)"
    );

    // Below the recorded epoch: StaleSupervisor.
    let stale = supervisor
        .send_bridge_command_raw(
            &fixture.host_peer_descriptor(),
            &rebind(2, &supervisor, mob_id),
            REPLY_TIMEOUT,
        )
        .await
        .expect("stale rebind reply");
    assert!(
        matches!(
            stale,
            BridgeReply::Rejected {
                cause: BridgeRejectionCause::StaleSupervisor,
                ..
            }
        ),
        "expected StaleSupervisor, got {stale:?}"
    );

    // Strictly higher epoch: accepted.
    let advanced = supervisor
        .send_bridge_command_raw(
            &fixture.host_peer_descriptor(),
            &rebind(4, &supervisor, mob_id),
            REPLY_TIMEOUT,
        )
        .await
        .expect("advancing rebind reply");
    assert!(
        matches!(advanced, BridgeReply::HostRebound(_)),
        "expected HostRebound at the advanced epoch, got {advanced:?}"
    );

    // Sender guard: even a self-consistent attacker payload carries no
    // rotation authority. The proposed next supervisor is data; the envelope
    // must be signed by the host's RECORDED current supervisor.
    let attacker = spawn_peer_comms_endpoint("rebind-attacker", true, None).await;
    attacker.trust(fixture.host_peer_descriptor()).await;
    let hijack = attacker
        .send_bridge_command_raw(
            &fixture.host_peer_descriptor(),
            &rebind(u64::MAX, &attacker, mob_id),
            REPLY_TIMEOUT,
        )
        .await
        .expect("self-consistent attacker receives typed rejection");
    assert!(
        matches!(
            hijack,
            BridgeReply::Rejected {
                cause: BridgeRejectionCause::SenderMismatch,
                ..
            }
        ),
        "self-authorized maximal-epoch takeover must be rejected: {hijack:?}"
    );

    // The hijack advanced nothing: the recorded supervisor still replay-acks
    // at the recorded epoch. (Had the hijack been accepted, epoch 5 would be
    // recorded and this rebind would be StaleSupervisor.)
    let post_hijack = supervisor
        .send_bridge_command_raw(
            &fixture.host_peer_descriptor(),
            &rebind(4, &supervisor, mob_id),
            REPLY_TIMEOUT,
        )
        .await
        .expect("post-hijack replay rebind reply");
    assert!(
        matches!(post_hijack, BridgeReply::HostRebound(_)),
        "the rejected hijack must not advance the recorded binding, got {post_hijack:?}"
    );

    // A real rotation is proposed by the next authority but signed by the
    // recorded current authority. Once committed, the next authority can
    // replay the exact tuple idempotently.
    let rotated = spawn_peer_comms_endpoint("rebind-rotated-supervisor", true, None).await;
    rotated.trust(fixture.host_peer_descriptor()).await;
    let rotate = rebind(5, &rotated, mob_id);
    let rotated_reply = supervisor
        .send_bridge_command_raw(&fixture.host_peer_descriptor(), &rotate, REPLY_TIMEOUT)
        .await
        .expect("old-authority-signed rotation reply");
    assert!(
        matches!(rotated_reply, BridgeReply::HostRebound(_)),
        "recorded current authority must be able to authorize the next: {rotated_reply:?}"
    );
    let rotated_replay = rotated
        .send_bridge_command_raw(&fixture.host_peer_descriptor(), &rotate, REPLY_TIMEOUT)
        .await
        .expect("new-authority exact replay reply");
    assert!(
        matches!(rotated_replay, BridgeReply::HostRebound(_)),
        "new authority must replay its recorded tuple: {rotated_replay:?}"
    );

    // Rebind for a mob this daemon never bound: typed NotBound.
    let unbound = supervisor
        .send_bridge_command_raw(
            &fixture.host_peer_descriptor(),
            &rebind(1, &supervisor, "mob-never-bound"),
            REPLY_TIMEOUT,
        )
        .await
        .expect("unbound rebind reply");
    assert!(
        matches!(
            unbound,
            BridgeReply::Rejected {
                cause: BridgeRejectionCause::NotBound,
                ..
            }
        ),
        "expected NotBound, got {unbound:?}"
    );

    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn rebind_commit_ack_loss_fail_stops_live_actor_and_restart_recovers_terminal() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture = spawn_host_daemon_fixture(HostFixtureOptions {
        persistence_ack_loss: HostPersistenceAckLossFault::Rebind,
        ..HostFixtureOptions::named("rebind-uncertain-terminal-host")
    })
    .await
    .expect("spawn host daemon fixture");
    let current = spawn_peer_comms_endpoint("rebind-uncertain-current", true, None).await;
    let next = spawn_peer_comms_endpoint("rebind-uncertain-next", true, None).await;
    current.trust(fixture.host_peer_descriptor()).await;
    next.trust(fixture.host_peer_descriptor()).await;

    let mob_id = "mob-rebind-uncertain-terminal";
    let bind = raw_bind_host_command(&current, mob_id, &fixture.current_descriptor(), 1);
    assert!(matches!(
        current
            .send_bridge_command_raw(&fixture.host_peer_descriptor(), &bind, REPLY_TIMEOUT)
            .await
            .expect("initial bind reply"),
        BridgeReply::BindHost(_)
    ));

    let rotation = BridgeCommand::RebindHost(BridgeHostRebindPayload {
        supervisor: supervisor_spec_for_endpoint(&next, mob_id),
        epoch: 2,
        binding_generation: 1,
        protocol_version: BridgeProtocolVersion::V4,
        mob_id: mob_id.to_string(),
        required_capabilities: Default::default(),
    });
    assert!(matches!(
        current
            .send_bridge_command_raw(&fixture.host_peer_descriptor(), &rotation, REPLY_TIMEOUT,)
            .await
            .expect("ambiguous rebind reply"),
        BridgeReply::Rejected {
            cause: BridgeRejectionCause::Internal,
            ..
        }
    ));

    let committed = fixture.host_binding_record(mob_id).await;
    assert_eq!(
        committed.supervisor_peer_id,
        next.runtime.public_key().to_peer_id().to_string(),
        "the injected error happens after the exact next authority is durable"
    );
    assert_eq!(committed.epoch, 2);
    assert_eq!(committed.binding_generation, 1);

    let stopped = next
        .send_bridge_command_raw(&fixture.host_peer_descriptor(), &rotation, REPLY_TIMEOUT)
        .await
        .expect("fail-stopped actor reply");
    let BridgeReply::Rejected { cause, reason } = stopped else {
        panic!("uncertain durable terminal must reject later commands, got {stopped:?}");
    };
    assert_eq!(cause, BridgeRejectionCause::Internal);
    assert!(
        reason.contains("fail-stopped") && reason.contains("restart required"),
        "sticky rejection must identify the restart boundary: {reason}"
    );

    let fixture = fixture.restart().await;
    next.trust(fixture.host_peer_descriptor()).await;
    assert!(matches!(
        next.send_bridge_command_raw(&fixture.host_peer_descriptor(), &rotation, REPLY_TIMEOUT,)
            .await
            .expect("post-restart exact rebind replay"),
        BridgeReply::HostRebound(_)
    ));

    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn rebind_same_supervisor_identity_replaces_stale_address() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture = spawn_host_daemon_fixture(
        HostFixtureOptions::named("rebind-address-host").with_member_build(),
    )
    .await
    .expect("spawn host daemon fixture");
    let keypair = meerkat_comms::Keypair::generate();
    let original =
        spawn_peer_comms_endpoint("rebind-address-supervisor", true, Some(keypair.clone())).await;
    original.trust(fixture.host_peer_descriptor()).await;
    let mob_id = "mob-rebind-address-refresh";
    let bind = raw_bind_host_command(&original, mob_id, &fixture.current_descriptor(), 7);
    assert!(matches!(
        original
            .send_bridge_command_raw(&fixture.host_peer_descriptor(), &bind, REPLY_TIMEOUT)
            .await
            .expect("initial bind reply"),
        BridgeReply::BindHost(_)
    ));
    let old_address = original.self_descriptor().address.to_string();
    let materialize = sample_materialize_payload(
        &original,
        7,
        sample_portable_member_spec(mob_id, "address-member", "worker"),
        1,
        1,
        MaterializeLaunchMode::Fresh {},
    );
    let materialized = original
        .send_bridge_command_raw(
            &fixture.host_peer_descriptor(),
            &BridgeCommand::MaterializeMember(materialize),
            REPLY_TIMEOUT,
        )
        .await
        .expect("materialize member before rebind");
    let BridgeReply::MemberMaterialized(materialized) = materialized else {
        panic!("expected materialized member")
    };

    // Keep the old listener alive so port-0 cannot be recycled: the second
    // endpoint has the same signing identity/PeerId and a provably new route.
    let restarted =
        spawn_peer_comms_endpoint("rebind-address-supervisor", true, Some(keypair)).await;
    restarted.trust(fixture.host_peer_descriptor()).await;
    let new_descriptor = restarted.self_descriptor();
    assert_ne!(new_descriptor.address.to_string(), old_address);
    let rebind = BridgeCommand::RebindHost(BridgeHostRebindPayload {
        supervisor: supervisor_spec_for_endpoint(&restarted, mob_id),
        epoch: 8,
        binding_generation: 1,
        protocol_version: BridgeProtocolVersion::V4,
        mob_id: mob_id.to_string(),
        required_capabilities: Default::default(),
    });
    let rebind_reply = restarted
        .send_bridge_command_raw(&fixture.host_peer_descriptor(), &rebind, REPLY_TIMEOUT)
        .await
        .expect("same-identity address-refresh rebind reply");
    assert!(
        matches!(rebind_reply, BridgeReply::HostRebound(_)),
        "expected HostRebound for the same supervisor identity at its new address, got {rebind_reply:?}"
    );

    let peer_id = new_descriptor.peer_id.to_string();
    let matching = fixture
        .runtime
        .trusted_peers_shared()
        .entries()
        .into_iter()
        .filter(|peer| peer.peer_id.to_string() == peer_id)
        .collect::<Vec<_>>();
    assert_eq!(
        matching.len(),
        1,
        "host must project one endpoint per peer id"
    );
    assert_eq!(
        matching[0].address.to_string(),
        new_descriptor.address.to_string(),
        "host trust must move to the restarted controller's new address"
    );
    assert_ne!(matching[0].address.to_string(), old_address);

    let stored = fixture.host_binding_record(mob_id).await;
    assert_eq!(
        stored.materialized["address-member"].supervisor_address,
        new_descriptor.address.to_string(),
        "accepted rebind must durably refresh every materialized revival row"
    );

    // The already-live member must accept the new epoch/route before the host
    // ACKs rebind; durable refresh alone would leave it pinned to epoch 7.
    let member = member_descriptor_from_ack("address-member", &materialized);
    restarted.trust(member.clone()).await;
    let poll = raw_poll_member_events_command(
        &restarted,
        mob_id,
        8,
        member_incarnation_from_ack(&fixture, mob_id, "address-member", &materialized, 1, 1, 1),
        BridgeEventCursor::Tail,
        Some(1),
        Some(0),
    );
    assert!(matches!(
        restarted
            .send_bridge_command_raw(&member, &poll, REPLY_TIMEOUT)
            .await
            .expect("live member accepts refreshed supervisor"),
        BridgeReply::MemberEventsPage(_)
    ));

    // Exact replay from the refreshed endpoint proves responses route over
    // the replacement trust row rather than the stale address.
    assert!(matches!(
        restarted
            .send_bridge_command_raw(&fixture.host_peer_descriptor(), &rebind, REPLY_TIMEOUT)
            .await
            .expect("refreshed endpoint replay reply"),
        BridgeReply::HostRebound(_)
    ));

    // Restart boot revival combines binding key/epoch with the endpoint in
    // each materialized row. The refreshed controller must still receive the
    // member reply after zero-traffic revival.
    let fixture = fixture.restart().await;
    restarted.trust(fixture.host_peer_descriptor()).await;
    let status = restarted
        .send_bridge_command_raw(
            &fixture.host_peer_descriptor(),
            &raw_host_status_command(&restarted, mob_id, 8),
            REPLY_TIMEOUT,
        )
        .await
        .expect("host serves status after address-refresh revival");
    let BridgeReply::HostStatus(status) = status else {
        panic!("expected HostStatus serving barrier after address-refresh revival")
    };
    assert!(
        status
            .members
            .iter()
            .any(|member| { member.agent_identity == "address-member" && member.healthy }),
        "serving barrier must report the address-refresh member healthy: {status:?}",
    );
    assert!(matches!(
        restarted
            .send_bridge_command_raw(&member, &poll, REPLY_TIMEOUT)
            .await
            .expect("revived member routes to refreshed supervisor endpoint"),
        BridgeReply::MemberEventsPage(_)
    ));

    fixture.shutdown().await;
}

// ===========================================================================
// DSL-level consumed-token crash-window pin.
// ===========================================================================

#[test]
fn consumed_token_exact_bind_replay_is_accepted_from_recorded_authority() {
    let mut authority = MobHostBindingAuthorityAuthority::new();
    let bind = |token_valid: bool| MobHostBindingAuthorityInput::ResolveHostBind {
        mob_id: meerkat_mob::machines::mob_host_binding_authority::MobId("mob-1".to_string()),
        supervisor_peer_id: meerkat_mob::machines::mob_host_binding_authority::PeerId(
            "supervisor-a".to_string(),
        ),
        supervisor_signing_key: meerkat_mob::machines::mob_host_binding_authority::PeerSigningKey(
            [1; 32],
        ),
        epoch: 3,
        binding_generation: 1,
        sender_matches_supervisor: true,
        address_matches: true,
        token_valid,
    };

    let accepted = MobHostBindingAuthorityMutator::apply(&mut authority, bind(true))
        .expect("fresh bind fires");
    assert!(matches!(
        accepted.effects().first(),
        Some(MobHostBindingAuthorityEffect::HostBindAccepted { .. })
    ));

    // Retry of the SAME tuple after the shell consumed + re-minted the token:
    // T0 is no longer valid for fresh admission, but the already-recorded
    // authority tuple authenticates exact replay.
    let stale_token_retry = MobHostBindingAuthorityMutator::apply(&mut authority, bind(false))
        .expect("bound-state retry fires replay transition");
    assert!(matches!(
        stale_token_retry.effects().first(),
        Some(MobHostBindingAuthorityEffect::HostBindAccepted {
            epoch: 3,
            binding_generation: 1,
            ..
        })
    ));

    // The same tuple WITH a valid current token converges as a replay accept.
    let replay = MobHostBindingAuthorityMutator::apply(&mut authority, bind(true))
        .expect("valid-token replay fires");
    assert!(matches!(
        replay.effects().first(),
        Some(MobHostBindingAuthorityEffect::HostBindAccepted { epoch: 3, .. })
    ));
    assert_eq!(
        authority.state().supervisor_epochs.get(
            &meerkat_mob::machines::mob_host_binding_authority::MobId("mob-1".to_string())
        ),
        Some(&3),
        "replay must not mutate the recorded binding"
    );
}

// ===========================================================================
// RevokeHost clears facts; a fresh descriptor re-binds (§W4.3)
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn revoke_host_clears_machine_facts_and_fresh_descriptor_rebinds() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture = spawn_host_daemon_fixture(HostFixtureOptions::named("revoke-host"))
        .await
        .expect("spawn host daemon fixture");
    let controlling = create_controlling_mob("revoke-ceremony").await;

    let d0 = fixture.current_descriptor();
    let report = controlling
        .handle
        .bind_host(descriptor_to_bind_request(&d0))
        .await
        .expect("bind");
    let bound_epoch = fixture
        .host_binding_record(controlling.mob_id.as_str())
        .await
        .epoch;

    // RevokeHost clears the machine host facts fail-closed on the HostRevoked
    // mirror, and the deletion witness minted from that transition deletes
    // the durable record — the record deletion below can only happen if the
    // machine facts were cleared (the per-map clearing itself is pinned
    // in-crate: runtime/tests.rs
    // test_revoke_host_clears_machine_facts_and_deletes_record).
    controlling
        .handle
        .revoke_host(&report.host_id)
        .await
        .expect("revoke_host");
    assert!(
        controlling
            .storage_metadata
            .list_mob_host_authorities(&controlling.mob_id)
            .await
            .expect("host authority records after revoke")
            .is_empty(),
        "RevokeHost must delete the durable MobHostAuthorityRecord (FLAG-3)"
    );
    assert!(
        fixture
            .store
            .list_mob_host_bindings()
            .await
            .expect("host bindings after revoke")
            .is_empty(),
        "controller acknowledgement is gated on the host deleting its active binding"
    );
    let receipt: MobHostRevocationReceipt = serde_json::from_slice(
        &fixture
            .store
            .load_mob_host_revocation(controlling.mob_id.as_str())
            .await
            .expect("load host revoke receipt")
            .expect("host revoke receipt present"),
    )
    .expect("decode host revoke receipt");
    assert_eq!(receipt.epoch, bound_epoch);

    // Re-bind after revoke with the CURRENT (fresh, unconsumed) descriptor:
    // the host has no active binding and atomically clears the old revoke
    // receipt while installing the replacement binding.
    let d1 = fixture.current_descriptor();
    let report2 = controlling
        .handle
        .bind_host(descriptor_to_bind_request(&d1))
        .await
        .expect("re-bind after revoke with a fresh descriptor");
    assert_eq!(report2.host_id, report.host_id);
    let records = controlling
        .storage_metadata
        .list_mob_host_authorities(&controlling.mob_id)
        .await
        .expect("host authority records after re-bind");
    assert_eq!(records.len(), 1, "the re-bind re-records the host facts");
    assert_eq!(records[0].host_id, report.host_id);
    assert_eq!(records[0].bind_phase, MobHostBindPhaseRecord::Bound);
    assert!(
        fixture
            .store
            .load_mob_host_revocation(controlling.mob_id.as_str())
            .await
            .expect("load receipt after replacement bind")
            .is_none(),
        "a fresh bind must supersede and clear the old revoke receipt"
    );

    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn revoke_commit_ack_loss_fail_stops_live_actor_and_restart_recovers_receipt() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture = spawn_host_daemon_fixture(HostFixtureOptions {
        persistence_ack_loss: HostPersistenceAckLossFault::Revoke,
        ..HostFixtureOptions::named("revoke-uncertain-terminal-host")
    })
    .await
    .expect("spawn host daemon fixture");
    let supervisor = spawn_peer_comms_endpoint("revoke-uncertain-supervisor", true, None).await;
    supervisor.trust(fixture.host_peer_descriptor()).await;

    let mob_id = "mob-revoke-uncertain-terminal";
    let epoch = 4;
    let bind = raw_bind_host_command(&supervisor, mob_id, &fixture.current_descriptor(), epoch);
    assert!(matches!(
        supervisor
            .send_bridge_command_raw(&fixture.host_peer_descriptor(), &bind, REPLY_TIMEOUT)
            .await
            .expect("initial bind reply"),
        BridgeReply::BindHost(_)
    ));

    let revoke = BridgeCommand::RevokeHost(BridgeHostRevokePayload {
        supervisor: supervisor_spec_for_endpoint(&supervisor, mob_id),
        epoch,
        binding_generation: 1,
        protocol_version: BridgeProtocolVersion::V4,
        mob_id: mob_id.to_string(),
    });
    assert!(matches!(
        supervisor
            .send_bridge_command_raw(&fixture.host_peer_descriptor(), &revoke, REPLY_TIMEOUT)
            .await
            .expect("ambiguous revoke reply"),
        BridgeReply::Rejected {
            cause: BridgeRejectionCause::Internal,
            ..
        }
    ));
    assert!(
        fixture
            .store
            .load_mob_host_binding(mob_id)
            .await
            .expect("load active binding after ambiguous revoke")
            .is_none(),
        "the active row was already atomically removed"
    );
    assert!(
        fixture
            .store
            .load_mob_host_revocation(mob_id)
            .await
            .expect("load receipt after ambiguous revoke")
            .is_some(),
        "the exact revoke receipt was already committed"
    );

    let stopped = supervisor
        .send_bridge_command_raw(&fixture.host_peer_descriptor(), &revoke, REPLY_TIMEOUT)
        .await
        .expect("fail-stopped actor reply");
    let BridgeReply::Rejected { cause, reason } = stopped else {
        panic!("uncertain durable terminal must reject later commands, got {stopped:?}");
    };
    assert_eq!(cause, BridgeRejectionCause::Internal);
    assert!(
        reason.contains("fail-stopped") && reason.contains("restart required"),
        "sticky rejection must identify the restart boundary: {reason}"
    );

    let fixture = fixture.restart().await;
    supervisor.trust(fixture.host_peer_descriptor()).await;
    let replay = supervisor
        .send_bridge_command_raw(&fixture.host_peer_descriptor(), &revoke, REPLY_TIMEOUT)
        .await
        .expect("post-restart exact revoke replay");
    let BridgeReply::HostRevoked(receipt) = replay else {
        panic!("restart must recover and replay the durable receipt, got {replay:?}");
    };
    assert_eq!(receipt.mob_id, mob_id);
    assert_eq!(receipt.epoch, epoch);
    assert_eq!(receipt.binding_generation, 1);

    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn revoke_receipt_replays_after_restart_and_old_authority_cannot_cross_replacement_bind() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture = spawn_host_daemon_fixture(HostFixtureOptions::named("revoke-replay-host"))
        .await
        .expect("spawn host daemon fixture");
    let old = spawn_peer_comms_endpoint("revoke-old-supervisor", true, None).await;
    old.trust(fixture.host_peer_descriptor()).await;
    let mob_id = "mob-revoke-replay";
    let epoch = 7;
    let bind = raw_bind_host_command(&old, mob_id, &fixture.current_descriptor(), epoch);
    assert!(matches!(
        old.send_bridge_command_raw(&fixture.host_peer_descriptor(), &bind, REPLY_TIMEOUT)
            .await
            .expect("raw bind reply"),
        BridgeReply::BindHost(_)
    ));

    let old_revoke = BridgeCommand::RevokeHost(BridgeHostRevokePayload {
        supervisor: supervisor_spec_for_endpoint(&old, mob_id),
        epoch,
        binding_generation: 1,
        protocol_version: BridgeProtocolVersion::V4,
        mob_id: mob_id.to_string(),
    });
    let assert_revoked = |reply: BridgeReply| match reply {
        BridgeReply::HostRevoked(response) => {
            assert_eq!(response.mob_id, mob_id);
            assert_eq!(response.epoch, epoch);
        }
        other => panic!("expected durable HostRevoked receipt, got {other:?}"),
    };
    assert_revoked(
        old.send_bridge_command_raw(&fixture.host_peer_descriptor(), &old_revoke, REPLY_TIMEOUT)
            .await
            .expect("initial revoke reply"),
    );
    assert!(
        fixture
            .store
            .load_mob_host_binding(mob_id)
            .await
            .expect("load binding after revoke")
            .is_none()
    );
    assert!(
        fixture
            .store
            .load_mob_host_revocation(mob_id)
            .await
            .expect("load receipt after revoke")
            .is_some()
    );

    // Simulate reply loss by discarding the first acknowledgement: the exact
    // authenticated retry replays the durable receipt without a live binding.
    assert_revoked(
        old.send_bridge_command_raw(&fixture.host_peer_descriptor(), &old_revoke, REPLY_TIMEOUT)
            .await
            .expect("same-process exact revoke retry"),
    );

    let fixture = fixture.restart().await;
    old.trust(fixture.host_peer_descriptor()).await;
    assert!(
        fixture.host_binding_records().await.is_empty(),
        "boot recovery must not revive a revoked binding"
    );
    assert_revoked(
        old.send_bridge_command_raw(&fixture.host_peer_descriptor(), &old_revoke, REPLY_TIMEOUT)
            .await
            .expect("post-restart exact revoke retry"),
    );
    assert!(matches!(
        old.send_bridge_command_raw(
            &fixture.host_peer_descriptor(),
            &raw_host_status_command(&old, mob_id, epoch),
            REPLY_TIMEOUT,
        )
        .await
        .expect("post-restart status reply"),
        BridgeReply::Rejected {
            cause: BridgeRejectionCause::NotBound,
            ..
        }
    ));

    // A new controller authority may freshly bind the same mob. Its bind
    // atomically supersedes the old receipt; a delayed old-key revoke cannot
    // cross that replacement boundary.
    let replacement = spawn_peer_comms_endpoint("revoke-replacement-supervisor", true, None).await;
    replacement.trust(fixture.host_peer_descriptor()).await;
    let replacement_epoch = 8;
    let replacement_bind = support::raw_bind_host_command_for_generation(
        &replacement,
        mob_id,
        &fixture.current_descriptor(),
        replacement_epoch,
        2,
    );
    assert!(matches!(
        replacement
            .send_bridge_command_raw(
                &fixture.host_peer_descriptor(),
                &replacement_bind,
                REPLY_TIMEOUT,
            )
            .await
            .expect("replacement bind reply"),
        BridgeReply::BindHost(_)
    ));
    assert!(
        fixture
            .store
            .load_mob_host_revocation(mob_id)
            .await
            .expect("load superseded receipt")
            .is_none()
    );
    assert!(matches!(
        old.send_bridge_command_raw(&fixture.host_peer_descriptor(), &old_revoke, REPLY_TIMEOUT,)
            .await
            .expect("delayed old revoke reply"),
        BridgeReply::Rejected {
            cause: BridgeRejectionCause::SenderMismatch,
            ..
        }
    ));
    let active = fixture.host_binding_record(mob_id).await;
    assert_eq!(
        active.supervisor_peer_id,
        replacement.runtime.public_key().to_peer_id().to_string()
    );
    assert_eq!(active.epoch, replacement_epoch);
    assert_eq!(active.binding_generation, 2);

    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn revoke_cleanup_failure_keeps_binding_and_emits_no_receipt_then_retry_converges() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture = spawn_host_daemon_fixture(
        HostFixtureOptions::named("revoke-cleanup-failure-host").with_member_build(),
    )
    .await
    .expect("spawn member-build host daemon fixture");
    let supervisor = spawn_peer_comms_endpoint("revoke-cleanup-supervisor", true, None).await;
    supervisor.trust(fixture.host_peer_descriptor()).await;
    let mob_id = "mob-revoke-cleanup-failure";
    let materialized = bind_then_materialize(&supervisor, &fixture, mob_id, "cleanup-worker").await;

    // Corrupt the durable cleanup address through the raw store seam. The
    // host must reject before its first teardown effect, leaving the active
    // binding as the retry anchor and installing no terminal receipt.
    let original_bytes = fixture
        .store
        .load_mob_host_binding(mob_id)
        .await
        .expect("load original binding")
        .expect("original binding present");
    let mut corrupt: MobHostBindingRecord =
        serde_json::from_slice(&original_bytes).expect("decode original binding");
    corrupt
        .materialized
        .get_mut("cleanup-worker")
        .expect("materialized row")
        .session_id = "not-a-session-id".to_string();
    let corrupt_bytes = serde_json::to_vec(&corrupt).expect("encode corrupt binding");
    assert!(
        fixture
            .store
            .compare_and_put_mob_host_binding(mob_id, &original_bytes, &corrupt_bytes)
            .await
            .expect("install corrupt cleanup row")
    );

    let revoke = BridgeCommand::RevokeHost(BridgeHostRevokePayload {
        supervisor: supervisor_spec_for_endpoint(&supervisor, mob_id),
        epoch: 1,
        binding_generation: 1,
        protocol_version: BridgeProtocolVersion::V4,
        mob_id: mob_id.to_string(),
    });
    assert!(matches!(
        supervisor
            .send_bridge_command_raw(&fixture.host_peer_descriptor(), &revoke, REPLY_TIMEOUT)
            .await
            .expect("failed cleanup revoke reply"),
        BridgeReply::Rejected {
            cause: BridgeRejectionCause::Internal,
            ..
        }
    ));
    assert_eq!(
        fixture
            .store
            .load_mob_host_binding(mob_id)
            .await
            .expect("binding after cleanup failure"),
        Some(corrupt_bytes.clone()),
        "cleanup failure must retain the exact active binding retry anchor"
    );
    assert!(
        fixture
            .store
            .load_mob_host_revocation(mob_id)
            .await
            .expect("receipt after cleanup failure")
            .is_none(),
        "cleanup failure must never mint a terminal receipt"
    );
    assert!(
        fixture
            .member_session_exists(&materialized.session_id)
            .await,
        "prevalidation failure must happen before member teardown"
    );

    assert!(
        fixture
            .store
            .compare_and_put_mob_host_binding(mob_id, &corrupt_bytes, &original_bytes)
            .await
            .expect("repair cleanup row")
    );
    let retry = supervisor
        .send_bridge_command_raw(&fixture.host_peer_descriptor(), &revoke, REPLY_TIMEOUT)
        .await
        .expect("repaired revoke retry");
    let BridgeReply::HostRevoked(receipt) = retry else {
        panic!("repaired revoke must converge, got {retry:?}");
    };
    assert_eq!(receipt.released_members, vec!["cleanup-worker"]);
    assert!(
        fixture
            .store
            .load_mob_host_binding(mob_id)
            .await
            .expect("binding after repaired revoke")
            .is_none()
    );
    assert!(
        fixture
            .store
            .load_mob_host_revocation(mob_id)
            .await
            .expect("receipt after repaired revoke")
            .is_some()
    );
    assert!(
        !fixture
            .member_session_exists(&materialized.session_id)
            .await,
        "successful retry must dispose the materialized member"
    );

    fixture.shutdown().await;
}

// ===========================================================================
// Mixed demux on one acceptor (§11 demux rows at the harness level)
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn mixed_demux_routes_by_to_with_per_identity_acks_and_typed_rejects() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;

    // The test owns the registry (the daemon drives this same seam through
    // MobHostActor; phase 3 adds member registration from materialization
    // transitions — the API and owner gate exist NOW).
    let registry = Arc::new(HostAcceptorIdentityRegistry::new());
    let owner: Arc<dyn Any + Send + Sync> = Arc::new(());
    registry
        .install_owner(Arc::clone(&owner))
        .expect("install registry owner");

    // Host identity + two member identities behind ONE acceptor.
    let mut identities = Vec::new();
    for name in ["demux-host", "demux-member-a", "demux-member-b"] {
        let keypair = meerkat_comms::Keypair::generate();
        let secret = keypair.secret_bytes();
        let endpoint = spawn_peer_comms_endpoint(name, false, Some(keypair)).await;
        registry
            .register_identity(
                &owner,
                endpoint.runtime.public_key(),
                Arc::new(meerkat_comms::Keypair::from_secret(secret)),
                endpoint
                    .runtime
                    .tool_material()
                    .router()
                    .inbox_sender()
                    .clone(),
            )
            .expect("register identity");
        identities.push(endpoint);
    }
    let acceptor = spawn_host_acceptor(HostAcceptorConfig {
        listen_tcp: std::net::SocketAddr::from(([127, 0, 0, 1], 0)),
        advertise_address: None,
        registry: Arc::clone(&registry),
        pairing: None,
        bounds: meerkat_comms::HostAcceptorBounds::default(),
    })
    .await
    .expect("spawn acceptor");
    let acceptor_address = format!("tcp://{}", acceptor.local_addr());

    let sender = spawn_peer_comms_endpoint("demux-sender", true, None).await;
    let mut targets = Vec::new();
    for endpoint in &identities {
        let descriptor = TrustedPeerDescriptor::unsigned_with_pubkey(
            endpoint.runtime.participant_name(),
            endpoint.runtime.public_key().to_peer_id().to_string(),
            *endpoint.runtime.public_key().as_bytes(),
            acceptor_address.clone(),
        )
        .expect("target descriptor at the acceptor address");
        sender.trust(descriptor.clone()).await;
        // Inbound admission on each registered identity uses ITS OWN
        // trusted-peers view (per-member classification survives demux).
        endpoint.trust(sender.self_descriptor()).await;
        targets.push(descriptor);
    }

    // Route by `to`: each identity receives exactly its own message, and the
    // ack is signed by THAT identity's keypair — the sender's router accepts
    // an ack only if `ack.from == sent_to`, so an Ok receipt IS the
    // per-identity ack-signing assertion.
    for (index, (endpoint, descriptor)) in identities.iter().zip(&targets).enumerate() {
        let body = format!("demux-hello-{index}");
        let receipt = sender
            .runtime
            .send(CommsCommand::PeerMessage {
                content_taint: None,
                to: PeerRoute::with_display_name(descriptor.peer_id, descriptor.name.clone()),
                body: body.clone(),
                blocks: None,
                handling_mode: HandlingMode::Queue,
                objective_id: None,
            })
            .await
            .expect("demuxed send must be acked by the addressed identity");
        assert!(
            matches!(receipt, SendReceipt::PeerMessageSent { .. }),
            "unexpected receipt: {receipt:?}"
        );
        endpoint
            .wait_for_message_body(&body, REPLY_TIMEOUT)
            .await
            .expect("addressed identity receives its message");
        // No cross-delivery: the other identities never see this body.
        for (other_index, other) in identities.iter().enumerate() {
            if other_index != index {
                assert!(
                    other
                        .drain_message_bodies()
                        .await
                        .iter()
                        .all(|delivered| delivered != &body),
                    "identity {other_index} must not receive a message addressed to {index}"
                );
            }
        }
    }

    // Misaddressed: an envelope for a key never registered on this acceptor
    // is rejected without an ack — the sender sees the peer as offline.
    let stranger = meerkat_comms::Keypair::generate();
    let misaddressed = TrustedPeerDescriptor::unsigned_with_pubkey(
        "demux-stranger",
        stranger.public_key().to_peer_id().to_string(),
        *stranger.public_key().as_bytes(),
        acceptor_address.clone(),
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
        "a misaddressed envelope must not be acked: {refused:?}"
    );

    // Owner gate: removal under a foreign owner token fails and mutates
    // nothing; the identity stays deliverable.
    let foreign_owner: Arc<dyn Any + Send + Sync> = Arc::new(());
    let member_b = &identities[2];
    let member_b_key = member_b.runtime.public_key();
    assert!(
        registry
            .remove_identity(&foreign_owner, &member_b_key)
            .is_err(),
        "a foreign owner must not mutate the registry"
    );
    let receipt = sender
        .runtime
        .send(CommsCommand::PeerMessage {
            content_taint: None,
            to: PeerRoute::with_display_name(targets[2].peer_id, targets[2].name.clone()),
            body: "still-registered".to_string(),
            blocks: None,
            handling_mode: HandlingMode::Queue,
            objective_id: None,
        })
        .await
        .expect("identity still registered after refused removal");
    assert!(matches!(receipt, SendReceipt::PeerMessageSent { .. }));
    member_b
        .wait_for_message_body("still-registered", REPLY_TIMEOUT)
        .await
        .expect("member-b delivery after refused removal");

    // Registered-then-removed (distinct from never-registered): same typed
    // reject surface, sender sees offline.
    assert!(
        registry
            .remove_identity(&owner, &member_b_key)
            .expect("owner-gated removal succeeds"),
        "removal under the installed owner must report the identity removed"
    );
    let after_removal = sender
        .runtime
        .send(CommsCommand::PeerMessage {
            content_taint: None,
            to: PeerRoute::with_display_name(targets[2].peer_id, targets[2].name.clone()),
            body: "after-removal".to_string(),
            blocks: None,
            handling_mode: HandlingMode::Queue,
            objective_id: None,
        })
        .await;
    assert!(
        after_removal.is_err(),
        "a removed identity must reject like an unregistered one: {after_removal:?}"
    );

    acceptor.shutdown().await;
}

// ===========================================================================
// Controlling-side ceremony failure matrix (§W4.3) against a scripted host
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn bind_host_garbled_reply_fail_stops_until_cold_retry_converges() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let scripted = spawn_scripted_host_peer("garble-host").await;
    let controlling = create_controlling_mob("garble-ceremony").await;
    let host_peer_id = scripted
        .descriptor
        .identity
        .resolve()
        .expect("scripted identity")
        .peer_id
        .to_string();

    scripted.garble_next_bind_reply();
    let error = controlling
        .handle
        .bind_host(descriptor_to_bind_request(&scripted.descriptor))
        .await
        .expect_err("a garbled reply must fail the ceremony typed");
    assert!(
        !format!("{error:?}").is_empty(),
        "decode failure must be a typed error"
    );

    // No commit happened: no durable record exists (records are written only
    // under a CommitHostBind transition witness). The post-send result is
    // nevertheless ambiguous, so the live actor retains Requested/trust and
    // fail-stops; only cold exact replay may continue.
    assert!(
        controlling
            .storage_metadata
            .list_mob_host_authorities(&controlling.mob_id)
            .await
            .expect("records after garbled reply")
            .is_empty()
    );

    let live_retry = controlling
        .handle
        .bind_host(descriptor_to_bind_request(&scripted.descriptor))
        .await
        .expect_err("post-send ambiguity must close the live actor");
    assert!(matches!(
        live_retry,
        meerkat_mob::MobError::ActorCommandChannelClosed
            | meerkat_mob::MobError::ActorReplyChannelClosed
    ));
    assert_eq!(
        scripted.bind_count(),
        1,
        "fail-stop forbids a same-process second BindHost"
    );

    // Fault cleared: cold replay reconstructs the Requested window and the
    // exact retry converges (DEC-P2-10).
    let controlling = controlling.restart_after_actor_fail_stop().await;
    let report = controlling
        .handle
        .bind_host(descriptor_to_bind_request(&scripted.descriptor))
        .await
        .expect("cold retry after cleared fault succeeds");
    assert_eq!(report.host_id, host_peer_id);
    assert_eq!(scripted.bind_count(), 2);

    scripted.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn recovered_started_rejection_cannot_abort_prior_uncertain_send_or_unblock_destroy() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let scripted = spawn_scripted_host_peer("recovered-started-reject-host").await;
    let controlling = create_controlling_mob("recovered-started-reject").await;

    // The host serves the original T0 bind, but its terminal is garbled. The
    // durable controller truth is Started-only: the original send may still
    // have taken effect even after this process disappears.
    scripted.garble_next_bind_reply();
    controlling
        .handle
        .bind_host(descriptor_to_bind_request(&scripted.descriptor))
        .await
        .expect_err("ambiguous original bind must fail-stop");
    let controlling = controlling.restart_after_actor_fail_stop().await;

    // A retry presents a different token and receives a provably pre-write
    // rejection for THIS retry. It cannot retroactively prove the earlier T0
    // send had no effect, so the recovered Started anchor must stay open.
    let mut wrong_token = descriptor_to_bind_request(&scripted.descriptor);
    wrong_token.bootstrap_token = BridgeBootstrapToken::from("different-retry-token");
    controlling
        .handle
        .bind_host(wrong_token)
        .await
        .expect_err("recovered Started rejection remains quarantined");
    let controlling = controlling.restart_after_actor_fail_stop().await;

    let destroy = controlling.handle.destroy().await;
    let error = destroy.expect_err("unfinished original send must block destroy");
    assert!(
        matches!(&error, meerkat_mob::MobDestroyError::Mob(_))
            && format!("{error:?}").contains("unfinished"),
        "unfinished host-bind authority must block destroy with a typed mob error, got: {error:?}",
    );

    // The original exact request remains the only convergence path; once it
    // ACKs canonically the controller can confirm and complete the anchor.
    controlling
        .handle
        .bind_host(descriptor_to_bind_request(&scripted.descriptor))
        .await
        .expect("original exact bind request converges the retained anchor");

    scripted.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn bind_host_reply_peer_id_mismatch_never_commits() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let scripted = spawn_scripted_host_peer("peer-id-mismatch-host").await;
    let controlling = create_controlling_mob("peer-id-mismatch").await;

    scripted.override_next_bind_peer_id("peer-somebody-else");
    let error = controlling
        .handle
        .bind_host(descriptor_to_bind_request(&scripted.descriptor))
        .await
        .expect_err("a reply naming an unexpected host identity must never commit");
    assert!(
        !format!("{error:?}").is_empty(),
        "canonical-identity mismatch must be a typed error"
    );
    assert_eq!(
        scripted.bind_count(),
        1,
        "the scripted host served the bind; the CONTROLLING side refused the commit"
    );

    // No CommitHostBind may fire on an identity-mismatched reply: the durable
    // record is written only under a CommitHostBind transition witness, so
    // its absence pins the missing commit (the machine-map absence itself is
    // pinned in-crate: runtime/tests.rs
    // test_bind_host_reply_identity_mismatch_quarantines_and_fail_stops).
    assert!(
        controlling
            .storage_metadata
            .list_mob_host_authorities(&controlling.mob_id)
            .await
            .expect("records after identity mismatch")
            .is_empty()
    );

    let live_retry = controlling
        .handle
        .bind_host(descriptor_to_bind_request(&scripted.descriptor))
        .await
        .expect_err("identity mismatch must fail-stop before another live bind");
    assert!(matches!(
        live_retry,
        meerkat_mob::MobError::ActorCommandChannelClosed
            | meerkat_mob::MobError::ActorReplyChannelClosed
    ));
    assert_eq!(
        scripted.bind_count(),
        1,
        "fail-stop forbids a same-process retry after identity mismatch"
    );

    let controlling = controlling.restart_after_actor_fail_stop().await;
    controlling
        .handle
        .bind_host(descriptor_to_bind_request(&scripted.descriptor))
        .await
        .expect("cold honest retry succeeds");
    assert_eq!(scripted.bind_count(), 2);

    scripted.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn bind_host_failure_leaves_window_requested_and_retry_is_idempotent() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let scripted = spawn_scripted_host_peer("fail-once-host").await;
    let controlling = create_controlling_mob("fail-once").await;
    let host_peer_id = scripted
        .descriptor
        .identity
        .resolve()
        .expect("scripted identity")
        .peer_id
        .to_string();

    scripted.fail_next_bind();
    let error = controlling
        .handle
        .bind_host(descriptor_to_bind_request(&scripted.descriptor))
        .await
        .expect_err("an injected host failure surfaces typed");
    assert!(!format!("{error:?}").is_empty());

    // No commit on failure: no durable record may exist while the ceremony
    // has not committed.
    assert!(
        controlling
            .storage_metadata
            .list_mob_host_authorities(&controlling.mob_id)
            .await
            .expect("records after failed bind")
            .is_empty(),
        "a failed ceremony must not persist a host authority record"
    );

    // DEC-P2-10: a certified pre-write rejection leaves the bind window in
    // `Requested` — the retry
    // through the SAME window converging is the behavioral pin (the
    // `Requested` machine phase itself is pinned in-crate:
    // runtime/tests.rs test_bind_host_surfaces_typed_rejection_and_retries).
    let report = controlling
        .handle
        .bind_host(descriptor_to_bind_request(&scripted.descriptor))
        .await
        .expect("retry through the open window succeeds");
    assert_eq!(report.host_id, host_peer_id);
    assert_eq!(
        scripted.bind_count(),
        1,
        "exactly one successful bind serve"
    );

    scripted.shutdown();
}
