//! Multi-host mobs phase 3 — member RELEASE, both sides of the wire:
//!   * controlling-side remote retire realization (design-revival R T-5..T-9,
//!     ADJ-10 retry convergence; == F T-L9's controlling half), and
//!   * member-host `ReleaseMember` serving (design-host H T14–T17: full
//!     disposal, replay/fence matrix, ephemeral degradation, already-archived
//!     fold; == F T-L9's host half — merge noted).
//!
//! The wire→machine disposal-fold TABLE rows (R T-3/T-4, F T-U4) live in
//! `host_release_disposal.rs`.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::time::Duration;

use meerkat_core::agent::CommsRuntime as _;
use meerkat_core::{CommsCommand, HandlingMode, PeerRoute};
use meerkat_mob::AgentIdentity;
use meerkat_mob::MobSessionService as _;
use meerkat_mob::runtime::bridge_protocol::{
    BridgeRejectionCause, BridgeReply, MemberSessionDisposal as WireMemberSessionDisposal,
    RuntimeReleaseCause,
};
use support::{
    HostFixtureOptions, REAL_COMMS_TEST_LOCK, bind_then_materialize, create_controlling_mob,
    raw_release_member_command, spawn_host_daemon_fixture, spawn_peer_comms_endpoint,
    spawn_scripted_host_peer,
};

const REPLY_TIMEOUT: Duration = Duration::from_secs(30);

// ===========================================================================
// R T-5 — remote retire end-to-end: machine tuple → ReleaseMember → durable
// archive ON HOST B → disposal witness recorded; nothing archived locally
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn remote_retire_releases_on_host_b_and_records_archived_disposal() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture =
        spawn_host_daemon_fixture(HostFixtureOptions::named("rel-t5-host").with_member_build())
            .await
            .expect("member-build fixture");
    let controlling = create_controlling_mob("rel-t5").await;
    let report = controlling.bind_fixture(&fixture).await;

    controlling
        .spawn_placed("worker", "b2", &report.host_id)
        .await
        .expect("placed spawn commits");
    let row = fixture
        .host_binding_record(controlling.mob_id.as_ref())
        .await
        .materialized
        .get("b2")
        .cloned()
        .expect("materialized row");
    let session_id = meerkat_core::SessionId::parse(&row.session_id).expect("session id");

    controlling
        .handle
        .retire(AgentIdentity::from("b2"))
        .await
        .expect("remote retire converges through ReleaseMember");

    // Host B: durably archived (commit-first), release region populated,
    // materialized region cleared.
    let record = fixture
        .host_binding_record(controlling.mob_id.as_ref())
        .await;
    assert!(
        record.materialized.is_empty(),
        "the materialized row moves to the release region"
    );
    let released = record.released.get("b2").expect("release-dedup row for b2");
    assert_eq!(
        (released.generation, released.fence_token),
        (row.generation, row.fence_token),
        "release recorded at the machine tuple"
    );
    let member_service = fixture.member_session_service();
    assert!(
        !member_service
            .has_live_session(&session_id)
            .await
            .unwrap_or(true),
        "no live runtime session may survive the release quiesce"
    );

    // Controlling side: the roster no longer lists b2, and the controlling
    // service never held (so never archived) the member session.
    assert!(
        !controlling
            .handle
            .list_members()
            .await
            .iter()
            .any(|entry| entry.agent_identity == "b2"),
        "retired member leaves the roster"
    );
    assert!(
        controlling
            .service
            .load_persisted_session(&session_id)
            .await
            .expect("controlling read")
            .is_none(),
        "no local archive may be attempted for a placed member (§19.L3)"
    );

    fixture.shutdown().await;
}

// ===========================================================================
// R T-6 — ephemeral host: retire SUCCEEDS with the typed degradation
// RuntimeReleasedOnly{NoDurableSessions}
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn remote_retire_on_ephemeral_host_is_typed_degradation_not_failure() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture = spawn_host_daemon_fixture(HostFixtureOptions::named("rel-t6-host").ephemeral())
        .await
        .expect("ephemeral member fixture");
    let controlling = create_controlling_mob("rel-t6").await;
    let report = controlling.bind_fixture(&fixture).await;

    controlling
        .spawn_placed("worker", "b2", &report.host_id)
        .await
        .expect("placed spawn commits on the ephemeral host");
    controlling
        .handle
        .retire(AgentIdentity::from("b2"))
        .await
        .expect("retire succeeds — typed degradation, not failure (D5)");

    let record = fixture
        .host_binding_record(controlling.mob_id.as_ref())
        .await;
    let released = record.released.get("b2").expect("release row");
    let disposal = serde_json::to_value(released.disposal).expect("disposal serializes");
    assert!(
        disposal
            .to_string()
            .to_lowercase()
            .contains("no_durable_sessions"),
        "machine records RuntimeReleasedOnlyNoDurableSessions, got {disposal}"
    );

    fixture.shutdown().await;
}

// ===========================================================================
// R T-7/T-8 — release failure is LOUD; retry re-realizes from recorded
// facts and converges (ADJ-10), disposal recorded exactly once
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn failed_release_is_typed_and_retry_converges_at_the_recorded_tuple() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let scripted = spawn_scripted_host_peer("rel-t7-scripted").await;
    let controlling = create_controlling_mob("rel-t7").await;
    let report = controlling.bind_scripted(&scripted).await;

    controlling
        .spawn_placed("worker", "b2", &report.host_id)
        .await
        .expect("placed spawn commits");

    scripted.fail_next_release(
        BridgeRejectionCause::Unavailable,
        "injected release failure",
    );
    let error = controlling
        .handle
        .retire(AgentIdentity::from("b2"))
        .await
        .expect_err("a failed release must surface typed — ArchiveSession is critical");
    assert!(
        !format!("{error:?}").is_empty(),
        "typed bridge cause expected"
    );
    assert_eq!(
        scripted.release_count(),
        1,
        "an authenticated rejection certifies no effect; cleanup must not hide it behind an internal retry"
    );

    // Retry converges: shell re-realization from the machine-recorded
    // (generation, fence, host) facts — SAME tuple on the wire (ADJ-10).
    controlling
        .handle
        .retire(AgentIdentity::from("b2"))
        .await
        .expect("retire retry converges");
    let payloads = scripted.received_release_payloads();
    assert_eq!(
        payloads.len(),
        2,
        "one failed attempt plus one caller-owned converging retry on the wire"
    );
    let first = &payloads[0];
    let last = payloads.last().expect("last release payload");
    assert_eq!(
        (
            first.agent_identity.as_str(),
            first.generation,
            first.fence_token
        ),
        (
            last.agent_identity.as_str(),
            last.generation,
            last.fence_token
        ),
        "the retry re-realizes the SAME machine-recorded tuple"
    );
    assert!(
        !controlling
            .handle
            .list_members()
            .await
            .iter()
            .any(|entry| entry.agent_identity == "b2"),
        "the member is retired after convergence"
    );

    scripted.shutdown();
}

// ===========================================================================
// R T-9 — destroy path drives the release ceremony for placed members
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn destroy_releases_placed_members_and_reports_clean() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let scripted = spawn_scripted_host_peer("rel-t9-scripted").await;
    let controlling = create_controlling_mob("rel-t9").await;
    let report = controlling.bind_scripted(&scripted).await;

    controlling
        .spawn_placed("worker", "b2", &report.host_id)
        .await
        .expect("placed spawn commits");
    let destroy = controlling
        .handle
        .destroy()
        .await
        .expect("destroy with a placed member completes");
    assert!(
        destroy.orphaned_remote_members.is_empty(),
        "the release ceremony ran for the placed member: {destroy:?}"
    );
    assert_eq!(
        scripted.release_count(),
        1,
        "placed destroy realizes exactly one remote ReleaseMember and no local session-detach substitute"
    );

    scripted.shutdown();
}

// ===========================================================================
// H T14 — host-side raw release: quiesce→archive order, registry removal,
// row moves to the release region
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn release_full_disposal_archives_and_removes_the_member_identity() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture =
        spawn_host_daemon_fixture(HostFixtureOptions::named("rel-t14-host").with_member_build())
            .await
            .expect("member-build fixture");
    let probe = spawn_peer_comms_endpoint("rel-t14-supervisor", true, None).await;
    probe.trust(fixture.host_peer_descriptor()).await;
    let ack = bind_then_materialize(&probe, &fixture, "mob-t14", "b2").await;

    let reply = probe
        .send_bridge_command_raw(
            &fixture.host_peer_descriptor(),
            &raw_release_member_command(&probe, "mob-t14", 1, "b2", 1, 1),
            REPLY_TIMEOUT,
        )
        .await
        .expect("release reply");
    let BridgeReply::MemberReleased(released) = &reply else {
        panic!("expected MemberReleased, got {reply:?}");
    };
    assert_eq!(
        released.disposal,
        WireMemberSessionDisposal::Archived,
        "persistent substrate ⇒ commit-first archive"
    );

    // Durable regions: materialized cleared, release row present.
    let record = fixture.host_binding_record("mob-t14").await;
    assert!(record.materialized.is_empty());
    assert!(record.released.contains_key("b2"));

    // Registry entry REMOVED: a member-addressed envelope is now
    // misaddressed (no ack — the sender sees the peer offline).
    let member_descriptor = support::member_descriptor_from_ack("mob-t14/b2", &ack);
    probe.trust(member_descriptor.clone()).await;
    let refused = probe
        .runtime
        .send(CommsCommand::PeerMessage {
            content_taint: None,
            to: PeerRoute::with_display_name(member_descriptor.peer_id, member_descriptor.name),
            body: "after release".to_string(),
            blocks: None,
            handling_mode: HandlingMode::Queue,
            objective_id: None,
        })
        .await;
    assert!(
        refused.is_err(),
        "a released identity must reject like an unregistered one: {refused:?}"
    );

    fixture.shutdown().await;
}

// ===========================================================================
// H T15 — release replay + fence matrix: Replay / StaleFence / Unsupported
// (ahead + unknown; "never silently absorbed")
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn release_replay_and_fence_matrix() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture =
        spawn_host_daemon_fixture(HostFixtureOptions::named("rel-t15-host").with_member_build())
            .await
            .expect("member-build fixture");
    let probe = spawn_peer_comms_endpoint("rel-t15-supervisor", true, None).await;
    probe.trust(fixture.host_peer_descriptor()).await;
    bind_then_materialize(&probe, &fixture, "mob-t15", "b2").await;

    let send = |command| {
        let probe = &probe;
        let fixture = &fixture;
        async move {
            probe
                .send_bridge_command_raw(&fixture.host_peer_descriptor(), &command, REPLY_TIMEOUT)
                .await
                .expect("typed release reply")
        }
    };

    // Tuple BELOW the materialized row ⇒ StaleFence (materialized at (1,1);
    // (0,1) sorts below).
    let reply = send(raw_release_member_command(&probe, "mob-t15", 1, "b2", 0, 1)).await;
    assert!(
        matches!(
            reply,
            BridgeReply::Rejected {
                cause: BridgeRejectionCause::StaleFence,
                ..
            }
        ),
        "expected StaleFence, got {reply:?}"
    );

    // Tuple ABOVE the materialized row ⇒ Unsupported ("a member state this
    // host never produced" — never silently absorbed).
    let reply = send(raw_release_member_command(&probe, "mob-t15", 1, "b2", 3, 9)).await;
    assert!(
        matches!(
            reply,
            BridgeReply::Rejected {
                cause: BridgeRejectionCause::Unsupported,
                ..
            }
        ),
        "expected Unsupported for the ahead tuple, got {reply:?}"
    );

    // Unknown member ⇒ Unsupported.
    let reply = send(raw_release_member_command(
        &probe, "mob-t15", 1, "ghost", 1, 1,
    ))
    .await;
    assert!(
        matches!(
            reply,
            BridgeReply::Rejected {
                cause: BridgeRejectionCause::Unsupported,
                ..
            }
        ),
        "expected Unsupported for an unknown member, got {reply:?}"
    );

    // Exact tuple ⇒ Admitted disposal; replay of the released tuple ⇒
    // ReleaseReplay with the RECORDED disposal, no second archive (the
    // archive-authority idempotency is pinned by byte-equal replies).
    let release = raw_release_member_command(&probe, "mob-t15", 1, "b2", 1, 1);
    let first = send(release.clone()).await;
    let BridgeReply::MemberReleased(first_released) = &first else {
        panic!("expected MemberReleased, got {first:?}");
    };
    assert_eq!(first_released.disposal, WireMemberSessionDisposal::Archived);
    let replay = send(release).await;
    let BridgeReply::MemberReleased(replayed) = &replay else {
        panic!("expected replayed MemberReleased, got {replay:?}");
    };
    assert_eq!(
        replayed.disposal, first_released.disposal,
        "replay maps the RECORDED machine disposal through the same table"
    );

    fixture.shutdown().await;
}

// ===========================================================================
// H T16 — ephemeral substrate: raw release ⇒ RuntimeReleasedOnly
// {NoDurableSessions} on the wire
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn release_on_ephemeral_host_is_runtime_released_only() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture = spawn_host_daemon_fixture(HostFixtureOptions::named("rel-t16-host").ephemeral())
        .await
        .expect("ephemeral member fixture");
    let probe = spawn_peer_comms_endpoint("rel-t16-supervisor", true, None).await;
    probe.trust(fixture.host_peer_descriptor()).await;
    bind_then_materialize(&probe, &fixture, "mob-t16", "b2").await;

    let reply = probe
        .send_bridge_command_raw(
            &fixture.host_peer_descriptor(),
            &raw_release_member_command(&probe, "mob-t16", 1, "b2", 1, 1),
            REPLY_TIMEOUT,
        )
        .await
        .expect("release reply");
    let BridgeReply::MemberReleased(released) = &reply else {
        panic!("expected MemberReleased, got {reply:?}");
    };
    assert_eq!(
        released.disposal,
        WireMemberSessionDisposal::RuntimeReleasedOnly {
            cause: RuntimeReleaseCause::NoDurableSessions
        },
        "capability degradation declared at bind, never silent"
    );

    fixture.shutdown().await;
}

// ===========================================================================
// H T17 — out-of-band-archived session folds success-class: wire
// AlreadyArchived, machine records Archived
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn release_already_archived_folds_success_class() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture =
        spawn_host_daemon_fixture(HostFixtureOptions::named("rel-t17-host").with_member_build())
            .await
            .expect("member-build fixture");
    let probe = spawn_peer_comms_endpoint("rel-t17-supervisor", true, None).await;
    probe.trust(fixture.host_peer_descriptor()).await;
    let ack = bind_then_materialize(&probe, &fixture, "mob-t17", "b2").await;

    // Archive the member's session out-of-band through the service.
    let session_id = meerkat_core::SessionId::parse(&ack.session_id).expect("session id");
    fixture
        .member_session_service()
        .archive_with_mob_lifecycle_authority(&session_id)
        .await
        .expect("out-of-band archive");

    let reply = probe
        .send_bridge_command_raw(
            &fixture.host_peer_descriptor(),
            &raw_release_member_command(&probe, "mob-t17", 1, "b2", 1, 1),
            REPLY_TIMEOUT,
        )
        .await
        .expect("release reply");
    let BridgeReply::MemberReleased(released) = &reply else {
        panic!("expected MemberReleased, got {reply:?}");
    };
    assert_eq!(
        released.disposal,
        WireMemberSessionDisposal::AlreadyArchived,
        "the wire keeps the AlreadyArchived distinction"
    );

    // Machine record folds it to Archived (DEC-P3H-6): the release-dedup row
    // never holds AlreadyArchived, so a REPLAY reports Archived.
    let record = fixture.host_binding_record("mob-t17").await;
    let disposal = serde_json::to_value(record.released.get("b2").expect("row").disposal)
        .expect("disposal serializes");
    assert!(
        !disposal.to_string().to_lowercase().contains("already"),
        "machine records the Archived fold, got {disposal}"
    );
    let replay = probe
        .send_bridge_command_raw(
            &fixture.host_peer_descriptor(),
            &raw_release_member_command(&probe, "mob-t17", 1, "b2", 1, 1),
            REPLY_TIMEOUT,
        )
        .await
        .expect("replay reply");
    let BridgeReply::MemberReleased(replayed) = &replay else {
        panic!("expected replayed MemberReleased, got {replay:?}");
    };
    assert_eq!(
        replayed.disposal,
        WireMemberSessionDisposal::Archived,
        "AlreadyArchived is never replayed — the recorded fold is Archived"
    );

    fixture.shutdown().await;
}
