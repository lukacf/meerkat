//! A mob Stop that lands right after an autonomous member's turn reached the
//! model reports the member's run, never an error.
//!
//! Before #1500, Stop interrupted members through `interrupt_member`, which
//! treated a runtime that had just committed back to Attached (the turn
//! finishing) as a hard `Runtime not ready: attached` error. Stop now holds
//! run starts and cancels the exact current run; a run that ends between the
//! hold and the cancel is `RunEndedBeforeCancel`.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use meerkat_client::LlmRequest;
use meerkat_core::Message;
use meerkat_core::types::HandlingMode;
use meerkat_mob::{AgentIdentity, MemberStopRun, MobBackendKind, MobHandle, MobRuntimeMode};
use support::{CouncilFixture, ScriptedTurn, TurnGate, council_definition, identity, user_text};
use tokio::sync::watch;

const PROMPT: &str = "STOP-REPRO-HELLO";

/// Whether this model request carries the host's message. A host send is
/// delivered as an external-event system notice.
fn prompt_reached(request: &LlmRequest) -> bool {
    user_text(request).contains(PROMPT)
        || request.messages.iter().any(|message| match message {
            Message::SystemNotice(notice) => notice
                .body
                .as_deref()
                .is_some_and(|body| body.contains(PROMPT)),
            _ => false,
        })
}

/// An autonomous member of a runtime-backed mob, idle.
async fn autonomous_member(fixture: &CouncilFixture) -> MobHandle {
    let mob_id = fixture.source_mob_id();
    // Externally addressable, as a host's direct member send requires.
    let mut definition = council_definition(mob_id.as_str());
    for binding in definition.profiles.values_mut() {
        if let meerkat_mob::ProfileBinding::Inline(profile) = binding {
            profile.external_addressable = true;
        }
    }
    fixture
        .state
        .mob_create_definition(definition)
        .await
        .expect("create mob");
    fixture
        .state
        .mob_spawn(
            &mob_id,
            "participant".into(),
            identity("helper"),
            Some(MobRuntimeMode::AutonomousHost),
            Some(MobBackendKind::Session),
            None,
        )
        .await
        .expect("spawn the autonomous member");
    fixture.state.handle_for(&mob_id).await.expect("mob handle")
}

async fn send_prompt(handle: &MobHandle) {
    handle
        .member(&AgentIdentity::from("helper"))
        .await
        .expect("member handle")
        .send(PROMPT, HandlingMode::Queue)
        .await
        .expect("deliver the prompt");
}

fn assert_reported_run(report: &meerkat_mob::MobStopReport) {
    let outcome = report
        .members
        .get(&AgentIdentity::from("helper"))
        .unwrap_or_else(|| panic!("the stop reports the member: {report:?}"));
    assert!(
        matches!(
            outcome.run,
            MemberStopRun::CancelledAtBoundary { .. }
                | MemberStopRun::RunEndedBeforeCancel { .. }
                | MemberStopRun::NoRun
        ),
        "{report:?}"
    );
}

/// The reported scenario: the model answers at once, and Stop is called as
/// soon as the request reached it, so the run is finishing as Stop lands.
/// Repeated on fresh mobs, each Stop arriving one scheduler yield later, to
/// cover both sides of the window.
#[tokio::test(flavor = "multi_thread")]
async fn stop_right_after_the_turn_reaches_the_model_reports_the_run() {
    for attempt in 0..24 {
        let (reached_tx, mut reached_rx) = watch::channel(0_usize);
        let reached_tx = Arc::new(reached_tx);
        let fixture = CouncilFixture::new_runtime_backed(move |request| {
            if prompt_reached(request) {
                reached_tx.send_modify(|count| *count += 1);
            }
            ScriptedTurn::Text("ok".to_string())
        });
        let handle = autonomous_member(&fixture).await;
        send_prompt(&handle).await;
        tokio::time::timeout(
            Duration::from_secs(30),
            reached_rx.wait_for(|count| *count > 0),
        )
        .await
        .expect("the turn reaches the model")
        .expect("request watch open");
        // Walk Stop's arrival across the end of the turn, one scheduler
        // yield further each attempt.
        for _ in 0..attempt {
            tokio::task::yield_now().await;
        }
        let report = handle
            .stop()
            .await
            .unwrap_or_else(|error| panic!("attempt {attempt}: stop failed: {error}"));
        assert_reported_run(&report);
        fixture.teardown().await;
    }
}

/// The same with the model call held open: Stop lands while the run is in
/// flight and cancels it at its boundary.
#[tokio::test(flavor = "multi_thread")]
async fn stop_during_an_in_flight_model_call_cancels_the_run() {
    let gate = TurnGate::new();
    let script_gate = Arc::clone(&gate);
    let fixture = CouncilFixture::new_runtime_backed(move |request| {
        if prompt_reached(request) {
            ScriptedTurn::Gated(Arc::clone(&script_gate), "ok".to_string())
        } else {
            ScriptedTurn::Text("ok".to_string())
        }
    });
    let handle = autonomous_member(&fixture).await;
    send_prompt(&handle).await;
    gate.wait_entered(1).await;
    let stopping = {
        let handle = handle.clone();
        tokio::spawn(async move { handle.stop().await })
    };
    // Release the model call whether or not Stop's cancel reaches it first.
    gate.open();
    let report = tokio::time::timeout(Duration::from_secs(30), stopping)
        .await
        .expect("stop completes")
        .expect("join")
        .unwrap_or_else(|error| panic!("stop failed: {error}"));
    assert_reported_run(&report);
    fixture.teardown().await;
}
