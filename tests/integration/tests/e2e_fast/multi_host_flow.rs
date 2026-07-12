//! Multi-host mobs phase 6 — deterministic e2e-fast ratchet row (the
//! §18.11:1043 flow beat; design-flow-spine FLAG-P6F-9 / ADJ-P6-13).
//!
//! ONE condensed cross-host flow walk the GitHub-CI lane actually runs:
//! bind Host B (member-build fixture, scripted completing client) → spawn a
//! placed TurnDriven worker → run a single-step flow targeting it → the
//! step completes via the member-side tracked turn + durable journal + pump
//! sidecar (NO console subscription — the A17 pin) → the host journal holds
//! exactly one RunCompleted row → the obligation set drains to rest.
//!
//! Deterministic: scripted member client, loopback port-0 listeners, no
//! live providers. The full failure matrix lives in
//! meerkat-mob/tests/cross_host_flows.rs (int lane).

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::support;

use std::time::Duration;

use meerkat_mob::{FlowId, MobRunStatus, StepId};
use support::{
    HostFixtureOptions, REAL_COMMS_TEST_LOCK, create_controlling_mob_with_flows,
    scripted_member_client_completing, single_step_flow, spawn_host_daemon_fixture,
    wait_for_flow_run_terminal, wait_until,
};

const WAIT: Duration = Duration::from_secs(60);

#[tokio::test(flavor = "multi_thread")]
async fn multi_host_flow_step_completes_via_journal_and_sidecar() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;

    let fixture = spawn_host_daemon_fixture(HostFixtureOptions {
        member_llm_client: Some(scripted_member_client_completing("e2e-flow-done")),
        ..HostFixtureOptions::named("e2e-flow-host-b").with_member_build()
    })
    .await
    .expect("spawn member-build host fixture");
    let controlling = create_controlling_mob_with_flows(
        "e2e-multi-host-flow",
        vec![(
            FlowId::from("cross-host-step"),
            single_step_flow("worker", "run the remote step"),
        )],
    )
    .await;
    let report = controlling.bind_fixture(&fixture).await;
    let mob_id = controlling.mob_id.to_string();

    controlling
        .spawn_placed("worker", "flow-b1", &report.host_id)
        .await
        .expect("flow-b1 materializes on Host B");

    // NO console subscription anywhere in this walk: completion rides the
    // obligation-driven pump + journal sidecar (A17), never a watcher.
    let run_id = controlling
        .handle
        .run_flow(FlowId::from("cross-host-step"), serde_json::json!({}))
        .await
        .expect("cross-host flow starts");
    let run = wait_for_flow_run_terminal(&controlling.handle, &run_id, WAIT).await;
    assert_eq!(
        run.status,
        MobRunStatus::Completed,
        "the remote step completes via journal+sidecar; failures={:?}",
        run.failure_ledger
    );
    assert_eq!(
        run.root_step_outputs.get(&StepId::from("step-1")),
        Some(&serde_json::json!({ "flow-b1": "e2e-flow-done" })),
        "the step output is the member terminal's payload, keyed by member \
         (the local-parity collection shape)"
    );

    // The obligation window drained (recorded at dispatch, resolved at
    // sidecar consume).
    wait_until("remote turn obligations to drain", || async {
        controlling
            .handle
            .remote_turn_obligations_observation()
            .is_empty()
    })
    .await;

    // ACK processing may prune the durably recorded host row in the same pump
    // cycle that completes the controller run. Assert the converged contract,
    // not an unstable intermediate retention window.
    wait_until(
        "host turn-outcome journal to drain after controller ack",
        || async { fixture.turn_outcome_rows(&mob_id).await.is_empty() },
    )
    .await;

    fixture.shutdown().await;
}
