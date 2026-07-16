//! Multi-host mobs phase 6 — remote flow dispatch end-to-end (§18.11:1043):
//! the T-F1..T-F11 rows of design-flow-spine §5.1.
//!
//! Deterministic two-hosts-in-one-process battery (loopback TCP acceptor,
//! scripted member LLM clients, no live providers) — ungated, riding the
//! `cargo int` GitHub-CI ratchet. A flow step targeting a PLACED member rides
//! the ONE delivery command (`DeliverMemberInput` + `BridgeTurnDirective`),
//! is admitted member-side as a tracked turn, journals its terminal in the
//! host's `MobHostBindingAuthority`, and completes controlling-side via the
//! poll pump's `turn_outcomes` sidecar feeding the
//! `RemoteFlowTicketRegistry` — never via a console subscription (A17).
//!
//! Machine catalog + V4 wire are FROZEN: these tests consume phase-1 types
//! and pin phase-6 runtime realization only.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::time::Duration;

use meerkat_mob::runtime::bridge_protocol::{
    BridgeCommand, BridgeDeliveryOutcome, BridgeDeliveryRejectionCause, BridgeEventCursor,
    BridgeReply, BridgeTurnCorrelation, BridgeTurnDirective, BridgeTurnOutcomeAck,
    MaterializeLaunchMode,
};
use meerkat_mob::{
    AgentIdentity, FlowId, FlowStepDispatchRejectKind, MobError, MobRunStatus, StepId,
};
use support::{
    ControllingMob, FIXTURE_BRIDGE_TIMEOUT, HostFixtureOptions, REAL_COMMS_TEST_LOCK, StallGate,
    bind_then_materialize, collect_mob_stream_until, create_controlling_mob,
    create_controlling_mob_with_flows, member_descriptor_from_ack, member_incarnation_from_ack,
    raw_deliver_member_input_command, raw_poll_member_events_command_with_outcome_protocol,
    sample_materialize_payload, sample_portable_member_spec,
    scripted_member_client_calling_tool_then_completing, scripted_member_client_completing,
    scripted_member_client_stalling, single_step_flow, single_step_flow_with_blocked_tools,
    single_step_flow_with_timeout, spawn_host_daemon_fixture, spawn_peer_comms_endpoint,
    spawn_scripted_host_peer, spawn_scripted_member_turn_responder, wait_for_flow_run_terminal,
    wait_until,
};

const RUN_WAIT: Duration = Duration::from_secs(60);

fn identity(name: &str) -> AgentIdentity {
    AgentIdentity::from(name)
}

/// Deadline-poll the phase-6 obligation projection until it is empty
/// (`pending_remote_turn_outcomes` — resolved obligations leave the set).
async fn wait_for_obligations_resolved(controlling: &ControllingMob) {
    wait_until("remote turn obligations resolved", || async {
        controlling
            .handle
            .remote_turn_obligations_observation()
            .is_empty()
    })
    .await;
}

// ==========================================================================
// T-F1 — remote TurnDriven flow step completes via journal + sidecar
// ==========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn remote_turn_driven_flow_step_completes_via_journal_and_sidecar() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let mut opts = HostFixtureOptions::named("xhf-t1-host-b").with_member_build();
    opts.member_llm_client = Some(scripted_member_client_completing("done"));
    let fixture = spawn_host_daemon_fixture(opts)
        .await
        .expect("spawn member-build host fixture");
    let controlling = create_controlling_mob_with_flows(
        "xhf-t1",
        vec![(
            FlowId::from("remote-step"),
            single_step_flow("worker", "go"),
        )],
    )
    .await;
    let report = controlling.bind_fixture(&fixture).await;
    let mob_id = controlling.mob_id.to_string();

    controlling
        .spawn_placed("worker", "b1", &report.host_id)
        .await
        .expect("b1 materializes on host B");

    // NO console subscription is created anywhere in this test: flow-step
    // completion must ride the obligation-driven pump (A17), not a watcher.
    let run_id = controlling
        .handle
        .run_flow(FlowId::from("remote-step"), serde_json::json!({}))
        .await
        .expect("run flow against the placed member");
    let run = wait_for_flow_run_terminal(&controlling.handle, &run_id, RUN_WAIT).await;

    assert_eq!(
        run.status,
        MobRunStatus::Completed,
        "remote step must complete via journal+sidecar; failures={:?}",
        run.failure_ledger
    );
    // The step output is the member's terminal RunCompleted result.
    let step_output = run
        .root_step_outputs
        .get(&StepId::from("step-1"))
        .expect("completed run records the step output");
    assert_eq!(
        step_output,
        &serde_json::json!({ "b1": "done" }),
        "step output is the remote terminal's result payload, keyed by member \
         (the local-parity collection shape)"
    );

    // Obligation window closed: recorded at dispatch, resolved at consume.
    wait_for_obligations_resolved(&controlling).await;

    // The next sequential pump poll acknowledges the exact consumed row and
    // the member host prunes it durably. Completion therefore cannot grow an
    // unbounded serve-all journal.
    wait_until("consumed turn outcome pruned", || async {
        fixture.turn_outcome_rows(&mob_id).await.is_empty()
    })
    .await;

    // The placed member never ran through a controlling-local session: the
    // ref-carried session id names the REMOTE resident session (ADJ-24),
    // and the CONTROLLING service holds no session under it (local
    // start_turn / inject paths untouched).
    let b1 = controlling
        .handle
        .get_member(&identity("b1"))
        .await
        .expect("get b1")
        .expect("b1 present");
    let remote_session = b1
        .bridge_session_id()
        .expect("placed member ref names its remote resident session")
        .clone();
    assert!(
        meerkat_core::service::SessionService::read(controlling.service.as_ref(), &remote_session)
            .await
            .is_err(),
        "the controlling service must hold no session for the placed member's remote session id"
    );

    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn oversized_terminal_event_resolves_promptly_via_sidecar() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let mut output = "oversized-terminal-output".to_string();
    output.push_str(&"\u{0000}\"\\🦀".repeat(100_000));

    let mut opts = HostFixtureOptions::named("xhf-oversized-terminal-host").with_member_build();
    opts.member_llm_client = Some(scripted_member_client_completing(&output));
    let fixture = spawn_host_daemon_fixture(opts)
        .await
        .expect("spawn member-build host fixture");
    let controlling = create_controlling_mob_with_flows(
        "xhf-oversized-terminal",
        vec![(
            FlowId::from("oversized-failure"),
            single_step_flow_with_timeout("worker", "fail largely", 45_000),
        )],
    )
    .await;
    let report = controlling.bind_fixture(&fixture).await;

    controlling
        .spawn_placed("worker", "oversized-worker", &report.host_id)
        .await
        .expect("member materializes on host B");
    let mut router = controlling
        .handle
        .subscribe_mob_events()
        .await
        .expect("merged stream subscribes before the oversized turn");
    let run_id = controlling
        .handle
        .run_flow(FlowId::from("oversized-failure"), serde_json::json!({}))
        .await
        .expect("run directed remote failure");
    let run = tokio::time::timeout(
        Duration::from_secs(30),
        wait_for_flow_run_terminal(&controlling.handle, &run_id, RUN_WAIT),
    )
    .await
    .expect("compacted sidecar resolves before the 45-second turn timeout");

    assert_eq!(run.status, MobRunStatus::Failed);
    let reason = run
        .failure_ledger
        .iter()
        .find(|entry| entry.step_id.as_str() == "step-1")
        .map(|entry| entry.reason.as_str())
        .expect("failed step is recorded");
    assert!(
        reason.contains("remote terminal event row at durable seq")
            && reason.contains("exceeded the bridge budget")
            && reason.contains("payload for")
            && reason.contains("unavailable"),
        "the sidecar terminal resolves honestly without fabricating its omitted payload: {reason}"
    );
    let truncations = collect_mob_stream_until(
        &mut router,
        |event| {
            event.source.identity.as_str() == "oversized-worker"
                && matches!(
                    event.envelope.payload,
                    meerkat_core::AgentEvent::StreamTruncated {
                        reason:
                            meerkat_core::event::StreamTruncationReason::OversizedRemoteEvent {
                                encoded_bytes,
                                max_bytes,
                                ..
                            }
                    } if encoded_bytes > max_bytes
                )
        },
        Duration::from_secs(5),
    )
    .await;
    assert!(
        truncations.iter().any(|event| matches!(
            event.envelope.payload,
            meerkat_core::AgentEvent::StreamTruncated {
                reason: meerkat_core::event::StreamTruncationReason::OversizedRemoteEvent { .. }
            }
        )),
        "the immutable terminal event row was omitted typed"
    );

    wait_for_obligations_resolved(&controlling).await;
    fixture.shutdown().await;
}

// ==========================================================================
// T-F2 — overlay enforced member-side: denied tool ⇒ access_denied INSIDE
// the remote turn; run still completes
// ==========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn remote_overlay_enforced_member_side_denied_tool_yields_access_denied() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let mut opts = HostFixtureOptions::named("xhf-t2-host-b").with_member_build();
    // The member calls the (installed, comms-tools) `send_message` tool once,
    // then completes. The step overlay blocks it, so the call must be denied
    // INSIDE the remote turn by the member-side dispatcher gate.
    opts.member_llm_client = Some(scripted_member_client_calling_tool_then_completing(
        "send_message",
        serde_json::json!({"to": "nobody", "message": "hi"}),
        "overlay-done",
    ));
    let fixture = spawn_host_daemon_fixture(opts)
        .await
        .expect("spawn member-build host fixture");
    let controlling = create_controlling_mob_with_flows(
        "xhf-t2",
        vec![(
            FlowId::from("overlay-step"),
            single_step_flow_with_blocked_tools("worker", "go", &["send_message"]),
        )],
    )
    .await;
    let report = controlling.bind_fixture(&fixture).await;
    let mob_id = controlling.mob_id.to_string();

    controlling
        .spawn_placed("worker", "b2", &report.host_id)
        .await
        .expect("b2 materializes on host B");

    let run_id = controlling
        .handle
        .run_flow(FlowId::from("overlay-step"), serde_json::json!({}))
        .await
        .expect("run overlay flow");
    let run = wait_for_flow_run_terminal(&controlling.handle, &run_id, RUN_WAIT).await;
    // The overlay is enforced ON THE MEMBER HOST: the blocked tool leaves
    // the turn's visible set, and a (scripted) rogue call to an invisible
    // tool is core-canonically a TYPED `access_denied` run failure — the
    // same semantics a local flow step with this client exhibits. The step
    // fails typed with the member-side denial, attributed through the
    // journal + sidecar (a failed terminal rides the same machinery).
    assert_eq!(
        run.status,
        MobRunStatus::Failed,
        "the member-side overlay denial fails the step typed; failures={:?}",
        run.failure_ledger
    );
    assert!(
        run.failure_ledger.iter().any(|entry| {
            entry.step_id.as_str() == "step-1" && entry.reason.contains("not allowed by policy")
        }),
        "the step failure carries the member-side typed denial: {:?}",
        run.failure_ledger
    );

    // Member-side session history records the denied call INSIDE the remote
    // turn (the call reached the member host; the denial happened THERE).
    let member_session_id = fixture
        .host_binding_record(&mob_id)
        .await
        .materialized
        .get("b2")
        .expect("b2 materialized row")
        .session_id
        .clone();
    let history = fixture
        .member_session_history(&member_session_id)
        .await
        .expect("member session history reads");
    let rendered = serde_json::to_string(&history.messages).expect("render member history");
    assert!(
        rendered.contains("send_message"),
        "member transcript records the denied tool call: {rendered}"
    );

    // Failed terminals ride the same exact-ack prune path as successful
    // terminals after the failure has been consumed controlling-side.
    wait_until("consumed failed turn outcome pruned", || async {
        fixture.turn_outcome_rows(&mob_id).await.is_empty()
    })
    .await;

    fixture.shutdown().await;
}

// ==========================================================================
// T-F3 — remote AutonomousHost step WITHOUT overlay completes
// ==========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn remote_autonomous_step_without_overlay_completes() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let mut opts = HostFixtureOptions::named("xhf-t3-host-b").with_member_build();
    opts.member_llm_client = Some(scripted_member_client_completing("auto-done"));
    let fixture = spawn_host_daemon_fixture(opts)
        .await
        .expect("spawn member-build host fixture");
    let controlling = create_controlling_mob_with_flows(
        "xhf-t3",
        vec![(FlowId::from("auto-step"), single_step_flow("worker", "go"))],
    )
    .await;
    let report = controlling.bind_fixture(&fixture).await;

    let mut spec = support::placed_spawn_spec("worker", "b3", &report.host_id);
    spec.runtime_mode = Some(meerkat_mob::MobRuntimeMode::AutonomousHost);
    controlling
        .handle
        .spawn_spec(spec)
        .await
        .expect("autonomous b3 materializes on host B");

    let run_id = controlling
        .handle
        .run_flow(FlowId::from("auto-step"), serde_json::json!({}))
        .await
        .expect("run autonomous flow");
    let run = wait_for_flow_run_terminal(&controlling.handle, &run_id, RUN_WAIT).await;
    assert_eq!(
        run.status,
        MobRunStatus::Completed,
        "overlay-free autonomous remote step completes via journal+sidecar; failures={:?}",
        run.failure_ledger
    );

    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn placed_autonomous_kickoff_barrier_resolves_started_with_objective() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let mut opts = HostFixtureOptions::named("xhf-kickoff-host-b").with_member_build();
    opts.member_llm_client = Some(scripted_member_client_completing("kickoff-done"));
    let fixture = spawn_host_daemon_fixture(opts)
        .await
        .expect("spawn member-build host fixture");
    let controlling = create_controlling_mob("xhf-kickoff").await;
    let report = controlling.bind_fixture(&fixture).await;
    let member = identity("kickoff-worker");
    let objective_id = meerkat_core::interaction::ObjectiveId::new();

    let mut spec = support::placed_spawn_spec("worker", member.as_str(), &report.host_id);
    spec.runtime_mode = Some(meerkat_mob::MobRuntimeMode::AutonomousHost);
    spec.objective_id = Some(objective_id);
    controlling
        .handle
        .spawn_spec(spec)
        .await
        .expect("placed autonomous member materializes");

    let snapshots = controlling
        .handle
        .wait_for_members_kickoff_complete(std::slice::from_ref(&member), Some(RUN_WAIT))
        .await
        .expect("placed kickoff barrier resolves from the pumped exact terminal");
    let kickoff = snapshots[0]
        .1
        .kickoff
        .as_ref()
        .expect("autonomous member exposes kickoff state");
    assert_eq!(kickoff.phase, meerkat_mob::MobMemberKickoffPhase::Started);
    assert_eq!(kickoff.objective_id, Some(objective_id));

    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn stopping_placed_autonomous_kickoff_resolves_cancelled_without_timeout() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let gate = StallGate::new();
    let mut opts = HostFixtureOptions::named("xhf-kickoff-stop-host-b").with_member_build();
    opts.member_llm_client = Some(scripted_member_client_stalling(gate.clone()));
    let fixture = spawn_host_daemon_fixture(opts)
        .await
        .expect("spawn member-build host fixture");
    let controlling = create_controlling_mob("xhf-kickoff-stop").await;
    let report = controlling.bind_fixture(&fixture).await;
    let member = identity("kickoff-stalled-worker");

    let mut spec = support::placed_spawn_spec("worker", member.as_str(), &report.host_id);
    spec.runtime_mode = Some(meerkat_mob::MobRuntimeMode::AutonomousHost);
    controlling
        .handle
        .spawn_spec(spec)
        .await
        .expect("placed autonomous member materializes");
    let ready = controlling
        .handle
        .wait_for_members_ready(std::slice::from_ref(&member), Some(Duration::from_secs(2)))
        .await
        .expect("placed readiness comes from committed remote placement, not a local session");
    assert_eq!(ready[0].1.status, meerkat_mob::MobMemberStatus::Active);
    let before_stop = controlling
        .handle
        .member_status(&member)
        .await
        .expect("read starting kickoff");
    assert_eq!(
        before_stop
            .kickoff
            .as_ref()
            .expect("autonomous member exposes kickoff state")
            .phase,
        meerkat_mob::MobMemberKickoffPhase::Starting
    );

    controlling
        .handle
        .stop()
        .await
        .expect("stop controlling mob");
    let snapshots = controlling
        .handle
        .wait_for_members_kickoff_complete(
            std::slice::from_ref(&member),
            Some(Duration::from_secs(2)),
        )
        .await
        .expect("stop resolves the placed kickoff barrier immediately");
    assert_eq!(
        snapshots[0]
            .1
            .kickoff
            .as_ref()
            .expect("stopped member retains terminal kickoff snapshot")
            .phase,
        meerkat_mob::MobMemberKickoffPhase::Cancelled
    );

    gate.release();
    fixture.shutdown().await;
}

// ==========================================================================
// T-F4 — overlay+autonomous rejected AT DISPATCH, same cause local and
// remote; zero deliveries, zero injector calls
// ==========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn overlay_autonomous_rejected_at_dispatch_same_cause_local_and_remote() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let mut opts = HostFixtureOptions::named("xhf-t4-host-b").with_member_build();
    opts.member_llm_client = Some(scripted_member_client_completing("never"));
    let fixture = spawn_host_daemon_fixture(opts)
        .await
        .expect("spawn member-build host fixture");
    let controlling = create_controlling_mob_with_flows(
        "xhf-t4",
        vec![
            (
                FlowId::from("overlay-local"),
                single_step_flow_with_blocked_tools("quiet-worker", "go", &["send_message"]),
            ),
            (
                FlowId::from("overlay-remote"),
                single_step_flow_with_blocked_tools("worker", "go", &["send_message"]),
            ),
        ],
    )
    .await;
    let report = controlling.bind_fixture(&fixture).await;
    let mob_id = controlling.mob_id.to_string();

    // Local autonomous member (session backend, distinct role so each
    // overlay flow targets exactly one member).
    let mut local_spec = meerkat_mob::SpawnMemberSpec::new("quiet-worker", "l-auto")
        .with_backend(meerkat_mob::MobBackendKind::Session);
    local_spec.runtime_mode = Some(meerkat_mob::MobRuntimeMode::AutonomousHost);
    controlling
        .handle
        .spawn_spec(local_spec)
        .await
        .expect("spawn local autonomous member");

    // Placed autonomous member.
    let mut placed_spec = support::placed_spawn_spec("worker", "b-auto", &report.host_id);
    placed_spec.runtime_mode = Some(meerkat_mob::MobRuntimeMode::AutonomousHost);
    controlling
        .handle
        .spawn_spec(placed_spec)
        .await
        .expect("spawn placed autonomous member");

    for flow in ["overlay-local", "overlay-remote"] {
        let run_id = controlling
            .handle
            .run_flow(FlowId::from(flow), serde_json::json!({}))
            .await
            .expect("overlay flow starts (rejection is per-step, at dispatch)");
        let run = wait_for_flow_run_terminal(&controlling.handle, &run_id, RUN_WAIT).await;
        assert_eq!(
            run.status,
            MobRunStatus::Failed,
            "{flow}: overlay+autonomous step fails typed at dispatch"
        );
        // ONE machine arm, ONE error variant — identical cause both sides
        // (§18.11:1043 "same cause local and remote").
        let expected = MobError::FlowStepDispatchRejected {
            target: identity("ignored"),
            kind: FlowStepDispatchRejectKind::OverlayAutonomous,
        };
        let expected_marker = match expected {
            MobError::FlowStepDispatchRejected { kind, .. } => kind.to_string(),
            _ => unreachable!(),
        };
        assert!(
            run.failure_ledger
                .iter()
                .any(|entry| entry.reason.contains(&expected_marker)),
            "{flow}: failure ledger names the typed dispatch-reject cause \
             ({expected_marker}), got {:?}",
            run.failure_ledger
        );
    }

    // Rejected BEFORE delivery: the member host never saw a directed turn —
    // no journal row, and the placed member ran no turn.
    assert!(
        fixture.turn_outcome_rows(&mob_id).await.is_empty(),
        "no directed turn ever reached the member host"
    );

    fixture.shutdown().await;
}

// ==========================================================================
// T-F5a — Deduplicated redelivery converges on one outcome (member-side
// idempotency + journal convergence, raw probe against the real member)
// ==========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn deduplicated_redelivery_converges_on_one_outcome() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let mut opts = HostFixtureOptions::named("xhf-t5-host-b").with_member_build();
    opts.member_llm_client = Some(scripted_member_client_completing("dedup-done"));
    let fixture = spawn_host_daemon_fixture(opts)
        .await
        .expect("spawn member-build host fixture");

    let probe = spawn_peer_comms_endpoint("xhf-t5-supervisor", true, None).await;
    probe.trust(fixture.host_peer_descriptor()).await;
    let mob_id = format!("xhf-t5-{}", uuid::Uuid::new_v4().simple());
    let ack = bind_then_materialize(&probe, &fixture, &mob_id, "dedup-member").await;
    let member = member_descriptor_from_ack("dedup-member", &ack);
    let expected_member =
        member_incarnation_from_ack(&fixture, &mob_id, "dedup-member", &ack, 1, 1, 1);
    probe.trust(member.clone()).await;

    let input_id = uuid::Uuid::new_v4().to_string();
    let directive = BridgeTurnDirective {
        correlation: BridgeTurnCorrelation {
            run_id: "run-dedup".to_string(),
            step_id: "step-1".to_string(),
        },
        tool_overlay: None,
    };
    let command = raw_deliver_member_input_command(
        &probe,
        &mob_id,
        &input_id,
        "please complete",
        expected_member.clone(),
        Some(directive),
    );

    // First delivery: accepted; the tracked turn runs.
    let first = probe
        .send_bridge_command_raw(&member, &command, FIXTURE_BRIDGE_TIMEOUT)
        .await
        .expect("first directive delivery reply");
    match first {
        BridgeReply::Delivery(response) => {
            assert!(
                matches!(response.outcome, BridgeDeliveryOutcome::Accepted),
                "first delivery is accepted: {response:?}"
            );
            assert_eq!(
                response.canonical_input_id.as_deref(),
                Some(input_id.as_str()),
                "accepted runtime InputId is the controller payload/journal UUID"
            );
        }
        other => panic!("expected delivery reply, got {other:?}"),
    }

    // The journal converges on ONE row before and regardless of redelivery.
    wait_until("turn outcome journal row", || async {
        fixture.turn_outcome_rows(&mob_id).await.len() == 1
    })
    .await;

    // Byte-identical redelivery at the SAME input_id: member dedup replies
    // Deduplicated; NO second turn, NO second journal row.
    let second = probe
        .send_bridge_command_raw(&member, &command, FIXTURE_BRIDGE_TIMEOUT)
        .await
        .expect("redelivery reply");
    match second {
        BridgeReply::Delivery(response) => match response.outcome {
            BridgeDeliveryOutcome::Deduplicated { existing_input_id } => {
                assert_eq!(existing_input_id, input_id);
                assert_eq!(
                    response.canonical_input_id.as_deref(),
                    Some(input_id.as_str())
                );
            }
            other => panic!("redelivery at the same input_id must deduplicate: {other:?}"),
        },
        other => panic!("expected delivery reply, got {other:?}"),
    }

    let rows = fixture.turn_outcome_rows(&mob_id).await;
    assert_eq!(
        rows.len(),
        1,
        "journal holds exactly one row after redelivery: {rows:?}"
    );
    assert_eq!(rows[0].input_id, input_id);

    // Exactly one turn ran on the member session.
    let history = fixture
        .member_session_history(&ack.session_id)
        .await
        .expect("member history reads");
    let user_turns = history
        .messages
        .iter()
        .filter(|message| {
            serde_json::to_string(message)
                .unwrap_or_default()
                .contains("please complete")
        })
        .count();
    assert_eq!(
        user_turns, 1,
        "the directed input materialized exactly once"
    );

    let page_command = raw_poll_member_events_command_with_outcome_protocol(
        &probe,
        &mob_id,
        1,
        expected_member.clone(),
        BridgeEventCursor::Tail,
        Some(1),
        Some(0),
        Vec::new(),
        Some(1),
    );
    let page = probe
        .send_bridge_command_raw(&member, &page_command, FIXTURE_BRIDGE_TIMEOUT)
        .await
        .expect("exact terminal sidecar page");
    let BridgeReply::MemberEventsPage(page) = page else {
        panic!("expected member events page, got {page:?}")
    };
    assert_eq!(page.turn_outcomes.len(), 1);
    assert_eq!(page.turn_outcomes[0].input_id, input_id);
    let ack = BridgeTurnOutcomeAck {
        generation: page.turn_outcomes[0].generation,
        fence_token: page.turn_outcomes[0].fence_token,
        input_id: input_id.clone(),
    };
    let ack_command = raw_poll_member_events_command_with_outcome_protocol(
        &probe,
        &mob_id,
        1,
        expected_member,
        BridgeEventCursor::Tail,
        Some(1),
        Some(0),
        vec![ack],
        Some(1),
    );
    let _ = probe
        .send_bridge_command_raw(&member, &ack_command, FIXTURE_BRIDGE_TIMEOUT)
        .await
        .expect("exact terminal ACK applies");
    assert!(
        fixture.turn_outcome_rows(&mob_id).await.is_empty(),
        "the one exact input/journal key closes only after its exact terminal ACK"
    );

    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn stale_plain_and_directed_deliveries_cannot_cross_rematerialization() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let mut opts = HostFixtureOptions::named("xhf-stale-delivery-host").with_member_build();
    opts.member_llm_client = Some(scripted_member_client_completing("must-not-run"));
    let fixture = spawn_host_daemon_fixture(opts)
        .await
        .expect("spawn member-build host fixture");
    let probe = spawn_peer_comms_endpoint("xhf-stale-delivery-supervisor", true, None).await;
    probe.trust(fixture.host_peer_descriptor()).await;
    let mob_id = format!("xhf-stale-delivery-{}", uuid::Uuid::new_v4().simple());
    let old_ack = bind_then_materialize(&probe, &fixture, &mob_id, "stale-member").await;
    let old_expected =
        member_incarnation_from_ack(&fixture, &mob_id, "stale-member", &old_ack, 1, 1, 1);
    let held_plain = raw_deliver_member_input_command(
        &probe,
        &mob_id,
        &uuid::Uuid::new_v4().to_string(),
        "stale plain work",
        old_expected.clone(),
        None,
    );
    let held_directed = raw_deliver_member_input_command(
        &probe,
        &mob_id,
        &uuid::Uuid::new_v4().to_string(),
        "stale directed work",
        old_expected,
        Some(BridgeTurnDirective {
            correlation: BridgeTurnCorrelation {
                run_id: "stale-run".to_string(),
                step_id: "stale-step".to_string(),
            },
            tool_overlay: None,
        }),
    );

    let replacement_payload = sample_materialize_payload(
        &probe,
        1,
        sample_portable_member_spec(&mob_id, "stale-member", "worker"),
        2,
        2,
        MaterializeLaunchMode::Fresh {},
    );
    let replacement_reply = probe
        .send_bridge_command_raw(
            &fixture.host_peer_descriptor(),
            &BridgeCommand::MaterializeMember(replacement_payload),
            FIXTURE_BRIDGE_TIMEOUT,
        )
        .await
        .expect("superseding materialize reply");
    let BridgeReply::MemberMaterialized(new_ack) = replacement_reply else {
        panic!("expected superseding materialize ack, got {replacement_reply:?}")
    };
    let new_member = member_descriptor_from_ack("stale-member", &new_ack);
    probe.trust(new_member.clone()).await;
    let new_expected =
        member_incarnation_from_ack(&fixture, &mob_id, "stale-member", &new_ack, 1, 2, 2);
    let initial_history_len = fixture
        .member_session_history(&new_ack.session_id)
        .await
        .expect("new member history reads")
        .messages
        .len();

    for held in [held_plain, held_directed] {
        let reply = probe
            .send_bridge_command_raw(&new_member, &held, FIXTURE_BRIDGE_TIMEOUT)
            .await
            .expect("stale held delivery returns typed rejection");
        let BridgeReply::Delivery(response) = reply else {
            panic!("expected delivery response, got {reply:?}")
        };
        match response.outcome {
            BridgeDeliveryOutcome::Rejected {
                cause: BridgeDeliveryRejectionCause::StaleMemberIncarnation { current },
                ..
            } => assert_eq!(current, new_expected),
            other => panic!("stale delivery must reject before admission, got {other:?}"),
        }
    }

    let final_history_len = fixture
        .member_session_history(&new_ack.session_id)
        .await
        .expect("new member history reads after stale delivery")
        .messages
        .len();
    assert_eq!(
        final_history_len, initial_history_len,
        "neither stale ordinary nor stale directed delivery appends new-incarnation input"
    );
    assert!(
        fixture.turn_outcome_rows(&mob_id).await.is_empty(),
        "stale directed delivery never opens Pending/outcome journal custody"
    );

    fixture.shutdown().await;
}

// ==========================================================================
// Outcome protocol — bounded pages, exact durable prune, and no tombstone
// when an acknowledgement races ahead of a delayed journal commit.
// ==========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn outcome_pages_are_bounded_ack_pruned_and_delay_safe() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let mut opts = HostFixtureOptions::named("xhf-outcome-pages-host").with_member_build();
    opts.member_llm_client = Some(scripted_member_client_completing("done"));
    let fixture = spawn_host_daemon_fixture(opts)
        .await
        .expect("spawn member-build host fixture");
    let probe = spawn_peer_comms_endpoint("xhf-outcome-pages-supervisor", true, None).await;
    probe.trust(fixture.host_peer_descriptor()).await;
    let mob_id = format!("xhf-outcome-pages-{}", uuid::Uuid::new_v4().simple());
    let materialized = bind_then_materialize(&probe, &fixture, &mob_id, "paged-member").await;
    let member = member_descriptor_from_ack("paged-member", &materialized);
    let expected_member =
        member_incarnation_from_ack(&fixture, &mob_id, "paged-member", &materialized, 1, 1, 1);
    probe.trust(member.clone()).await;

    for ordinal in 0..3 {
        let input_id = uuid::Uuid::new_v4().to_string();
        let command = raw_deliver_member_input_command(
            &probe,
            &mob_id,
            &input_id,
            "complete",
            expected_member.clone(),
            Some(BridgeTurnDirective {
                correlation: BridgeTurnCorrelation {
                    run_id: format!("paged-run-{ordinal}"),
                    step_id: "step-1".to_string(),
                },
                tool_overlay: None,
            }),
        );
        let reply = probe
            .send_bridge_command_raw(&member, &command, FIXTURE_BRIDGE_TIMEOUT)
            .await
            .expect("directed delivery reply");
        assert!(matches!(
            reply,
            BridgeReply::Delivery(response)
                if matches!(response.outcome, BridgeDeliveryOutcome::Accepted)
        ));
    }
    wait_until("three durable turn outcomes", || async {
        fixture.turn_outcome_rows(&mob_id).await.len() == 3
    })
    .await;

    let poll = |acks: Vec<BridgeTurnOutcomeAck>| {
        raw_poll_member_events_command_with_outcome_protocol(
            &probe,
            &mob_id,
            1,
            expected_member.clone(),
            BridgeEventCursor::Tail,
            Some(1),
            Some(0),
            acks,
            Some(2),
        )
    };
    let first = probe
        .send_bridge_command_raw(&member, &poll(Vec::new()), FIXTURE_BRIDGE_TIMEOUT)
        .await
        .expect("first bounded outcome page");
    let BridgeReply::MemberEventsPage(first) = first else {
        panic!("expected first member events page")
    };
    assert_eq!(first.turn_outcomes.len(), 2);
    assert!(!first.outcomes_complete);
    let first_acks: Vec<_> = first
        .turn_outcomes
        .iter()
        .map(|row| BridgeTurnOutcomeAck {
            generation: row.generation,
            fence_token: row.fence_token,
            input_id: row.input_id.clone(),
        })
        .collect();

    let second = probe
        .send_bridge_command_raw(&member, &poll(first_acks), FIXTURE_BRIDGE_TIMEOUT)
        .await
        .expect("second bounded outcome page");
    let BridgeReply::MemberEventsPage(second) = second else {
        panic!("expected second member events page")
    };
    assert_eq!(second.turn_outcomes.len(), 1);
    assert!(second.outcomes_complete);
    assert_eq!(
        fixture.turn_outcome_rows(&mob_id).await.len(),
        1,
        "first page acknowledgements prune their durable rows"
    );

    let delayed_input = uuid::Uuid::new_v4().to_string();
    let final_acks = vec![
        BridgeTurnOutcomeAck {
            generation: second.turn_outcomes[0].generation,
            fence_token: second.turn_outcomes[0].fence_token,
            input_id: second.turn_outcomes[0].input_id.clone(),
        },
        // This row does not exist yet. The host must not tombstone it.
        BridgeTurnOutcomeAck {
            generation: expected_member.generation,
            fence_token: expected_member.fence_token,
            input_id: delayed_input.clone(),
        },
    ];
    let final_page = probe
        .send_bridge_command_raw(&member, &poll(final_acks), FIXTURE_BRIDGE_TIMEOUT)
        .await
        .expect("final acknowledgement page");
    assert!(matches!(
        final_page,
        BridgeReply::MemberEventsPage(page)
            if page.turn_outcomes.is_empty() && page.outcomes_complete
    ));
    assert!(fixture.turn_outcome_rows(&mob_id).await.is_empty());

    let delayed = raw_deliver_member_input_command(
        &probe,
        &mob_id,
        &delayed_input,
        "complete after early ack",
        expected_member.clone(),
        Some(BridgeTurnDirective {
            correlation: BridgeTurnCorrelation {
                run_id: "paged-delayed-run".to_string(),
                step_id: "step-1".to_string(),
            },
            tool_overlay: None,
        }),
    );
    let _ = probe
        .send_bridge_command_raw(&member, &delayed, FIXTURE_BRIDGE_TIMEOUT)
        .await
        .expect("delayed directed delivery reply");
    wait_until("delayed outcome survives early ack", || async {
        fixture
            .turn_outcome_rows(&mob_id)
            .await
            .iter()
            .any(|row| row.input_id == delayed_input)
    })
    .await;
    let delayed_page = probe
        .send_bridge_command_raw(&member, &poll(Vec::new()), FIXTURE_BRIDGE_TIMEOUT)
        .await
        .expect("delayed outcome page");
    assert!(matches!(
        delayed_page,
        BridgeReply::MemberEventsPage(page)
            if page.turn_outcomes.iter().any(|row| row.input_id == delayed_input)
    ));

    fixture.shutdown().await;
}

// ==========================================================================
// T-F5b — the executor's ADJ-4 single resend: byte-identical command at the
// SAME input_id, only on a dropped reply
// ==========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn directive_send_resends_once_at_same_input_id_on_dropped_reply() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let scripted_host = spawn_scripted_host_peer("xhf-t5b-host").await;
    let member_endpoint =
        std::sync::Arc::new(spawn_peer_comms_endpoint("xhf-t5b-member", true, None).await);
    let responder = spawn_scripted_member_turn_responder(std::sync::Arc::clone(&member_endpoint));
    scripted_host.script_member_identity("b5", support::member_identity_of(&member_endpoint));
    scripted_host.bind_member_endpoint("b5", std::sync::Arc::clone(&member_endpoint));

    let controlling = create_controlling_mob_with_flows(
        "xhf-t5b",
        vec![(
            FlowId::from("resend-step"),
            single_step_flow_with_timeout("worker", "go", 3_000),
        )],
    )
    .await;
    controlling.bind_scripted(&scripted_host).await;
    let host_id = scripted_host
        .endpoint
        .runtime
        .public_key()
        .to_peer_id()
        .to_string();
    controlling
        .spawn_placed("worker", "b5", &host_id)
        .await
        .expect("b5 materializes on the scripted host");

    // First reply dropped: the delivery is served + recorded, the reply is
    // lost — the ADJ-4 transient class.
    responder.drop_next_deliver_replies(1);

    let run_id = controlling
        .handle
        .run_flow(FlowId::from("resend-step"), serde_json::json!({}))
        .await
        .expect("run resend flow");
    let run = wait_for_flow_run_terminal(&controlling.handle, &run_id, RUN_WAIT).await;

    // No real turn runs behind the scripted member, so the step resolves
    // through the timeout ladder — typed, never a phantom completion.
    assert_eq!(
        run.status,
        MobRunStatus::Failed,
        "scripted member serves no terminal; the step times out typed"
    );

    // Exactly TWO delivery attempts, byte-identical, same input_id.
    let received = responder.received_deliveries();
    assert_eq!(
        received.len(),
        2,
        "exactly one resend after the dropped reply: {} deliveries; failure ledger: {:?}",
        received.len(),
        run.failure_ledger,
    );
    assert_eq!(
        received[0], received[1],
        "the resend is byte-identical (same input_id, same directive)"
    );
    assert!(
        received[0].turn.is_some(),
        "the delivery carries the turn directive"
    );

    let obligations = controlling.handle.remote_turn_obligations_observation();
    assert_eq!(
        obligations.len(),
        1,
        "a timeout without terminal or certified no-effect keeps durable custody"
    );
    assert_eq!(
        obligations[0].1, received[0].input_id,
        "custody remains keyed to the exact byte-identical resend UUID"
    );

    responder.shutdown();
    scripted_host.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn placed_submit_work_ingress_admission_carries_exact_incarnation() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let scripted_host = spawn_scripted_host_peer("xhf-submit-admission-host").await;
    let member_endpoint = std::sync::Arc::new(
        spawn_peer_comms_endpoint("xhf-submit-admission-member", true, None).await,
    );
    let responder = spawn_scripted_member_turn_responder(std::sync::Arc::clone(&member_endpoint));
    scripted_host.script_member_identity(
        "admitted-worker",
        support::member_identity_of(&member_endpoint),
    );
    scripted_host.bind_member_endpoint("admitted-worker", std::sync::Arc::clone(&member_endpoint));

    let controlling = create_controlling_mob("xhf-submit-admission").await;
    controlling.bind_scripted(&scripted_host).await;
    let host_id = scripted_host
        .endpoint
        .runtime
        .public_key()
        .to_peer_id()
        .to_string();
    controlling
        .spawn_placed("worker", "admitted-worker", &host_id)
        .await
        .expect("placed worker materializes on scripted host");
    let roster_entry = controlling
        .handle
        .get_member(&identity("admitted-worker"))
        .await
        .expect("get placed member")
        .expect("placed member exists");
    let session_id = roster_entry
        .bridge_session_id()
        .expect("placed member carries remote session")
        .to_string();

    controlling
        .handle
        .member(&identity("admitted-worker"))
        .await
        .expect("placed member handle")
        .send(
            "admit with exact incarnation",
            meerkat_core::HandlingMode::Queue,
        )
        .await
        .expect("placed ingress admission succeeds");

    let deliveries = responder.received_deliveries();
    assert_eq!(deliveries.len(), 1, "one admission delivery");
    uuid::Uuid::parse_str(&deliveries[0].input_id)
        .expect("placed admission transport id is a canonical UUID");
    assert!(
        deliveries[0].transcript_interaction_id.is_none(),
        "absent caller transcript identity stays absent and independent from transport dedup"
    );
    let expected = deliveries[0]
        .expected_member
        .as_ref()
        .expect("placed ingress admission carries exact incarnation");
    assert_eq!(expected.mob_id, controlling.mob_id.to_string());
    assert_eq!(expected.agent_identity, "admitted-worker");
    assert_eq!(expected.host_id, host_id);
    assert_eq!(expected.member_session_id, session_id);
    assert_eq!(
        expected.generation,
        roster_entry.agent_runtime_id.generation.get()
    );
    assert_eq!(expected.fence_token, roster_entry.fence_token.get());

    responder.shutdown();
    scripted_host.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn placed_submit_work_completion_survives_two_lost_replies_after_acceptance() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let scripted_host = spawn_scripted_host_peer("xhf-submit-completion-host").await;
    let member_endpoint = std::sync::Arc::new(
        spawn_peer_comms_endpoint("xhf-submit-completion-member", true, None).await,
    );
    let responder = spawn_scripted_member_turn_responder(std::sync::Arc::clone(&member_endpoint));
    responder.complete_plain_deliveries_via_event_pump();
    scripted_host.script_member_identity(
        "completed-worker",
        support::member_identity_of(&member_endpoint),
    );
    scripted_host.bind_member_endpoint("completed-worker", std::sync::Arc::clone(&member_endpoint));

    let controlling = create_controlling_mob("xhf-submit-completion").await;
    controlling.bind_scripted(&scripted_host).await;
    let host_id = scripted_host
        .endpoint
        .runtime
        .public_key()
        .to_peer_id()
        .to_string();
    controlling
        .spawn_placed("worker", "completed-worker", &host_id)
        .await
        .expect("placed worker materializes on scripted host");
    let roster_entry = controlling
        .handle
        .get_member(&identity("completed-worker"))
        .await
        .expect("get placed member")
        .expect("placed member exists");
    let session_id = roster_entry
        .bridge_session_id()
        .expect("placed member carries remote session")
        .to_string();
    responder.set_expected_incarnation(
        roster_entry.agent_runtime_id.generation.get(),
        roster_entry.fence_token.get(),
    );

    // Both the Accepted response and the one byte-identical resend's
    // Deduplicated response are lost. The transport error is not no-effect
    // evidence: the already-registered exact waiter must still converge from
    // the durable interaction sidecar produced by the first acceptance.
    responder.drop_next_deliver_replies(2);
    let receipt = tokio::time::timeout(
        Duration::from_secs(20),
        controlling
            .handle
            .member(&identity("completed-worker"))
            .await
            .expect("member handle")
            .internal_turn("complete through the remote pump"),
    )
    .await
    .expect("placed completion stays bounded")
    .expect("two lost replies still converge from terminal completion");
    assert_eq!(receipt.identity.as_str(), "completed-worker");

    let deliveries = responder.received_deliveries();
    assert_eq!(
        deliveries.len(),
        2,
        "two lost replies still cause only the one bounded resend"
    );
    assert_eq!(deliveries[0], deliveries[1], "resend is byte-identical");
    assert!(
        deliveries[0].turn.is_none(),
        "SubmitWork is an ordinary delivery"
    );
    uuid::Uuid::parse_str(&deliveries[0].input_id)
        .expect("production SubmitWork payload correlation is a UUID");
    assert_eq!(
        deliveries[0].transcript_interaction_id.as_deref(),
        Some(deliveries[0].input_id.as_str()),
        "tracked completion transcript identity must equal its sidecar/waiter key"
    );
    let expected = deliveries[0]
        .expected_member
        .as_ref()
        .expect("placed ordinary delivery carries exact incarnation");
    assert_eq!(expected.mob_id, controlling.mob_id.to_string());
    assert_eq!(expected.agent_identity, "completed-worker");
    assert_eq!(expected.host_id, host_id);
    assert_eq!(expected.member_session_id, session_id);
    assert_eq!(
        expected.generation,
        roster_entry.agent_runtime_id.generation.get()
    );
    assert_eq!(expected.fence_token, roster_entry.fence_token.get());

    responder.shutdown();
    scripted_host.shutdown();
}

// ==========================================================================
// T-F6 — generation bump mid-step fails the step typed (before the step
// timeout elapses)
// ==========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn generation_bump_mid_step_fails_step_typed() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let gate = StallGate::new();
    let mut opts = HostFixtureOptions::named("xhf-t6-host-b").with_member_build();
    opts.member_llm_client = Some(scripted_member_client_stalling(gate.clone()));
    let fixture = spawn_host_daemon_fixture(opts)
        .await
        .expect("spawn member-build host fixture");
    let controlling = create_controlling_mob_with_flows(
        "xhf-t6",
        vec![(
            FlowId::from("gen-bump-step"),
            // Step timeout FAR above the wait bound: the typed generation
            // failure must beat it (fail-fast, not timeout-laundered).
            single_step_flow_with_timeout("worker", "go", 120_000),
        )],
    )
    .await;
    let report = controlling.bind_fixture(&fixture).await;

    controlling
        .spawn_placed("worker", "b6", &report.host_id)
        .await
        .expect("b6 materializes on host B");

    let run_id = controlling
        .handle
        .run_flow(FlowId::from("gen-bump-step"), serde_json::json!({}))
        .await
        .expect("run gen-bump flow");

    // Wait until the directed turn is in flight (obligation recorded), then
    // respawn the placed member (ADJ-24 re-materialization, generation bump).
    wait_until("remote turn obligation recorded", || async {
        !controlling
            .handle
            .remote_turn_obligations_observation()
            .is_empty()
    })
    .await;
    controlling
        .handle
        .respawn(identity("b6"), None)
        .await
        .expect("respawn bumps the placed member's generation");

    let run = wait_for_flow_run_terminal(&controlling.handle, &run_id, RUN_WAIT).await;
    assert_eq!(
        run.status,
        MobRunStatus::Failed,
        "generation bump mid-step fails the run typed"
    );
    assert!(
        run.failure_ledger
            .iter()
            .any(|entry| entry.reason.contains("generation")),
        "failure names the generation change: {:?}",
        run.failure_ledger
    );

    // Obligation resolved on the typed-failure path; no cross-generation
    // attribution ever happened (the step failed, it did not complete).
    wait_for_obligations_resolved(&controlling).await;
    gate.release();
    fixture.shutdown().await;
}

// ==========================================================================
// T-F7 — pre-V4 decode reject is a typed step failure (no member turn ran)
// ==========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn pre_v4_decode_reject_is_typed_step_failure() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let scripted_host = spawn_scripted_host_peer("xhf-t7-host").await;
    let member_endpoint =
        std::sync::Arc::new(spawn_peer_comms_endpoint("xhf-t7-member", true, None).await);
    let responder = spawn_scripted_member_turn_responder(std::sync::Arc::clone(&member_endpoint));
    scripted_host.script_member_identity("b7", support::member_identity_of(&member_endpoint));
    scripted_host.bind_member_endpoint("b7", std::sync::Arc::clone(&member_endpoint));

    let controlling = create_controlling_mob_with_flows(
        "xhf-t7",
        vec![(
            FlowId::from("decode-step"),
            single_step_flow("worker", "go"),
        )],
    )
    .await;
    controlling.bind_scripted(&scripted_host).await;
    let host_id = scripted_host
        .endpoint
        .runtime
        .public_key()
        .to_peer_id()
        .to_string();
    controlling
        .spawn_placed("worker", "b7", &host_id)
        .await
        .expect("b7 materializes on the scripted host");

    // The pre-V4 receiver shape: a directive-bearing payload fails the
    // member's deny_unknown_fields decode → typed rejection reply (never an
    // undirected turn).
    responder.reject_next_deliver_decode();

    let run_id = controlling
        .handle
        .run_flow(FlowId::from("decode-step"), serde_json::json!({}))
        .await
        .expect("run decode flow");
    let run = wait_for_flow_run_terminal(&controlling.handle, &run_id, RUN_WAIT).await;
    assert_eq!(
        run.status,
        MobRunStatus::Failed,
        "decode reject fails the step typed at dispatch"
    );
    assert!(
        run.failure_ledger
            .iter()
            .any(|entry| entry.reason.contains("turn_directive_unsupported")),
        "run ledger preserves the pre-V4 semantic unsupported cause: {:?}",
        run.failure_ledger
    );

    // The decode reject fired at admission: no turn ran (the responder
    // records rejected deliveries separately from admitted ones).
    assert_eq!(
        responder.admitted_delivery_count(),
        0,
        "no member turn was admitted"
    );
    assert_eq!(
        responder.received_deliveries().len(),
        1,
        "a terminal command-level decode rejection is not resent"
    );

    wait_for_obligations_resolved(&controlling).await;
    responder.shutdown();
    scripted_host.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
#[cfg(feature = "test-support")]
async fn recovered_pre_v4_decode_reject_closes_same_id_pending_without_a_turn() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let scripted_host = spawn_scripted_host_peer("xhf-t7-recovered-host").await;
    let member_endpoint =
        std::sync::Arc::new(spawn_peer_comms_endpoint("xhf-t7-recovered-member", true, None).await);
    let responder = spawn_scripted_member_turn_responder(std::sync::Arc::clone(&member_endpoint));
    scripted_host.script_member_identity(
        "b7-recovered",
        support::member_identity_of(&member_endpoint),
    );
    scripted_host.bind_member_endpoint("b7-recovered", std::sync::Arc::clone(&member_endpoint));

    let controlling = create_controlling_mob_with_flows(
        "xhf-t7-recovered",
        vec![(
            FlowId::from("decode-recovery-step"),
            single_step_flow_with_timeout("worker", "go", 120_000),
        )],
    )
    .await;
    controlling.bind_scripted(&scripted_host).await;
    let host_id = scripted_host
        .endpoint
        .runtime
        .public_key()
        .to_peer_id()
        .to_string();
    controlling
        .spawn_placed("worker", "b7-recovered", &host_id)
        .await
        .expect("recovery worker materializes on the scripted host");

    // Original send + ADJ-4 resend both time out before admission. The
    // controller therefore retains Pending custody, while this fixture can
    // prove that neither attempt ran a member turn.
    responder.silence_next_deliveries_before_admission(2);
    let run_id = controlling
        .handle
        .run_flow(FlowId::from("decode-recovery-step"), serde_json::json!({}))
        .await
        .expect("start flow whose remote input remains ambiguous");
    wait_until("both no-admission delivery attempts", || async {
        responder.received_deliveries().len() >= 2
    })
    .await;
    let before_restart = responder.received_deliveries();
    assert_eq!(
        before_restart.len(),
        2,
        "one original plus one exact resend"
    );
    assert_eq!(before_restart[0], before_restart[1]);
    assert_eq!(responder.admitted_delivery_count(), 0);
    assert_eq!(
        controlling
            .handle
            .remote_turn_obligations_observation()
            .len(),
        1,
        "ambiguous no-admission timeouts retain exact Pending custody"
    );
    let durable_input_id = before_restart[0].input_id.clone();
    let durable_intents = controlling
        .storage_runs
        .list_remote_turn_intents(&run_id)
        .await
        .expect("read replay-complete remote-turn intent before restart");
    assert_eq!(
        durable_intents.len(),
        1,
        "ambiguous Pending custody must retain one private replay intent"
    );
    assert_eq!(
        durable_intents[0].obligation.input_id, durable_input_id,
        "private replay intent and public delivery must share one idempotency key"
    );

    // The recovered reconciler replays the durable command at the SAME id.
    // A pre-V4 receiver rejects the directive at command decode, which is an
    // authenticated no-effect result and must close the recovered ticket.
    responder.reject_next_deliver_decode();
    let controlling = controlling.crash_restart().await;
    wait_until("recovered same-id decode rejection", || async {
        responder.received_deliveries().len() >= 3
    })
    .await;
    wait_for_obligations_resolved(&controlling).await;
    let recovered_run = wait_for_flow_run_terminal(&controlling.handle, &run_id, RUN_WAIT).await;
    assert_eq!(recovered_run.status, MobRunStatus::Failed);
    assert!(
        recovered_run
            .failure_ledger
            .iter()
            .any(|entry| entry.reason.contains("turn_directive_unsupported")),
        "recovered terminal preserves the same semantic unsupported cause: {:?}",
        recovered_run.failure_ledger
    );

    let deliveries = responder.received_deliveries();
    assert_eq!(deliveries.len(), 3, "the terminal replay is not resent");
    assert!(
        deliveries
            .iter()
            .all(|delivery| delivery.input_id == durable_input_id),
        "recovery reuses the exact durable idempotency key: {deliveries:?}"
    );
    assert_eq!(
        responder.admitted_delivery_count(),
        0,
        "neither the ambiguous setup nor recovered decode rejection ran a turn"
    );

    responder.shutdown();
    scripted_host.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn command_unavailable_resends_once_then_retains_ambiguous_obligation_without_a_turn() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let scripted_host = spawn_scripted_host_peer("xhf-t7-unavailable-host").await;
    let member_endpoint = std::sync::Arc::new(
        spawn_peer_comms_endpoint("xhf-t7-unavailable-member", true, None).await,
    );
    let responder = spawn_scripted_member_turn_responder(std::sync::Arc::clone(&member_endpoint));
    scripted_host.script_member_identity(
        "b7-unavailable",
        support::member_identity_of(&member_endpoint),
    );
    scripted_host.bind_member_endpoint("b7-unavailable", std::sync::Arc::clone(&member_endpoint));

    let controlling = create_controlling_mob_with_flows(
        "xhf-t7-unavailable",
        vec![(
            FlowId::from("unavailable-step"),
            single_step_flow_with_timeout("worker", "go", 30_000),
        )],
    )
    .await;
    controlling.bind_scripted(&scripted_host).await;
    let host_id = scripted_host
        .endpoint
        .runtime
        .public_key()
        .to_peer_id()
        .to_string();
    controlling
        .spawn_placed("worker", "b7-unavailable", &host_id)
        .await
        .expect("unavailable worker materializes on the scripted host");

    responder.reject_next_deliver_unavailable(2);
    let run_id = controlling
        .handle
        .run_flow(FlowId::from("unavailable-step"), serde_json::json!({}))
        .await
        .expect("start unavailable flow");
    let run = wait_for_flow_run_terminal(&controlling.handle, &run_id, RUN_WAIT).await;
    assert_eq!(run.status, MobRunStatus::Failed);
    let deliveries = responder.received_deliveries();
    assert_eq!(deliveries.len(), 2, "Unavailable gets exactly one resend");
    assert_eq!(deliveries[0], deliveries[1], "resend is byte-identical");
    assert_eq!(
        responder.admitted_delivery_count(),
        0,
        "both Unavailable replies reject before member input admission"
    );
    assert_eq!(
        controlling
            .handle
            .remote_turn_obligations_observation()
            .len(),
        1,
        "command-level Unavailable is retryable but is not certified no-effect; exact custody must remain discoverable for reconciliation"
    );

    responder.shutdown();
    scripted_host.shutdown();
}

// ==========================================================================
// T-F8 — unreachable host: typed failure after exactly one resend
// ==========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn directive_to_unreachable_host_fails_typed_after_single_resend() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let scripted_host = spawn_scripted_host_peer("xhf-t8-host").await;
    let member_endpoint =
        std::sync::Arc::new(spawn_peer_comms_endpoint("xhf-t8-member", true, None).await);
    let responder = spawn_scripted_member_turn_responder(std::sync::Arc::clone(&member_endpoint));
    scripted_host.script_member_identity("b8", support::member_identity_of(&member_endpoint));
    scripted_host.bind_member_endpoint("b8", std::sync::Arc::clone(&member_endpoint));

    let controlling = create_controlling_mob_with_flows(
        "xhf-t8",
        vec![(
            FlowId::from("unreachable-step"),
            single_step_flow_with_timeout("worker", "go", 3_000),
        )],
    )
    .await;
    controlling.bind_scripted(&scripted_host).await;
    let host_id = scripted_host
        .endpoint
        .runtime
        .public_key()
        .to_peer_id()
        .to_string();
    controlling
        .spawn_placed("worker", "b8", &host_id)
        .await
        .expect("b8 materializes on the scripted host");

    // Drop BOTH replies: attempt + single resend both time out — the wire
    // twin of an unreachable/partitioned member host.
    responder.drop_next_deliver_replies(2);

    let run_id = controlling
        .handle
        .run_flow(FlowId::from("unreachable-step"), serde_json::json!({}))
        .await
        .expect("run unreachable flow");
    let run = wait_for_flow_run_terminal(&controlling.handle, &run_id, RUN_WAIT).await;
    assert_eq!(
        run.status,
        MobRunStatus::Failed,
        "unreachable member host fails the step typed"
    );

    // Exactly two attempts: the original send + the single ADJ-4 resend.
    let received = responder.received_deliveries();
    assert_eq!(
        received.len(),
        2,
        "exactly one resend, then terminal typed failure"
    );
    assert_eq!(
        received[0], received[1],
        "partition retry is byte-identical"
    );

    let obligations = controlling.handle.remote_turn_obligations_observation();
    assert_eq!(
        obligations.len(),
        1,
        "two lost replies prove neither no-effect nor terminal; custody must survive"
    );
    assert_eq!(obligations[0].1, received[0].input_id);
    responder.shutdown();
    scripted_host.shutdown();
}

// ==========================================================================
// T-F9 — TurnDirectiveUnsupported at admission (fail-closed backstop on a
// runtime that cannot host tracked turns)
// ==========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn turn_directive_unsupported_rejected_at_admission() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    // Ephemeral host: no durable journal, so no tracked-turn registration —
    // the machine classifies such placements RejectedHostIncapable and never
    // dispatches; this row drives the RAW backstop directly.
    let fixture = spawn_host_daemon_fixture(HostFixtureOptions::named("xhf-t9-host-b").ephemeral())
        .await
        .expect("spawn ephemeral host fixture");

    let probe = spawn_peer_comms_endpoint("xhf-t9-supervisor", true, None).await;
    probe.trust(fixture.host_peer_descriptor()).await;
    let mob_id = format!("xhf-t9-{}", uuid::Uuid::new_v4().simple());
    let ack = bind_then_materialize(&probe, &fixture, &mob_id, "eph-member").await;
    let member = member_descriptor_from_ack("eph-member", &ack);
    probe.trust(member.clone()).await;

    let directive = BridgeTurnDirective {
        correlation: BridgeTurnCorrelation {
            run_id: "run-t9".to_string(),
            step_id: "step-1".to_string(),
        },
        tool_overlay: None,
    };
    let command = raw_deliver_member_input_command(
        &probe,
        &mob_id,
        &uuid::Uuid::new_v4().to_string(),
        "directed",
        member_incarnation_from_ack(&fixture, &mob_id, "eph-member", &ack, 1, 1, 1),
        Some(directive),
    );
    let reply = probe
        .send_bridge_command_raw(&member, &command, FIXTURE_BRIDGE_TIMEOUT)
        .await
        .expect("directive-bearing delivery reply");
    match reply {
        BridgeReply::Delivery(response) => match response.outcome {
            BridgeDeliveryOutcome::Rejected { cause, .. } => {
                assert!(
                    matches!(
                        cause,
                        BridgeDeliveryRejectionCause::TurnDirectiveUnsupported { .. }
                    ),
                    "admission reject cause is TurnDirectiveUnsupported: {cause:?}"
                );
            }
            other => panic!("directive-bearing delivery must be rejected, got {other:?}"),
        },
        other => panic!("expected delivery reply, got {other:?}"),
    }

    // No turn ran, no journal row.
    assert!(
        fixture.turn_outcome_rows(&mob_id).await.is_empty(),
        "no journal row for the rejected directive"
    );

    fixture.shutdown().await;
}

// ==========================================================================
// T-F10 — flow completion requires NO console subscription (A17 pin, kept
// separate from T-F1 so regressions cannot mask it)
// ==========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn flow_completion_requires_no_console_subscription() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let mut opts = HostFixtureOptions::named("xhf-t10-host-b").with_member_build();
    opts.member_llm_client = Some(scripted_member_client_completing("standalone-done"));
    let fixture = spawn_host_daemon_fixture(opts)
        .await
        .expect("spawn member-build host fixture");
    let controlling = create_controlling_mob_with_flows(
        "xhf-t10",
        vec![(
            FlowId::from("standalone-step"),
            single_step_flow("worker", "go"),
        )],
    )
    .await;
    let report = controlling.bind_fixture(&fixture).await;

    controlling
        .spawn_placed("worker", "b10", &report.host_id)
        .await
        .expect("b10 materializes on host B");

    // NO subscriber exists before or during the run.
    let run_id = controlling
        .handle
        .run_flow(FlowId::from("standalone-step"), serde_json::json!({}))
        .await
        .expect("run standalone flow");
    let run = wait_for_flow_run_terminal(&controlling.handle, &run_id, RUN_WAIT).await;
    assert_eq!(
        run.status,
        MobRunStatus::Completed,
        "flow completes with zero subscriptions attached; failures={:?}",
        run.failure_ledger
    );
    wait_for_obligations_resolved(&controlling).await;

    // Subscribing only AFTER the terminal still serves the historical rows
    // (the durable log is the read authority; observation was never load-
    // bearing for completion).
    let stream = controlling
        .handle
        .subscribe_agent_events(&identity("b10"))
        .await;
    assert!(
        stream.is_ok(),
        "post-terminal subscription to the placed member is servable: {:?}",
        stream.err()
    );

    fixture.shutdown().await;
}

// ==========================================================================
// T-F11 — hard cancel ends a stalled remote turn; Cancel scope gates it
// ==========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn hard_cancel_member_ends_stalled_remote_turn() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let gate = StallGate::new();
    let mut opts = HostFixtureOptions::named("xhf-t11-host-b").with_member_build();
    opts.member_llm_client = Some(scripted_member_client_stalling(gate.clone()));
    let fixture = spawn_host_daemon_fixture(opts)
        .await
        .expect("spawn member-build host fixture");
    let controlling = support::create_controlling_mob("xhf-t11").await;
    let report = controlling.bind_fixture(&fixture).await;

    controlling
        .spawn_placed("worker", "b11", &report.host_id)
        .await
        .expect("b11 materializes on host B");

    // Merged-stream observation for the cancelled turn's terminal.
    let router = controlling
        .handle
        .subscribe_mob_events()
        .await
        .expect("mob-wide merged stream subscribes");

    // Start a stalled member turn through the ordinary member-send path
    // (delivery acks at admission; the turn itself parks on host B).
    let member = controlling
        .handle
        .member(&identity("b11"))
        .await
        .expect("member handle for b11");
    member
        .send("stall forever", meerkat_core::HandlingMode::Queue)
        .await
        .expect("stalled member turn is admitted");

    // Hard cancel is defined on the CURRENT run: converge on the stalled
    // run actually starting member-side before cancelling (the verb ack
    // otherwise races the run start and no-ops on an idle runtime). The
    // member runtime's control snapshot is the deterministic signal.
    let mob_id = controlling.mob_id.to_string();
    let member_session_text = fixture
        .host_binding_record(&mob_id)
        .await
        .materialized
        .get("b11")
        .expect("b11 materialized row")
        .session_id
        .clone();
    let member_session = meerkat_core::SessionId::parse(&member_session_text)
        .expect("materialized session id parses");
    let member_adapter = fixture
        .member_runtime_adapter
        .clone()
        .expect("member-build fixture exposes the runtime adapter");
    support::wait_until("stalled member run bound member-side", || async {
        member_adapter
            .meerkat_machine_archive_snapshot(&member_session)
            .await
            .is_some_and(|snapshot| snapshot.control.current_run_id.is_some())
    })
    .await;

    // A non-owner principal WITHOUT Cancel scope is denied typed at the gate.
    controlling
        .grant(
            "watcher-only",
            &[meerkat_mob::machines::mob_machine::ControlScope::List],
            None,
        )
        .await;
    let denied = controlling
        .handle_as("watcher-only")
        .hard_cancel_member(
            support::control_principal("watcher-only"),
            identity("b11"),
            "not allowed",
        )
        .await;
    match denied {
        Err(MobError::ScopeDenied(denial)) => {
            assert_eq!(
                denial.required,
                meerkat_mob::machines::mob_machine::ControlScope::Cancel,
                "hard cancel requires the Cancel scope"
            );
        }
        other => panic!("expected ScopeDenied for hard cancel, got {other:?}"),
    }

    // The owner hard-cancels the stalled remote turn: typed ack.
    controlling
        .handle
        .hard_cancel_member(
            meerkat_mob::MobControlPrincipal::Owner,
            identity("b11"),
            "test hard cancel",
        )
        .await
        .expect("owner hard cancel is acked by the member host");

    // ACK is terminal truth, not signal-delivery truth: the receiver
    // level-reasserts the exact run-scoped cancel and withholds this RPC reply
    // until that run is unbound/terminal. No caller retry loop is needed (and
    // a byte-identical transport retry after reply loss cannot target a newer
    // run because the request carries the original expected_run_id).
    let after_ack = member_adapter
        .meerkat_machine_archive_snapshot(&member_session)
        .await
        .expect("member runtime snapshot exists after hard-cancel ACK");
    assert_eq!(
        after_ack.control.current_run_id, None,
        "hard-cancel ACK must not precede exact-run terminal/unbound truth"
    );

    drop(router);
    gate.release();
    fixture.shutdown().await;
}
