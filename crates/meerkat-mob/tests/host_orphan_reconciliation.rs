//! Multi-host mobs phase 3 — orphan reconciliation (design-revival W-E rows
//! T-10/T-11 == design-fold-ins T-L8; §9 row 2 / §21.2 one-poll-two-consumers).
//!
//! Two halves, honestly constructed:
//!   * HOST half (real member-build fixture, raw probe AS supervisor): the
//!     "materialized on host, commit lost" orphan, then the SWEEP CONTRACT
//!     driven verb-by-verb — HostStatus read → ReleaseMember at the
//!     HOST-REPORTED tuple → ReleaseAdmitted disposal → replay no-op. Nothing
//!     host-side special-cases reconciliation ("same admission, same reply").
//!   * CONTROLLING half (scripted host): the I7 timeout-exhausted orphan
//!     seed, then the (re)bind-triggered sweep releases at the stale tuple;
//!     a second sweep is a no-op; a CURRENT member is never released.
//!
//! T-11's unknown-mob_id-record leg (typed log + skip) is a controlling-side
//! in-crate row (A2 lane) — the scripted inventory cannot forge records for
//! another mob's key through the mob-keyed serving path.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::time::Duration;

use meerkat_mob::runtime::bridge_protocol::{
    BridgeCommand, BridgeReply, MaterializeLaunchMode,
    MemberSessionDisposal as WireMemberSessionDisposal,
};
use support::{
    HostFixtureOptions, REAL_COMMS_TEST_LOCK, create_controlling_mob, descriptor_to_bind_request,
    raw_bind_host_command, raw_host_status_command, raw_release_member_command,
    sample_materialize_payload, sample_portable_member_spec, spawn_host_daemon_fixture,
    spawn_peer_comms_endpoint, spawn_scripted_host_peer,
};

const REPLY_TIMEOUT: Duration = Duration::from_secs(30);

// ===========================================================================
// T-10 (host half) — orphan released at the host-reported stale tuple; the
// second sweep converges through the release-dedup replay
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn orphan_released_at_stale_fence() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture =
        spawn_host_daemon_fixture(HostFixtureOptions::named("orphan-host").with_member_build())
            .await
            .expect("member-build fixture");
    let probe = spawn_peer_comms_endpoint("orphan-supervisor", true, None).await;
    probe.trust(fixture.host_peer_descriptor()).await;

    // The orphan: materialized on the host, "commit lost" — no ladder ever
    // ran, no roster anywhere.
    let bind = raw_bind_host_command(&probe, "mob-orphan", &fixture.current_descriptor(), 1);
    let reply = probe
        .send_bridge_command_raw(&fixture.host_peer_descriptor(), &bind, REPLY_TIMEOUT)
        .await
        .expect("bind");
    assert!(matches!(reply, BridgeReply::BindHost(_)));
    let payload = sample_materialize_payload(
        &probe,
        1,
        sample_portable_member_spec("mob-orphan", "ghost", "worker"),
        4,
        7,
        MaterializeLaunchMode::Fresh {},
    );
    let reply = probe
        .send_bridge_command_raw(
            &fixture.host_peer_descriptor(),
            &BridgeCommand::MaterializeMember(payload),
            REPLY_TIMEOUT,
        )
        .await
        .expect("orphan materialize");
    let BridgeReply::MemberMaterialized(ack) = &reply else {
        panic!("expected MemberMaterialized, got {reply:?}");
    };

    // Sweep step 1: HostStatus reports the orphan row at ITS tuple.
    let reply = probe
        .send_bridge_command_raw(
            &fixture.host_peer_descriptor(),
            &raw_host_status_command(&probe, "mob-orphan", 1),
            REPLY_TIMEOUT,
        )
        .await
        .expect("host status");
    let BridgeReply::HostStatus(status) = &reply else {
        panic!("expected HostStatus, got {reply:?}");
    };
    let record = status
        .members
        .iter()
        .find(|member| member.agent_identity == "ghost")
        .expect("the orphan row is reported");
    assert_eq!((record.generation, record.fence_token), (4, 7));

    // Sweep step 2: ReleaseMember at the HOST-REPORTED tuple — the host's
    // ResolveReleaseAdmission admits exactly its recorded materialized tuple.
    let release = raw_release_member_command(
        &probe,
        "mob-orphan",
        1,
        "ghost",
        record.generation,
        record.fence_token,
    );
    let reply = probe
        .send_bridge_command_raw(&fixture.host_peer_descriptor(), &release, REPLY_TIMEOUT)
        .await
        .expect("orphan release");
    let BridgeReply::MemberReleased(released) = &reply else {
        panic!("expected MemberReleased, got {reply:?}");
    };
    assert_eq!(released.disposal, WireMemberSessionDisposal::Archived);

    // Host truth: session archived (not live), materialized region empty,
    // release-dedup row present; the next HostStatus reports zero members.
    let session_id = meerkat_core::SessionId::parse(&ack.session_id).expect("session id");
    assert!(
        !fixture
            .member_session_service()
            .has_live_session(&session_id)
            .await
            .unwrap_or(true),
        "the orphan session was disposed"
    );
    let record = fixture.host_binding_record("mob-orphan").await;
    assert!(record.materialized.is_empty());
    assert!(record.released.contains_key("ghost"));
    let reply = probe
        .send_bridge_command_raw(
            &fixture.host_peer_descriptor(),
            &raw_host_status_command(&probe, "mob-orphan", 1),
            REPLY_TIMEOUT,
        )
        .await
        .expect("post-release host status");
    let BridgeReply::HostStatus(status) = &reply else {
        panic!("expected HostStatus, got {reply:?}");
    };
    assert!(
        status.members.is_empty(),
        "zero members after reconciliation, got {:?}",
        status.members
    );

    // Second sweep is a no-op: the identical release converges through the
    // dedup replay with the recorded disposal.
    let reply = probe
        .send_bridge_command_raw(&fixture.host_peer_descriptor(), &release, REPLY_TIMEOUT)
        .await
        .expect("replayed orphan release");
    let BridgeReply::MemberReleased(replayed) = &reply else {
        panic!("expected replayed MemberReleased, got {reply:?}");
    };
    assert_eq!(replayed.disposal, released.disposal);

    fixture.shutdown().await;
}

// ===========================================================================
// T-10 (controlling half) — a host-only orphan seed is reclaimed by the
// (re)bind-triggered sweep; the roster never showed the orphan
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn rebind_sweep_reclaims_the_lost_commit_orphan() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let scripted = spawn_scripted_host_peer("orphan-scripted").await;
    let controlling = create_controlling_mob("orphan-sweep").await;
    controlling.bind_scripted(&scripted).await;

    // Seed the post-crash shape directly on the scripted host. The normal
    // failed-spawn path now synchronously releases/certifies absence before
    // returning, so it is not an honest way to retain an orphan for this
    // independently-owned rebind sweep.
    scripted.seed_orphan_member_row("ghost", 0, 1);
    assert_eq!(scripted.scripted_member_rows().len(), 1, "orphan row held");
    assert!(
        controlling.handle.list_members().await.is_empty(),
        "the roster NEVER shows a member the machine didn't commit"
    );

    // Trigger the sweep: re-bind (HostRebound absorption is a trigger site).
    controlling
        .handle
        .bind_host(descriptor_to_bind_request(&scripted.descriptor))
        .await
        .expect("re-bind converges and triggers the sweep");
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if scripted.scripted_member_rows().is_empty()
                && !scripted.received_release_payloads().is_empty()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("rebind-triggered reconciliation releases the host-only orphan");

    // The sweep read HostStatus and released the orphan at the HOST-REPORTED
    // (stale) tuple.
    let releases = scripted.received_release_payloads();
    assert_eq!(releases.len(), 1, "exactly one orphan release");
    assert_eq!(releases[0].agent_identity, "ghost");
    let (_, orphan_generation, orphan_fence) = {
        // The row was pruned on release; the release payload carries the
        // tuple the host reported.
        ("ghost", releases[0].generation, releases[0].fence_token)
    };
    // The release rides the exact host-reported fixture tuple.
    assert_eq!(orphan_generation, 0);
    assert_eq!(orphan_fence, 1);
    assert!(
        scripted.scripted_member_rows().is_empty(),
        "the host inventory is clean after the sweep"
    );

    // Second sweep is a no-op: another rebind sends NO further release.
    controlling
        .handle
        .bind_host(descriptor_to_bind_request(&scripted.descriptor))
        .await
        .expect("second re-bind");
    assert_eq!(
        scripted.received_release_payloads().len(),
        1,
        "an empty inventory sweeps to a no-op"
    );

    scripted.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn periodic_host_status_poll_reclaims_orphan_without_rebind() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let scripted = spawn_scripted_host_peer("orphan-periodic-scripted").await;
    let controlling = create_controlling_mob("orphan-periodic").await;
    let report = controlling.bind_scripted(&scripted).await;

    // Create the host-only orphan after the eager bind sweep has already
    // completed. No later bind/rebind is allowed to rescue it.
    scripted.seed_orphan_member_row("ghost", 0, 1);
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if scripted.scripted_member_rows().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("periodic HostStatus/reconciliation loop must reclaim the orphan");
    assert_eq!(
        scripted.received_release_payloads().len(),
        1,
        "the shared periodic poll releases the orphan exactly once"
    );

    let hosts = controlling.handle.hosts().expect("host projection");
    let host = hosts
        .hosts
        .iter()
        .find(|host| host.host_id.0 == report.host_id)
        .expect("bound host row");
    assert_eq!(
        host.control_reachability,
        Some(meerkat_contracts::wire::WireReachability::Reachable)
    );
    assert!(host.last_seen_ms.is_some());
    assert_eq!(host.freshness_reason.as_deref(), Some("host_status_ack"));

    scripted.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn partitioned_periodic_host_poll_never_stalls_actor_commands() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let scripted = spawn_scripted_host_peer("host-poll-partitioned").await;
    let controlling = create_controlling_mob("host-poll-nonblocking").await;
    controlling.bind_scripted(&scripted).await;

    // Bind performs an eager status read. Drop the next reply only after
    // that ceremony, then wait until the periodic driver has issued it. The
    // bridge request remains in flight for its 10s deadline.
    let baseline = scripted.host_status_count();
    scripted.drop_next_host_status_replies(1);
    tokio::time::timeout(Duration::from_secs(8), async {
        while scripted.host_status_count() == baseline {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("periodic HostStatus request starts");

    // Actor-routed lifecycle reads remain prompt while that network wait is
    // outstanding. Running the poll inline on MobActor would block this for
    // the full bridge timeout.
    let phase = tokio::time::timeout(Duration::from_secs(1), controlling.handle.status())
        .await
        .expect("a partitioned host poll must not occupy the mob actor")
        .expect("mob phase read");
    assert_eq!(phase, meerkat_mob::MobState::Running);

    scripted.shutdown();
}

// ===========================================================================
// T-11 — the sweep never releases a CURRENT member (host tuple == machine
// live tuple ⇒ confirmed, nothing on the wire)
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn sweep_never_releases_a_current_member() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let scripted = spawn_scripted_host_peer("current-scripted").await;
    let controlling = create_controlling_mob("current-sweep").await;
    let report = controlling.bind_scripted(&scripted).await;

    controlling
        .spawn_placed("worker", "b2", &report.host_id)
        .await
        .expect("placed spawn commits");

    let status = controlling
        .handle
        .member_status(&meerkat_mob::AgentIdentity::from("b2"))
        .await
        .expect("placed member status");
    assert_eq!(status.placement.as_deref(), Some(report.host_id.as_str()));
    assert_eq!(
        status.control_reachability,
        Some(meerkat_contracts::wire::WireReachability::Reachable)
    );
    assert_eq!(
        status.lifecycle_capabilities,
        Some(meerkat_contracts::wire::WireMemberLifecycleCapabilities {
            transcript_edits: false,
            revisions: false,
            resume_after_restart: true,
        })
    );
    assert_eq!(status.non_portable_disabled, Some(Vec::new()));

    // Trigger the sweep via re-bind: the host row tuple equals the machine's
    // live tuple ⇒ confirmed, NO ReleaseMember on the wire.
    controlling
        .handle
        .bind_host(descriptor_to_bind_request(&scripted.descriptor))
        .await
        .expect("re-bind triggers the sweep");
    assert_eq!(
        scripted.release_count(),
        0,
        "a current member must never be swept"
    );
    assert!(
        controlling
            .handle
            .list_members()
            .await
            .iter()
            .any(|entry| entry.agent_identity == "b2"),
        "the committed member survives the sweep"
    );

    scripted.shutdown();
}
