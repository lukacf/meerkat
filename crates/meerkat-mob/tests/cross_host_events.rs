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
    spawn_peer_comms_endpoint, unregister_session_until_terminal, wait_until,
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

    // The pump writes its durable cursor after it has fanned a page out, and
    // only while no ACK, obligation or completion waiter holds cursor custody;
    // a controlling stop aborts a write still in flight, and rows past the
    // durable cursor are re-read after a restart (losing a cursor only re-reads
    // events). This row is about resuming FROM the durable record, so restart
    // only once that record covers every row the first turn consumed.
    let max_before = before_keys.iter().map(|(_, seq)| *seq).max().unwrap();
    let before_generations: BTreeSet<u64> = before_keys
        .iter()
        .map(|(generation, _)| *generation)
        .collect();
    let metadata = Arc::clone(&controlling.storage_metadata);
    let mob_id = controlling.mob_id.clone();
    wait_until(
        "e1's durable event cursor to cover every row the first turn consumed",
        || {
            let metadata = Arc::clone(&metadata);
            let mob_id = mob_id.clone();
            let before_generations = before_generations.clone();
            async move {
                metadata
                    .list_member_event_cursors(&mob_id)
                    .await
                    .is_ok_and(|records| {
                        records.into_iter().any(|record| {
                            record.agent_identity == "e1"
                                && before_generations.contains(&record.generation)
                                && record.next_seq > max_before
                        })
                    })
            }
        },
    )
    .await;

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
    // Join the seed registration's owned teardown to terminal completion:
    // the explicit resume below reoccupies this SessionId and must not race
    // a saga that merely outlived the caller grace (issue #1104).
    assert!(
        unregister_session_until_terminal(runtime_adapter, &session_id).await,
        "generic seed attachment must hold an exact registration to quiesce"
    );
    member_service
        .event_log_await_projection_drain(&session_id)
        .await
        .expect("drain the quiesced seed incarnation's durable event projection");
    let latest = member_service
        .event_log_latest_seq(&session_id)
        .await
        .expect("read quiesced seed session durable watermark")
        .expect("the seeded turn populated the durable log");

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

// ==========================================================================
// A placed prompt-context fork child is given its planned context and its
// fork notice as typed rows on its member host; a host before protocol V7
// keeps the prompt prefix; a feature probe that never answers fails the
// spawn before anything is recorded.
// ==========================================================================

/// The fork notice blocks in `rows`, as wire JSON.
fn b2_fork_notices(rows: &[meerkat_core::types::Message]) -> Vec<serde_json::Value> {
    rows.iter()
        .filter_map(|row| match row {
            meerkat_core::types::Message::SystemNotice(notice) => Some(notice),
            _ => None,
        })
        .flat_map(|notice| notice.blocks.iter())
        .map(|block| serde_json::to_value(block).expect("notice block wire json"))
        .filter(|block| block["type"] == "fork_boundary")
        .collect()
}

/// The index of the first user row of `rows` whose text contains `needle`.
fn b2_user_row(rows: &[meerkat_core::types::Message], needle: &str) -> Option<usize> {
    rows.iter().position(|row| {
        matches!(row, meerkat_core::types::Message::User(user) if user.text_content().contains(needle))
    })
}

/// The index of the first input row of `rows` that contains `needle`: a
/// user row, or the peer message a placed member's input arrives as.
fn b2_input_row(rows: &[meerkat_core::types::Message], needle: &str) -> Option<usize> {
    rows.iter().position(|row| {
        matches!(
            row,
            meerkat_core::types::Message::User(_) | meerkat_core::types::Message::SystemNotice(_)
        ) && serde_json::to_string(row).is_ok_and(|json| json.contains(needle))
    })
}

/// The rows of `member`'s session on `fixture`, once an input row containing
/// `needle` and an answer after it are there.
async fn b2_placed_rows_after_turn(
    fixture: &support::HostDaemonFixture,
    controlling: &ControllingMob,
    member: &str,
    needle: &str,
) -> Vec<meerkat_core::types::Message> {
    let service = fixture.member_session_service();
    tokio::time::timeout(WAIT, async {
        loop {
            let session = fixture
                .host_binding_record(controlling.mob_id.as_str())
                .await
                .materialized
                .get(member)
                .map(|row| row.session_id.clone());
            if let Some(session) = session
                && let Ok(id) = meerkat_core::SessionId::parse(&session)
                && let Ok(Some(loaded)) = service.load_persisted_session(&id).await
            {
                let rows = loaded.messages().to_vec();
                if let Some(task) = b2_input_row(&rows, needle)
                    && rows[task..]
                        .iter()
                        .any(|row| matches!(row, meerkat_core::types::Message::BlockAssistant(_)))
                {
                    return rows;
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{member} answers its first input on the member host"))
}

/// The local controlling session's rows of `member`, once a user row
/// containing `needle` and an answer after it are there.
async fn b2_local_rows_after_turn(
    controlling: &ControllingMob,
    member: &str,
    needle: &str,
) -> Vec<meerkat_core::types::Message> {
    tokio::time::timeout(WAIT, async {
        loop {
            if let Ok(status) = controlling.handle.member_status(&identity(member)).await
                && let Some(id) = status.current_session_id
                && let Ok(history) = meerkat_core::service::SessionServiceHistoryExt::read_history(
                    controlling.service.as_ref(),
                    &id,
                    meerkat_core::service::SessionHistoryQuery::default(),
                )
                .await
            {
                let rows = history.messages;
                if let Some(task) = b2_user_row(&rows, needle)
                    && rows[task..]
                        .iter()
                        .any(|row| matches!(row, meerkat_core::types::Message::BlockAssistant(_)))
                {
                    return rows;
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{member} answers its first input"))
}

/// A placed source with one recorded turn on `host_id`.
async fn b2_placed_source_with_a_turn(
    controlling: &ControllingMob,
    member: &str,
    host_id: &str,
    marker: &str,
) {
    controlling
        .spawn_placed("worker", member, host_id)
        .await
        .expect("the placed source materializes");
    b2_source_turn(controlling, member, marker).await;
}

/// A local source with one recorded turn.
async fn b2_local_source_with_a_turn(controlling: &ControllingMob, member: &str, marker: &str) {
    controlling
        .handle
        .spawn_spec(
            meerkat_mob::SpawnMemberSpec::new("worker", member)
                .with_backend(meerkat_mob::MobBackendKind::Session),
        )
        .await
        .expect("the local source spawns");
    b2_source_turn(controlling, member, marker).await;
}

async fn b2_source_turn(controlling: &ControllingMob, member: &str, marker: &str) {
    controlling
        .handle
        .member(&identity(member))
        .await
        .expect("source member handle")
        .send(marker, HandlingMode::Queue)
        .await
        .expect("the source turn admits");
    wait_until("the source transcript is recorded", || async {
        controlling
            .handle
            .member_history(MobControlPrincipal::Owner, identity(member), None, None)
            .await
            .map(|page| page.page.message_count >= 2)
            .unwrap_or(false)
    })
    .await;
}

/// A prompt-context fork of `source` placed on `host_id`, with a first input.
/// The child runs autonomously, so its first input rides the durable placed
/// kickoff.
fn b2_placed_fork_spec(
    member: &str,
    host_id: &str,
    source: &str,
    first_input: &str,
) -> meerkat_mob::SpawnMemberSpec {
    let mut spec = placed_spawn_spec("worker", member, host_id);
    spec.runtime_mode = Some(meerkat_mob::MobRuntimeMode::AutonomousHost);
    spec.launch_mode = meerkat_mob::MemberLaunchMode::Fork {
        source_member_id: identity(source),
        fork_context: meerkat_mob::ForkContext::LastMessages { count: 2 },
    };
    spec.initial_message = Some(meerkat_core::types::ContentInput::from(first_input));
    spec
}

/// On a member host that serves protocol V7, a placed context-fork child
/// holds its planned context as a typed injected-context row and one fork
/// notice before its first input, and the first input carries neither.
#[tokio::test(flavor = "multi_thread")]
async fn a_placed_context_fork_child_holds_its_typed_rows_before_its_first_input() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let mut opts = HostFixtureOptions::named("xhe-b2r1-host").with_member_build();
    opts.member_llm_client = Some(scripted_member_client_completing("b2r1-reply"));
    let fixture = spawn_host_daemon_fixture(opts)
        .await
        .expect("spawn member-build host fixture");
    let controlling = create_controlling_mob("xhe-b2r1").await;
    let report = controlling.bind_fixture(&fixture).await;
    b2_placed_source_with_a_turn(
        &controlling,
        "source-b2r1",
        &report.host_id,
        "b2r1-tail-marker",
    )
    .await;

    controlling
        .handle
        .spawn_spec(b2_placed_fork_spec(
            "child-b2r1",
            &report.host_id,
            "source-b2r1",
            "b2r1-first-input",
        ))
        .await
        .expect("the placed fork child spawns");

    let rows =
        b2_placed_rows_after_turn(&fixture, &controlling, "child-b2r1", "b2r1-first-input").await;
    let task = b2_input_row(&rows, "b2r1-first-input").expect("the first input row");
    let first_input = serde_json::to_string(&rows[task]).expect("first input row json");
    assert!(
        !first_input.contains("[fork_notice]")
            && !first_input.contains("Forked conversation context"),
        "the first input is a row of its own, without a prompt prefix: {first_input}"
    );
    // The planned excerpt carries the source's answer.
    let injected = rows[..task].iter().position(|row| {
        matches!(
            row,
            meerkat_core::types::Message::User(user)
                if user.transcript_role == meerkat_core::types::TranscriptUserRole::InjectedContext
                    && user.text_content().contains("b2r1-reply")
        )
    });
    assert!(
        injected.is_some(),
        "the planned context is a typed injected-context row before the first input: {rows:#?}"
    );
    assert_eq!(
        b2_fork_notices(&rows[..task]).len(),
        1,
        "one fork notice before the first input: {rows:#?}"
    );
    fixture.shutdown().await;
}

/// The placed child's create build on its member host carries the typed
/// source, naming the source member and the source's session there.
#[tokio::test(flavor = "multi_thread")]
async fn a_placed_context_fork_child_build_carries_its_typed_source() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let mut opts = HostFixtureOptions::named("xhe-b2r3-host").with_member_build();
    opts.member_llm_client = Some(scripted_member_client_completing("b2r3-reply"));
    opts.record_member_creates = true;
    let fixture = spawn_host_daemon_fixture(opts)
        .await
        .expect("spawn member-build host fixture");
    let controlling = create_controlling_mob("xhe-b2r3").await;
    let report = controlling.bind_fixture(&fixture).await;
    b2_placed_source_with_a_turn(
        &controlling,
        "source-b2r3",
        &report.host_id,
        "b2r3-tail-marker",
    )
    .await;
    controlling
        .handle
        .spawn_spec(b2_placed_fork_spec(
            "child-b2r3",
            &report.host_id,
            "source-b2r3",
            "b2r3-first-input",
        ))
        .await
        .expect("the placed fork child spawns");

    let creates = fixture
        .failing_create
        .as_ref()
        .expect("the fixture records member creates")
        .recorded_creates();
    let create_of = |member: &str| {
        creates
            .iter()
            .find(|create| {
                create
                    .mob_member_binding
                    .as_ref()
                    .is_some_and(|binding| binding.member == member)
            })
            .unwrap_or_else(|| panic!("the create of {member}: {creates:#?}"))
    };
    let source_binding = create_of("source-b2r3").mob_member_binding.clone();
    let source_session = fixture
        .host_binding_record(controlling.mob_id.as_str())
        .await
        .materialized
        .get("source-b2r3")
        .map(|row| row.session_id.clone())
        .expect("the source's materialized row");
    let child = create_of("child-b2r3");
    let typed = child
        .prompt_context_fork_source
        .as_ref()
        .unwrap_or_else(|| panic!("the placed child's build carries its typed source: {child:#?}"));
    assert_eq!(Some(&typed.source_member), source_binding.as_ref());
    assert_eq!(
        typed.source_session_id.as_ref().map(ToString::to_string),
        Some(source_session),
        "the typed source names the placed source's session"
    );
    assert!(
        child.fork_seed.is_some(),
        "the placed child's build records its fork seed: {child:#?}"
    );
    fixture.shutdown().await;
}

/// A context fork of a PLACED source compares the source's visible tools
/// with the child's, read from the source's member host, and the notice
/// names the source's session there.
#[tokio::test(flavor = "multi_thread")]
async fn a_context_fork_of_a_placed_source_compares_its_tools() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let mut opts = HostFixtureOptions::named("xhe-b2r4-host").with_member_build();
    opts.member_llm_client = Some(scripted_member_client_completing("b2r4-reply"));
    let fixture = spawn_host_daemon_fixture(opts)
        .await
        .expect("spawn member-build host fixture");
    let controlling = create_controlling_mob("xhe-b2r4").await;
    let report = controlling.bind_fixture(&fixture).await;
    b2_placed_source_with_a_turn(
        &controlling,
        "source-b2r4",
        &report.host_id,
        "b2r4-tail-marker",
    )
    .await;
    let mut spec = meerkat_mob::SpawnMemberSpec::new("worker", "child-b2r4")
        .with_backend(meerkat_mob::MobBackendKind::Session);
    spec.launch_mode = meerkat_mob::MemberLaunchMode::Fork {
        source_member_id: identity("source-b2r4"),
        fork_context: meerkat_mob::ForkContext::LastMessages { count: 2 },
    };
    spec.initial_message = Some(meerkat_core::types::ContentInput::from("b2r4-first-input"));
    controlling
        .handle
        .spawn_spec(spec)
        .await
        .expect("the local fork child of a placed source spawns");

    let rows = b2_local_rows_after_turn(&controlling, "child-b2r4", "b2r4-first-input").await;
    let notices = b2_fork_notices(&rows);
    let [notice] = notices.as_slice() else {
        panic!("one fork notice: {rows:#?}");
    };
    assert_eq!(
        notice["tools"]["comparison"], "compared",
        "the placed source's tools are read and compared: {notice}"
    );
    let source_session = fixture
        .host_binding_record(controlling.mob_id.as_str())
        .await
        .materialized
        .get("source-b2r4")
        .map(|row| row.session_id.clone())
        .expect("the source's materialized row");
    assert_eq!(
        notice["source"]["session_id"], source_session,
        "the notice names the placed source's session: {notice}"
    );
    fixture.shutdown().await;
}

/// A LOCAL context-fork child of a PLACED source: its create build on the
/// controlling host carries the typed source, naming the source member and
/// the source's session on its member host. The placed source itself is
/// never built on the controlling host.
#[tokio::test(flavor = "multi_thread")]
async fn a_local_context_fork_child_of_a_placed_source_carries_its_typed_source() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let mut opts = HostFixtureOptions::named("xhe-b2r5-host").with_member_build();
    opts.member_llm_client = Some(scripted_member_client_completing("b2r5-reply"));
    let fixture = spawn_host_daemon_fixture(opts)
        .await
        .expect("spawn member-build host fixture");
    let controlling = create_controlling_mob("xhe-b2r5").await;
    let report = controlling.bind_fixture(&fixture).await;
    b2_placed_source_with_a_turn(
        &controlling,
        "source-b2r5",
        &report.host_id,
        "b2r5-tail-marker",
    )
    .await;
    let mut spec = meerkat_mob::SpawnMemberSpec::new("worker", "child-b2r5")
        .with_backend(meerkat_mob::MobBackendKind::Session);
    spec.launch_mode = meerkat_mob::MemberLaunchMode::Fork {
        source_member_id: identity("source-b2r5"),
        fork_context: meerkat_mob::ForkContext::LastMessages { count: 2 },
    };
    controlling
        .handle
        .spawn_spec(spec)
        .await
        .expect("the local fork child of a placed source spawns");

    let creates = controlling.recorded_member_creates();
    let creates_of = |member: &str| {
        creates
            .iter()
            .filter(|create| {
                create
                    .mob_member_binding
                    .as_ref()
                    .is_some_and(|binding| binding.member == member)
            })
            .collect::<Vec<_>>()
    };
    assert!(
        creates_of("source-b2r5").is_empty(),
        "the placed source is built on its member host only: {creates:#?}"
    );
    let child_creates = creates_of("child-b2r5");
    let [child] = child_creates.as_slice() else {
        panic!("one build of the local child on the controlling host: {creates:#?}");
    };
    let typed = child
        .prompt_context_fork_source
        .as_ref()
        .unwrap_or_else(|| panic!("the local child's build carries its typed source: {child:#?}"));
    assert_eq!(
        typed.source_member,
        meerkat_core::MobMemberBinding {
            mob_id: controlling.mob_id.to_string(),
            role: "worker".to_string(),
            member: "source-b2r5".to_string(),
        },
        "the typed source names the placed source member"
    );
    let source_session = fixture
        .host_binding_record(controlling.mob_id.as_str())
        .await
        .materialized
        .get("source-b2r5")
        .map(|row| row.session_id.clone())
        .expect("the source's materialized row");
    assert_eq!(
        typed.source_session_id.as_ref().map(ToString::to_string),
        Some(source_session),
        "the typed source names the placed source's session on its member host"
    );
    fixture.shutdown().await;
}

/// A scripted host plus a scripted member endpoint for `child`, so the
/// child's first input is observable.
async fn b2_scripted_host_with_child(
    name: &str,
    children: &[&str],
) -> (
    support::ScriptedHostPeer,
    Vec<support::ScriptedMemberTurnResponder>,
) {
    let scripted = support::spawn_scripted_host_peer(&format!("{name}-host")).await;
    let mut responders = Vec::new();
    for child in children {
        let endpoint = Arc::new(
            spawn_peer_comms_endpoint(&format!("{name}-{child}-member"), true, None).await,
        );
        responders.push(support::spawn_scripted_member_turn_responder(Arc::clone(
            &endpoint,
        )));
        scripted.script_member_identity(child, support::member_identity_of(&endpoint));
        scripted.bind_member_endpoint(child, endpoint);
    }
    (scripted, responders)
}

/// The text of the first delivery `responder` received that contains
/// `needle`.
async fn b2_delivered_text(
    responder: &support::ScriptedMemberTurnResponder,
    needle: &str,
) -> String {
    let mut text = None;
    wait_until("the child's first input is delivered", || {
        text = responder
            .received_deliveries()
            .iter()
            .map(|delivery| delivery.content.text_content())
            .find(|content| content.contains(needle));
        let found = text.is_some();
        async move { found }
    })
    .await;
    text.expect("the delivered first input")
}

fn b2_materialize_wire(scripted: &support::ScriptedHostPeer, member: &str) -> serde_json::Value {
    let payloads = scripted.received_materialize_payloads();
    let payload = payloads
        .iter()
        .find(|payload| payload.spec.agent_identity == member)
        .unwrap_or_else(|| panic!("the materialize of {member}"));
    serde_json::to_value(payload).expect("materialize payload wire json")
}

/// Toward a host before protocol V7, the controller's feature probe is
/// refused before it is read, the child is materialized in the V4 shape,
/// and its first input carries the planned context and the notice as a
/// prompt prefix.
#[tokio::test(flavor = "multi_thread")]
async fn a_placed_context_fork_on_a_host_before_v7_keeps_the_prompt_prefix() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let (scripted, responders) = b2_scripted_host_with_child("xhe-b2r2", &["child-b2r2"]).await;
    let controlling = create_controlling_mob("xhe-b2r2").await;
    let report = controlling.bind_scripted(&scripted).await;
    b2_local_source_with_a_turn(&controlling, "source-b2r2", "b2r2-tail-marker").await;

    controlling
        .handle
        .spawn_spec(b2_placed_fork_spec(
            "child-b2r2",
            &report.host_id,
            "source-b2r2",
            "b2r2-first-input",
        ))
        .await
        .expect("the placed fork child spawns on a host before V7");

    let kinds = scripted.received_command_kinds();
    assert!(
        kinds.iter().any(|kind| kind == "host_features"),
        "the controller asked the host for its features: {kinds:?}"
    );
    let wire = b2_materialize_wire(&scripted, "child-b2r2");
    assert!(
        wire.get("prompt_context_fork").is_none(),
        "no V7 field toward a host before V7: {wire}"
    );
    assert_eq!(wire["protocol_version"], 4, "the V4 shape: {wire}");
    let first_input = b2_delivered_text(&responders[0], "b2r2-first-input").await;
    assert!(
        first_input.contains("[fork_notice]") && first_input.contains("b2r2-tail-marker"),
        "the planned context and the notice ride the first input: {first_input}"
    );
    for responder in &responders {
        responder.shutdown();
    }
    scripted.shutdown();
}

/// A host that listed the feature and then refuses the V7
/// materialize before reading it (it was downgraded in between) fails the
/// spawn with the typed version refusal and leaves no carrier; the next fork
/// asks again and falls back to the prompt prefix.
#[tokio::test(flavor = "multi_thread")]
async fn a_v7_materialize_refused_after_the_probe_fails_typed_and_the_next_fork_falls_back() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let (scripted, responders) =
        b2_scripted_host_with_child("xhe-b2r7", &["child-b2r7a", "child-b2r7b"]).await;
    let controlling = create_controlling_mob("xhe-b2r7").await;
    let report = controlling.bind_scripted(&scripted).await;
    b2_local_source_with_a_turn(&controlling, "source-b2r7", "b2r7-tail-marker").await;

    scripted.serve_v7(support::ScriptedV7Mode::Features(vec![
        "prompt_context_fork".to_string(),
    ]));
    scripted.refuse_next_v7_materialize();
    let error = controlling
        .handle
        .spawn_spec(b2_placed_fork_spec(
            "child-b2r7a",
            &report.host_id,
            "source-b2r7",
            "b2r7a-first-input",
        ))
        .await
        .expect_err("a refused V7 materialize fails the spawn");
    let rendered = format!("{error:?} {error}");
    assert!(
        rendered.contains("UnsupportedProtocolVersion") && rendered.contains('7'),
        "the typed refusal names the version: {rendered}"
    );
    assert!(
        controlling
            .storage_metadata
            .load_placed_spawn(&controlling.mob_id, "child-b2r7a")
            .await
            .expect("load the refused child's carrier")
            .is_none(),
        "the refused spawn leaves no carrier"
    );

    scripted.serve_v7(support::ScriptedV7Mode::BeforeV7);
    controlling
        .handle
        .spawn_spec(b2_placed_fork_spec(
            "child-b2r7b",
            &report.host_id,
            "source-b2r7",
            "b2r7b-first-input",
        ))
        .await
        .expect("the next fork falls back to the prompt prefix");
    let probes = scripted
        .received_command_kinds()
        .iter()
        .filter(|kind| *kind == "host_features")
        .count();
    assert_eq!(probes, 2, "every placed context fork asks the host once");
    let wire = b2_materialize_wire(&scripted, "child-b2r7b");
    assert!(wire.get("prompt_context_fork").is_none(), "{wire}");
    let first_input = b2_delivered_text(&responders[1], "b2r7b-first-input").await;
    assert!(first_input.contains("[fork_notice]"), "{first_input}");
    for responder in &responders {
        responder.shutdown();
    }
    scripted.shutdown();
}

/// A feature probe that never answers fails the spawn, bounded, naming
/// the probe, before any carrier or materialize exists.
#[tokio::test(flavor = "multi_thread")]
async fn a_feature_probe_that_never_answers_fails_the_spawn_before_anything_is_recorded() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let (scripted, responders) = b2_scripted_host_with_child("xhe-b2c2", &["child-b2c2"]).await;
    let controlling = create_controlling_mob("xhe-b2c2").await;
    let report = controlling.bind_scripted(&scripted).await;
    b2_local_source_with_a_turn(&controlling, "source-b2c2", "b2c2-tail-marker").await;

    scripted.serve_v7(support::ScriptedV7Mode::HoldFeatures);
    let started = std::time::Instant::now();
    let error = controlling
        .handle
        .spawn_spec(b2_placed_fork_spec(
            "child-b2c2",
            &report.host_id,
            "source-b2c2",
            "b2c2-first-input",
        ))
        .await
        .expect_err("an unanswered feature probe fails the spawn");
    assert!(
        error.to_string().contains("HostFeatures"),
        "the failure names the probe: {error}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(30),
        "the probe is bounded: {:?}",
        started.elapsed()
    );
    assert!(
        controlling
            .storage_metadata
            .load_placed_spawn(&controlling.mob_id, "child-b2c2")
            .await
            .expect("load the child's carrier")
            .is_none(),
        "no carrier exists"
    );
    assert!(
        scripted
            .received_materialize_payloads()
            .iter()
            .all(|payload| payload.spec.agent_identity != "child-b2c2"),
        "nothing was materialized"
    );
    for responder in &responders {
        responder.shutdown();
    }
    scripted.shutdown();
}

/// A placed context-fork child whose live runtime is gone is revived like
/// any placed member: the controller re-issues its recorded tuple as a
/// Resume without a fork, the member host answers it from the recorded row,
/// and the child keeps the seed recorded at its first build. Nothing records
/// a second seed.
#[tokio::test(flavor = "multi_thread")]
async fn a_placed_context_fork_child_is_revived_from_its_recorded_row() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let mut opts = HostFixtureOptions::named("xhe-b2m1-host").with_member_build();
    opts.member_llm_client = Some(scripted_member_client_completing("b2m1-reply"));
    opts.record_member_creates = true;
    let fixture = spawn_host_daemon_fixture(opts)
        .await
        .expect("spawn member-build host fixture");
    let controlling = create_controlling_mob("xhe-b2m1").await;
    let report = controlling.bind_fixture(&fixture).await;
    b2_placed_source_with_a_turn(
        &controlling,
        "source-b2m1",
        &report.host_id,
        "b2m1-tail-marker",
    )
    .await;

    // A turn-driven fork child without a first input: its turns ride the
    // controller's deliveries, and a failed delivery triggers the revival.
    let mut spec = placed_spawn_spec("worker", "child-b2m1", &report.host_id);
    spec.launch_mode = meerkat_mob::MemberLaunchMode::Fork {
        source_member_id: identity("source-b2m1"),
        fork_context: meerkat_mob::ForkContext::LastMessages { count: 2 },
    };
    controlling
        .handle
        .spawn_spec(spec)
        .await
        .expect("the placed fork child spawns");
    let before = fixture
        .host_binding_record(controlling.mob_id.as_str())
        .await
        .materialized
        .get("child-b2m1")
        .cloned()
        .expect("the child's materialized row");
    assert!(
        before.prompt_context_fork_digest.is_some(),
        "the child was materialized with its fork"
    );
    let seeds = || {
        fixture
            .failing_create
            .as_ref()
            .expect("the fixture records member creates")
            .recorded_creates()
            .into_iter()
            .filter(|create| {
                create
                    .mob_member_binding
                    .as_ref()
                    .is_some_and(|binding| binding.member == "child-b2m1")
            })
            .filter_map(|create| create.fork_seed)
            .collect::<Vec<_>>()
    };
    let seeded = seeds();
    assert_eq!(
        seeded.len(),
        1,
        "the first build records the seed: {seeded:?}"
    );

    // Kill the child's live runtime on its member host; its durable row
    // stays.
    let session_id = meerkat_core::SessionId::parse(&before.session_id).expect("session id");
    fixture
        .member_session_service()
        .discard_live_session(&session_id)
        .await
        .expect("discard the child's live session");
    let member = controlling
        .handle
        .member(&identity("child-b2m1"))
        .await
        .expect("child handle");
    let _first = member.send("b2m1-are-you-there", HandlingMode::Queue).await;
    wait_until("the fork child to revive on its member host", || async {
        fixture
            .member_session_service()
            .has_live_session(&session_id)
            .await
            .unwrap_or(false)
    })
    .await;
    member
        .send("b2m1-task-after-revival", HandlingMode::Queue)
        .await
        .expect("the revived fork child serves a turn");

    // The child holds the context recorded at its first build: one fork
    // notice.
    let rows = b2_placed_rows_after_turn(
        &fixture,
        &controlling,
        "child-b2m1",
        "b2m1-task-after-revival",
    )
    .await;
    assert_eq!(b2_fork_notices(&rows).len(), 1, "one fork notice: {rows:?}");
    // The revival was answered from the row: the same tuple and fork, and no
    // second seed.
    let after = fixture
        .host_binding_record(controlling.mob_id.as_str())
        .await
        .materialized
        .get("child-b2m1")
        .cloned()
        .expect("the child's row survives the revival");
    assert_eq!(
        (
            after.generation,
            after.fence_token,
            &after.session_id,
            &after.prompt_context_fork_digest
        ),
        (
            before.generation,
            before.fence_token,
            &before.session_id,
            &before.prompt_context_fork_digest
        ),
        "the revival keeps the recorded tuple and fork"
    );
    assert_eq!(seeds(), seeded, "a revival records no second seed");
    fixture.shutdown().await;
}
