//! #1471: a mob Stop that lands on an autonomous member's run after the
//! runtime staged it and signalled its turn start, but before the member's
//! agent claimed the turn, cancels that run. The turn ends at its first
//! boundary instead of running to completion, and the Stop report says what
//! happened to the run.
//!
//! This is the only window between staging and the agent's turn in which a
//! Stop can land: the runtime loop holds the session mutation gate from
//! staging until just before it hands the run to the executor, and a Stop's
//! hold and cancel take that same gate.
//!
//! The scripted model answers the member's first call with a tool call, so a
//! turn that is not cancelled runs the tool and calls the model again. A turn
//! cancelled at its first boundary stops after the first call.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use meerkat_client::LlmRequest;
use meerkat_core::Message;
use meerkat_core::types::HandlingMode;
use meerkat_mob::{AgentIdentity, MemberStopRun, MobBackendKind, MobHandle, MobRuntimeMode};
use support::{CouncilFixture, ScriptedTurn, council_definition, identity, user_text};
use tokio::sync::watch;

const PROMPT: &str = "STAGED-RUN-STOP";

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

/// An autonomous, externally addressable member of a runtime-backed mob.
async fn autonomous_member(fixture: &CouncilFixture) -> MobHandle {
    let mob_id = fixture.source_mob_id();
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

#[tokio::test(flavor = "multi_thread")]
async fn stop_cancels_a_run_staged_before_the_agent_claimed_its_turn() {
    let (calls_tx, calls_rx) = watch::channel(0_usize);
    let calls_tx = Arc::new(calls_tx);
    let fixture = CouncilFixture::new_runtime_backed(move |request| {
        if !prompt_reached(request) {
            return ScriptedTurn::Text("ok".to_string());
        }
        let mut first = false;
        calls_tx.send_modify(|calls| {
            *calls += 1;
            first = *calls == 1;
        });
        if first {
            ScriptedTurn::ToolCall {
                id: "staged-run-call".to_string(),
                name: "no_such_tool".to_string(),
                args: serde_json::json!({}),
            }
        } else {
            ScriptedTurn::Text("done".to_string())
        }
    });
    let handle = autonomous_member(&fixture).await;
    let helper = AgentIdentity::from("helper");
    let session_id = handle
        .get_member(&helper)
        .await
        .expect("read the member")
        .and_then(|entry| entry.bridge_session_id().cloned())
        .expect("session-backed member");
    let adapter = fixture
        .runtime_adapter
        .clone()
        .expect("runtime-backed fixture");

    // The member's own kickoff turn runs first; the gate below is for the
    // prompt's run, so let the kickoff settle before arming it.
    handle
        .wait_for_kickoff_complete(Some(Duration::from_secs(30)))
        .await
        .expect("the member's kickoff settles");

    // Park the member's runtime loop after it staged the prompt's run and
    // signalled its turn start, before the agent is handed the turn.
    let (staged, release) = adapter.arm_runtime_loop_before_executor_apply_test_hook(session_id);
    let mut dispatches = adapter.boundary_cancel_dispatches();
    handle
        .member(&helper)
        .await
        .expect("member handle")
        .send(PROMPT, HandlingMode::Queue)
        .await
        .expect("deliver the prompt");
    // Hang guards only: each wait is on a positive event.
    tokio::time::timeout(Duration::from_secs(30), staged)
        .await
        .expect("the member's run is staged")
        .expect("runtime-loop hook armed");

    // The Stop lands in that window: its cancel targets the staged run.
    let stopping = {
        let handle = handle.clone();
        tokio::spawn(async move { handle.stop().await })
    };
    tokio::time::timeout(
        Duration::from_secs(30),
        dispatches.wait_for(|dispatches| *dispatches >= 1),
    )
    .await
    .expect("the Stop's boundary cancel is dispatched")
    .expect("dispatch watch open");
    release.send(()).expect("release the staged run");

    let report = tokio::time::timeout(Duration::from_secs(60), stopping)
        .await
        .expect("the stop completes")
        .expect("join")
        .unwrap_or_else(|error| panic!("stop failed: {error}"));
    let outcome = report
        .members
        .get(&helper)
        .unwrap_or_else(|| panic!("the stop reports the member: {report:?}"));
    assert!(
        matches!(outcome.run, MemberStopRun::CancelledAtBoundary { .. }),
        "the stop cancelled the staged run at its boundary: {report:?}"
    );
    assert_eq!(
        *calls_rx.borrow(),
        1,
        "the cancelled turn ends at its first boundary: the tool call never \
         runs and the model is not called again"
    );
    fixture.teardown().await;
}
