//! Multi-host mobs phase 6 — events / history / completion backbone
//! (§11:313/:314): the T-E1..T-E7 + T-E9 rows of design-flow-spine §5.2.
//!
//! Specs owned by the flow-spine lane; realization is the events-backbone
//! lane (poll pump, member-host serving arms, cursor persistence, merged
//! stream fan-in, history proxy, fork placement switch) — TDD both
//! directions. Deterministic two-hosts-in-one-process battery; the durable
//! `(generation, seq)` cursor discipline (gotcha 8) is the through-line:
//! only the owning host's durable event seq ever crosses the wire.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use meerkat_core::HandlingMode;
use meerkat_core::service::{CreateSessionRequest, DeferredPromptPolicy, InitialTurnPolicy};
use meerkat_mob::runtime::bridge_protocol::{BridgeEventCursor, BridgeRejectionCause, BridgeReply};
use meerkat_mob::{AgentIdentity, MobControlPrincipal};
use support::{
    ControllingMob, FIXTURE_BRIDGE_TIMEOUT, HostFixtureOptions, REAL_COMMS_TEST_LOCK, StallGate,
    bind_then_materialize, collect_mob_stream_until, create_controlling_mob,
    member_descriptor_from_ack, member_incarnation_from_ack, placed_spawn_spec,
    raw_deliver_member_input_command, raw_poll_member_events_command,
    scripted_member_client_completing, scripted_member_client_stalling, spawn_host_daemon_fixture,
    spawn_peer_comms_endpoint, wait_until,
};

const WAIT: Duration = Duration::from_secs(60);

fn identity(name: &str) -> AgentIdentity {
    AgentIdentity::from(name)
}

/// The dual-cursor dedup key of one merged-stream item: the SOURCE member's
/// `(generation, per-source seq)` — generation from the attribution runtime
/// id, seq from the envelope (the owning stream's counter).
fn source_cursor_key(event: &meerkat_mob::AttributedEvent) -> (u64, u64) {
    (event.source.generation.get(), event.envelope.seq)
}

/// Drive one completed member turn and collect every merged-stream item for
/// `member` up to (and including) its run terminal.
async fn drive_turn_and_collect(
    controlling: &ControllingMob,
    router: &mut meerkat_mob::runtime::MobEventRouterHandle,
    member: &str,
    prompt: &str,
) -> Vec<meerkat_mob::AttributedEvent> {
    let handle = controlling
        .handle
        .member(&identity(member))
        .await
        .expect("member handle");
    handle
        .send(prompt, HandlingMode::Queue)
        .await
        .expect("member send admits");
    collect_mob_stream_until(
        router,
        |attributed| {
            attributed.source.identity.as_str() == member
                && matches!(
                    attributed.envelope.payload,
                    meerkat_core::AgentEvent::RunCompleted { .. }
                        | meerkat_core::AgentEvent::RunFailed { .. }
                )
        },
        WAIT,
    )
    .await
}

// ==========================================================================
// T-E1 — durable cursor resumes across a pump restart (controlling restart:
// the pump manager rebuilds from the persisted (generation, seq) record)
// ==========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn durable_cursor_resumes_across_pump_restart() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let mut opts = HostFixtureOptions::named("xhe-t1-host-b").with_member_build();
    opts.member_llm_client = Some(scripted_member_client_completing("t1-done"));
    let fixture = spawn_host_daemon_fixture(opts)
        .await
        .expect("spawn member-build host fixture");
    let controlling = create_controlling_mob("xhe-t1").await;
    let report = controlling.bind_fixture(&fixture).await;

    controlling
        .spawn_placed("worker", "e1", &report.host_id)
        .await
        .expect("e1 materializes on host B");

    // First incarnation of the pump: consume one full turn.
    let mut router = controlling
        .handle
        .subscribe_mob_events()
        .await
        .expect("merged stream subscribes");
    let before = drive_turn_and_collect(&controlling, &mut router, "e1", "turn one").await;
    let before_keys: BTreeSet<(u64, u64)> = before
        .iter()
        .filter(|event| event.source.identity.as_str() == "e1")
        .map(source_cursor_key)
        .collect();
    assert!(
        !before_keys.is_empty(),
        "the first turn produced remote rows through the pump"
    );
    drop(router);

    // Controlling restart: kills the pump; the manager must resume from the
    // DURABLE cursor record, not from Tail and not from seq 1.
    let controlling = controlling.restart().await;
    let mut router = controlling
        .handle
        .subscribe_mob_events()
        .await
        .expect("merged stream re-subscribes after restart");
    let after = drive_turn_and_collect(&controlling, &mut router, "e1", "turn two").await;
    let after_keys: BTreeSet<(u64, u64)> = after
        .iter()
        .filter(|event| event.source.identity.as_str() == "e1")
        .map(source_cursor_key)
        .collect();
    assert!(
        !after_keys.is_empty(),
        "the resumed pump serves the second turn's rows"
    );

    // No duplicate: the resumed cursor never re-serves consumed rows.
    let overlap: Vec<_> = before_keys.intersection(&after_keys).collect();
    assert!(
        overlap.is_empty(),
        "durable cursor resume must not re-serve consumed rows: {overlap:?}"
    );
    // No gap within the resumed window: same generation, strictly advancing
    // seq domain past the pre-restart watermark.
    let max_before = before_keys.iter().map(|(_, seq)| *seq).max().unwrap();
    let min_after = after_keys.iter().map(|(_, seq)| *seq).min().unwrap();
    assert!(
        min_after > max_before,
        "resumed rows continue the durable seq domain (min_after={min_after} > max_before={max_before})"
    );

    fixture.shutdown().await;
}

// ==========================================================================
// T-E2 — member respawn: generation bump resets the seq domain; the stream
// carries BOTH incarnations, disambiguated by the dual cursor
// ==========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn member_respawn_generation_bump_resets_seq_domain() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let mut opts = HostFixtureOptions::named("xhe-t2-host-b").with_member_build();
    opts.member_llm_client = Some(scripted_member_client_completing("t2-done"));
    let fixture = spawn_host_daemon_fixture(opts)
        .await
        .expect("spawn member-build host fixture");
    let controlling = create_controlling_mob("xhe-t2").await;
    let report = controlling.bind_fixture(&fixture).await;

    controlling
        .spawn_placed("worker", "e2", &report.host_id)
        .await
        .expect("e2 materializes on host B");

    let mut router = controlling
        .handle
        .subscribe_mob_events()
        .await
        .expect("merged stream subscribes");
    let first = drive_turn_and_collect(&controlling, &mut router, "e2", "first incarnation").await;
    let first_keys: BTreeSet<(u64, u64)> = first
        .iter()
        .filter(|event| event.source.identity.as_str() == "e2")
        .map(source_cursor_key)
        .collect();
    let first_generation = first_keys.iter().map(|(generation, _)| *generation).max();
    assert!(first_generation.is_some(), "first incarnation observed");

    // Respawn: ADJ-24 re-materialization with a generation bump; re-fencing
    // is INTENTIONAL (gotcha 9 — no monotonic-fence "fix" here).
    controlling
        .handle
        .respawn(identity("e2"), None)
        .await
        .expect("respawn bumps e2's generation");

    let second =
        drive_turn_and_collect(&controlling, &mut router, "e2", "second incarnation").await;
    let second_generation_keys: BTreeSet<(u64, u64)> = second
        .iter()
        .filter(|event| {
            event.source.identity.as_str() == "e2"
                && Some(event.source.generation.get()) > first_generation
        })
        .map(source_cursor_key)
        .collect();
    assert!(
        !second_generation_keys.is_empty(),
        "the new incarnation's rows carry a HIGHER generation"
    );

    // Seq domain reset: the new incarnation restarts at the domain floor —
    // its first observed seq is no greater than the FIRST incarnation's
    // first observed seq (both are fresh logs).
    let first_floor = first_keys.iter().map(|(_, seq)| *seq).min().unwrap();
    let second_floor = second_generation_keys
        .iter()
        .map(|(_, seq)| *seq)
        .min()
        .unwrap();
    assert!(
        second_floor <= first_floor,
        "generation bump resets the seq domain (second floor {second_floor} <= first floor {first_floor})"
    );

    fixture.shutdown().await;
}

// ==========================================================================
// T-E2b — same-session Resume: an idle replacement has an empty event page
// whose host-resolved generation floor is above the controller's initial
// seq=1 cursor. The REAL pump must accept that empty page and durably advance
// to the resolved floor instead of retrying seq 1 forever.
// ==========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn empty_same_session_resume_page_advances_real_pump_to_resolved_floor() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let mut opts = HostFixtureOptions::named("xhe-t2b-host-b").with_member_build();
    opts.member_llm_client = Some(scripted_member_client_completing("t2b-done"));
    let fixture = spawn_host_daemon_fixture(opts)
        .await
        .expect("spawn member-build host fixture");
    let controlling = create_controlling_mob("xhe-t2b").await;
    let member = identity("resume-floor-e2b");
    let comms_name =
        meerkat_core::MemberCommsName::new(controlling.mob_id.as_ref(), "worker", member.as_str())
            .expect("fixture member comms name")
            .to_string();

    // Seed the member host with a durable session that already owns event
    // rows, but has never belonged to the controlling mob. Its generic
    // surface attachment has no host-materializer sidecar and therefore may
    // not be adopted by SessionId. Quiesce it through its exact machine owner
    // first; the placed spawn below then exercises the production explicit
    // `Resume` path over persisted-only state, captures `latest + 1` as the
    // generation floor, and rebuilds the exact durable identity.
    let member_service = fixture
        .member_concrete_service
        .as_ref()
        .expect("persistent member service");
    let runtime_adapter = fixture
        .member_runtime_adapter
        .as_ref()
        .expect("member runtime adapter");
    let seed_session = meerkat_core::Session::new();
    let session_id = seed_session.id().clone();
    let executor_service = Arc::clone(member_service);
    let executor_adapter = Arc::clone(runtime_adapter);
    meerkat::surface::materialize_session(
        member_service,
        runtime_adapter,
        seed_session,
        CreateSessionRequest {
            model: "claude-haiku-4-5-20251001".to_string(),
            prompt: "populate the durable pre-resume event log".into(),
            injected_context: Vec::new(),
            system_prompt: meerkat_core::SystemPromptOverride::Inherit,
            max_tokens: None,
            event_tx: None,
            initial_turn: InitialTurnPolicy::RunImmediately,
            deferred_prompt_policy: DeferredPromptPolicy::Discard,
            build: Some(meerkat_core::service::SessionBuildOptions {
                comms_name: Some(comms_name),
                ..Default::default()
            }),
            labels: None,
        },
        move |id| {
            meerkat::surface::default_persistent_executor(
                Arc::clone(&executor_service),
                Arc::clone(&executor_adapter),
                id,
            )
        },
    )
    .await
    .expect("seed resumable member-host session through machine commit authority");
    let latest = member_service
        .event_log_latest_seq(&session_id)
        .await
        .expect("read seeded session durable watermark")
        .expect("the seeded turn populated the durable log");
    runtime_adapter
        .unregister_session(&session_id)
        .await
        .expect("quiesce generic seed attachment before host-owned explicit resume");

    let report = controlling.bind_fixture(&fixture).await;

    controlling
        .handle
        .spawn_spec(
            placed_spawn_spec("worker", member.as_str(), &report.host_id)
                .with_resume_bridge_session_id(session_id.clone()),
        )
        .await
        .expect("placed member resumes the seeded durable session on the member host");

    let after = fixture
        .host_binding_record(controlling.mob_id.as_ref())
        .await
        .materialized
        .get(member.as_str())
        .cloned()
        .expect("resumed host materialization row");
    assert_eq!(after.session_id, session_id.to_string());
    assert!(
        after.generation_start_seq > 1,
        "same-session Resume records a floor above the old generation"
    );
    assert_eq!(
        after.generation_start_seq,
        latest + 1,
        "the idle replacement has no post-floor event rows, so its first page is empty"
    );

    let _router = controlling
        .handle
        .subscribe_mob_events()
        .await
        .expect("real controller pump subscribes to the idle resumed member");
    let metadata = Arc::clone(&controlling.storage_metadata);
    let mob_id = controlling.mob_id.clone();
    let expected_identity = member.to_string();
    let expected_session = after.session_id.clone();
    let expected_generation = after.generation;
    let expected_fence = after.fence_token;
    let expected_floor = after.generation_start_seq;
    wait_until(
        "empty Resume page cursor to reach the resolved floor",
        || {
            let metadata = Arc::clone(&metadata);
            let mob_id = mob_id.clone();
            let expected_identity = expected_identity.clone();
            let expected_session = expected_session.clone();
            async move {
                metadata
                    .list_member_event_cursors(&mob_id)
                    .await
                    .is_ok_and(|records| {
                        records.into_iter().any(|record| {
                            record.agent_identity == expected_identity
                                && record.member_session_id == expected_session
                                && record.generation == expected_generation
                                && record.fence_token == Some(expected_fence)
                                && record.next_seq == expected_floor
                        })
                    })
            }
        },
    )
    .await;

    fixture.shutdown().await;
}

// ==========================================================================
// T-E3 — the mob-wide stream INCLUDES remote members (kill-site removal)
// ==========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn mob_wide_stream_includes_remote_members() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let mut opts = HostFixtureOptions::named("xhe-t3-host-b").with_member_build();
    opts.member_llm_client = Some(scripted_member_client_completing("t3-done"));
    let fixture = spawn_host_daemon_fixture(opts)
        .await
        .expect("spawn member-build host fixture");
    let controlling = create_controlling_mob("xhe-t3").await;
    let report = controlling.bind_fixture(&fixture).await;

    // One local + one placed member, both active.
    controlling
        .handle
        .spawn_spec(
            meerkat_mob::SpawnMemberSpec::new("worker", "local-e3")
                .with_backend(meerkat_mob::MobBackendKind::Session),
        )
        .await
        .expect("spawn local member");
    controlling
        .spawn_placed("worker", "remote-e3", &report.host_id)
        .await
        .expect("remote-e3 materializes on host B");

    // The mob-wide subscription must yield BOTH (the handle.rs fail-closed
    // "no remote subscription realization yet" stub is GONE).
    let mut router = controlling
        .handle
        .subscribe_mob_events()
        .await
        .expect("mob-wide subscription includes placed members");

    let local_events =
        drive_turn_and_collect(&controlling, &mut router, "local-e3", "local turn").await;
    assert!(
        local_events
            .iter()
            .any(|event| event.source.identity.as_str() == "local-e3"),
        "local member rows flow"
    );

    let remote_events =
        drive_turn_and_collect(&controlling, &mut router, "remote-e3", "remote turn").await;
    let remote_row = remote_events
        .iter()
        .find(|event| event.source.identity.as_str() == "remote-e3")
        .expect("remote member rows flow through the merged stream");

    // Attribution facts are correct for the remote rows.
    assert_eq!(remote_row.source.identity.as_str(), "remote-e3");
    // Fresh identities mint `Generation::INITIAL` (= 0, ADJ-24).
    assert_eq!(remote_row.source.generation.get(), 0);
    assert_eq!(remote_row.role.as_str(), "worker");
    assert!(
        remote_row.source_fence_token.get() >= 1,
        "remote rows carry the incarnation fence token"
    );

    fixture.shutdown().await;
}

// ==========================================================================
// T-E4 — remote member completion is observable via the merged stream (the
// terminal arrives through the pump, never synthesized from a dispatch-ack)
// ==========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn remote_member_completion_via_merged_stream() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let gate = StallGate::new();
    let mut opts = HostFixtureOptions::named("xhe-t4-host-b").with_member_build();
    opts.member_llm_client = Some(scripted_member_client_stalling(gate.clone()));
    let fixture = spawn_host_daemon_fixture(opts)
        .await
        .expect("spawn member-build host fixture");
    let controlling = create_controlling_mob("xhe-t4").await;
    let report = controlling.bind_fixture(&fixture).await;

    controlling
        .spawn_placed("worker", "e4", &report.host_id)
        .await
        .expect("e4 materializes on host B");

    let mut router = controlling
        .handle
        .subscribe_mob_events()
        .await
        .expect("merged stream subscribes");

    // The delivery acks at admission while the turn is parked: NO terminal
    // may appear on the merged stream while the member is genuinely running
    // — a dispatch-ack is not a completion.
    let member = controlling
        .handle
        .member(&identity("e4"))
        .await
        .expect("member handle");
    member
        .send("stall then finish", HandlingMode::Queue)
        .await
        .expect("delivery acks at admission");

    let premature = tokio::time::timeout(Duration::from_millis(1_500), async {
        collect_mob_stream_until(
            &mut router,
            |attributed| {
                attributed.source.identity.as_str() == "e4"
                    && matches!(
                        attributed.envelope.payload,
                        meerkat_core::AgentEvent::RunCompleted { .. }
                            | meerkat_core::AgentEvent::RunFailed { .. }
                    )
            },
            Duration::from_secs(5),
        )
        .await
    })
    .await;
    assert!(
        premature.is_err(),
        "no terminal is observable while the remote turn is still running \
         (completion must not degrade to the dispatch-ack)"
    );

    // Release the gate: the REAL terminal arrives through the pump.
    gate.release();
    let events = collect_mob_stream_until(
        &mut router,
        |attributed| {
            attributed.source.identity.as_str() == "e4"
                && matches!(
                    attributed.envelope.payload,
                    meerkat_core::AgentEvent::RunCompleted { .. }
                        | meerkat_core::AgentEvent::RunFailed { .. }
                )
        },
        WAIT,
    )
    .await;
    assert!(
        events.iter().any(|event| matches!(
            event.envelope.payload,
            meerkat_core::AgentEvent::RunCompleted { .. }
        )),
        "the released turn's RunCompleted terminal rides the merged stream"
    );

    fixture.shutdown().await;
}

// ==========================================================================
// T-E5 — a long-poll in flight does not block lifecycle commands (the
// single-flight relaxation proof; DEC-P6E-14/15)
// ==========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn long_poll_does_not_block_lifecycle_commands() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let mut opts = HostFixtureOptions::named("xhe-t5-host-b").with_member_build();
    opts.member_llm_client = Some(scripted_member_client_completing("t5-done"));
    let fixture = spawn_host_daemon_fixture(opts)
        .await
        .expect("spawn member-build host fixture");
    let controlling = create_controlling_mob("xhe-t5").await;
    let report = controlling.bind_fixture(&fixture).await;

    controlling
        .spawn_placed("worker", "idle-e5", &report.host_id)
        .await
        .expect("idle-e5 materializes on host B");

    // Subscribe: the pump opens a long-poll against the idle member (its
    // wait window is the pump's 10s `wait_ms`).
    let _router = controlling
        .handle
        .subscribe_mob_events()
        .await
        .expect("merged stream subscribes (pump long-poll in flight)");
    // Give the pump a beat to get its long-poll in flight.
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Lifecycle verbs against the SAME host must complete well under the
    // poll window — reads (long-polls) and ordinary sends interleave; only
    // authority rotation takes the write lock.
    let started = tokio::time::Instant::now();
    controlling
        .spawn_placed("worker", "second-e5", &report.host_id)
        .await
        .expect("second placed spawn completes during the long-poll");
    controlling
        .handle
        .retire(identity("second-e5"))
        .await
        .expect("retire completes during the long-poll");
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_secs(8),
        "lifecycle verbs must not serialize behind the 10s long-poll window; took {elapsed:?}"
    );

    fixture.shutdown().await;
}

// ==========================================================================
// T-E6 — StaleCursor on ephemeral ring overrun (typed reject; consumer
// restarts from the watermark)
// ==========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn stale_cursor_on_ephemeral_overrun() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    const RING_CAPACITY: usize = 16;
    let mut opts = HostFixtureOptions::named("xhe-t6-host-b")
        .ephemeral()
        .with_event_ring_capacity(
            std::num::NonZeroUsize::new(RING_CAPACITY).expect("non-zero test ring capacity"),
        );
    opts.member_llm_client = Some(scripted_member_client_completing("t6-done"));
    let fixture = spawn_host_daemon_fixture(opts)
        .await
        .expect("spawn ephemeral host fixture");

    let probe = spawn_peer_comms_endpoint("xhe-t6-supervisor", true, None).await;
    probe.trust(fixture.host_peer_descriptor()).await;
    let mob_id = format!("xhe-t6-{}", uuid::Uuid::new_v4().simple());
    let ack = bind_then_materialize(&probe, &fixture, &mob_id, "ring-member").await;
    let member = member_descriptor_from_ack("ring-member", &ack);
    let expected_member =
        member_incarnation_from_ack(&fixture, &mob_id, "ring-member", &ack, 1, 1, 1);
    probe.trust(member.clone()).await;

    // Overrun the bounded ring: drive plain (undirected) turns until the
    // served watermark passes the ring capacity. Directive-bearing turns are
    // machine-rejected for ephemeral hosts (cross-checked in U-3), so this
    // member is events-only by construction.
    let mut watermark = 0u64;
    let mut generation = 1u64;
    // Cap generously: the loop breaks as soon as the watermark passes the
    // ring capacity; per-turn event volume is a fixture detail.
    for turn in 0..128u32 {
        let command = raw_deliver_member_input_command(
            &probe,
            &mob_id,
            &uuid::Uuid::new_v4().to_string(),
            "spin",
            expected_member.clone(),
            None,
        );
        let reply = probe
            .send_bridge_command_raw(&member, &command, FIXTURE_BRIDGE_TIMEOUT)
            .await
            .expect("ring delivery reply");
        assert!(
            matches!(reply, BridgeReply::Delivery(_)),
            "undirected delivery admits on the ephemeral host: {reply:?}"
        );

        if turn % 4 == 3 {
            // Read the current watermark through a Tail poll.
            let poll = raw_poll_member_events_command(
                &probe,
                &mob_id,
                1,
                expected_member.clone(),
                BridgeEventCursor::Tail,
                Some(1),
                Some(0),
            );
            let reply = probe
                .send_bridge_command_raw(&member, &poll, FIXTURE_BRIDGE_TIMEOUT)
                .await
                .expect("tail poll reply");
            if let BridgeReply::MemberEventsPage(page) = reply {
                watermark = page.watermark;
                generation = page.generation;
            }
            if watermark > (RING_CAPACITY as u64 * 2) {
                break;
            }
        }
    }
    assert!(
        watermark > RING_CAPACITY as u64,
        "the ring must overrun its capacity for this row (watermark={watermark})"
    );

    // Poll from the long-evicted domain start: typed StaleCursor carrying
    // the watermark + generation the consumer restarts from.
    let stale = raw_poll_member_events_command(
        &probe,
        &mob_id,
        1,
        expected_member.clone(),
        BridgeEventCursor::At { generation, seq: 1 },
        Some(16),
        Some(0),
    );
    let reply = probe
        .send_bridge_command_raw(&member, &stale, FIXTURE_BRIDGE_TIMEOUT)
        .await
        .expect("stale poll reply");
    let (served_watermark, served_generation) = match reply {
        BridgeReply::Rejected {
            cause:
                BridgeRejectionCause::StaleCursor {
                    watermark,
                    generation,
                },
            ..
        } => (watermark, generation),
        other => panic!("expected typed StaleCursor rejection, got {other:?}"),
    };
    assert!(
        served_watermark >= RING_CAPACITY as u64,
        "watermark reflects the ring head"
    );
    assert_eq!(served_generation, generation);

    // Restart-from-watermark serves (the labeled-gap consumer recovery).
    let recovered = raw_poll_member_events_command(
        &probe,
        &mob_id,
        1,
        expected_member,
        BridgeEventCursor::At {
            generation: served_generation,
            seq: served_watermark + 1,
        },
        Some(16),
        Some(0),
    );
    let reply = probe
        .send_bridge_command_raw(&member, &recovered, FIXTURE_BRIDGE_TIMEOUT)
        .await
        .expect("recovered poll reply");
    assert!(
        matches!(reply, BridgeReply::MemberEventsPage(_)),
        "polling from the watermark serves a page: {reply:?}"
    );

    fixture.shutdown().await;
}

// ==========================================================================
// T-E7 — remote history page == local page shape (one projection owner)
// ==========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn remote_history_page_equals_local_page_shape() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let mut opts = HostFixtureOptions::named("xhe-t7-host-b").with_member_build();
    opts.member_llm_client = Some(scripted_member_client_completing("same-answer"));
    let fixture = spawn_host_daemon_fixture(opts)
        .await
        .expect("spawn member-build host fixture");
    let controlling = create_controlling_mob("xhe-t7").await;
    let report = controlling.bind_fixture(&fixture).await;

    controlling
        .handle
        .spawn_spec(
            meerkat_mob::SpawnMemberSpec::new("worker", "local-e7")
                .with_backend(meerkat_mob::MobBackendKind::Session),
        )
        .await
        .expect("spawn local member");
    controlling
        .spawn_placed("worker", "remote-e7", &report.host_id)
        .await
        .expect("remote-e7 materializes on host B");

    // One identical driven turn each.
    for member in ["local-e7", "remote-e7"] {
        let handle = controlling
            .handle
            .member(&identity(member))
            .await
            .expect("member handle");
        handle
            .send("identical prompt", HandlingMode::Queue)
            .await
            .expect("member send admits");
    }
    // Converge: both transcripts hold the driven turn.
    wait_until("both members recorded the turn", || async {
        let local = controlling
            .handle
            .member_history(MobControlPrincipal::Owner, identity("local-e7"), None, None)
            .await;
        let remote = controlling
            .handle
            .member_history(
                MobControlPrincipal::Owner,
                identity("remote-e7"),
                None,
                None,
            )
            .await;
        match (local, remote) {
            (Ok(local), Ok(remote)) => {
                local.page.message_count >= 2 && remote.page.message_count >= 2
            }
            _ => false,
        }
    })
    .await;

    // Tail-addressed single-round-trip page (from_index absent + limit):
    // remote page shape == local page shape BY CONSTRUCTION (one projection
    // fn), including the mandatory message_count mirror (gotcha 14).
    let local_page = controlling
        .handle
        .member_history(
            MobControlPrincipal::Owner,
            identity("local-e7"),
            None,
            Some(2),
        )
        .await
        .expect("local history page");
    let remote_page = controlling
        .handle
        .member_history(
            MobControlPrincipal::Owner,
            identity("remote-e7"),
            None,
            Some(2),
        )
        .await
        .expect("remote history page");

    assert_eq!(
        local_page.page.message_count, remote_page.page.message_count,
        "identical conversations mirror the same message_count"
    );
    assert_eq!(
        local_page.page.from_index, remote_page.page.from_index,
        "tail addressing computes the same offset both sides"
    );
    assert_eq!(
        local_page.page.messages.len(),
        remote_page.page.messages.len(),
        "tail page carries the same row count"
    );
    assert_eq!(
        local_page.page.complete, remote_page.page.complete,
        "completeness flag matches"
    );
    assert_eq!(
        local_page.page.from_index,
        local_page.page.message_count.saturating_sub(2),
        "tail page from_index = message_count - limit"
    );

    fixture.shutdown().await;
}

// ==========================================================================
// T-E9 — remote-source fork context via ReadMemberHistory (fork placement
// switch; LastMessages needs no count pre-read)
// ==========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn remote_source_fork_context_via_read_member_history() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let mut opts = HostFixtureOptions::named("xhe-t9-host-b").with_member_build();
    opts.member_llm_client = Some(scripted_member_client_completing("tail-marker-reply"));
    let fixture = spawn_host_daemon_fixture(opts)
        .await
        .expect("spawn member-build host fixture");
    let controlling = create_controlling_mob("xhe-t9").await;
    let report = controlling.bind_fixture(&fixture).await;

    controlling
        .spawn_placed("worker", "source-e9", &report.host_id)
        .await
        .expect("source-e9 materializes on host B");

    // Seed the placed source's transcript with a recognizable tail.
    let source = controlling
        .handle
        .member(&identity("source-e9"))
        .await
        .expect("source member handle");
    source
        .send("tail-marker-prompt", HandlingMode::Queue)
        .await
        .expect("source turn admits");
    wait_until("source transcript recorded", || async {
        controlling
            .handle
            .member_history(
                MobControlPrincipal::Owner,
                identity("source-e9"),
                None,
                None,
            )
            .await
            .map(|page| page.page.message_count >= 2)
            .unwrap_or(false)
    })
    .await;

    // Fork-spawn a LOCAL child from the PLACED source: `LastMessages{2}` is
    // one tail-addressed ReadMemberHistory round trip (no count pre-read).
    // `ForkSourceUnavailable{RemoteReadUnavailable}` is NO LONGER the answer
    // for a reachable placed source.
    let mut spec = meerkat_mob::SpawnMemberSpec::new("worker", "fork-child-e9")
        .with_backend(meerkat_mob::MobBackendKind::Session);
    spec.launch_mode = meerkat_mob::MemberLaunchMode::Fork {
        source_member_id: identity("source-e9"),
        fork_context: meerkat_mob::ForkContext::LastMessages { count: 2 },
    };
    // The fork context rides the child's initial prompt (the fork_helper
    // production shape): an initial message drives the first turn that
    // transcribes it.
    spec.initial_message = Some(meerkat_core::types::ContentInput::from(
        "continue from the forked context",
    ));
    controlling
        .handle
        .spawn_spec(spec)
        .await
        .expect("fork-spawn from a placed source succeeds via the history proxy");

    // The child's transcript carries the rendered fork context (the source's
    // last messages, read over the bridge). The initial turn that transcribes
    // it is dispatched off the actor loop, so converge on the marker.
    let child_session = controlling
        .member_session_id(&identity("fork-child-e9"))
        .await;
    wait_until("fork child transcribed the forked context", || async {
        let child_history = meerkat_core::service::SessionServiceHistoryExt::read_history(
            controlling.service.as_ref(),
            &child_session,
            meerkat_core::service::SessionHistoryQuery::default(),
        )
        .await;
        match child_history {
            Ok(history) => serde_json::to_string(&history.messages)
                .map(|rendered| rendered.contains("tail-marker"))
                .unwrap_or(false),
            Err(_) => false,
        }
    })
    .await;

    // An unreachable host still yields the typed shape: partition the host
    // and fork again — typed ForkSourceUnavailable{RemoteReadUnavailable},
    // now meaning "tried and failed".
    let partitioned = fixture.partition().await;
    let mut spec = meerkat_mob::SpawnMemberSpec::new("worker", "fork-child-e9b")
        .with_backend(meerkat_mob::MobBackendKind::Session);
    spec.launch_mode = meerkat_mob::MemberLaunchMode::Fork {
        source_member_id: identity("source-e9"),
        fork_context: meerkat_mob::ForkContext::LastMessages { count: 2 },
    };
    let unreachable = controlling.handle.spawn_spec(spec).await;
    match unreachable {
        Err(meerkat_mob::MobError::ForkSourceUnavailable { cause, .. }) => {
            assert!(
                matches!(
                    cause,
                    meerkat_mob::ForkSourceUnavailableCause::RemoteReadUnavailable
                ),
                "unreachable placed source keeps the typed cause: {cause:?}"
            );
        }
        other => panic!("expected typed ForkSourceUnavailable, got {other:?}"),
    }

    let fixture = partitioned.restore().await;
    fixture.shutdown().await;
}
