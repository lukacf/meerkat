//! The runner end to end, headless: effects in, UI messages out, through the
//! real runtime-backed service (`PersistentSessionService` + `MeerkatMachine`)
//! with a scripted LLM. Covers a turn, a queued turn, a steered message
//! applied at a tool boundary (and one typed while a slow shell call runs),
//! interrupt (also during a slow tool), resume by id and `last`, `-c`
//! across process lifetimes, and a renamed workspace.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::sync::Arc;

use async_trait::async_trait;
use common::{Rig, RigOptions, ScriptedLlmClient, Step, Workspace, copy_tree, user_texts};
use meerkat_core::{AgentEvent, Message};
use meerkat_tui::approval::classifier::{
    ActionClassifier, ClassifierError, ClassifierInput, ClassifierVerdict,
};
use meerkat_tui::bootstrap::ApprovalOverrides;
use meerkat_tui::delivery::PendingId;
use meerkat_tui::runner::Start;
use meerkat_tui::session::resolve_session_ref;
use meerkat_tui::ui::{Block, Effect, EscalationAnswer, Msg, StatusUpdate, TurnFailure};
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
    assert_eq!(outcome.result, Err(TurnFailure::Interrupted));
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_builtins_the_tui_owns_are_denied_by_the_tool_policy() {
    let ws = Workspace::new();
    let llm = ScriptedLlmClient::new(vec![
        Step::Tool {
            id: "swap",
            name: "brain_swap",
            args: json!({ "model": "claude-opus-4-8" }),
        },
        Step::Tool {
            id: "patch",
            name: "apply_patch",
            args: json!({ "input": "*** Begin Patch\n*** End Patch\n" }),
        },
        Step::Text("done"),
    ]);
    let mut rig = Rig::start(&ws.root, &llm, Start::New).await;
    rig.send(Effect::StartTurn("try them".into()));
    rig.turn_finished().await;
    let requests = llm.requests();
    let result_of = |id: &str| {
        requests
            .iter()
            .flatten()
            .find_map(|m| match m {
                Message::ToolResults { results, .. } => {
                    results.iter().find(|r| r.tool_use_id == id).cloned()
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("no result for {id}"))
    };
    for id in ["swap", "patch"] {
        let result = result_of(id);
        assert!(result.is_error, "{id}: {result:?}");
    }
    // The model never changed.
    let meta = rig
        .session(&rig.first_session)
        .await
        .session_metadata()
        .unwrap();
    assert_eq!(meta.model, common::MODEL);
    rig.quit().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_same_prompt_after_a_failed_turn_is_not_added_twice() {
    let ws = Workspace::new();
    let llm = ScriptedLlmClient::new(vec![Step::RejectTools, Step::Text("fixed")]);
    let mut rig = Rig::start(&ws.root, &llm, Start::New).await;
    rig.send(Effect::StartTurn("fix the build".into()));
    let outcome = rig.turn_finished().await;
    assert!(
        matches!(outcome.result, Err(TurnFailure::Failed(_))),
        "{:?}",
        outcome.result
    );
    // The failed run committed the prompt.
    let session = rig.session(&rig.first_session).await;
    assert_eq!(
        user_texts(session.messages()),
        vec!["fix the build".to_string()]
    );

    // Re-typed (with stray whitespace): a retry, not a second copy.
    rig.send(Effect::StartTurn("  fix the build\n".into()));
    rig.wait_for(
        |msg| matches!(msg, Msg::Notice { text, .. } if text.contains("already in the transcript")),
    )
    .await;
    let outcome = rig.turn_finished().await;
    assert_eq!(outcome.result.expect("retry succeeds").text, "fixed");
    let expected = vec![
        "fix the build".to_string(),
        "(retrying the request above)".to_string(),
    ];
    assert_eq!(user_texts(&llm.requests()[1]), expected);
    let session = rig.session(&rig.first_session).await;
    assert_eq!(user_texts(session.messages()), expected);
    rig.quit().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_different_prompt_after_a_failed_turn_is_sent_as_written() {
    let ws = Workspace::new();
    let llm = ScriptedLlmClient::new(vec![
        Step::RejectTools,
        Step::Text("first"),
        Step::Text("second"),
    ]);
    let mut rig = Rig::start(&ws.root, &llm, Start::New).await;
    rig.send(Effect::StartTurn("fix the build".into()));
    assert!(rig.turn_finished().await.result.is_err());

    rig.send(Effect::StartTurn("run the tests instead".into()));
    assert_eq!(
        rig.turn_finished().await.result.expect("turn").text,
        "first"
    );
    // Only right after the failure: a repeat after a success is a message.
    rig.send(Effect::StartTurn("run the tests instead".into()));
    assert_eq!(
        rig.turn_finished().await.result.expect("turn").text,
        "second"
    );
    let session = rig.session(&rig.first_session).await;
    assert_eq!(
        user_texts(session.messages()),
        vec![
            "fix the build".to_string(),
            "run the tests instead".to_string(),
            "run the tests instead".to_string(),
        ]
    );
    rig.quit().await;
}

/// Allows every reviewed call: the slow-tool cases are about delivery, not
/// the approval gate.
struct AllowAll;

#[async_trait]
impl ActionClassifier for AllowAll {
    async fn classify(
        &self,
        _input: ClassifierInput<'_>,
    ) -> Result<ClassifierVerdict, ClassifierError> {
        Ok(ClassifierVerdict::allow("ok"))
    }
}

fn allow_shell() -> RigOptions {
    RigOptions {
        approval: ApprovalOverrides {
            classifier: Some(Arc::new(AllowAll)),
            ..ApprovalOverrides::default()
        },
        ..RigOptions::default()
    }
}

/// The dogfood case: a message typed while the model's shell call runs
/// reaches the model after that call's result, and the call runs to its end.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_message_typed_while_a_slow_tool_runs_reaches_the_model_after_its_result() {
    let ws = Workspace::new();
    let llm = ScriptedLlmClient::new(vec![
        Step::Tool {
            id: "slow",
            name: "shell",
            args: json!({ "command": "sleep 2; echo done sleeping", "timeout_secs": 30 }),
        },
        Step::Text("finished"),
        Step::Text("follow-up answer"),
    ]);
    let mut rig = Rig::start_opts(&ws.root, &llm, Start::New, allow_shell()).await;
    rig.send(Effect::StartTurn("do something for a while".into()));
    tool_started(&rig, "slow").await;
    rig.send(steer(1, "typed while it sleeps"));
    let ack = rig
        .wait_for(|msg| matches!(msg, Msg::SteerAck { .. }))
        .await;
    assert!(
        matches!(ack, Msg::SteerAck { result: Ok(()), .. }),
        "{ack:?}"
    );

    rig.wait_for(|msg| matches!(msg, Msg::SteerDelivered(PendingId(1))))
        .await;
    let outcome = rig.turn_finished().await;
    assert_eq!(outcome.result.expect("turn succeeds").text, "finished");
    assert!(outcome.undelivered.is_empty());
    // One run: the shell ran to its end, its result reached the model, and
    // the typed message followed it in the same request.
    let requests = llm.requests();
    assert_eq!(requests.len(), 2, "no follow-up turn");
    let second = &requests[1];
    let result_at = second
        .iter()
        .position(|m| {
            matches!(m, Message::ToolResults { results, .. }
            if results.iter().any(|r| !r.is_error && r.text_content().contains("done sleeping")))
        })
        .expect("the shell result reached the model");
    let typed_at = second
        .iter()
        .position(|m| matches!(m, Message::User(u) if u.text_content() == "typed while it sleeps"))
        .expect("the typed message reached the model");
    assert!(typed_at > result_at);
    let session = rig.session(&rig.first_session).await;
    assert!(
        !session
            .messages()
            .iter()
            .any(|m| matches!(m, Message::SystemNotice(_))),
        "nothing was closed by recovery"
    );
    rig.quit().await;
}

/// Allows every reviewed call with a stated reason.
struct AllowBecause(&'static str);

#[async_trait]
impl ActionClassifier for AllowBecause {
    async fn classify(
        &self,
        _input: ClassifierInput<'_>,
    ) -> Result<ClassifierVerdict, ClassifierError> {
        Ok(ClassifierVerdict::allow(self.0))
    }
}

/// A call the classifier allowed gets a dim note under its tool block with
/// the classifier's reason; a tier-1 call gets none.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_classifier_allowed_call_gets_a_note_and_a_tier1_call_does_not() {
    let ws = Workspace::new();
    std::fs::write(ws.root.join("scratch.txt"), "x").unwrap();
    let llm = ScriptedLlmClient::new(vec![
        Step::Tools(vec![
            ("tier1", "shell", json!({ "command": "ls" })),
            (
                "reviewed",
                "shell",
                json!({ "command": "rm -rf scratch.txt" }),
            ),
        ]),
        Step::Text("cleaned up"),
    ]);
    let options = RigOptions {
        approval: ApprovalOverrides {
            classifier: Some(Arc::new(AllowBecause(
                "removes an in-project scratch file\nthe user asked to clean up",
            ))),
            ..ApprovalOverrides::default()
        },
        ..RigOptions::default()
    };
    let mut rig = Rig::start_opts(&ws.root, &llm, Start::New, options).await;
    rig.send(Effect::StartTurn("clean up the scratch file".into()));
    let note = rig
        .wait_for(
            |msg| matches!(msg, Msg::ToolNote { tool_use_id, .. } if tool_use_id == "reviewed"),
        )
        .await;
    let Msg::ToolNote { text, .. } = note else {
        panic!("not a note: {note:?}");
    };
    assert_eq!(
        text,
        "allowed by classifier: removes an in-project scratch file the user asked to clean up"
    );
    assert_eq!(
        rig.turn_finished().await.result.expect("turn").text,
        "cleaned up"
    );
    assert!(
        !ws.root.join("scratch.txt").exists(),
        "the reviewed call ran"
    );
    let notes: Vec<&Msg> = rig
        .seen
        .iter()
        .filter(|msg| matches!(msg, Msg::ToolNote { .. }))
        .collect();
    assert_eq!(notes.len(), 1, "only the reviewed call is noted: {notes:?}");
    // The classifier allow no longer logs a separate approval line.
    assert!(!rig.seen.iter().any(|msg| matches!(msg, Msg::Approval(_))));
    rig.quit().await;
}

/// The model writes a protected `.env.test`: even with a classifier that
/// allows everything, the UI gets an escalation naming the protected path;
/// answering allow runs the write.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_protected_write_is_put_to_the_user_and_runs_when_allowed() {
    let ws = Workspace::new();
    let llm = ScriptedLlmClient::new(vec![
        Step::Tool {
            id: "env",
            name: "write_file",
            args: json!({ "path": ".env.test", "content": "API_URL=http://localhost\n" }),
        },
        Step::Text("configured"),
    ]);
    let mut rig = Rig::start_opts(&ws.root, &llm, Start::New, allow_shell()).await;
    rig.send(Effect::StartTurn("add a test env file".into()));
    let Msg::Escalation(prompt, id) = rig.wait_for(|msg| matches!(msg, Msg::Escalation(..))).await
    else {
        unreachable!()
    };
    assert_eq!(prompt.tool, "write_file");
    assert!(
        prompt
            .reason
            .as_deref()
            .is_some_and(|r| r.contains("protected path: `.env.test` (secrets)")),
        "{prompt:?}"
    );
    rig.send(Effect::RespondEscalation(id, EscalationAnswer::AllowOnce));
    assert_eq!(
        rig.turn_finished().await.result.expect("turn").text,
        "configured"
    );
    assert_eq!(
        std::fs::read_to_string(ws.root.join(".env.test")).unwrap(),
        "API_URL=http://localhost\n"
    );
    assert!(
        !rig.seen
            .iter()
            .any(|msg| matches!(msg, Msg::ToolNote { .. })),
        "no classifier note"
    );
    rig.quit().await;
}

/// Waits until the tool call `id` has started executing.
async fn tool_started(rig: &Rig, id: &str) {
    common::within(async {
        loop {
            let started = rig.events.lock().iter().any(|event| {
                matches!(event, AgentEvent::ToolExecutionStarted { id: started, .. } if started == id)
            });
            if started {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn interrupting_a_slow_tool_returns_the_messages_typed_meanwhile() {
    let ws = Workspace::new();
    let llm = ScriptedLlmClient::new(vec![
        Step::Tool {
            id: "slow",
            name: "shell",
            args: json!({ "command": "sleep 20; echo done sleeping", "timeout_secs": 30 }),
        },
        Step::Text("after the interrupt"),
    ]);
    let mut rig = Rig::start_opts(&ws.root, &llm, Start::New, allow_shell()).await;
    rig.send(Effect::StartTurn("do something for a while".into()));
    tool_started(&rig, "slow").await;
    rig.send(steer(1, "Test"));
    rig.wait_for(|msg| matches!(msg, Msg::SteerAck { .. }))
        .await;
    rig.send(steer(2, "Hepp"));
    rig.wait_for(|msg| matches!(msg, Msg::SteerAck { .. }))
        .await;
    rig.send(Effect::Interrupt);
    let outcome = rig.turn_finished().await;
    // Interrupted (not "failed: interrupted by the user"), and both typed
    // messages come back: neither ran as a follow-up turn.
    assert_eq!(outcome.result, Err(TurnFailure::Interrupted));
    assert_eq!(outcome.undelivered, vec![PendingId(1), PendingId(2)]);
    assert_eq!(
        llm.requests().len(),
        1,
        "no follow-up turn after the interrupt"
    );
    // The interrupt closed the run through recovery: the call is answered
    // as outcome unknown and the notice says the input was not requeued.
    let session = rig.session(&rig.first_session).await;
    assert!(session.messages().iter().any(|m| matches!(
        m,
        Message::ToolResults { results, .. }
            if results.iter().any(|r| r.is_error && r.text_content().contains("outcome_uncertain"))
    )));
    assert!(
        !user_texts(session.messages())
            .iter()
            .any(|t| t == "Test" || t == "Hepp")
    );
    // The session stays usable.
    rig.send(Effect::StartTurn("Test\n\nHepp".into()));
    assert_eq!(
        rig.turn_finished().await.result.expect("turn").text,
        "after the interrupt"
    );
    rig.quit().await;
}

/// Esc with a message typed during a slow tool: the message is pulled back
/// (it never reaches the model) and the turn runs on: the tool ends, its
/// result reaches the model, the turn completes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn recalling_a_message_typed_during_a_slow_tool_leaves_the_run_alone() {
    let ws = Workspace::new();
    let llm = ScriptedLlmClient::new(vec![
        Step::Tool {
            id: "slow",
            name: "shell",
            args: json!({ "command": "sleep 2; echo done sleeping", "timeout_secs": 30 }),
        },
        Step::Text("finished"),
        Step::Text("follow-up answer"),
    ]);
    let mut rig = Rig::start_opts(&ws.root, &llm, Start::New, allow_shell()).await;
    rig.send(Effect::StartTurn("do something for a while".into()));
    tool_started(&rig, "slow").await;
    rig.send(steer(1, "never mind"));
    let ack = rig
        .wait_for(|msg| matches!(msg, Msg::SteerAck { .. }))
        .await;
    assert!(
        matches!(ack, Msg::SteerAck { result: Ok(()), .. }),
        "{ack:?}"
    );
    rig.send(Effect::RecallQueued);
    let recalled = rig
        .wait_for(|msg| matches!(msg, Msg::Recalled { .. }))
        .await;
    assert!(
        matches!(&recalled, Msg::Recalled { recalled, errors }
            if recalled == &[PendingId(1)] && errors.is_empty()),
        "{recalled:?}"
    );

    let outcome = rig.turn_finished().await;
    assert_eq!(outcome.result.expect("turn succeeds").text, "finished");
    assert!(outcome.undelivered.is_empty(), "already back in the editor");
    let requests = llm.requests();
    assert_eq!(requests.len(), 2, "no follow-up turn");
    assert!(
        requests[1]
            .iter()
            .any(|m| matches!(m, Message::ToolResults { results, .. }
        if results.iter().any(|r| !r.is_error && r.text_content().contains("done sleeping"))))
    );
    for request in &requests {
        assert!(!user_texts(request).contains(&"never mind".to_string()));
    }
    let session = rig.session(&rig.first_session).await;
    assert!(!user_texts(session.messages()).contains(&"never mind".to_string()));
    assert!(
        !session
            .messages()
            .iter()
            .any(|m| matches!(m, Message::SystemNotice(_))),
        "nothing was closed by recovery"
    );
    rig.quit().await;
}

/// A recall after the steer joined the run finds nothing to pull back: the
/// turn runs on with the steer delivered.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn recalling_a_steer_that_already_joined_the_run_leaves_it_delivered() {
    let ws = Workspace::new();
    let gate = Arc::new(Notify::new());
    let llm = ScriptedLlmClient::new(vec![
        Step::Tool {
            id: "slow",
            name: "shell",
            args: json!({ "command": "sleep 1; echo done sleeping", "timeout_secs": 30 }),
        },
        Step::Gated(Arc::clone(&gate), Box::new(Step::Text("finished"))),
    ]);
    let mut rig = Rig::start_opts(&ws.root, &llm, Start::New, allow_shell()).await;
    rig.send(Effect::StartTurn("do something for a while".into()));
    tool_started(&rig, "slow").await;
    rig.send(steer(1, "typed while it sleeps"));
    rig.wait_for(|msg| matches!(msg, Msg::SteerAck { .. }))
        .await;
    rig.wait_for(|msg| matches!(msg, Msg::SteerDelivered(PendingId(1))))
        .await;
    llm.wait_call(1).await;
    rig.send(Effect::RecallQueued);
    let recalled = rig
        .wait_for(|msg| matches!(msg, Msg::Recalled { .. }))
        .await;
    assert!(
        matches!(&recalled, Msg::Recalled { recalled, errors }
            if recalled.is_empty() && errors.is_empty()),
        "{recalled:?}"
    );
    gate.notify_one();

    let outcome = rig.turn_finished().await;
    assert_eq!(outcome.result.expect("turn succeeds").text, "finished");
    assert!(outcome.undelivered.is_empty());
    let requests = llm.requests();
    assert_eq!(requests.len(), 2, "no follow-up turn");
    assert!(user_texts(&requests[1]).contains(&"typed while it sleeps".to_string()));
    rig.quit().await;
}
