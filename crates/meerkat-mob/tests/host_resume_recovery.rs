//! Multi-host mobs phase 3 — controlling cold-restart with placed members
//! (design-revival R T-15/T-16; §19.L5 gotcha 12 + ADJ-8 fence reseed).
//!
//! The synthetic "placement present but owner_bridge_session_id=None ⇒ typed
//! error, not a silent skip" leg (W-F.2) is an IN-CRATE row (A2 lane): it
//! requires seeding corrupted machine state below the public builder surface.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::time::Duration;

use meerkat_core::HandlingMode;
use meerkat_mob::AgentIdentity;
use meerkat_mob::MobSessionService as _;
use support::{
    HostFixtureOptions, REAL_COMMS_TEST_LOCK, create_controlling_mob,
    scripted_member_client_completing, spawn_host_daemon_fixture,
};

const WAIT: Duration = Duration::from_secs(30);

// ===========================================================================
// R T-15 — reconcile_resume recreates the LOCAL member and SKIPS the placed
// one: no local session is ever created for a remote id (gotcha 12)
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn controlling_restart_skips_local_recreate_for_placed_members() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture =
        spawn_host_daemon_fixture(HostFixtureOptions::named("resume-t15-host").with_member_build())
            .await
            .expect("member-build fixture");
    let controlling = create_controlling_mob("resume-t15").await;
    let report = controlling.bind_fixture(&fixture).await;

    // One local + one placed member.
    controlling
        .handle
        .spawn_spec(meerkat_mob::SpawnMemberSpec::new("worker", "local-1"))
        .await
        .expect("local spawn");
    controlling
        .spawn_placed("worker", "b2", &report.host_id)
        .await
        .expect("placed spawn");
    let placed_row = fixture
        .host_binding_record(controlling.mob_id.as_ref())
        .await
        .materialized
        .get("b2")
        .cloned()
        .expect("placed row");
    let placed_session = meerkat_core::SessionId::parse(&placed_row.session_id).expect("id");

    // Cold restart over the same storage handles.
    let controlling = controlling.restart().await;

    // Both members are in the resumed roster...
    let members = controlling.handle.list_members().await;
    for identity in ["local-1", "b2"] {
        assert!(
            members.iter().any(|entry| entry.agent_identity == identity),
            "{identity} must survive the resume, roster {members:?}"
        );
    }
    // ...the LOCAL one was locally recreated (it serves a turn from the
    // controlling realm)...
    let local = controlling
        .handle
        .member(&AgentIdentity::from("local-1"))
        .await
        .expect("local member handle");
    local
        .send("post-restart local turn", HandlingMode::Queue)
        .await
        .expect("recreated local member serves");
    // ...and the PLACED one was NOT: the controlling realm holds no session
    // for the remote id (the recreate loop skipped it — placement gate).
    assert!(
        controlling
            .service
            .load_persisted_session(&placed_session)
            .await
            .expect("controlling read")
            .is_none(),
        "reconcile_resume must never locally recreate a placed member's session"
    );
    // The placed member still serves THROUGH ITS HOST (ops owner anchored to
    // the owner bridge session; delivery rides the machine peer facts).
    let placed = controlling
        .handle
        .member(&AgentIdentity::from("b2"))
        .await
        .expect("placed member handle");
    tokio::time::timeout(
        WAIT,
        placed.send("post-restart remote turn", HandlingMode::Queue),
    )
    .await
    .expect("delivery does not hang")
    .expect("placed member serves after the controlling restart");

    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn controlling_restart_keeps_started_placed_autonomous_member_running() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let mut options = HostFixtureOptions::named("resume-autonomous-host").with_member_build();
    options.member_llm_client = Some(scripted_member_client_completing("kickoff-complete"));
    let fixture = spawn_host_daemon_fixture(options)
        .await
        .expect("member-build fixture");
    let controlling = create_controlling_mob("resume-autonomous").await;
    let report = controlling.bind_fixture(&fixture).await;
    let member = AgentIdentity::from("remote-autonomous");
    let mut spec = support::placed_spawn_spec("worker", member.as_str(), &report.host_id);
    spec.runtime_mode = Some(meerkat_mob::MobRuntimeMode::AutonomousHost);
    spec.objective_id = Some(meerkat_core::interaction::ObjectiveId::new());
    controlling
        .handle
        .spawn_spec(spec)
        .await
        .expect("placed autonomous spawn");
    controlling
        .handle
        .wait_for_members_kickoff_complete(std::slice::from_ref(&member), Some(WAIT))
        .await
        .expect("placed kickoff completes before restart");

    let controlling = controlling.restart().await;
    assert_eq!(
        controlling
            .handle
            .status()
            .await
            .expect("resumed mob status"),
        meerkat_mob::MobState::Running,
        "cold startup must not run local autonomous readiness against a placed peer-only member"
    );
    let ready = controlling
        .handle
        .wait_for_members_ready(std::slice::from_ref(&member), Some(WAIT))
        .await
        .expect("committed placed incarnation remains ready after restart");
    assert_eq!(ready[0].1.status, meerkat_mob::MobMemberStatus::Active);

    let placed = controlling
        .handle
        .member(&member)
        .await
        .expect("placed autonomous handle after restart");
    tokio::time::timeout(
        WAIT,
        placed.send("post-restart remote turn", HandlingMode::Queue),
    )
    .await
    .expect("post-restart delivery does not hang")
    .expect("placed autonomous member serves after controlling restart");

    fixture.shutdown().await;
}

// ===========================================================================
// R T-16 / ADJ-8 — fence-counter reseed: a post-restart respawn of a
// pre-restart member mints a fence the host admits as Superseding, never
// a spurious StaleFence
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn restart_reseeded_fence_counter_never_draws_stale_fence() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture =
        spawn_host_daemon_fixture(HostFixtureOptions::named("resume-t16-host").with_member_build())
            .await
            .expect("member-build fixture");
    let controlling = create_controlling_mob("resume-t16").await;
    let report = controlling.bind_fixture(&fixture).await;

    controlling
        .spawn_placed("worker", "b2", &report.host_id)
        .await
        .expect("pre-restart placed spawn");
    let before = fixture
        .host_binding_record(controlling.mob_id.as_ref())
        .await
        .materialized
        .get("b2")
        .cloned()
        .expect("pre-restart row");

    let controlling = controlling.restart().await;

    // Retire + respawn the SAME identity post-restart: every fresh mint must
    // sort ABOVE the host-recorded tuple (ADJ-8: next_fence_token reseeded to
    // max(replayed fence maxima) + 1). A spurious StaleFence would fail here.
    controlling
        .handle
        .retire(AgentIdentity::from("b2"))
        .await
        .expect("post-restart retire of a pre-restart member (no spurious StaleFence)");
    controlling
        .spawn_placed("worker", "b2", &report.host_id)
        .await
        .expect("post-restart respawn admits as Superseding, never StaleFence");

    let after = fixture
        .host_binding_record(controlling.mob_id.as_ref())
        .await
        .materialized
        .get("b2")
        .cloned()
        .expect("post-restart row");
    assert!(
        (after.generation, after.fence_token) > (before.generation, before.fence_token),
        "the respawn tuple must sort above the pre-restart tuple: {:?} !> {:?}",
        (after.generation, after.fence_token),
        (before.generation, before.fence_token)
    );

    fixture.shutdown().await;
}
