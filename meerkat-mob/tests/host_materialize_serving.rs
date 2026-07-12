//! Multi-host mobs phase 3 — MEMBER-HOST `MaterializeMember` serving
//! (design-host-materialize-serving §2 rows T1–T13, T20, T26), raw-probe-as-
//! supervisor over the member-build `HostDaemonFixture`.
//!
//! Merges recorded here:
//!   * F T-L3 (real-host materialize dedup)  → T4
//!   * F T-L4 (raw stale-fence materialize)  → T6
//!   * H T14–T17 (release serving)           → `host_member_release.rs`
//!   * H T18/T19 (restart revival)           → `host_member_revival.rs`
//!   * H T25 (e2e-fast module)               → `tests/integration/tests/multi_host_spawn.rs`
//!
//! The `ModelUnresolvable` preflight leg of T8 is UNREACHABLE from a decoded
//! spec in v1 (provider is REQUIRED on `PortableProfile`, so the
//! (model, provider) pair always names a constructible client — DEC-P3H-8);
//! the arm stays machine-kernel-pinned (phase-1 `mob_host_binding_authority.rs`).
//!
//! ADJ-9 (ensure-on-replay) is pinned controlling-side in
//! `host_member_revival.rs`; here T4 pins the recorded-ack half.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::time::Duration;

use meerkat_core::agent::CommsRuntime as _;
use meerkat_core::{CommsCommand, HandlingMode, PeerRoute, SendReceipt};
use meerkat_mob::runtime::bridge_protocol::{
    BridgeCommand, BridgeEventCursor, BridgeRejectionCause, BridgeReply, BridgeTurnOutcomeAck,
    MaterializeLaunchMode, MaterializeLaunchOutcome, PortableMcpDecl, WireFlowTurnOutcome,
    portable_member_spec_digest,
};
use meerkat_mob::runtime::host_actor::{
    MaterializedMemberRow, MobHostActorError, MobHostBindingRecord,
};
use meerkat_mob::runtime::host_observation::{
    HostTurnOutcomeAckRequest, HostTurnOutcomePendingRequest, HostTurnOutcomeRecordRequest,
};
use meerkat_runtime::member_observation::TrackedTurnOutcomeRecord;
use meerkat_runtime::{RuntimeState, SessionServiceRuntimeExt as _};
use support::{
    HostFixtureOptions, HostPersistenceAckLossFault, REAL_COMMS_TEST_LOCK, StallGate,
    bind_then_materialize, member_descriptor_from_ack, member_incarnation_from_ack,
    raw_bind_host_command, raw_deliver_member_input_command, raw_host_status_command,
    raw_poll_member_events_command, raw_release_member_command, sample_materialize_payload,
    sample_portable_member_spec, scripted_member_client_stalling, spawn_host_daemon_fixture,
    spawn_peer_comms_endpoint,
};
use tokio::sync::oneshot;

const REPLY_TIMEOUT: Duration = Duration::from_secs(30);

async fn member_build_fixture(name: &str) -> support::HostDaemonFixture {
    spawn_host_daemon_fixture(HostFixtureOptions::named(name).with_member_build())
        .await
        .expect("spawn member-build host fixture")
}

/// Raw-bind `mob_id` from `sender` (epoch 1) — precondition for the
/// materialize rows below.
async fn raw_bind(
    sender: &support::PeerCommsEndpoint,
    fixture: &support::HostDaemonFixture,
    mob_id: &str,
    epoch: u64,
) {
    let command = raw_bind_host_command(sender, mob_id, &fixture.current_descriptor(), epoch);
    let reply = sender
        .send_bridge_command_raw(&fixture.host_peer_descriptor(), &command, REPLY_TIMEOUT)
        .await
        .expect("raw bind reply");
    assert!(matches!(reply, BridgeReply::BindHost(_)), "got {reply:?}");
}

fn assert_rejected(reply: &BridgeReply, expected: &BridgeRejectionCause) {
    match reply {
        BridgeReply::Rejected { cause, .. } if cause == expected => {}
        other => panic!("expected Rejected({expected:?}), got {other:?}"),
    }
}

async fn assert_fail_stopped_reply<T: std::fmt::Debug>(
    reply: oneshot::Receiver<Result<T, String>>,
) {
    let error = tokio::time::timeout(REPLY_TIMEOUT, reply)
        .await
        .expect("fail-stop reply timeout")
        .expect("host actor retained fail-stop reply sender")
        .expect_err("fail-stopped mutation lane must reject");
    assert!(
        error.contains("fail-stopped") && error.contains("restart required"),
        "sticky rejection must identify the restart boundary: {error}"
    );
}

async fn host_status_member_health(
    probe: &support::PeerCommsEndpoint,
    fixture: &support::HostDaemonFixture,
    mob_id: &str,
    identity: &str,
) -> bool {
    let reply = probe
        .send_bridge_command_raw(
            &fixture.host_peer_descriptor(),
            &raw_host_status_command(probe, mob_id, 1),
            REPLY_TIMEOUT,
        )
        .await
        .expect("host status reply");
    let BridgeReply::HostStatus(status) = reply else {
        panic!("expected HostStatus, got {reply:?}");
    };
    status
        .members
        .into_iter()
        .find(|row| row.agent_identity == identity)
        .unwrap_or_else(|| panic!("host status omitted materialized member {identity}"))
        .healthy
}

/// Assert the bounded post-create failure contract for one member-host test:
/// durable discovery remains, while the exact actor and machine attachment are
/// gone. A subsequent Resume attempt additionally proves the private runtime
/// sidecar did not survive and block reattachment.
async fn assert_only_persisted_inert_member_session(
    fixture: &support::HostDaemonFixture,
) -> meerkat_core::SessionId {
    let service = fixture.member_session_service();
    let sessions = service
        .list(meerkat_core::service::SessionQuery::default())
        .await
        .expect("list preserved member sessions");
    assert_eq!(
        sessions.len(),
        1,
        "post-create failure must preserve exactly its durable session"
    );
    let session_id = sessions[0].session_id.clone();
    assert!(
        service
            .load_persisted_session(&session_id)
            .await
            .expect("load preserved member session")
            .is_some(),
        "post-create failure must leave the session discoverable and resumable"
    );
    assert!(
        !service
            .session_known_to_archive_authority(&session_id)
            .await
            .expect("read member archive authority"),
        "post-create failure must not archive the durable session"
    );
    assert!(
        !service
            .live_session_actor_registered(&session_id)
            .await
            .expect("read live member actor census"),
        "failed exact incarnation must discard its live actor"
    );
    assert!(
        !fixture
            .member_runtime_adapter
            .as_ref()
            .expect("member runtime adapter")
            .contains_session(&session_id)
            .await,
        "failed exact incarnation must remove its machine attachment"
    );
    session_id
}

async fn replay_exact_materialization(
    probe: &support::PeerCommsEndpoint,
    fixture: &support::HostDaemonFixture,
    mob_id: &str,
    identity: &str,
) -> meerkat_mob::runtime::bridge_protocol::BridgeMaterializedResponse {
    let payload = sample_materialize_payload(
        probe,
        1,
        sample_portable_member_spec(mob_id, identity, "worker"),
        1,
        1,
        MaterializeLaunchMode::Fresh {},
    );
    let reply = probe
        .send_bridge_command_raw(
            &fixture.host_peer_descriptor(),
            &BridgeCommand::MaterializeMember(payload),
            REPLY_TIMEOUT,
        )
        .await
        .expect("exact materialize replay reply");
    let BridgeReply::MemberMaterialized(ack) = reply else {
        panic!("expected replayed MemberMaterialized, got {reply:?}");
    };
    ack
}

// ===========================================================================
// T1 — materialize before bind ⇒ NotBound; no session, no row
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn materialize_before_bind_rejects_not_bound() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture = member_build_fixture("mat-t1-host").await;
    let probe = spawn_peer_comms_endpoint("mat-t1-probe", true, None).await;
    probe.trust(fixture.host_peer_descriptor()).await;

    let payload = sample_materialize_payload(
        &probe,
        1,
        sample_portable_member_spec("mob-t1", "b2", "worker"),
        1,
        1,
        MaterializeLaunchMode::Fresh {},
    );
    let reply = probe
        .send_bridge_command_raw(
            &fixture.host_peer_descriptor(),
            &BridgeCommand::MaterializeMember(payload),
            REPLY_TIMEOUT,
        )
        .await
        .expect("typed NotBound reply, never a drop");
    assert_rejected(&reply, &BridgeRejectionCause::NotBound);
    assert!(
        fixture.host_binding_records().await.is_empty(),
        "an unbound reject must not persist"
    );

    fixture.shutdown().await;
}

// ===========================================================================
// T2 — rung-0 arms: sender mismatch; stale supervisor epoch
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn materialize_sender_mismatch_and_stale_epoch_reject() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture = member_build_fixture("mat-t2-host").await;
    let supervisor = spawn_peer_comms_endpoint("mat-t2-supervisor", true, None).await;
    supervisor.trust(fixture.host_peer_descriptor()).await;
    raw_bind(&supervisor, &fixture, "mob-t2", 3).await;

    // Sender ≠ recorded supervisor ⇒ SenderMismatch. The reject routes only
    // through the attacker's signed, source-confined callback capability;
    // its payload supervisor address remains identity metadata, never route
    // authority.
    let attacker = spawn_peer_comms_endpoint("mat-t2-attacker", true, None).await;
    attacker.trust(fixture.host_peer_descriptor()).await;
    let payload = sample_materialize_payload(
        &attacker,
        3,
        sample_portable_member_spec("mob-t2", "b2", "worker"),
        1,
        1,
        MaterializeLaunchMode::Fresh {},
    );
    let reply = attacker
        .send_bridge_command_raw(
            &fixture.host_peer_descriptor(),
            &BridgeCommand::MaterializeMember(payload),
            REPLY_TIMEOUT,
        )
        .await
        .expect("typed reply");
    assert_rejected(&reply, &BridgeRejectionCause::SenderMismatch);

    // Correct sender, epoch below the recorded binding ⇒ StaleSupervisor.
    let payload = sample_materialize_payload(
        &supervisor,
        2,
        sample_portable_member_spec("mob-t2", "b2", "worker"),
        1,
        1,
        MaterializeLaunchMode::Fresh {},
    );
    let reply = supervisor
        .send_bridge_command_raw(
            &fixture.host_peer_descriptor(),
            &BridgeCommand::MaterializeMember(payload),
            REPLY_TIMEOUT,
        )
        .await
        .expect("typed reply");
    assert_rejected(&reply, &BridgeRejectionCause::StaleSupervisor);

    // Nothing half-built.
    let record = fixture.host_binding_record("mob-t2").await;
    assert!(record.materialized.is_empty());

    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn materialize_declared_supervisor_must_match_durable_host_authority() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture = member_build_fixture("mat-t2b-host").await;
    let supervisor = spawn_peer_comms_endpoint("mat-t2b-supervisor", true, None).await;
    supervisor.trust(fixture.host_peer_descriptor()).await;
    raw_bind(&supervisor, &fixture, "mob-t2b", 3).await;

    // The recorded supervisor signs the command, but carries a different
    // peer as the descriptor that would seed member-side trust. Rung 0
    // authenticates the signer; the payload-to-durable-authority binding
    // must still fail before build or persistence.
    let delegated = spawn_peer_comms_endpoint("mat-t2b-delegated", true, None).await;
    let payload = sample_materialize_payload(
        &delegated,
        3,
        sample_portable_member_spec("mob-t2b", "b2", "worker"),
        1,
        1,
        MaterializeLaunchMode::Fresh {},
    );
    let reply = supervisor
        .send_bridge_command_raw(
            &fixture.host_peer_descriptor(),
            &BridgeCommand::MaterializeMember(payload),
            REPLY_TIMEOUT,
        )
        .await
        .expect("authenticated signer receives typed rejection");
    assert_rejected(&reply, &BridgeRejectionCause::SenderMismatch);

    let record = fixture.host_binding_record("mob-t2b").await;
    assert!(
        record.materialized.is_empty(),
        "a mismatched declared supervisor must have no build or durable-row effect"
    );

    fixture.shutdown().await;
}

// ===========================================================================
// T3 — fresh build: ack material, durable rows, member-identity demux
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn materialize_fresh_builds_session_registers_identity_and_acks() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture = member_build_fixture("mat-t3-host").await;
    let probe = spawn_peer_comms_endpoint("mat-t3-supervisor", true, None).await;
    probe.trust(fixture.host_peer_descriptor()).await;

    let spec = sample_portable_member_spec("mob-t3", "b2", "worker");
    let sent_digest = portable_member_spec_digest(&spec).expect("digest computes");
    raw_bind(&probe, &fixture, "mob-t3", 1).await;
    let payload =
        sample_materialize_payload(&probe, 1, spec, 1, 1, MaterializeLaunchMode::Fresh {});
    let reply = probe
        .send_bridge_command_raw(
            &fixture.host_peer_descriptor(),
            &BridgeCommand::MaterializeMember(payload),
            REPLY_TIMEOUT,
        )
        .await
        .expect("materialize reply");
    let BridgeReply::MemberMaterialized(ack) = &reply else {
        panic!("expected MemberMaterialized, got {reply:?}");
    };

    // Ack material (D1: identity-first; the pubkey is the trust subject).
    assert!(
        ack.member_pubkey.starts_with("ed25519:"),
        "member pubkey is the ed25519: form, got {}",
        ack.member_pubkey
    );
    assert!(!ack.member_peer_id.is_empty());
    assert_eq!(
        ack.advertised_address,
        fixture.advertised_address(),
        "the member is reachable at the HOST ACCEPTOR address (demux)"
    );
    assert_eq!(ack.spec_digest, sent_digest, "digest echo");
    assert!(!ack.engine_version.is_empty());
    assert_eq!(ack.launch_outcome, MaterializeLaunchOutcome::Fresh);
    assert!(
        fixture.member_session_exists(&ack.session_id).await,
        "the built session lives on Host B's member service"
    );

    // Durable row.
    let record = fixture.host_binding_record("mob-t3").await;
    let row = record.materialized.get("b2").expect("materialized row");
    assert_eq!(row.generation, 1);
    assert_eq!(row.fence_token, 1);
    assert_eq!(row.session_id, ack.session_id);
    assert_eq!(row.spec_digest, sent_digest);

    // Acceptor demux now routes an envelope addressed to the MEMBER pubkey
    // (the phase-2 demux row's member-identity case, finally non-degenerate):
    // the supervisor is trusted member-side by the host-seeded supervisor
    // bind (DEC-P3H-4), so its message classifies and the per-identity ack
    // (`ack.from == sent_to`) IS the assertion.
    let member_descriptor = support::member_descriptor_from_ack("mob-t3/b2", ack);
    probe.trust(member_descriptor.clone()).await;
    let receipt = probe
        .runtime
        .send(CommsCommand::PeerMessage {
            content_taint: None,
            to: PeerRoute::with_display_name(member_descriptor.peer_id, member_descriptor.name),
            body: "hello materialized member".to_string(),
            blocks: None,
            handling_mode: HandlingMode::Queue,
            objective_id: None,
        })
        .await
        .expect("demuxed member-addressed send is acked by the member identity");
    assert!(matches!(receipt, SendReceipt::PeerMessageSent { .. }));

    fixture.shutdown().await;
}

// ===========================================================================
// T4 — replay same tuple + same digest ⇒ recorded ack, no second session
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn materialize_replay_same_tuple_same_digest_returns_recorded_ack() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture = member_build_fixture("mat-t4-host").await;
    let probe = spawn_peer_comms_endpoint("mat-t4-supervisor", true, None).await;
    probe.trust(fixture.host_peer_descriptor()).await;
    raw_bind(&probe, &fixture, "mob-t4", 1).await;

    let spec = sample_portable_member_spec("mob-t4", "b2", "worker");
    let payload =
        sample_materialize_payload(&probe, 1, spec, 1, 1, MaterializeLaunchMode::Fresh {});
    let command = BridgeCommand::MaterializeMember(payload);
    let first = probe
        .send_bridge_command_raw(&fixture.host_peer_descriptor(), &command, REPLY_TIMEOUT)
        .await
        .expect("first materialize");
    let BridgeReply::MemberMaterialized(first_ack) = &first else {
        panic!("expected MemberMaterialized, got {first:?}");
    };

    let replay = probe
        .send_bridge_command_raw(&fixture.host_peer_descriptor(), &command, REPLY_TIMEOUT)
        .await
        .expect("replay materialize");
    let BridgeReply::MemberMaterialized(replay_ack) = &replay else {
        panic!("expected replayed MemberMaterialized, got {replay:?}");
    };
    assert_eq!(
        first_ack, replay_ack,
        "the replay returns the RECORDED ack verbatim (launch_outcome included)"
    );
    let record = fixture.host_binding_record("mob-t4").await;
    assert_eq!(
        record.materialized.len(),
        1,
        "one materialized row despite two sends"
    );

    fixture.shutdown().await;
}

// ===========================================================================
// T5 — same tuple, different digest ⇒ SpecDigestMismatch (A12)
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn materialize_same_tuple_different_digest_rejects_spec_digest_mismatch() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture = member_build_fixture("mat-t5-host").await;
    let probe = spawn_peer_comms_endpoint("mat-t5-supervisor", true, None).await;
    probe.trust(fixture.host_peer_descriptor()).await;
    let first_ack = bind_then_materialize(&probe, &fixture, "mob-t5", "b2").await;

    // Same (identity, gen, fence), honestly-digested DIFFERENT spec.
    let mut mutated = sample_portable_member_spec("mob-t5", "b2", "worker");
    mutated.overlay.additional_instructions = Some(vec!["now different".to_string()]);
    let payload =
        sample_materialize_payload(&probe, 1, mutated, 1, 1, MaterializeLaunchMode::Fresh {});
    let reply = probe
        .send_bridge_command_raw(
            &fixture.host_peer_descriptor(),
            &BridgeCommand::MaterializeMember(payload),
            REPLY_TIMEOUT,
        )
        .await
        .expect("typed reply");
    assert_rejected(&reply, &BridgeRejectionCause::SpecDigestMismatch);

    // One key never names two builds: the recorded row is untouched.
    let record = fixture.host_binding_record("mob-t5").await;
    assert_eq!(
        record.materialized.get("b2").expect("row").session_id,
        first_ack.session_id
    );

    fixture.shutdown().await;
}

// ===========================================================================
// T6 — lower tuple ⇒ StaleFence (== F T-L4)
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn materialize_lower_tuple_rejects_stale_fence() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture = member_build_fixture("mat-t6-host").await;
    let probe = spawn_peer_comms_endpoint("mat-t6-supervisor", true, None).await;
    probe.trust(fixture.host_peer_descriptor()).await;
    raw_bind(&probe, &fixture, "mob-t6", 1).await;

    // Commit at (2, 5).
    let spec = sample_portable_member_spec("mob-t6", "b2", "worker");
    let payload = sample_materialize_payload(
        &probe,
        1,
        spec.clone(),
        2,
        5,
        MaterializeLaunchMode::Fresh {},
    );
    let reply = probe
        .send_bridge_command_raw(
            &fixture.host_peer_descriptor(),
            &BridgeCommand::MaterializeMember(payload),
            REPLY_TIMEOUT,
        )
        .await
        .expect("materialize at (2,5)");
    assert!(matches!(reply, BridgeReply::MemberMaterialized(_)));

    // (2, 4) is below the recorded tuple ⇒ StaleFence; row unchanged.
    let payload =
        sample_materialize_payload(&probe, 1, spec, 2, 4, MaterializeLaunchMode::Fresh {});
    let reply = probe
        .send_bridge_command_raw(
            &fixture.host_peer_descriptor(),
            &BridgeCommand::MaterializeMember(payload),
            REPLY_TIMEOUT,
        )
        .await
        .expect("typed reply");
    assert_rejected(&reply, &BridgeRejectionCause::StaleFence);
    let record = fixture.host_binding_record("mob-t6").await;
    let row = record.materialized.get("b2").expect("row");
    assert_eq!((row.generation, row.fence_token), (2, 5));

    fixture.shutdown().await;
}

// ===========================================================================
// T7 — superseding tuple admits and REPLACES (the A20 revival
// re-materialization shape); stored spec swaps atomically
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn materialize_superseding_tuple_admits_and_replaces() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture = member_build_fixture("mat-t7-host").await;
    let probe = spawn_peer_comms_endpoint("mat-t7-supervisor", true, None).await;
    probe.trust(fixture.host_peer_descriptor()).await;
    let first_ack = bind_then_materialize(&probe, &fixture, "mob-t7", "b2").await;

    let mut superseding_spec = sample_portable_member_spec("mob-t7", "b2", "worker");
    superseding_spec.overlay.additional_instructions = Some(vec!["superseding build".to_string()]);
    let superseding_digest =
        portable_member_spec_digest(&superseding_spec).expect("digest computes");
    let payload = sample_materialize_payload(
        &probe,
        1,
        superseding_spec,
        2,
        2,
        MaterializeLaunchMode::Fresh {},
    );
    let reply = probe
        .send_bridge_command_raw(
            &fixture.host_peer_descriptor(),
            &BridgeCommand::MaterializeMember(payload),
            REPLY_TIMEOUT,
        )
        .await
        .expect("superseding materialize");
    let BridgeReply::MemberMaterialized(ack) = &reply else {
        panic!("expected MemberMaterialized, got {reply:?}");
    };
    assert_ne!(
        ack.session_id, first_ack.session_id,
        "a superseding fresh build is a NEW session"
    );

    // The old row was REPLACED atomically: one row, the NEW tuple + spec.
    let record = fixture.host_binding_record("mob-t7").await;
    assert_eq!(record.materialized.len(), 1);
    let row = record.materialized.get("b2").expect("row");
    assert_eq!((row.generation, row.fence_token), (2, 2));
    assert_eq!(row.spec_digest, superseding_digest);
    assert_eq!(
        portable_member_spec_digest(&row.spec).expect("stored spec digest"),
        superseding_digest,
        "the stored spec bytes are the SUPERSEDING spec (§15.7 atomic replace)"
    );

    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn superseding_fresh_build_failure_preserves_replayable_old_snapshot() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture = spawn_host_daemon_fixture(HostFixtureOptions {
        failing_create_session_call: Some(2),
        ..HostFixtureOptions::named("mat-t7-failed-replacement-host").with_member_build()
    })
    .await
    .expect("fault-injected member-build fixture");
    let probe = spawn_peer_comms_endpoint("mat-t7-failed-replacement-supervisor", true, None).await;
    probe.trust(fixture.host_peer_descriptor()).await;
    let mob_id = format!(
        "mob-t7-failed-replacement-{}",
        uuid::Uuid::new_v4().simple()
    );
    let first_ack = bind_then_materialize(&probe, &fixture, &mob_id, "b2").await;

    let mut replacement_spec = sample_portable_member_spec(&mob_id, "b2", "worker");
    replacement_spec.overlay.additional_instructions =
        Some(vec!["replacement whose build fails".to_string()]);
    let replacement = sample_materialize_payload(
        &probe,
        1,
        replacement_spec,
        2,
        2,
        MaterializeLaunchMode::Fresh {},
    );
    let reply = probe
        .send_bridge_command_raw(
            &fixture.host_peer_descriptor(),
            &BridgeCommand::MaterializeMember(replacement),
            REPLY_TIMEOUT,
        )
        .await
        .expect("faulted superseding build reply");
    assert_rejected(&reply, &BridgeRejectionCause::Internal);
    assert!(
        fixture
            .failing_create
            .as_ref()
            .expect("selected create fault wrapper")
            .fired(),
        "the deterministic second create_session fault must fire"
    );

    let record = fixture.host_binding_record(&mob_id).await;
    let old_row = record.materialized.get("b2").expect("old owner row");
    assert_eq!((old_row.generation, old_row.fence_token), (1, 1));
    assert_eq!(old_row.session_id, first_ack.session_id);
    assert!(
        fixture.member_session_exists(&first_ack.session_id).await,
        "a failed pre-commit replacement must not archive the authoritative old snapshot"
    );

    let replayed = replay_exact_materialization(&probe, &fixture, &mob_id, "b2").await;
    assert_eq!(
        replayed, first_ack,
        "exact old-tuple replay must revive and return the recorded acknowledgement"
    );

    fixture.shutdown().await;
}

// ===========================================================================
// T8 — preflight matrix: each arm rejects typed, nothing half-builds
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn materialize_preflight_matrix_rejects_typed() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture = member_build_fixture("mat-t8-host").await;
    let probe = spawn_peer_comms_endpoint("mat-t8-supervisor", true, None).await;
    probe.trust(fixture.host_peer_descriptor()).await;
    raw_bind(&probe, &fixture, "mob-t8", 1).await;

    let send = |payload| {
        let probe = &probe;
        let fixture = &fixture;
        async move {
            probe
                .send_bridge_command_raw(
                    &fixture.host_peer_descriptor(),
                    &BridgeCommand::MaterializeMember(payload),
                    REPLY_TIMEOUT,
                )
                .await
                .expect("typed preflight reply")
        }
    };

    // AuthBindingUnresolvable: Tier-2 probe scripted false.
    fixture
        .preflight
        .as_ref()
        .expect("scripted probe")
        .set_binding_resolvable(false);
    let mut spec = sample_portable_member_spec("mob-t8", "auth-miss", "worker");
    spec.overlay.auth_binding = Some(meerkat_contracts::wire::WireAuthBindingRef {
        realm: meerkat_core::RealmId::parse("some-realm").expect("realm slug parses"),
        binding: meerkat_core::BindingId::parse("some-binding").expect("binding slug parses"),
        profile: None,
    });
    let reply = send(sample_materialize_payload(
        &probe,
        1,
        spec,
        1,
        1,
        MaterializeLaunchMode::Fresh {},
    ))
    .await;
    assert!(
        matches!(
            reply,
            BridgeReply::Rejected {
                cause: BridgeRejectionCause::AuthBindingUnresolvable { .. },
                ..
            }
        ),
        "expected AuthBindingUnresolvable, got {reply:?}"
    );
    fixture
        .preflight
        .as_ref()
        .expect("scripted probe")
        .set_binding_resolvable(true);

    // EnvKeyMissing{key}: a required env name absent from the daemon process
    // env (names only ever travel — A2/R5).
    let mut spec = sample_portable_member_spec("mob-t8", "env-miss", "worker");
    spec.required_env_keys = vec!["RKAT_TEST_SURELY_UNSET_KEY_93251".to_string()];
    let reply = send(sample_materialize_payload(
        &probe,
        1,
        spec,
        1,
        1,
        MaterializeLaunchMode::Fresh {},
    ))
    .await;
    assert!(
        matches!(
            &reply,
            BridgeReply::Rejected {
                cause: BridgeRejectionCause::EnvKeyMissing { key },
                ..
            } if key == "RKAT_TEST_SURELY_UNSET_KEY_93251"
        ),
        "expected EnvKeyMissing with the offending name, got {reply:?}"
    );

    // McpCommandMissing{server}: stdio command undiscoverable on the host.
    let mut spec = sample_portable_member_spec("mob-t8", "mcp-miss", "worker");
    spec.profile.tools.mcp_servers.insert(
        "docs".to_string(),
        PortableMcpDecl::Stdio {
            command: "rkat-test-definitely-not-a-binary-49812".to_string(),
            args: Vec::new(),
            required_env_keys: Vec::new(),
            connect_timeout_secs: None,
        },
    );
    let reply = send(sample_materialize_payload(
        &probe,
        1,
        spec,
        1,
        1,
        MaterializeLaunchMode::Fresh {},
    ))
    .await;
    assert!(
        matches!(
            &reply,
            BridgeReply::Rejected {
                cause: BridgeRejectionCause::McpCommandMissing { server },
                ..
            } if server == "docs"
        ),
        "expected McpCommandMissing naming the server, got {reply:?}"
    );

    // MemoryStoreUnavailable arm maps to wire Unavailable (ADJ-20).
    let mut spec = sample_portable_member_spec("mob-t8", "mem-miss", "worker");
    spec.profile.tools.memory = true;
    // The fixture declared memory_store=true at bind; regress the capability
    // fact is a rebind-scale event — instead spin a memory-less fixture.
    let memless = spawn_host_daemon_fixture(HostFixtureOptions {
        memory_store: false,
        ..HostFixtureOptions::named("mat-t8-memless").with_member_build()
    })
    .await
    .expect("memory-less fixture");
    let memprobe = spawn_peer_comms_endpoint("mat-t8-memprobe", true, None).await;
    memprobe.trust(memless.host_peer_descriptor()).await;
    raw_bind(&memprobe, &memless, "mob-t8", 1).await;
    let reply = memprobe
        .send_bridge_command_raw(
            &memless.host_peer_descriptor(),
            &BridgeCommand::MaterializeMember(sample_materialize_payload(
                &memprobe,
                1,
                spec,
                1,
                1,
                MaterializeLaunchMode::Fresh {},
            )),
            REPLY_TIMEOUT,
        )
        .await
        .expect("typed reply");
    match &reply {
        BridgeReply::Rejected { cause, reason } => {
            assert_eq!(*cause, BridgeRejectionCause::Unavailable);
            assert!(
                reason.contains("memory store unavailable on host"),
                "ADJ-20 reason string, got {reason}"
            );
        }
        other => panic!("expected Unavailable, got {other:?}"),
    }
    memless.shutdown().await;

    // R7: nothing half-built anywhere.
    let record = fixture.host_binding_record("mob-t8").await;
    assert!(
        record.materialized.is_empty(),
        "no preflight reject may leave a machine row"
    );

    fixture.shutdown().await;
}

// ===========================================================================
// T8b — Resume launch on an ephemeral substrate ⇒ RealmBackendUnavailable
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn resume_launch_on_ephemeral_substrate_rejects_realm_backend_unavailable() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture = spawn_host_daemon_fixture(HostFixtureOptions::named("mat-t8b-host").ephemeral())
        .await
        .expect("ephemeral member fixture");
    let probe = spawn_peer_comms_endpoint("mat-t8b-probe", true, None).await;
    probe.trust(fixture.host_peer_descriptor()).await;
    raw_bind(&probe, &fixture, "mob-t8b", 1).await;

    let payload = sample_materialize_payload(
        &probe,
        1,
        sample_portable_member_spec("mob-t8b", "b2", "worker"),
        1,
        1,
        MaterializeLaunchMode::Resume {
            session_id: meerkat_core::SessionId::new().to_string(),
        },
    );
    let reply = probe
        .send_bridge_command_raw(
            &fixture.host_peer_descriptor(),
            &BridgeCommand::MaterializeMember(payload),
            REPLY_TIMEOUT,
        )
        .await
        .expect("typed reply");
    assert_rejected(&reply, &BridgeRejectionCause::RealmBackendUnavailable);

    fixture.shutdown().await;
}

// ===========================================================================
// T9 — digest recompute guards the received spec: a lying carried digest
// never reaches the machine dedup rows
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn materialize_digest_recompute_guards_received_spec() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture = member_build_fixture("mat-t9-host").await;
    let probe = spawn_peer_comms_endpoint("mat-t9-supervisor", true, None).await;
    probe.trust(fixture.host_peer_descriptor()).await;
    raw_bind(&probe, &fixture, "mob-t9", 1).await;

    let mut payload = sample_materialize_payload(
        &probe,
        1,
        sample_portable_member_spec("mob-t9", "b2", "worker"),
        1,
        1,
        MaterializeLaunchMode::Fresh {},
    );
    payload.spec_digest = "sha256:0000000000000000000000000000000000000000".to_string();
    let reply = probe
        .send_bridge_command_raw(
            &fixture.host_peer_descriptor(),
            &BridgeCommand::MaterializeMember(payload),
            REPLY_TIMEOUT,
        )
        .await
        .expect("typed reply");
    assert_rejected(&reply, &BridgeRejectionCause::SpecDigestMismatch);

    // The lying digest never seeded a dedup row: an honest send at the SAME
    // tuple runs Fresh and succeeds.
    let honest = sample_materialize_payload(
        &probe,
        1,
        sample_portable_member_spec("mob-t9", "b2", "worker"),
        1,
        1,
        MaterializeLaunchMode::Fresh {},
    );
    let reply = probe
        .send_bridge_command_raw(
            &fixture.host_peer_descriptor(),
            &BridgeCommand::MaterializeMember(honest),
            REPLY_TIMEOUT,
        )
        .await
        .expect("honest materialize");
    assert!(matches!(reply, BridgeReply::MemberMaterialized(_)));

    fixture.shutdown().await;
}

// ===========================================================================
// T10 — resume launch outcomes: live/snapshot resume + unknown id typed
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn materialize_resume_launch_outcomes() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture = member_build_fixture("mat-t10-host").await;
    let probe = spawn_peer_comms_endpoint("mat-t10-supervisor", true, None).await;
    probe.trust(fixture.host_peer_descriptor()).await;
    let fresh_ack = bind_then_materialize(&probe, &fixture, "mob-t10", "b2").await;

    // Superseding resume of the recorded session (the DEC-R6 controlling
    // re-issue shape): outcome is a RESUME class, session id preserved.
    let payload = sample_materialize_payload(
        &probe,
        1,
        sample_portable_member_spec("mob-t10", "b2", "worker"),
        2,
        2,
        MaterializeLaunchMode::Resume {
            session_id: fresh_ack.session_id.clone(),
        },
    );
    let reply = probe
        .send_bridge_command_raw(
            &fixture.host_peer_descriptor(),
            &BridgeCommand::MaterializeMember(payload),
            REPLY_TIMEOUT,
        )
        .await
        .expect("resume materialize");
    let BridgeReply::MemberMaterialized(resumed) = &reply else {
        panic!("expected MemberMaterialized, got {reply:?}");
    };
    assert_eq!(resumed.session_id, fresh_ack.session_id);
    assert_eq!(
        resumed.launch_outcome,
        MaterializeLaunchOutcome::ResumedFromSnapshot,
        "a higher-generation same-session Resume must quiesce and rebuild across the durable event boundary"
    );

    // Resume of an UNKNOWN id: typed ResumeSessionNotFound — never a silent
    // Fresh (§19.F row 1).
    let payload = sample_materialize_payload(
        &probe,
        1,
        sample_portable_member_spec("mob-t10", "b3", "worker"),
        1,
        1,
        MaterializeLaunchMode::Resume {
            session_id: meerkat_core::SessionId::new().to_string(),
        },
    );
    let reply = probe
        .send_bridge_command_raw(
            &fixture.host_peer_descriptor(),
            &BridgeCommand::MaterializeMember(payload),
            REPLY_TIMEOUT,
        )
        .await
        .expect("typed reply");
    assert_rejected(&reply, &BridgeRejectionCause::ResumeSessionNotFound);

    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn same_session_generation_cutover_drains_old_events_before_floor_capture() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let gate = StallGate::new();
    let mut options = HostFixtureOptions::named("mat-cutover-host").with_member_build();
    options.member_llm_client = Some(scripted_member_client_stalling(gate.clone()));
    let fixture = spawn_host_daemon_fixture(options)
        .await
        .expect("member-build fixture");
    let probe = spawn_peer_comms_endpoint("mat-cutover-supervisor", true, None).await;
    probe.trust(fixture.host_peer_descriptor()).await;
    let mob_id = format!("mob-cutover-{}", uuid::Uuid::new_v4().simple());
    let fresh = bind_then_materialize(&probe, &fixture, &mob_id, "cutover").await;
    let member = member_descriptor_from_ack("cutover", &fresh);
    let fresh_incarnation =
        member_incarnation_from_ack(&fixture, &mob_id, "cutover", &fresh, 1, 1, 1);
    probe.trust(member.clone()).await;

    let delivery = raw_deliver_member_input_command(
        &probe,
        &mob_id,
        &uuid::Uuid::new_v4().to_string(),
        "stall in generation one",
        fresh_incarnation.clone(),
        None,
    );
    let reply = probe
        .send_bridge_command_raw(&member, &delivery, REPLY_TIMEOUT)
        .await
        .expect("old-generation delivery reply");
    assert!(matches!(reply, BridgeReply::Delivery(_)));

    // Observe RunStarted so the cutover definitely races a live old turn,
    // rather than merely replacing an idle session.
    let mut cursor = 1;
    let mut saw_run_started = false;
    let mut old_observed_watermark = 0;
    for _ in 0..8 {
        let poll = raw_poll_member_events_command(
            &probe,
            &mob_id,
            1,
            fresh_incarnation.clone(),
            BridgeEventCursor::At {
                generation: 1,
                seq: cursor,
            },
            Some(64),
            Some(5_000),
        );
        let reply = probe
            .send_bridge_command_raw(&member, &poll, REPLY_TIMEOUT)
            .await
            .expect("old-generation event poll");
        let BridgeReply::MemberEventsPage(page) = reply else {
            panic!("expected old-generation event page, got {reply:?}");
        };
        old_observed_watermark = page.watermark;
        saw_run_started |= page.events.iter().any(|row| {
            matches!(
                row.envelope.payload,
                meerkat_core::AgentEvent::RunStarted { .. }
            )
        });
        cursor = page.next_seq;
        if saw_run_started {
            break;
        }
    }
    assert!(
        saw_run_started,
        "the old generation must be live before Resume"
    );

    let payload = sample_materialize_payload(
        &probe,
        1,
        sample_portable_member_spec(&mob_id, "cutover", "worker"),
        2,
        2,
        MaterializeLaunchMode::Resume {
            session_id: fresh.session_id.clone(),
        },
    );
    let reply = probe
        .send_bridge_command_raw(
            &fixture.host_peer_descriptor(),
            &BridgeCommand::MaterializeMember(payload),
            REPLY_TIMEOUT,
        )
        .await
        .expect("generation cutover reply");
    let BridgeReply::MemberMaterialized(resumed) = reply else {
        panic!("expected resumed materialization, got {reply:?}");
    };
    assert_eq!(
        resumed.launch_outcome,
        MaterializeLaunchOutcome::ResumedFromSnapshot
    );

    let record = fixture.host_binding_record(&mob_id).await;
    let floor = record
        .materialized
        .get("cutover")
        .expect("replacement row")
        .generation_start_seq;
    assert!(
        floor > old_observed_watermark,
        "the captured generation floor follows every old row observed before quiescence"
    );

    let resumed_member = member_descriptor_from_ack("cutover", &resumed);
    let resumed_incarnation =
        member_incarnation_from_ack(&fixture, &mob_id, "cutover", &resumed, 1, 2, 2);
    probe.trust(resumed_member.clone()).await;
    let poll = raw_poll_member_events_command(
        &probe,
        &mob_id,
        1,
        resumed_incarnation,
        BridgeEventCursor::At {
            generation: 2,
            seq: 1,
        },
        Some(64),
        Some(1_000),
    );
    let reply = probe
        .send_bridge_command_raw(&resumed_member, &poll, REPLY_TIMEOUT)
        .await
        .expect("replacement-generation event poll");
    let BridgeReply::MemberEventsPage(page) = reply else {
        panic!("expected replacement-generation event page, got {reply:?}");
    };
    assert_eq!(page.generation, 2);
    assert!(page.events.iter().all(|row| row.durable_seq >= floor));
    assert!(
        page.events.iter().all(|row| !matches!(
            row.envelope.payload,
            meerkat_core::AgentEvent::RunStarted { .. }
                | meerkat_core::AgentEvent::RunCompleted { .. }
                | meerkat_core::AgentEvent::RunFailed { .. }
        )),
        "the canceled old turn must be fully drained below the replacement floor: {:?}",
        page.events
    );

    gate.release();
    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn generation_cutover_projection_failure_preserves_replayable_old_owner() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let mut options = HostFixtureOptions::named("mat-cutover-fault-host").with_member_build();
    options.failing_first_projection_drain = true;
    let fixture = spawn_host_daemon_fixture(options)
        .await
        .expect("fault-injected member-build fixture");
    let probe = spawn_peer_comms_endpoint("mat-cutover-fault-supervisor", true, None).await;
    probe.trust(fixture.host_peer_descriptor()).await;
    let mob_id = format!("mob-cutover-fault-{}", uuid::Uuid::new_v4().simple());
    let fresh = bind_then_materialize(&probe, &fixture, &mob_id, "cutover").await;

    let replacement = sample_materialize_payload(
        &probe,
        1,
        sample_portable_member_spec(&mob_id, "cutover", "worker"),
        2,
        2,
        MaterializeLaunchMode::Resume {
            session_id: fresh.session_id.clone(),
        },
    );
    let reply = probe
        .send_bridge_command_raw(
            &fixture.host_peer_descriptor(),
            &BridgeCommand::MaterializeMember(replacement),
            REPLY_TIMEOUT,
        )
        .await
        .expect("faulted cutover reply");
    assert_rejected(&reply, &BridgeRejectionCause::Internal);

    let record = fixture.host_binding_record(&mob_id).await;
    let old_row = record.materialized.get("cutover").expect("old owner row");
    assert_eq!(old_row.generation, 1);
    assert_eq!(old_row.session_id, fresh.session_id);

    // Exact old-tuple replay must revive from the still-authoritative row.
    // The failed higher-generation attempt emitted no success and committed
    // no replacement ownership fact.
    let replay = sample_materialize_payload(
        &probe,
        1,
        sample_portable_member_spec(&mob_id, "cutover", "worker"),
        1,
        1,
        MaterializeLaunchMode::Fresh {},
    );
    let reply = probe
        .send_bridge_command_raw(
            &fixture.host_peer_descriptor(),
            &BridgeCommand::MaterializeMember(replay),
            REPLY_TIMEOUT,
        )
        .await
        .expect("old-owner replay reply");
    let BridgeReply::MemberMaterialized(replayed) = reply else {
        panic!("old durable owner must remain replayable, got {reply:?}");
    };
    assert_eq!(replayed.session_id, fresh.session_id);
    assert_eq!(replayed.launch_outcome, MaterializeLaunchOutcome::Fresh);
    assert!(fixture.member_session_exists(&fresh.session_id).await);

    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn revival_attach_failure_preserves_snapshot_for_exact_replay_retry() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture = member_build_fixture("mat-revival-attach-fault-host").await;
    let probe = spawn_peer_comms_endpoint("mat-revival-attach-fault-supervisor", true, None).await;
    probe.trust(fixture.host_peer_descriptor()).await;
    let mob_id = format!("mob-revival-attach-fault-{}", uuid::Uuid::new_v4().simple());
    let first_ack = bind_then_materialize(&probe, &fixture, &mob_id, "worker").await;

    let fixture = fixture
        .partition()
        .await
        .restore_with_stopping_first_executor_after_ensure()
        .await;
    probe.trust(fixture.host_peer_descriptor()).await;
    assert!(
        fixture.member_session_exists(&first_ack.session_id).await,
        "a failed boot revival tail must preserve the durable snapshot"
    );

    let replayed = replay_exact_materialization(&probe, &fixture, &mob_id, "worker").await;
    assert_eq!(
        replayed, first_ack,
        "the exact recorded tuple must retry revival after the one-shot attach failure"
    );
    assert!(fixture.member_session_exists(&first_ack.session_id).await);

    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn one_session_id_cannot_alias_two_host_member_owners() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture = member_build_fixture("mat-session-owner-host").await;
    let owner = spawn_peer_comms_endpoint("mat-session-owner", true, None).await;
    owner.trust(fixture.host_peer_descriptor()).await;
    let owner_mob = format!("mob-owner-{}", uuid::Uuid::new_v4().simple());
    let owned = bind_then_materialize(&owner, &fixture, &owner_mob, "owner").await;

    let intruder = spawn_peer_comms_endpoint("mat-session-intruder", true, None).await;
    intruder.trust(fixture.host_peer_descriptor()).await;
    let intruder_mob = format!("mob-intruder-{}", uuid::Uuid::new_v4().simple());
    raw_bind(&intruder, &fixture, &intruder_mob, 1).await;
    let alias = sample_materialize_payload(
        &intruder,
        1,
        sample_portable_member_spec(&intruder_mob, "intruder", "worker"),
        1,
        1,
        MaterializeLaunchMode::Resume {
            session_id: owned.session_id.clone(),
        },
    );
    let reply = intruder
        .send_bridge_command_raw(
            &fixture.host_peer_descriptor(),
            &BridgeCommand::MaterializeMember(alias),
            REPLY_TIMEOUT,
        )
        .await
        .expect("cross-owner Resume reply");
    assert_rejected(
        &reply,
        &BridgeRejectionCause::SessionOwnershipConflict {
            session_id: owned.session_id.clone(),
        },
    );

    let intruder_record = fixture.host_binding_record(&intruder_mob).await;
    assert!(
        intruder_record.materialized.is_empty(),
        "the rejected owner must not gain an alias row"
    );
    let owner_record = fixture.host_binding_record(&owner_mob).await;
    let owner_row = owner_record.materialized.get("owner").expect("owner row");
    assert_eq!(owner_row.session_id, owned.session_id);

    let release_alias = raw_release_member_command(&intruder, &intruder_mob, 1, "intruder", 1, 1);
    let reply = intruder
        .send_bridge_command_raw(
            &fixture.host_peer_descriptor(),
            &release_alias,
            REPLY_TIMEOUT,
        )
        .await
        .expect("rejected alias release reply");
    assert!(
        matches!(reply, BridgeReply::Rejected { .. }),
        "an ownerless alias cannot acquire release authority: {reply:?}"
    );
    assert!(
        fixture.member_session_exists(&owned.session_id).await,
        "rejected alias release must not dispose the legitimate owner's session"
    );
    assert_eq!(
        fixture
            .host_binding_record(&owner_mob)
            .await
            .materialized
            .get("owner")
            .expect("owner row after alias release")
            .session_id,
        owned.session_id
    );

    // The conflict does not poison the legitimate owner's exact replay or
    // session residency; owner identity remains the only persistence route.
    let replay = sample_materialize_payload(
        &owner,
        1,
        sample_portable_member_spec(&owner_mob, "owner", "worker"),
        1,
        1,
        MaterializeLaunchMode::Fresh {},
    );
    let reply = owner
        .send_bridge_command_raw(
            &fixture.host_peer_descriptor(),
            &BridgeCommand::MaterializeMember(replay),
            REPLY_TIMEOUT,
        )
        .await
        .expect("legitimate owner replay");
    let BridgeReply::MemberMaterialized(replayed) = reply else {
        panic!("legitimate owner replay must succeed, got {reply:?}");
    };
    assert_eq!(replayed.session_id, owned.session_id);
    assert!(fixture.member_session_exists(&owned.session_id).await);

    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn restart_rejects_durable_duplicate_session_owners() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture = member_build_fixture("mat-session-corrupt-host").await;
    let owner = spawn_peer_comms_endpoint("mat-session-corrupt-owner", true, None).await;
    owner.trust(fixture.host_peer_descriptor()).await;
    let owner_mob = format!("mob-corrupt-owner-{}", uuid::Uuid::new_v4().simple());
    let owned = bind_then_materialize(&owner, &fixture, &owner_mob, "owner").await;

    let intruder = spawn_peer_comms_endpoint("mat-session-corrupt-intruder", true, None).await;
    intruder.trust(fixture.host_peer_descriptor()).await;
    let intruder_mob = format!("mob-corrupt-intruder-{}", uuid::Uuid::new_v4().simple());
    raw_bind(&intruder, &fixture, &intruder_mob, 1).await;

    let owner_row = fixture
        .host_binding_record(&owner_mob)
        .await
        .materialized
        .get("owner")
        .expect("owner row")
        .clone();
    let intruder_bytes = fixture
        .store
        .load_mob_host_binding(&intruder_mob)
        .await
        .expect("load intruder binding")
        .expect("intruder binding present");
    let mut corrupt: meerkat_mob::runtime::host_actor::MobHostBindingRecord =
        serde_json::from_slice(&intruder_bytes).expect("decode intruder binding");
    let mut alias = owner_row;
    alias.spec = sample_portable_member_spec(&intruder_mob, "intruder", "worker");
    alias.spec_digest = portable_member_spec_digest(&alias.spec).expect("digest alias spec");
    alias.supervisor_name = format!("{intruder_mob}/__mob_supervisor__");
    alias.supervisor_address = intruder.runtime.advertised_address();
    assert_eq!(alias.session_id, owned.session_id);
    corrupt.materialized.insert("intruder".to_string(), alias);
    let corrupt_bytes = serde_json::to_vec(&corrupt).expect("encode duplicate-owner binding");
    assert!(
        fixture
            .store
            .compare_and_put_mob_host_binding(&intruder_mob, &intruder_bytes, &corrupt_bytes,)
            .await
            .expect("install duplicate-owner corruption")
    );

    let partitioned = fixture.partition().await;
    match partitioned.restore_result().await {
        Err(_) => {}
        Ok(restored) => {
            restored.shutdown().await;
            panic!("boot must reject durable state that aliases one SessionId to two owners");
        }
    }
}

async fn assert_corrupt_materialized_row_aborts_restart(
    case: &str,
    mutate: impl FnOnce(&mut MaterializedMemberRow),
) {
    let fixture = member_build_fixture(&format!("mat-row-corrupt-{case}-host")).await;
    let owner =
        spawn_peer_comms_endpoint(&format!("mat-row-corrupt-{case}-owner"), true, None).await;
    owner.trust(fixture.host_peer_descriptor()).await;
    let mob_id = format!("mob-row-corrupt-{case}-{}", uuid::Uuid::new_v4().simple());
    let identity = "worker";
    let _owned = bind_then_materialize(&owner, &fixture, &mob_id, identity).await;

    let original_bytes = fixture
        .store
        .load_mob_host_binding(&mob_id)
        .await
        .expect("load binding for corruption")
        .expect("binding exists");
    let mut corrupt: MobHostBindingRecord =
        serde_json::from_slice(&original_bytes).expect("decode binding for corruption");
    mutate(
        corrupt
            .materialized
            .get_mut(identity)
            .expect("materialized row exists"),
    );
    let corrupt_bytes = serde_json::to_vec(&corrupt).expect("encode corrupted binding");
    assert!(
        fixture
            .store
            .compare_and_put_mob_host_binding(&mob_id, &original_bytes, &corrupt_bytes)
            .await
            .expect("install materialized-row corruption")
    );

    let partitioned = fixture.partition().await;
    let error = match partitioned.restore_result().await {
        Err(error) => error,
        Ok(restored) => {
            restored.shutdown().await;
            panic!("boot must reject {case} durable materialized-row corruption");
        }
    };
    assert!(
        matches!(
            error,
            MobHostActorError::DurableMaterializedRowCorrupt { .. }
        ),
        "startup must abort with the typed materialized-row corruption error, got {error:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn restart_rejects_unique_session_with_outer_mob_spec_mismatch() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    assert_corrupt_materialized_row_aborts_restart("mob-key-mismatch", |row| {
        row.spec.mob_id.push_str("-forged");
        row.spec_digest = portable_member_spec_digest(&row.spec).expect("redigest forged spec");
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn restart_rejects_unique_session_with_map_key_spec_identity_mismatch() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    assert_corrupt_materialized_row_aborts_restart("identity-key-mismatch", |row| {
        row.spec.agent_identity = "forged-worker".to_string();
        row.spec_digest = portable_member_spec_digest(&row.spec).expect("redigest forged spec");
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn restart_rejects_unique_session_with_member_pubkey_peer_id_mismatch() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    assert_corrupt_materialized_row_aborts_restart("member-peer-mismatch", |row| {
        row.member_peer_id = meerkat_comms::Keypair::generate()
            .public_key()
            .to_peer_id()
            .to_string();
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn restart_rejects_unique_session_with_zero_generation_start_sequence() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    assert_corrupt_materialized_row_aborts_restart("zero-generation-start-seq", |row| {
        row.generation_start_seq = 0;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn restart_rejects_unique_session_with_zero_member_fence() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    assert_corrupt_materialized_row_aborts_restart("zero-member-fence", |row| {
        row.fence_token = 0;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn restart_accepts_initial_member_generation_zero() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture = member_build_fixture("mat-row-generation-zero-host").await;
    let owner = spawn_peer_comms_endpoint("mat-row-generation-zero-owner", true, None).await;
    owner.trust(fixture.host_peer_descriptor()).await;
    let mob_id = format!("mob-row-generation-zero-{}", uuid::Uuid::new_v4().simple());
    raw_bind(&owner, &fixture, &mob_id, 1).await;
    let payload = sample_materialize_payload(
        &owner,
        1,
        sample_portable_member_spec(&mob_id, "worker", "worker"),
        0,
        1,
        MaterializeLaunchMode::Fresh {},
    );
    let reply = owner
        .send_bridge_command_raw(
            &fixture.host_peer_descriptor(),
            &BridgeCommand::MaterializeMember(payload),
            REPLY_TIMEOUT,
        )
        .await
        .expect("generation-zero materialize reply");
    assert!(
        matches!(reply, BridgeReply::MemberMaterialized(_)),
        "initial generation zero is valid domain state, got {reply:?}"
    );

    let restarted = fixture.restart().await;
    assert_eq!(
        restarted
            .host_binding_record(&mob_id)
            .await
            .materialized
            .get("worker")
            .expect("generation-zero row survives recovery")
            .generation,
        0
    );
    restarted.shutdown().await;
}

// ===========================================================================
// T11 — overlay budget lands in the built session's build options (ADJ-1)
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn materialize_overlay_budget_lands_in_build_options() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture = member_build_fixture("mat-t11-host").await;
    let probe = spawn_peer_comms_endpoint("mat-t11-supervisor", true, None).await;
    probe.trust(fixture.host_peer_descriptor()).await;
    raw_bind(&probe, &fixture, "mob-t11", 1).await;

    let limits = meerkat_core::BudgetLimits {
        max_tokens: Some(4_096),
        max_duration: None,
        max_tool_calls: Some(3),
    };
    let mut spec = sample_portable_member_spec("mob-t11", "b2", "worker");
    spec.overlay.budget_limits = Some(limits.clone());
    let payload =
        sample_materialize_payload(&probe, 1, spec, 1, 1, MaterializeLaunchMode::Fresh {});
    let reply = probe
        .send_bridge_command_raw(
            &fixture.host_peer_descriptor(),
            &BridgeCommand::MaterializeMember(payload),
            REPLY_TIMEOUT,
        )
        .await
        .expect("budgeted materialize");
    let BridgeReply::MemberMaterialized(ack) = &reply else {
        panic!("expected MemberMaterialized, got {reply:?}");
    };
    let session = fixture
        .member_session_service()
        .load_persisted_session(
            &meerkat_core::SessionId::parse(&ack.session_id).expect("session id parses"),
        )
        .await
        .expect("read member session")
        .expect("member session exists");
    assert_eq!(
        session.build_state().and_then(|state| state.budget_limits),
        Some(limits),
        "the digest-covered overlay budget is what the session was built with"
    );

    fixture.shutdown().await;
}

// ===========================================================================
// T12 — build failure after preflight-admit records NOTHING; retry succeeds
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn materialize_build_failure_rolls_back_and_retry_succeeds() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture = spawn_host_daemon_fixture(HostFixtureOptions {
        failing_first_create_session: true,
        ..HostFixtureOptions::named("mat-t12-host").with_member_build()
    })
    .await
    .expect("failing-once fixture");
    let probe = spawn_peer_comms_endpoint("mat-t12-supervisor", true, None).await;
    probe.trust(fixture.host_peer_descriptor()).await;
    raw_bind(&probe, &fixture, "mob-t12", 1).await;

    let payload = sample_materialize_payload(
        &probe,
        1,
        sample_portable_member_spec("mob-t12", "b2", "worker"),
        1,
        1,
        MaterializeLaunchMode::Fresh {},
    );
    let command = BridgeCommand::MaterializeMember(payload);
    let reply = probe
        .send_bridge_command_raw(&fixture.host_peer_descriptor(), &command, REPLY_TIMEOUT)
        .await
        .expect("typed failure reply");
    assert!(
        matches!(reply, BridgeReply::Rejected { .. }),
        "build failure surfaces typed, got {reply:?}"
    );
    assert!(
        fixture
            .failing_create
            .as_ref()
            .expect("failing wrapper")
            .fired(),
        "the injected create_session failure fired"
    );
    // Failures record nothing: no dedup row.
    let record = fixture.host_binding_record("mob-t12").await;
    assert!(record.materialized.is_empty());

    // Retry at the SAME tuple runs Fresh again and succeeds.
    let reply = probe
        .send_bridge_command_raw(&fixture.host_peer_descriptor(), &command, REPLY_TIMEOUT)
        .await
        .expect("retry materialize");
    let BridgeReply::MemberMaterialized(ack) = &reply else {
        panic!("expected MemberMaterialized on retry, got {reply:?}");
    };
    assert!(fixture.member_session_exists(&ack.session_id).await);

    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn materialize_identity_mismatch_preserves_durable_session_and_quiesces_both_identities() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture = spawn_host_daemon_fixture(HostFixtureOptions {
        corrupt_first_create_session_result_identity: true,
        ..HostFixtureOptions::named("mat-identity-mismatch-retry-host").with_member_build()
    })
    .await
    .expect("identity-mismatch fixture");
    let probe =
        spawn_peer_comms_endpoint("mat-identity-mismatch-retry-supervisor", true, None).await;
    probe.trust(fixture.host_peer_descriptor()).await;
    let mob_id = "mob-identity-mismatch-retry";
    raw_bind(&probe, &fixture, mob_id, 1).await;

    let command = BridgeCommand::MaterializeMember(sample_materialize_payload(
        &probe,
        1,
        sample_portable_member_spec(mob_id, "b2", "worker"),
        1,
        1,
        MaterializeLaunchMode::Fresh {},
    ));
    let reply = probe
        .send_bridge_command_raw(&fixture.host_peer_descriptor(), &command, REPLY_TIMEOUT)
        .await
        .expect("identity mismatch rejection");
    let BridgeReply::Rejected { cause, reason } = reply else {
        panic!("corrupted create result must reject, got {reply:?}");
    };
    assert_eq!(cause, BridgeRejectionCause::Internal);
    assert!(
        reason.contains("session service returned"),
        "typed mismatch reason must survive successful cleanup: {reason}"
    );

    let fault = fixture
        .failing_create
        .as_ref()
        .expect("identity-mismatch wrapper");
    let actual = fault
        .actual_session_id()
        .expect("delegated service created a real session");
    let reported = fault
        .reported_session_id()
        .expect("wrapper reported a different session");
    assert_ne!(actual, reported);
    assert_eq!(
        fault.archive_failures_fired(),
        0,
        "post-create identity mismatch must not enter archival compensation"
    );
    let service = fixture.member_session_service();
    let adapter = fixture
        .member_runtime_adapter
        .as_ref()
        .expect("member runtime adapter");
    for (label, session_id) in [("actual", &actual), ("reported", &reported)] {
        assert!(
            !service
                .live_session_actor_registered(session_id)
                .await
                .expect("read live service actor census"),
            "{label} mismatch identity must not retain a live service actor"
        );
        assert!(
            !adapter.contains_session(session_id).await,
            "{label} mismatch identity must not retain runtime registration"
        );
    }
    assert!(
        service
            .load_persisted_session(&actual)
            .await
            .expect("read preserved actual session")
            .is_some(),
        "the real created Fresh snapshot must remain discoverable and resumable"
    );
    assert!(
        !service
            .session_known_to_archive_authority(&actual)
            .await
            .expect("read actual archive authority"),
        "identity mismatch must not archive the already-durable session"
    );
    assert!(
        fixture
            .host_binding_record(mob_id)
            .await
            .materialized
            .is_empty(),
        "an identity mismatch must never commit host materialization authority"
    );

    // Exact volatile cleanup converged, so this was a retryable typed mismatch
    // rather than sticky uncertainty. The same request now builds under an
    // honest result; reaching attachment also proves no stale sidecar survived.
    assert!(matches!(
        probe
            .send_bridge_command_raw(&fixture.host_peer_descriptor(), &command, REPLY_TIMEOUT)
            .await
            .expect("retry after converged mismatch cleanup"),
        BridgeReply::MemberMaterialized(_)
    ));

    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn executor_stop_between_ensure_and_attach_return_cleans_preinstalled_sidecar() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture = spawn_host_daemon_fixture(
        HostFixtureOptions::named("mat-attach-sidecar-race-host")
            .with_member_build()
            .stopping_first_executor_after_ensure(),
    )
    .await
    .expect("post-ensure-stop fixture");
    let probe = spawn_peer_comms_endpoint("mat-attach-sidecar-race-supervisor", true, None).await;
    probe.trust(fixture.host_peer_descriptor()).await;
    raw_bind(&probe, &fixture, "mob-attach-sidecar-race", 1).await;

    let payload = sample_materialize_payload(
        &probe,
        1,
        sample_portable_member_spec("mob-attach-sidecar-race", "b2", "worker"),
        1,
        1,
        MaterializeLaunchMode::Fresh {},
    );
    let command = BridgeCommand::MaterializeMember(payload);
    let reply = tokio::time::timeout(
        Duration::from_secs(5),
        probe.send_bridge_command_raw(&fixture.host_peer_descriptor(), &command, REPLY_TIMEOUT),
    )
    .await
    .expect("failed attachment cleanup must not wait for the 30s reconciliation grace")
    .expect("typed post-ensure stop reply");
    assert!(
        matches!(reply, BridgeReply::Rejected { .. }),
        "the stopped startup incarnation cannot be acknowledged: {reply:?}"
    );
    assert!(
        fixture
            .host_binding_record("mob-attach-sidecar-race")
            .await
            .materialized
            .is_empty(),
        "failed attachment must not commit a materialized row"
    );
    let preserved_session = assert_only_persisted_inert_member_session(&fixture).await;
    let resume = BridgeCommand::MaterializeMember(sample_materialize_payload(
        &probe,
        1,
        sample_portable_member_spec("mob-attach-sidecar-race", "b2", "worker"),
        1,
        1,
        MaterializeLaunchMode::Resume {
            session_id: preserved_session.to_string(),
        },
    ));

    let reply = tokio::time::timeout(
        Duration::from_secs(5),
        probe.send_bridge_command_raw(&fixture.host_peer_descriptor(), &resume, REPLY_TIMEOUT),
    )
    .await
    .expect("same-tuple retry must not encounter a stale cleanup sidecar")
    .expect("retry materialize reply");
    let BridgeReply::MemberMaterialized(ack) = reply else {
        panic!("explicit resume must rebuild the preserved session cleanly, got {reply:?}");
    };
    assert_eq!(ack.session_id, preserved_session.to_string());
    assert_eq!(
        ack.launch_outcome,
        MaterializeLaunchOutcome::ResumedFromSnapshot
    );
    assert!(
        host_status_member_health(&probe, &fixture, "mob-attach-sidecar-race", "b2",).await,
        "retry must publish a fully serving executor incarnation"
    );

    let replayed =
        replay_exact_materialization(&probe, &fixture, "mob-attach-sidecar-race", "b2").await;
    assert_eq!(replayed, ack, "exact replay remains healthy after repair");

    fixture.shutdown().await;
}

// ===========================================================================
// T13 — persist (CAS) failure drops prepared/volatile authority while the
// durable session remains discoverable and explicitly resumable
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn materialize_persist_failure_preserves_resumable_session_and_quiesces_exact_runtime() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture = spawn_host_daemon_fixture(HostFixtureOptions {
        failing_member_region_persistence: true,
        ..HostFixtureOptions::named("mat-t13-host").with_member_build()
    })
    .await
    .expect("failing-CAS fixture");
    let probe = spawn_peer_comms_endpoint("mat-t13-supervisor", true, None).await;
    probe.trust(fixture.host_peer_descriptor()).await;
    raw_bind(&probe, &fixture, "mob-t13", 1).await;

    let payload = sample_materialize_payload(
        &probe,
        1,
        sample_portable_member_spec("mob-t13", "b2", "worker"),
        1,
        1,
        MaterializeLaunchMode::Fresh {},
    );
    let command = BridgeCommand::MaterializeMember(payload);
    let reply = probe
        .send_bridge_command_raw(&fixture.host_peer_descriptor(), &command, REPLY_TIMEOUT)
        .await
        .expect("typed persist-failure reply");
    let BridgeReply::Rejected { cause, reason } = reply else {
        panic!("row-CAS failure must surface typed, got {reply:?}");
    };
    assert_eq!(cause, BridgeRejectionCause::Internal);
    assert!(
        reason.contains("persistence failed") && reason.contains("remains resumable"),
        "definite no-write must preserve the durable session: {reason}"
    );
    let preserved_session = assert_only_persisted_inert_member_session(&fixture).await;
    let record = fixture.host_binding_record("mob-t13").await;
    assert!(record.materialized.is_empty());

    // Explicit resume must traverse the full build/attachment ladder. Reaching
    // the injected CAS failure again proves the old attachment-local sidecar
    // was removed rather than transferred to this new incarnation.
    let resume = BridgeCommand::MaterializeMember(sample_materialize_payload(
        &probe,
        1,
        sample_portable_member_spec("mob-t13", "b2", "worker"),
        1,
        1,
        MaterializeLaunchMode::Resume {
            session_id: preserved_session.to_string(),
        },
    ));
    let reply = probe
        .send_bridge_command_raw(&fixture.host_peer_descriptor(), &resume, REPLY_TIMEOUT)
        .await
        .expect("typed Resume persist-failure reply");
    let BridgeReply::Rejected { cause, reason } = reply else {
        panic!("Resume must reach the injected row-CAS failure, got {reply:?}");
    };
    assert_eq!(cause, BridgeRejectionCause::Internal);
    assert!(
        reason.contains("persistence failed"),
        "Resume must not be blocked by stale attachment-local state: {reason}"
    );
    assert_eq!(
        assert_only_persisted_inert_member_session(&fixture).await,
        preserved_session
    );

    // Removing the persistence fault makes the same explicit Resume publish
    // the preserved session; no implicit recovery choreography is required.
    let fixture = fixture.partition().await.restore().await;
    probe.trust(fixture.host_peer_descriptor()).await;
    let reply = probe
        .send_bridge_command_raw(&fixture.host_peer_descriptor(), &resume, REPLY_TIMEOUT)
        .await
        .expect("Resume after restoring persistence");
    let BridgeReply::MemberMaterialized(ack) = reply else {
        panic!("preserved session must resume successfully, got {reply:?}");
    };
    assert_eq!(ack.session_id, preserved_session.to_string());
    assert_eq!(
        ack.launch_outcome,
        MaterializeLaunchOutcome::ResumedFromSnapshot
    );
    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn same_session_resume_definite_cas_failure_preserves_snapshot_for_retry() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture = member_build_fixture("mat-resume-cas-host").await;
    let probe = spawn_peer_comms_endpoint("mat-resume-cas-supervisor", true, None).await;
    probe.trust(fixture.host_peer_descriptor()).await;
    let mob_id = format!("mob-resume-cas-{}", uuid::Uuid::new_v4().simple());
    let fresh = bind_then_materialize(&probe, &fixture, &mob_id, "worker").await;

    // Reboot over the exact durable owner with a persistence facade that
    // rejects member-region CAS before writing. This is distinct from the
    // post-commit/unknown-outcome fixture: a reread proves the old row is
    // still current, so cleanup may preserve the Resume snapshot and return.
    let fixture = fixture
        .partition()
        .await
        .restore_with_failing_member_region_persistence()
        .await;
    probe.trust(fixture.host_peer_descriptor()).await;
    let replacement = BridgeCommand::MaterializeMember(sample_materialize_payload(
        &probe,
        1,
        sample_portable_member_spec(&mob_id, "worker", "worker"),
        2,
        2,
        MaterializeLaunchMode::Resume {
            session_id: fresh.session_id.clone(),
        },
    ));
    let reply = probe
        .send_bridge_command_raw(&fixture.host_peer_descriptor(), &replacement, REPLY_TIMEOUT)
        .await
        .expect("definite no-write Resume reply");
    let BridgeReply::Rejected { cause, reason } = reply else {
        panic!("injected member-row CAS failure must reject, got {reply:?}");
    };
    assert_eq!(cause, BridgeRejectionCause::Internal);
    assert!(
        reason.contains("persistence failed") && !reason.contains("durably uncertain"),
        "bounded reread must classify this failure as a definite no-write: {reason}"
    );

    let record = fixture.host_binding_record(&mob_id).await;
    let durable_owner = record
        .materialized
        .get("worker")
        .expect("old durable owner remains recorded");
    assert_eq!(
        (durable_owner.generation, durable_owner.fence_token),
        (1, 1)
    );
    assert_eq!(durable_owner.session_id, fresh.session_id);
    let session_id =
        meerkat_core::SessionId::parse(&fresh.session_id).expect("member session id parses");
    assert!(
        fixture
            .member_session_service()
            .load_persisted_session(&session_id)
            .await
            .expect("read preserved Resume snapshot")
            .is_some(),
        "definite no-write cleanup must not archive the same-session Resume snapshot"
    );
    assert!(
        !fixture
            .member_session_service()
            .has_live_session(&session_id)
            .await
            .expect("read post-failure live-session state"),
        "the failed replacement incarnation must quiesce while its snapshot stays durable"
    );

    // Remove the fault by cold-restoring the ordinary persistence facade.
    // Retrying the exact higher-generation Resume must now rebuild from the
    // snapshot that the failed CAS attempt deliberately preserved.
    let fixture = fixture.partition().await.restore().await;
    probe.trust(fixture.host_peer_descriptor()).await;
    let reply = probe
        .send_bridge_command_raw(&fixture.host_peer_descriptor(), &replacement, REPLY_TIMEOUT)
        .await
        .expect("retry Resume after restoring persistence");
    let BridgeReply::MemberMaterialized(resumed) = reply else {
        panic!("preserved snapshot must make the retry succeed, got {reply:?}");
    };
    assert_eq!(resumed.session_id, fresh.session_id);
    assert_eq!(
        resumed.launch_outcome,
        MaterializeLaunchOutcome::ResumedFromSnapshot
    );
    let record = fixture.host_binding_record(&mob_id).await;
    assert_eq!(
        record
            .materialized
            .get("worker")
            .expect("replacement durable owner")
            .generation,
        2
    );

    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn materialize_postcommit_unreadable_row_quiesces_and_fail_stops_until_restart() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture = spawn_host_daemon_fixture(HostFixtureOptions {
        persistence_ack_loss: HostPersistenceAckLossFault::Materialize,
        ..HostFixtureOptions::named("mat-uncertain-host").with_member_build()
    })
    .await
    .expect("postcommit-unreadable fixture");
    let probe = spawn_peer_comms_endpoint("mat-uncertain-supervisor", true, None).await;
    probe.trust(fixture.host_peer_descriptor()).await;
    let mob_id = "mob-materialize-uncertain";
    raw_bind(&probe, &fixture, mob_id, 1).await;

    let command = BridgeCommand::MaterializeMember(sample_materialize_payload(
        &probe,
        1,
        sample_portable_member_spec(mob_id, "b2", "worker"),
        1,
        1,
        MaterializeLaunchMode::Fresh {},
    ));
    let reply = probe
        .send_bridge_command_raw(&fixture.host_peer_descriptor(), &command, REPLY_TIMEOUT)
        .await
        .expect("ambiguous materialize reply");
    let BridgeReply::Rejected { cause, reason } = reply else {
        panic!("unreadable postcommit row must reject, got {reply:?}");
    };
    assert_eq!(cause, BridgeRejectionCause::Internal);
    assert!(
        reason.contains("durably uncertain") && reason.contains("cold recovery"),
        "the first failure must identify the ambiguous durable boundary: {reason}"
    );
    assert!(
        fixture
            .host_binding_record(mob_id)
            .await
            .materialized
            .contains_key("b2"),
        "the injected completion loss occurs after the new member row commits"
    );

    let stopped = probe
        .send_bridge_command_raw(&fixture.host_peer_descriptor(), &command, REPLY_TIMEOUT)
        .await
        .expect("fail-stopped actor reply");
    let BridgeReply::Rejected { cause, reason } = stopped else {
        panic!("uncertain materialize must reject later commands, got {stopped:?}");
    };
    assert_eq!(cause, BridgeRejectionCause::Internal);
    assert!(
        reason.contains("fail-stopped") && reason.contains("restart required"),
        "sticky rejection must identify the restart boundary: {reason}"
    );

    let fixture = fixture.restart().await;
    probe.trust(fixture.host_peer_descriptor()).await;
    assert!(matches!(
        probe
            .send_bridge_command_raw(&fixture.host_peer_descriptor(), &command, REPLY_TIMEOUT)
            .await
            .expect("cold replay reply"),
        BridgeReply::MemberMaterialized(_)
    ));

    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn durable_uncertainty_fail_stop_guards_every_internal_journal_mutation_lane() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture = spawn_host_daemon_fixture(HostFixtureOptions {
        persistence_ack_loss: HostPersistenceAckLossFault::TurnOutcomePending,
        ..HostFixtureOptions::named("journal-uncertain-host").with_member_build()
    })
    .await
    .expect("postcommit-unreadable journal fixture");
    let probe = spawn_peer_comms_endpoint("journal-uncertain-supervisor", true, None).await;
    probe.trust(fixture.host_peer_descriptor()).await;
    let mob_id = format!("mob-journal-uncertain-{}", uuid::Uuid::new_v4().simple());
    let ack = bind_then_materialize(&probe, &fixture, &mob_id, "worker").await;
    let expected_member = member_incarnation_from_ack(&fixture, &mob_id, "worker", &ack, 1, 1, 1);

    let (reply, received) = oneshot::channel();
    fixture
        .actor
        .observation_pending_sender()
        .send(HostTurnOutcomePendingRequest::Reserve {
            expected_member: expected_member.clone(),
            generation: 1,
            fence_token: 1,
            input_id: "committed-before-ack-loss".to_string(),
            fresh_window_start: Some(1),
            reply,
        })
        .await
        .expect("send faulting Pending reservation");
    let first_error = tokio::time::timeout(REPLY_TIMEOUT, received)
        .await
        .expect("faulting reservation reply timeout")
        .expect("faulting reservation reply sender retained")
        .expect_err("post-commit unreadable Pending reservation must reject");
    assert!(
        first_error.contains("uncertain"),
        "first error establishes durable uncertainty: {first_error}"
    );
    let durable_before = fixture.host_binding_record(&mob_id).await;

    let (reply, received) = oneshot::channel();
    fixture
        .actor
        .observation_pending_sender()
        .send(HostTurnOutcomePendingRequest::Reserve {
            expected_member: expected_member.clone(),
            generation: 1,
            fence_token: 1,
            input_id: "blocked-reserve".to_string(),
            fresh_window_start: Some(2),
            reply,
        })
        .await
        .expect("send blocked Pending reservation");
    assert_fail_stopped_reply(received).await;

    let (reply, received) = oneshot::channel();
    fixture
        .actor
        .observation_pending_sender()
        .send(HostTurnOutcomePendingRequest::Cancel {
            expected_member: expected_member.clone(),
            generation: 1,
            fence_token: 1,
            input_id: "committed-before-ack-loss".to_string(),
            reply,
        })
        .await
        .expect("send blocked Pending cancellation");
    assert_fail_stopped_reply(received).await;

    let (reply, received) = oneshot::channel();
    fixture
        .actor
        .observation_record_sender()
        .send(HostTurnOutcomeRecordRequest {
            expected_member: expected_member.clone(),
            record: TrackedTurnOutcomeRecord {
                input_id: "committed-before-ack-loss".to_string(),
                generation: 1,
                fence_token: 1,
                terminal_seq: 1,
                outcome: WireFlowTurnOutcome::RunCompleted,
            },
            reply,
        })
        .await
        .expect("send blocked outcome record");
    assert_fail_stopped_reply(received).await;

    let (reply, received) = oneshot::channel();
    fixture
        .actor
        .observation_ack_sender()
        .send(HostTurnOutcomeAckRequest {
            expected_member,
            acks: vec![BridgeTurnOutcomeAck {
                generation: 1,
                fence_token: 1,
                input_id: "committed-before-ack-loss".to_string(),
            }],
            reply,
        })
        .await
        .expect("send blocked outcome acknowledgement");
    assert_fail_stopped_reply(received).await;

    assert_eq!(
        fixture.host_binding_record(&mob_id).await,
        durable_before,
        "fail-stopped internal lanes must not mutate durable authority"
    );

    fixture.shutdown().await;
}

// ===========================================================================
// T20 — HostStatus reports the materialized inventory + capabilities;
// unbound mob ⇒ NotBound
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn host_status_reports_materialized_set_and_capabilities() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture = member_build_fixture("mat-t20-host").await;
    let probe = spawn_peer_comms_endpoint("mat-t20-supervisor", true, None).await;
    probe.trust(fixture.host_peer_descriptor()).await;
    let ack = bind_then_materialize(&probe, &fixture, "mob-t20", "b2").await;

    let reply = probe
        .send_bridge_command_raw(
            &fixture.host_peer_descriptor(),
            &raw_host_status_command(&probe, "mob-t20", 1),
            REPLY_TIMEOUT,
        )
        .await
        .expect("host status reply");
    let BridgeReply::HostStatus(status) = &reply else {
        panic!("expected HostStatus, got {reply:?}");
    };
    assert!(!status.capabilities.engine_version.is_empty());
    assert!(status.capabilities.durable_sessions);
    assert_eq!(status.members.len(), 1, "live rows only");
    let row = &status.members[0];
    assert_eq!(row.agent_identity, "b2");
    assert_eq!((row.generation, row.fence_token), (1, 1));
    assert_eq!(row.session_id, ack.session_id);
    assert_eq!(row.spec_digest, ack.spec_digest);
    assert!(row.healthy, "live census marks the fresh member healthy");

    // Unbound mob ⇒ NotBound.
    let reply = probe
        .send_bridge_command_raw(
            &fixture.host_peer_descriptor(),
            &raw_host_status_command(&probe, "mob-never-bound", 1),
            REPLY_TIMEOUT,
        )
        .await
        .expect("typed reply");
    assert_rejected(&reply, &BridgeRejectionCause::NotBound);

    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn host_status_marks_stopped_member_unhealthy_and_replay_repairs_it() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture = member_build_fixture("mat-health-stopped-host").await;
    let probe = spawn_peer_comms_endpoint("mat-health-stopped-supervisor", true, None).await;
    probe.trust(fixture.host_peer_descriptor()).await;
    let ack = bind_then_materialize(&probe, &fixture, "mob-health-stopped", "b2").await;
    let session_id =
        meerkat_core::SessionId::parse(&ack.session_id).expect("materialized session id parses");
    let adapter = fixture
        .member_runtime_adapter
        .as_ref()
        .expect("member-build fixture has runtime adapter");

    adapter
        .stop_runtime_executor(&session_id, "host health stopped-state regression")
        .await
        .expect("stop materialized runtime executor");
    assert!(
        matches!(
            adapter.runtime_state(&session_id).await,
            Ok(RuntimeState::Stopped) | Err(_)
        ),
        "stop cleanup may retain the registered Stopped witness or finish unregistering it"
    );
    assert!(
        !host_status_member_health(&probe, &fixture, "mob-health-stopped", "b2").await,
        "a stopped registered member is not serving"
    );

    let replayed = replay_exact_materialization(&probe, &fixture, "mob-health-stopped", "b2").await;
    assert_eq!(replayed, ack, "repair returns the exact recorded ack");
    assert!(
        host_status_member_health(&probe, &fixture, "mob-health-stopped", "b2").await,
        "exact replay must replace the stopped incarnation with a serving one"
    );

    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn host_status_marks_retired_registered_member_unhealthy() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture = member_build_fixture("mat-health-retired-host").await;
    let probe = spawn_peer_comms_endpoint("mat-health-retired-supervisor", true, None).await;
    probe.trust(fixture.host_peer_descriptor()).await;
    let ack = bind_then_materialize(&probe, &fixture, "mob-health-retired", "b2").await;
    let session_id =
        meerkat_core::SessionId::parse(&ack.session_id).expect("materialized session id parses");
    let adapter = fixture
        .member_runtime_adapter
        .as_ref()
        .expect("member-build fixture has runtime adapter");

    adapter
        .retire_runtime(&session_id)
        .await
        .expect("retire materialized runtime");
    assert_eq!(
        adapter
            .runtime_state(&session_id)
            .await
            .expect("read retired runtime state"),
        RuntimeState::Retired
    );
    assert!(
        adapter.contains_session(&session_id).await,
        "retired state must remain registered so registry membership cannot masquerade as health"
    );
    assert!(
        !host_status_member_health(&probe, &fixture, "mob-health-retired", "b2").await,
        "a retired registered member is not serving"
    );

    let replay = sample_materialize_payload(
        &probe,
        1,
        sample_portable_member_spec("mob-health-retired", "b2", "worker"),
        1,
        1,
        MaterializeLaunchMode::Fresh {},
    );
    let reply = probe
        .send_bridge_command_raw(
            &fixture.host_peer_descriptor(),
            &BridgeCommand::MaterializeMember(replay),
            REPLY_TIMEOUT,
        )
        .await
        .expect("retired exact replay reply");
    let BridgeReply::Rejected { cause, reason } = reply else {
        panic!("retired exact replay must fail closed, got {reply:?}");
    };
    assert!(
        matches!(cause, BridgeRejectionCause::ResumeSessionNotFound),
        "retired exact replay must return the stable non-recoverable cause, got {cause:?}"
    );
    assert!(
        reason.contains("controller must advance the member generation"),
        "retired exact replay must tell the controller to replace, not retry, got: {reason}"
    );

    adapter
        .unregister_session(&session_id)
        .await
        .expect("unregister retired runtime before replay repair");
    assert!(!adapter.contains_session(&session_id).await);
    let replay_after_unregister = sample_materialize_payload(
        &probe,
        1,
        sample_portable_member_spec("mob-health-retired", "b2", "worker"),
        1,
        1,
        MaterializeLaunchMode::Fresh {},
    );
    let reply = probe
        .send_bridge_command_raw(
            &fixture.host_peer_descriptor(),
            &BridgeCommand::MaterializeMember(replay_after_unregister),
            REPLY_TIMEOUT,
        )
        .await
        .expect("retired exact replay after unregister reply");
    let BridgeReply::Rejected { cause, reason } = reply else {
        panic!("unregistered Retired replay must remain terminal, got {reply:?}");
    };
    assert!(matches!(cause, BridgeRejectionCause::ResumeSessionNotFound));
    assert!(
        reason.contains("controller must advance the member generation"),
        "durable retired authority must preserve the non-recoverable result after unregister: {reason}"
    );
    assert!(
        !host_status_member_health(&probe, &fixture, "mob-health-retired", "b2").await,
        "a terminal retired incarnation remains unhealthy until a higher generation replaces it"
    );

    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn host_status_marks_member_without_live_mob_drain_unhealthy_and_replay_repairs_it() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture = member_build_fixture("mat-health-drain-host").await;
    let probe = spawn_peer_comms_endpoint("mat-health-drain-supervisor", true, None).await;
    probe.trust(fixture.host_peer_descriptor()).await;
    let ack = bind_then_materialize(&probe, &fixture, "mob-health-drain", "b2").await;
    let session_id =
        meerkat_core::SessionId::parse(&ack.session_id).expect("materialized session id parses");
    let adapter = fixture
        .member_runtime_adapter
        .as_ref()
        .expect("member-build fixture has runtime adapter");

    adapter
        .abort_comms_drain(&session_id)
        .await
        .expect("abort the member's mob-owned peer-ingress drain");
    assert!(
        matches!(
            adapter
                .runtime_state(&session_id)
                .await
                .expect("read runtime state after drain exit"),
            RuntimeState::Attached | RuntimeState::Running
        ),
        "the executor lifecycle remains live so health must specifically witness the drain"
    );
    assert!(
        !host_status_member_health(&probe, &fixture, "mob-health-drain", "b2").await,
        "a member without its mob-owned peer-ingress drain is not serving"
    );

    let replayed = replay_exact_materialization(&probe, &fixture, "mob-health-drain", "b2").await;
    assert_eq!(replayed, ack, "repair returns the exact recorded ack");
    assert!(
        host_status_member_health(&probe, &fixture, "mob-health-drain", "b2").await,
        "exact replay must replace the drain-dead incarnation with a serving one"
    );

    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn host_status_marks_dead_runtime_loop_unhealthy_and_exact_replay_repairs_it() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture = member_build_fixture("mat-health-dead-loop-host").await;
    let probe = spawn_peer_comms_endpoint("mat-health-dead-loop-supervisor", true, None).await;
    probe.trust(fixture.host_peer_descriptor()).await;
    let ack = bind_then_materialize(&probe, &fixture, "mob-health-dead-loop", "b2").await;
    let session_id =
        meerkat_core::SessionId::parse(&ack.session_id).expect("materialized session id parses");
    let adapter = fixture
        .member_runtime_adapter
        .as_ref()
        .expect("member-build fixture has runtime adapter");

    assert!(
        adapter
            .test_abort_runtime_loop_without_cleanup(&session_id)
            .await
            .expect("abort runtime loop without cooperative cleanup"),
        "materialized member must have had a runtime-loop task to abort"
    );
    assert!(
        adapter
            .session_has_executor(&session_id)
            .await
            .expect("read stale generated executor registration"),
        "fault injection deliberately leaves the generated registration Active"
    );
    assert!(
        fixture
            .member_session_service()
            .has_live_session(&session_id)
            .await
            .expect("read lingering service actor"),
        "fault injection deliberately bypasses terminal cleanup and leaves the service actor live"
    );
    assert!(
        !host_status_member_health(&probe, &fixture, "mob-health-dead-loop", "b2").await,
        "closed runtime-loop channels override stale Active/Attached authority"
    );

    let replayed =
        replay_exact_materialization(&probe, &fixture, "mob-health-dead-loop", "b2").await;
    assert_eq!(replayed, ack, "repair returns the exact recorded ack");
    assert!(
        host_status_member_health(&probe, &fixture, "mob-health-dead-loop", "b2").await,
        "exact replay must explicitly discard dead-loop service residue and rebuild within the request"
    );

    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn host_status_remains_healthy_while_the_session_actor_is_busy() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let gate = StallGate::new();
    let mut options = HostFixtureOptions::named("mat-health-busy-service-host").with_member_build();
    options.member_llm_client = Some(scripted_member_client_stalling(gate.clone()));
    let fixture = spawn_host_daemon_fixture(options)
        .await
        .expect("member-build fixture");
    let probe = spawn_peer_comms_endpoint("mat-health-busy-service-supervisor", true, None).await;
    probe.trust(fixture.host_peer_descriptor()).await;
    let ack = bind_then_materialize(&probe, &fixture, "mob-health-busy-service", "b2").await;
    let member = member_descriptor_from_ack("b2", &ack);
    let expected_member =
        member_incarnation_from_ack(&fixture, "mob-health-busy-service", "b2", &ack, 1, 1, 1);
    probe.trust(member.clone()).await;

    let delivery = raw_deliver_member_input_command(
        &probe,
        "mob-health-busy-service",
        &uuid::Uuid::new_v4().to_string(),
        "park the live session actor",
        expected_member.clone(),
        None,
    );
    let reply = probe
        .send_bridge_command_raw(&member, &delivery, REPLY_TIMEOUT)
        .await
        .expect("busy-session delivery reply");
    assert!(matches!(reply, BridgeReply::Delivery(_)));

    let poll = raw_poll_member_events_command(
        &probe,
        "mob-health-busy-service",
        1,
        expected_member,
        BridgeEventCursor::At {
            generation: 1,
            seq: 1,
        },
        Some(64),
        Some(5_000),
    );
    let reply = probe
        .send_bridge_command_raw(&member, &poll, REPLY_TIMEOUT)
        .await
        .expect("busy-session event poll");
    let BridgeReply::MemberEventsPage(page) = reply else {
        panic!("expected busy-session event page, got {reply:?}");
    };
    assert!(
        page.events.iter().any(|row| matches!(
            row.envelope.payload,
            meerkat_core::AgentEvent::RunStarted { .. }
        )),
        "the actor must be parked inside a live provider turn before health observation"
    );

    let healthy = tokio::time::timeout(
        Duration::from_secs(2),
        host_status_member_health(&probe, &fixture, "mob-health-busy-service", "b2"),
    )
    .await
    .expect("host health must not RPC into the busy actor");
    assert!(healthy, "a busy but fully serving actor remains healthy");

    let replayed = tokio::time::timeout(
        Duration::from_secs(2),
        replay_exact_materialization(&probe, &fixture, "mob-health-busy-service", "b2"),
    )
    .await
    .expect("exact replay must not RPC into or tear down the busy actor");
    assert_eq!(replayed, ack, "busy exact replay returns the recorded ack");

    gate.release();
    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn host_status_marks_missing_service_actor_unhealthy_and_exact_replay_repairs_it() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture = member_build_fixture("mat-health-service-host").await;
    let probe = spawn_peer_comms_endpoint("mat-health-service-supervisor", true, None).await;
    probe.trust(fixture.host_peer_descriptor()).await;
    let ack = bind_then_materialize(&probe, &fixture, "mob-health-service", "b2").await;
    let session_id =
        meerkat_core::SessionId::parse(&ack.session_id).expect("materialized session id parses");
    let adapter = fixture
        .member_runtime_adapter
        .as_ref()
        .expect("member-build fixture has runtime adapter");

    fixture
        .member_session_service()
        .discard_live_session(&session_id)
        .await
        .expect("discard only the member's live service actor");
    assert!(
        matches!(
            adapter
                .runtime_state(&session_id)
                .await
                .expect("read runtime state after service discard"),
            RuntimeState::Attached | RuntimeState::Running
        ),
        "machine lifecycle deliberately remains live for this false-healthy adversary"
    );
    assert!(
        adapter
            .session_has_executor(&session_id)
            .await
            .expect("read generated executor registration"),
        "runtime-loop registration deliberately remains Active"
    );
    assert!(
        !host_status_member_health(&probe, &fixture, "mob-health-service", "b2").await,
        "machine/drain tasks cannot serve turns without the session-service actor"
    );

    let replayed = replay_exact_materialization(&probe, &fixture, "mob-health-service", "b2").await;
    assert_eq!(replayed, ack, "repair returns the exact recorded ack");
    assert!(
        host_status_member_health(&probe, &fixture, "mob-health-service", "b2").await,
        "exact replay must rebuild the missing service actor and return fully serving"
    );

    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn host_status_marks_stale_idle_registration_without_executor_unhealthy() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture = member_build_fixture("mat-health-idle-host").await;
    let probe = spawn_peer_comms_endpoint("mat-health-idle-supervisor", true, None).await;
    probe.trust(fixture.host_peer_descriptor()).await;
    let ack = bind_then_materialize(&probe, &fixture, "mob-health-idle", "b2").await;
    let session_id =
        meerkat_core::SessionId::parse(&ack.session_id).expect("materialized session id parses");
    let adapter = fixture
        .member_runtime_adapter
        .as_ref()
        .expect("member-build fixture has runtime adapter");

    // Simulate stale registry reconstruction without the materializer's
    // executor attachment. The materializer still owns its old concrete
    // runtime and sidecar entries, while the newly registered machine is only
    // Idle; registry membership alone must never report this as healthy.
    adapter
        .unregister_session(&session_id)
        .await
        .expect("unregister serving runtime before idle-registration probe");
    assert!(!adapter.contains_session(&session_id).await);
    adapter
        .register_session(session_id.clone())
        .await
        .expect("register runtime without executor");
    assert_eq!(
        adapter
            .runtime_state(&session_id)
            .await
            .expect("read idle runtime state"),
        RuntimeState::Idle
    );
    assert!(adapter.contains_session(&session_id).await);
    assert!(
        !host_status_member_health(&probe, &fixture, "mob-health-idle", "b2").await,
        "an idle registration without an attached executor is not serving"
    );

    fixture.shutdown().await;
}

// ===========================================================================
// T26 — multi-tenancy: a second mob's BindHost still serves while a member
// is materialized for the first (A14, serialized not starved)
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn second_mob_bind_unblocked_by_member_load() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture = member_build_fixture("mat-t26-host").await;
    let probe_one = spawn_peer_comms_endpoint("mat-t26-supervisor-1", true, None).await;
    probe_one.trust(fixture.host_peer_descriptor()).await;
    bind_then_materialize(&probe_one, &fixture, "mob-t26-one", "b2").await;

    let probe_two = spawn_peer_comms_endpoint("mat-t26-supervisor-2", true, None).await;
    probe_two.trust(fixture.host_peer_descriptor()).await;
    let descriptor = fixture.current_descriptor();
    let bind = raw_bind_host_command(&probe_two, "mob-t26-two", &descriptor, 1);
    let reply = probe_two
        .send_bridge_command_raw(&fixture.host_peer_descriptor(), &bind, REPLY_TIMEOUT)
        .await
        .expect("second mob bind serves under member load");
    assert!(matches!(reply, BridgeReply::BindHost(_)), "got {reply:?}");
    assert_eq!(fixture.host_binding_records().await.len(), 2);

    fixture.shutdown().await;
}
