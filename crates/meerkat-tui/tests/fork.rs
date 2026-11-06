//! The TUI's `fork` tool end to end, through the real runtime-backed
//! service with a scripted LLM: the child starts with the parent's
//! conversation and the task, and its answer is the tool's result; its
//! calls pass the parent's approval gate, counted for the parent and named
//! as the fork's; an interrupt leaves no running fork; `fork_off` stays
//! denied and the child cannot fork again.
//!
//! Also the probe that decided how `fork` is built: the engine refuses a
//! durable fork of the session from inside its own running turn.

#![cfg(feature = "mob")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::sync::Arc;

use async_trait::async_trait;
use common::{Rig, ScriptedLlmClient, Step, Workspace, within};
use meerkat_core::Message;
use meerkat_core::service::SessionError;
use meerkat_core::types::ToolResult;
use meerkat_tui::approval::classifier::{
    ActionClassifier, ClassifierError, ClassifierInput, ClassifierVerdict,
};
use meerkat_tui::bootstrap::ApprovalOverrides;
use meerkat_tui::runner::Start;
use meerkat_tui::ui::{ApprovalNotice, Effect, Msg, StatusUpdate};
use parking_lot::Mutex;
use serde_json::json;

/// A classifier that replays verdicts and records the tools it judged.
#[derive(Default)]
struct ScriptedClassifier {
    verdicts: Mutex<Vec<ClassifierVerdict>>,
    seen: Mutex<Vec<String>>,
}

#[async_trait]
impl ActionClassifier for ScriptedClassifier {
    async fn classify(
        &self,
        input: ClassifierInput<'_>,
    ) -> Result<ClassifierVerdict, ClassifierError> {
        self.seen.lock().push(input.tool.to_string());
        let mut verdicts = self.verdicts.lock();
        if verdicts.is_empty() {
            Err(ClassifierError::Provider("script exhausted".into()))
        } else {
            Ok(verdicts.remove(0))
        }
    }
}

fn fork(id: &'static str, task: &str) -> Step {
    Step::Tool {
        id,
        name: "fork",
        args: json!({ "task": task, "expected_output": "FORK-EXPECTED" }),
    }
}

fn tool_result(messages: &[Message], id: &str) -> ToolResult {
    messages
        .iter()
        .find_map(|m| match m {
            Message::ToolResults { results, .. } => {
                results.iter().find(|r| r.tool_use_id == id).cloned()
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("no result for {id}"))
}

/// Wait for a message matching `pred`, unless one already arrived.
async fn saw(rig: &mut Rig, pred: impl Fn(&Msg) -> bool) {
    if !rig.seen.iter().any(&pred) {
        rig.wait_for(pred).await;
    }
}

fn no_helpers(msg: &Msg) -> bool {
    matches!(msg, Msg::Status(StatusUpdate::Helpers(h)) if h.is_empty())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fork_starts_with_the_parents_conversation_and_returns_its_answer() {
    let ws = Workspace::new();
    let llm = ScriptedLlmClient::new(vec![
        Step::Text("FIRST-ANSWER"),
        fork("f1", "FORK-TASK: check the other module"),
        Step::Text("FORK-REPORT: the other module is fine"),
        Step::Text("parent done"),
    ]);
    let mut rig = Rig::start(&ws.root, &llm, Start::New).await;
    rig.send(Effect::StartTurn("FIRST-ASK about the module".into()));
    rig.turn_finished().await;
    rig.send(Effect::StartTurn("SECOND-ASK fork a check".into()));
    let outcome = rig.turn_finished().await;
    assert_eq!(outcome.result.expect("turn succeeds").text, "parent done");

    let recorded = llm.recorded();
    assert_eq!(recorded.len(), 4, "parent, parent, fork, parent");
    let child = &recorded[2];
    let prompt = child.user();
    for part in [
        "FIRST-ASK",
        "FIRST-ANSWER",
        "SECOND-ASK",
        "FORK-TASK",
        "FORK-EXPECTED",
    ] {
        assert!(prompt.contains(part), "{part} missing from {prompt}");
    }
    // The context precedes the task.
    assert!(prompt.find("FIRST-ANSWER") < prompt.find("FORK-TASK"));
    // It runs on the parent's model, as a helper.
    assert_eq!(child.model, common::MODEL);
    assert!(child.system().contains("Working as a helper"));
    // The child's answer is the tool's result.
    let result = tool_result(&recorded[3].messages, "f1");
    assert!(!result.is_error, "{}", result.text_content());
    assert_eq!(
        result.text_content(),
        "FORK-REPORT: the other module is fine"
    );
    saw(&mut rig, no_helpers).await;
    assert!(rig.seen.iter().any(|m| matches!(
        m,
        Msg::Status(StatusUpdate::Helpers(h)) if h.len() == 1 && h[0].label == "fork-1"
    )));
    rig.quit().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_forks_calls_pass_the_parents_gate() {
    let ws = Workspace::new();
    let marker = ws.sibling("pwned");
    let command = format!("touch {}", marker.display());
    let classifier = Arc::new(ScriptedClassifier::default());
    classifier.verdicts.lock().push(ClassifierVerdict::Block {
        rule: Some("D1".into()),
        reason: "touches files outside the project".into(),
    });
    let llm = ScriptedLlmClient::new(vec![
        fork("f1", "touch the marker"),
        Step::Tool {
            id: "c1",
            name: "shell",
            args: json!({ "command": command }),
        },
        Step::Text("FORK-DONE: the touch was blocked"),
        Step::Text("parent done"),
    ]);
    let mut rig = Rig::start_with(
        &ws.root,
        &llm,
        Start::New,
        ApprovalOverrides {
            classifier: Some(Arc::clone(&classifier) as Arc<dyn ActionClassifier>),
            escalation_timeout: None,
        },
    )
    .await;
    rig.send(Effect::StartTurn("USER-ASK fork something".into()));
    rig.turn_finished().await;

    let recorded = llm.recorded();
    assert_eq!(recorded.len(), 4, "parent, fork ×2, parent");
    let denied = tool_result(&recorded[2].messages, "c1");
    assert!(denied.is_error);
    assert!(
        denied
            .text_content()
            .contains("Blocked by auto-mode safety policy: touches files outside the project"),
        "{}",
        denied.text_content()
    );
    assert!(!marker.exists(), "the denied command never ran");
    // Only the child's shell call reached the classifier: `fork` itself is
    // tier 1, like `delegate`.
    assert_eq!(classifier.seen.lock().clone(), vec!["shell".to_string()]);
    let (counters, _) = rig.approval.auto_state();
    assert_eq!(counters.total, 1, "counted on the parent's counters");
    saw(&mut rig, |m| {
        matches!(m, Msg::Approval(ApprovalNotice { allowed: false, tool, .. }) if tool == "[fork-1] shell")
    })
    .await;
    let result = tool_result(&recorded[3].messages, "f1");
    assert!(result.text_content().contains("FORK-DONE"));
    rig.quit().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_interrupt_leaves_no_running_fork() {
    let ws = Workspace::new();
    let llm = ScriptedLlmClient::new(vec![
        fork("f1", "wait forever"),
        Step::Hang,
        Step::Text("after"),
        Step::Text("after"),
    ]);
    let mut rig = Rig::start(&ws.root, &llm, Start::New).await;
    let id = rig.first_session.clone();
    rig.send(Effect::StartTurn("go".into()));
    llm.wait_call(1).await;
    saw(
        &mut rig,
        |m| matches!(m, Msg::Status(StatusUpdate::Helpers(h)) if h.len() == 1),
    )
    .await;
    rig.send(Effect::Interrupt);
    let outcome = rig.turn_finished().await;
    assert!(outcome.result.is_err(), "{outcome:?}");
    saw(&mut rig, no_helpers).await;
    let helpers = rig.helpers.clone().expect("helpers are hosted");
    assert!(helpers.live().is_empty());
    let mob = meerkat_mob::MobId::from(format!("implicit-{id}"));
    let handle = within(helpers.state().handle_for(&mob)).await.unwrap();
    let members = within(handle.list_members()).await;
    assert!(members.is_empty(), "members left: {members:?}");
    rig.quit().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fork_off_stays_denied_and_a_fork_cannot_fork() {
    let ws = Workspace::new();
    let llm = ScriptedLlmClient::new(vec![
        fork("f1", "say hi"),
        Step::Text("hi"),
        Step::Text("done"),
    ]);
    let mut rig = Rig::start(&ws.root, &llm, Start::New).await;
    rig.send(Effect::StartTurn("fork".into()));
    rig.turn_finished().await;
    let recorded = llm.recorded();
    let has = |i: usize, name: &str| recorded[i].tools.iter().any(|t| t == name);
    assert!(has(0, "fork"), "{:?}", recorded[0].tools);
    assert!(has(0, "delegate"), "{:?}", recorded[0].tools);
    assert!(!has(0, "fork_off"), "{:?}", recorded[0].tools);
    // The child: a helper's tools, without `fork` or `fork_off`.
    assert!(has(1, "read_file"), "{:?}", recorded[1].tools);
    assert!(has(1, "shell"), "{:?}", recorded[1].tools);
    assert!(!has(1, "fork"), "{:?}", recorded[1].tools);
    assert!(!has(1, "fork_off"), "{:?}", recorded[1].tools);
    rig.quit().await;
}

/// The probe behind the design (`.rct/rkat-tui/evidence.md`, "Fork"): a
/// durable fork of the session cut from inside its running turn, as a
/// `fork` tool call would ask for it, is refused as busy, at the committed
/// prefix too. (The engine's `CallerTurn` admission, which forks a running
/// caller, needs a mob member binding for the child.)
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_engine_refuses_a_durable_fork_of_a_running_session() {
    let ws = Workspace::new();
    let llm = ScriptedLlmClient::new(vec![
        Step::Text("FIRST-ANSWER"),
        Step::Hang,
        Step::Text("after"),
    ]);
    let mut rig = Rig::start(&ws.root, &llm, Start::New).await;
    let id = rig.first_session.clone();
    rig.send(Effect::StartTurn("first".into()));
    rig.turn_finished().await;
    let committed = rig.session(&id).await.messages().len();
    rig.send(Effect::StartTurn("second".into()));
    llm.wait_call(1).await;
    for count in [None, Some(committed)] {
        let refused = within(rig.service.fork_durable_session(&id, count, None, None)).await;
        assert!(
            matches!(refused, Err(SessionError::Busy { .. })),
            "{count:?}: {refused:?}"
        );
    }
    rig.send(Effect::Interrupt);
    rig.turn_finished().await;
    rig.quit().await;
}
