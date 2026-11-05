//! The runner end to end, headless: effects in, UI messages out, through the
//! real runtime-backed service (`PersistentSessionService` + `MeerkatMachine`)
//! with a scripted LLM. Covers a turn, a queued turn, a steered message
//! applied at a tool boundary, interrupt, resume by id and `last`, `-c`
//! across process lifetimes, and a renamed workspace.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::sync::Arc;

use common::{Rig, ScriptedLlmClient, Step, Workspace, copy_tree, user_texts};
use meerkat_core::{AgentEvent, Message};
use meerkat_tui::delivery::PendingId;
use meerkat_tui::runner::Start;
use meerkat_tui::session::resolve_session_ref;
use meerkat_tui::ui::{Block, Effect, Msg, StatusUpdate};
use serde_json::json;
use tokio::sync::Notify;

fn steer(id: u64, text: &str) -> Effect {
    Effect::Steer {
        id: PendingId(id),
        text: text.into(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn creates_a_session_and_completes_a_turn() {
    let ws = Workspace::new();
    let llm = ScriptedLlmClient::new(vec![Step::Text("hello there")]);
    let mut rig = Rig::start(&ws.root, &llm, Start::New).await;
    // The workspace-local realm lives inside the workspace.
    assert!(ws.root.join(".rkat/tui/.gitignore").is_file());

    rig.send(Effect::StartTurn("hi".into()));
    let outcome = rig.turn_finished().await;
    assert_eq!(outcome.result.expect("turn succeeds").text, "hello there");
    assert!(outcome.undelivered.is_empty());

    let session = rig.session(&rig.first_session).await;
    assert_eq!(user_texts(session.messages()), vec!["hi".to_string()]);
    // The persona reached the model.
    let first = &llm.requests()[0];
    assert!(first.iter().any(|m| matches!(
        m,
        Message::System(system) if system.content.contains("interactive coding agent")
    )));
    assert!(rig.quit().await.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_prompt_queued_behind_a_running_turn_runs_next() {
    let ws = Workspace::new();
    let gate = Arc::new(Notify::new());
    let llm = ScriptedLlmClient::new(vec![
        Step::Gated(Arc::clone(&gate), Box::new(Step::Text("first answer"))),
        Step::Text("second answer"),
    ]);
    let mut rig = Rig::start(&ws.root, &llm, Start::New).await;
    rig.send(Effect::StartTurn("one".into()));
    llm.wait_call(0).await;
    // A second prompt while the first runs: a `Queue` input behind it.
    rig.send(Effect::StartTurn("two".into()));
    gate.notify_one();

    let outcome = rig.turn_finished().await;
    assert_eq!(outcome.result.expect("turn succeeds").text, "second answer");
    let requests = llm.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        user_texts(&requests[1]),
        vec!["one".to_string(), "two".to_string()]
    );
    let session = rig.session(&rig.first_session).await;
    assert_eq!(
        user_texts(session.messages()),
        vec!["one".to_string(), "two".to_string()]
    );
    rig.quit().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_steered_message_is_applied_at_the_next_tool_boundary() {
    let ws = Workspace::new();
    std::fs::write(ws.root.join("notes.txt"), "the answer is 42\n").unwrap();
    let gate = Arc::new(Notify::new());
    let llm = ScriptedLlmClient::new(vec![
        Step::Gated(
            Arc::clone(&gate),
            Box::new(Step::Tool {
                id: "call-1",
                name: "read_file",
                args: json!({ "path": "notes.txt" }),
            }),
        ),
        Step::Text("done"),
    ]);
    let mut rig = Rig::start(&ws.root, &llm, Start::New).await;
    rig.send(Effect::StartTurn("read the notes".into()));
    llm.wait_call(0).await;
    // Typed while the model streams: steered into the running turn.
    rig.send(steer(1, "also mention the file name"));
    let ack = rig
        .wait_for(|msg| matches!(msg, Msg::SteerAck { .. }))
        .await;
    assert!(
        matches!(
            ack,
            Msg::SteerAck {
                id: PendingId(1),
                result: Ok(())
            }
        ),
        "{ack:?}"
    );
    gate.notify_one();

    rig.wait_for(|msg| matches!(msg, Msg::SteerDelivered(PendingId(1))))
        .await;
    let outcome = rig.turn_finished().await;
    assert_eq!(outcome.result.expect("turn succeeds").text, "done");
    assert!(outcome.undelivered.is_empty());

    // One run: the steer joined it at the tool boundary, after the tool
    // result (the file tool ran end to end) and before the next request.
    let requests = llm.requests();
    assert_eq!(requests.len(), 2, "no follow-up turn");
    let second = &requests[1];
    let tool_result_at = second
        .iter()
        .position(|m| {
            matches!(m, Message::ToolResults { results, .. }
            if results.iter().any(|r| r.text_content().contains("the answer is 42")))
        })
        .expect("the read_file result reached the model");
    let steer_at = second
        .iter()
        .position(
            |m| matches!(m, Message::User(u) if u.text_content() == "also mention the file name"),
        )
        .expect("the steered message reached the model");
    assert!(steer_at > tool_result_at);
    assert!(
        rig.events
            .lock()
            .iter()
            .any(|event| matches!(event, AgentEvent::BoundaryAppendApplied { .. }))
    );
    let session = rig.session(&rig.first_session).await;
    assert_eq!(
        user_texts(session.messages()),
        vec![
            "read the notes".to_string(),
            "also mention the file name".to_string()
        ]
    );
    rig.quit().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn interrupt_cancels_the_running_turn_and_returns_steers() {
    let ws = Workspace::new();
    // The first response never arrives.
    let never = Arc::new(Notify::new());
    let llm = ScriptedLlmClient::new(vec![
        Step::Gated(never, Box::new(Step::Text("never"))),
        Step::Text("after the interrupt"),
    ]);
    let mut rig = Rig::start(&ws.root, &llm, Start::New).await;
    rig.send(Effect::StartTurn("long task".into()));
    llm.wait_call(0).await;
    rig.send(steer(1, "and this"));
    rig.wait_for(|msg| matches!(msg, Msg::SteerAck { .. }))
        .await;
    rig.send(Effect::Interrupt);

    let outcome = rig.turn_finished().await;
    assert_eq!(outcome.result, Err("interrupted".to_string()));
    assert_eq!(outcome.undelivered, vec![PendingId(1)]);

    // The session stays usable, and the cancelled steer never runs.
    rig.send(Effect::StartTurn("next".into()));
    let outcome = rig.turn_finished().await;
    assert_eq!(
        outcome.result.expect("turn succeeds").text,
        "after the interrupt"
    );
    assert!(
        !user_texts(llm.requests().last().unwrap()).contains(&"and this".to_string()),
        "a cancelled steer must not reach the model"
    );
    rig.quit().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn new_and_resume_switch_sessions_in_process() {
    let ws = Workspace::new();
    let llm = ScriptedLlmClient::new(vec![
        Step::Text("first"),
        Step::Text("second"),
        Step::Text("third"),
    ]);
    let mut rig = Rig::start(&ws.root, &llm, Start::New).await;
    let first = rig.first_session.clone();
    rig.send(Effect::StartTurn("in the first session".into()));
    rig.turn_finished().await;

    rig.send(Effect::NewSession);
    let second = match rig
        .wait_for(|msg| matches!(msg, Msg::Status(StatusUpdate::Session(_))))
        .await
    {
        Msg::Status(StatusUpdate::Session(id)) => id,
        other => panic!("{other:?}"),
    };
    assert_ne!(second, first.to_string());
    rig.send(Effect::StartTurn("in the second session".into()));
    rig.turn_finished().await;
    assert_eq!(
        user_texts(llm.requests().last().unwrap()),
        vec!["in the second session".to_string()]
    );

    // Resume the first by id: the transcript comes back, and the next turn
    // continues its history.
    rig.send(Effect::ResumeSession(first.to_string()));
    let blocks = match rig
        .wait_for(|msg| matches!(msg, Msg::ResetTranscript(_)))
        .await
    {
        Msg::ResetTranscript(blocks) => blocks,
        other => panic!("{other:?}"),
    };
    assert!(blocks.contains(&Block::User {
        text: "in the first session".into()
    }));
    rig.send(Effect::StartTurn("back again".into()));
    rig.turn_finished().await;
    assert_eq!(
        user_texts(llm.requests().last().unwrap()),
        vec!["in the first session".to_string(), "back again".to_string()]
    );
    // `last` is the session updated most recently.
    let last = resolve_session_ref(&*rig.store, "last").await.unwrap();
    assert_eq!(last, first);
    rig.quit().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn continue_and_resume_by_id_across_lifetimes() {
    let ws = Workspace::new();
    let llm = ScriptedLlmClient::new(vec![Step::Text("remembered")]);
    let mut rig = Rig::start(&ws.root, &llm, Start::New).await;
    let first = rig.first_session.clone();
    rig.send(Effect::StartTurn("remember me".into()));
    rig.turn_finished().await;
    rig.quit().await;

    // `-c`: the workspace's most recent session, with its transcript.
    let llm = ScriptedLlmClient::new(vec![Step::Text("continued")]);
    let mut rig = Rig::start(&ws.root, &llm, Start::Resume("last".into())).await;
    assert_eq!(rig.first_session, first);
    let blocks = match rig
        .wait_for(|msg| matches!(msg, Msg::ResetTranscript(_)))
        .await
    {
        Msg::ResetTranscript(blocks) => blocks,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        blocks,
        vec![
            Block::User {
                text: "remember me".into()
            },
            Block::Assistant {
                md: "remembered".into()
            },
        ]
    );
    rig.send(Effect::StartTurn("still there?".into()));
    rig.turn_finished().await;
    assert_eq!(
        user_texts(&llm.requests()[0]),
        vec!["remember me".to_string(), "still there?".to_string()]
    );
    rig.quit().await;

    // `--resume <id prefix>`.
    let prefix = first.to_string()[..8].to_string();
    let llm = ScriptedLlmClient::new(vec![]);
    let rig = Rig::start(&ws.root, &llm, Start::Resume(prefix)).await;
    assert_eq!(rig.first_session, first);
    rig.quit().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn workspace_state_survives_a_rename() {
    let ws = Workspace::new();
    let llm = ScriptedLlmClient::new(vec![Step::Text("ok")]);
    let mut rig = Rig::start(&ws.root, &llm, Start::New).await;
    let first = rig.first_session.clone();
    rig.send(Effect::StartTurn("before the move".into()));
    rig.turn_finished().await;
    rig.quit().await;

    let moved = ws.sibling("renamed-project");
    copy_tree(&ws.root, &moved);
    std::fs::remove_dir_all(&ws.root).unwrap();

    let llm = ScriptedLlmClient::new(vec![]);
    let rig = Rig::start(&moved, &llm, Start::Resume("last".into())).await;
    assert_eq!(rig.first_session, first);
    let session = rig.session(&first).await;
    assert_eq!(
        user_texts(session.messages()),
        vec!["before the move".to_string()]
    );
    rig.quit().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_steer_without_a_running_turn_is_refused() {
    let ws = Workspace::new();
    let llm = ScriptedLlmClient::new(vec![]);
    let mut rig = Rig::start(&ws.root, &llm, Start::New).await;
    // A steer without a running turn is refused so the UI sends it as a turn.
    rig.send(steer(7, "idle"));
    let msg = rig
        .wait_for(|msg| matches!(msg, Msg::SteerAck { .. }))
        .await;
    assert!(
        matches!(
            msg,
            Msg::SteerAck {
                id: PendingId(7),
                result: Err(meerkat_tui::delivery::SteerFailure::NotRunning)
            }
        ),
        "{msg:?}"
    );
    rig.quit().await;
}
