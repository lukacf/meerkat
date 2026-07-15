//! Multi-host mobs phase 3 — member REVIVAL, both flavors:
//!   * HOST-AUTONOMOUS boot revival (A20/§14.6/§15.7): daemon restart
//!     recomposes members from the durable spec row with ZERO bridge traffic
//!     (design-revival R T-12 == design-host T18 == design-fold-ins T-F3 —
//!     merge: the policy/budget recompose pins ride the same walk);
//!   * revival BUILD failure is typed, never half-alive (H T19);
//!   * CONTROLLING-side revival of a committed placed member (R T-13,
//!     DEC-R6 + ADJ-9 ensure-on-replay at the SAME recorded tuple);
//!   * Broken refuses retry (R T-14 == F T-L7).

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::time::Duration;

use meerkat_core::HandlingMode;
use meerkat_core::agent::CommsRuntime as _;
use meerkat_core::{CommsCommand, PeerRoute, SendReceipt};
use meerkat_mob::AgentIdentity;
use meerkat_mob::MobError;
use meerkat_mob::runtime::bridge_protocol::BridgeReply;
use support::{
    HostFixtureOptions, REAL_COMMS_TEST_LOCK, bind_then_materialize, create_controlling_mob,
    member_descriptor_from_ack, raw_host_status_command, spawn_host_daemon_fixture,
    spawn_peer_comms_endpoint,
};

const REPLY_TIMEOUT: Duration = Duration::from_secs(30);

async fn wait_until<F, Fut>(what: &str, mut predicate: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    tokio::time::timeout(REPLY_TIMEOUT, async {
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

/// Delete the member's durable session rows from the fixture realm — the
/// R-design `wipe_durable_session` injector (JSONL session store).
fn wipe_durable_session(realm: &support::MemberRealm, session_id: &str) {
    let sessions_root = &realm.paths.sessions_root;
    let entries = match std::fs::read_dir(sessions_root) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().contains(session_id) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

// ===========================================================================
// R T-12 / H T18 / F T-F3 — host-autonomous revival: restart over the SAME
// store + identity + realm recomposes from the stored spec, zero bridge
// traffic, policy + budget + member pubkey preserved
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn restart_revives_member_from_stored_spec_with_zero_bridge_traffic() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture =
        spawn_host_daemon_fixture(HostFixtureOptions::named("rev-t12-host").with_member_build())
            .await
            .expect("member-build fixture");
    let probe = spawn_peer_comms_endpoint("rev-t12-supervisor", true, None).await;
    probe.trust(fixture.host_peer_descriptor()).await;

    // Materialize with a resolved policy + budget riding the digest-covered
    // overlay (the recompose facts T-F3 pins).
    let mut spec = support::sample_portable_member_spec("mob-t12", "b2", "worker");
    spec.overlay.tool_access_policy = Some(
        meerkat_mob::runtime::bridge_protocol::WireResolvedToolAccessPolicy::DenyList(vec![
            "shell_execute".to_string(),
        ]),
    );
    let limits = meerkat_core::BudgetLimits {
        max_tokens: None,
        max_duration: None,
        max_tool_calls: Some(2),
    };
    spec.overlay.budget_limits = Some(limits.clone());
    let payload = support::sample_materialize_payload(
        &probe,
        1,
        spec,
        1,
        1,
        meerkat_mob::runtime::bridge_protocol::MaterializeLaunchMode::Fresh {},
    );
    let bind = support::raw_bind_host_command(&probe, "mob-t12", &fixture.current_descriptor(), 1);
    let reply = probe
        .send_bridge_command_raw(&fixture.host_peer_descriptor(), &bind, REPLY_TIMEOUT)
        .await
        .expect("bind");
    assert!(matches!(reply, BridgeReply::BindHost(_)));
    let reply = probe
        .send_bridge_command_raw(
            &fixture.host_peer_descriptor(),
            &meerkat_mob::runtime::bridge_protocol::BridgeCommand::MaterializeMember(payload),
            REPLY_TIMEOUT,
        )
        .await
        .expect("materialize");
    let BridgeReply::MemberMaterialized(ack) = reply else {
        panic!("expected MemberMaterialized");
    };

    // Restart the daemon over its durable truth. NO supervisor exists during
    // the recover walk — any bridge send would land on the probe's inbox.
    let fixture = fixture.restart().await;
    let stray: Vec<_> = probe.drain_message_bodies().await;
    assert!(
        stray.is_empty(),
        "boot revival must produce ZERO bridge traffic, got {stray:?}"
    );

    // Recovery folded the materialized region and revived the member: the
    // session is live again on the SAME realm, and the member identity is
    // re-registered on the acceptor UNDER THE SAME PUBKEY (A20 — the
    // member-addressed send acks only if `ack.from == sent_to`).
    wait_until("revived member session to go live", || async {
        fixture
            .member_session_service()
            .has_live_session(&meerkat_core::SessionId::parse(&ack.session_id).expect("session id"))
            .await
            .unwrap_or(false)
    })
    .await;
    // Session-service liveness precedes acceptor identity registration during
    // boot revival. An authenticated HostStatus round-trip is the serving
    // barrier: the host responder starts only after revived member identities
    // have been registered. This explicit probe occurs after the zero-traffic
    // assertion above, so autonomous boot remains traffic-free.
    probe.trust(fixture.host_peer_descriptor()).await;
    let status = probe
        .send_bridge_command_raw(
            &fixture.host_peer_descriptor(),
            &raw_host_status_command(&probe, "mob-t12", 1),
            REPLY_TIMEOUT,
        )
        .await
        .expect("host serves status after boot revival");
    let BridgeReply::HostStatus(status) = status else {
        panic!("expected HostStatus serving barrier after boot revival")
    };
    assert!(
        status
            .members
            .iter()
            .any(|member| member.agent_identity == "b2" && member.healthy),
        "serving barrier must report the revived member healthy: {status:?}",
    );
    let member_descriptor = member_descriptor_from_ack("mob-t12/b2", &ack);
    probe.trust(member_descriptor.clone()).await;
    let receipt = probe
        .runtime
        .send(CommsCommand::PeerMessage {
            content_taint: None,
            to: PeerRoute::with_display_name(member_descriptor.peer_id, member_descriptor.name),
            body: "post-revival hello".to_string(),
            blocks: None,
            handling_mode: HandlingMode::Queue,
            objective_id: None,
        })
        .await
        .expect("revived member identity acks at the SAME pubkey");
    assert!(matches!(receipt, SendReceipt::PeerMessageSent { .. }));

    // Recomposed build facts: gate == originally received policy; budget ==
    // overlay copy (the durable session's rows survive the recompose).
    let session = fixture
        .member_session_service()
        .load_persisted_session(
            &meerkat_core::SessionId::parse(&ack.session_id).expect("session id"),
        )
        .await
        .expect("read revived session")
        .expect("revived session exists");
    assert_eq!(
        session
            .session_metadata()
            .expect("metadata")
            .tooling
            .tool_access_policy,
        Some(meerkat_core::ops::ToolAccessPolicy::DenyList(
            ["shell_execute"].into_iter().collect()
        )),
        "revived gate equals the originally received resolved policy"
    );
    assert_eq!(
        session.build_state().and_then(|state| state.budget_limits),
        Some(limits),
        "revived budget equals the digest-covered overlay copy"
    );

    fixture.shutdown().await;
}

// ===========================================================================
// H T19 — revival build failure: typed + logged, daemon STARTS, HostStatus
// reports healthy:false — never a half-alive member
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn restart_revival_build_failure_is_typed_not_half_alive() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture =
        spawn_host_daemon_fixture(HostFixtureOptions::named("rev-t19-host").with_member_build())
            .await
            .expect("member-build fixture");
    let probe = spawn_peer_comms_endpoint("rev-t19-supervisor", true, None).await;
    probe.trust(fixture.host_peer_descriptor()).await;
    let ack = bind_then_materialize(&probe, &fixture, "mob-t19", "b2").await;

    // Break the durable session before restart: revival's resume walk finds
    // no snapshot for the recorded session id. The canonical session
    // document must go (an out-of-band archive), not just the JSONL
    // projection file — a projection wipe alone recomposes fine.
    let realm = fixture.member_realm.clone().expect("member realm");
    fixture
        .member_session_service()
        .archive_with_mob_lifecycle_authority(
            &meerkat_core::SessionId::parse(&ack.session_id).expect("session id"),
        )
        .await
        .expect("out-of-band archive of the member session");
    // Quiesce the running fixture FIRST so the wipe is not raced by shutdown
    // persistence (the injector runs between quiesce and reboot).
    let fixture = fixture
        .restart_after(|| wipe_durable_session(&realm, &ack.session_id))
        .await;

    // The daemon STARTED (it serves) and reports the unrevived member
    // honestly: recorded row present, healthy:false.
    let reply = probe
        .send_bridge_command_raw(
            &fixture.host_peer_descriptor(),
            &raw_host_status_command(&probe, "mob-t19", 1),
            REPLY_TIMEOUT,
        )
        .await
        .expect("host status after failed revival");
    let BridgeReply::HostStatus(status) = &reply else {
        panic!("expected HostStatus, got {reply:?}");
    };
    let row = status
        .members
        .iter()
        .find(|member| member.agent_identity == "b2")
        .expect("the recorded-but-unrevived member is still reported");
    assert!(
        !row.healthy,
        "a member whose revival failed must report healthy:false, never half-alive"
    );

    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn restart_missing_persisted_required_env_starts_with_unhealthy_member() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture = spawn_host_daemon_fixture(
        HostFixtureOptions::named("rev-missing-env-host").with_member_build(),
    )
    .await
    .expect("member-build fixture");
    let probe = spawn_peer_comms_endpoint("rev-missing-env-supervisor", true, None).await;
    probe.trust(fixture.host_peer_descriptor()).await;
    let _ack = bind_then_materialize(&probe, &fixture, "mob-missing-env", "b2").await;
    let missing_key = format!(
        "MEERKAT_TEST_MISSING_REVIVAL_ENV_{}",
        uuid::Uuid::new_v4().simple()
    );
    assert!(std::env::var_os(&missing_key).is_none());

    // Model a separately written, correctly redigested durable spec while
    // the daemon is down. The host-local value is intentionally absent on
    // restart: availability must affect only this member, not host startup.
    let partitioned = fixture.partition().await;
    let partitioned = partitioned
        .rewrite_host_binding_record("mob-missing-env", |record| {
            let row = record
                .materialized
                .get_mut("b2")
                .expect("materialized member row");
            row.spec.required_env_keys.push(missing_key.clone());
            row.spec_digest =
                meerkat_mob::runtime::bridge_protocol::portable_member_spec_digest(&row.spec)
                    .expect("redigest durable member spec");
        })
        .await;
    let fixture = partitioned.restore().await;

    let reply = probe
        .send_bridge_command_raw(
            &fixture.host_peer_descriptor(),
            &raw_host_status_command(&probe, "mob-missing-env", 1),
            REPLY_TIMEOUT,
        )
        .await
        .expect("host remains available after member-local env failure");
    let BridgeReply::HostStatus(status) = &reply else {
        panic!("expected HostStatus, got {reply:?}");
    };
    let row = status
        .members
        .iter()
        .find(|member| member.agent_identity == "b2")
        .expect("recorded member remains projected");
    assert!(
        !row.healthy,
        "missing host-local required env must mark only the recovered member unhealthy"
    );

    fixture.shutdown().await;
}

// ===========================================================================
// R T-13 — controlling-side revival (DEC-R6 + ADJ-9): dead runtime on B,
// durable row kept ⇒ same-tuple Resume re-issue, ensure-on-replay ack,
// fence maps unchanged
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn controlling_revival_reissues_at_the_recorded_tuple_and_recovers() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture =
        spawn_host_daemon_fixture(HostFixtureOptions::named("rev-t13-host").with_member_build())
            .await
            .expect("member-build fixture");
    let controlling = create_controlling_mob("rev-t13").await;
    let report = controlling.bind_fixture(&fixture).await;

    controlling
        .spawn_placed("worker", "b2", &report.host_id)
        .await
        .expect("placed spawn commits");
    let before = fixture
        .host_binding_record(controlling.mob_id.as_ref())
        .await
        .materialized
        .get("b2")
        .cloned()
        .expect("materialized row");
    let session_id = meerkat_core::SessionId::parse(&before.session_id).expect("session id");

    // Kill the member's LIVE runtime on B; the durable row stays.
    fixture
        .member_session_service()
        .discard_live_session(&session_id)
        .await
        .expect("discard live member session");

    // Delivery-failure trigger → ClassifyMemberLiveMaterialization →
    // ReviveAuthorized → same-tuple MaterializeMember{launch: Resume} →
    // host ensure-on-replay (ADJ-9: recompose-if-dead, ack the RECORDED
    // response verbatim) → ResolveMemberRevivalSucceeded. The first send may
    // surface the failure or converge inline; the REVIVED member must serve.
    let member = controlling
        .handle
        .member(&AgentIdentity::from("b2"))
        .await
        .expect("member handle");
    let _first = member.send("are you alive?", HandlingMode::Queue).await;
    wait_until("member runtime to revive on Host B", || async {
        fixture
            .member_session_service()
            .has_live_session(&session_id)
            .await
            .unwrap_or(false)
    })
    .await;
    member
        .send("post-revival turn", HandlingMode::Queue)
        .await
        .expect("revived member serves a turn");

    // DEC-R6: the re-issue rode the SAME machine-recorded tuple — the host
    // row is unchanged (no generation/fence advance, same session).
    let after = fixture
        .host_binding_record(controlling.mob_id.as_ref())
        .await
        .materialized
        .get("b2")
        .cloned()
        .expect("materialized row survives revival");
    assert_eq!(
        (
            after.generation,
            after.fence_token,
            after.session_id.clone()
        ),
        (
            before.generation,
            before.fence_token,
            before.session_id.clone()
        ),
        "revival never advances the recorded tuple (fence maps unchanged)"
    );

    fixture.shutdown().await;
}

// ===========================================================================
// R T-14 / F T-L7 — Broken refuses retry: durable snapshot missing ⇒
// BrokenRecorded ⇒ typed MemberRestoreFailed, no new materialization
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn broken_member_refuses_revival_retry_typed() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture =
        spawn_host_daemon_fixture(HostFixtureOptions::named("rev-t14-host").with_member_build())
            .await
            .expect("member-build fixture");
    let controlling = create_controlling_mob("rev-t14").await;
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

    // Kill the runtime AND the durable snapshot: revival observes
    // DurableSnapshotMissing ⇒ BrokenRecorded. As in the T19 row above, the
    // canonical session document must go (an out-of-band archive), not just
    // the JSONL projection file — a projection wipe alone recomposes fine.
    let realm = fixture.member_realm.clone().expect("member realm");
    fixture
        .member_session_service()
        .discard_live_session(&session_id)
        .await
        .expect("discard live member session");
    fixture
        .member_session_service()
        .archive_with_mob_lifecycle_authority(&session_id)
        .await
        .expect("out-of-band archive of the member session");
    wipe_durable_session(&realm, &row.session_id);

    let member = controlling
        .handle
        .member(&AgentIdentity::from("b2"))
        .await
        .expect("member handle");
    // First trigger drives the revival ladder to failure...
    let _ = member.send("first trigger", HandlingMode::Queue).await;
    // ...and once Broken is recorded, a second trigger is refused TYPED with
    // no Classify authorization (`not_broken` guard) — MemberRestoreFailed.
    wait_until("broken record to refuse retries typed", || async {
        matches!(
            member.send("second trigger", HandlingMode::Queue).await,
            Err(MobError::MemberRestoreFailed { .. })
        )
    })
    .await;

    // No re-materialization was authorized for the broken identity: the host
    // row never advanced past the recorded tuple.
    let after = fixture
        .host_binding_record(controlling.mob_id.as_ref())
        .await
        .materialized
        .get("b2")
        .cloned()
        .expect("host row");
    assert_eq!(
        (after.generation, after.fence_token),
        (row.generation, row.fence_token),
        "BrokenRecorded must suppress RequestMemberMaterialization"
    );

    fixture.shutdown().await;
}
