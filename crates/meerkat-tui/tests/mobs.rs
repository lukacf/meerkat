//! Helpers (mob members) end to end, through the real runtime-backed
//! service with a scripted LLM: a `delegate` call spawns a helper that runs
//! one turn and returns; the helper's calls pass the delegating session's
//! approval gate (a read runs, a Tier-3 call is denied with the reason the
//! helper reads, judged against the user's words and counted for the
//! session); its events reach the UI nested and named, its usage lands in
//! the cost breakdown and the sidecar; an interrupt leaves no running
//! helper; a configured endpoint's session hands its model and binding on.

#![cfg(feature = "mob")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::sync::Arc;

use async_trait::async_trait;
use common::{Rig, RigOptions, ScriptedLlmClient, Step, Workspace, args_with, within};
use meerkat_core::types::ToolResult;
use meerkat_core::{Message, StreamScopeFrame};
use meerkat_tui::approval::classifier::{
    ActionClassifier, ClassifierError, ClassifierInput, ClassifierVerdict,
};
use meerkat_tui::bootstrap::{self, ApprovalOverrides};
use meerkat_tui::endpoints::{ENDPOINT_REALM, Endpoint, load_cost};
use meerkat_tui::models::{EndpointListing, ModelEntry, ModelLister, ModelSource};
use meerkat_tui::runner::Start;
use meerkat_tui::ui::{App, ApprovalNotice, Effect, Msg, StatusInfo, StatusUpdate};
use parking_lot::Mutex;
use serde_json::{Value, json};

/// A classifier that replays verdicts and records `(tool, transcript)`.
#[derive(Default)]
struct ScriptedClassifier {
    verdicts: Mutex<Vec<ClassifierVerdict>>,
    seen: Mutex<Vec<(String, String)>>,
}

#[async_trait]
impl ActionClassifier for ScriptedClassifier {
    async fn classify(
        &self,
        input: ClassifierInput<'_>,
    ) -> Result<ClassifierVerdict, ClassifierError> {
        self.seen
            .lock()
            .push((input.tool.to_string(), input.transcript.render()));
        let mut verdicts = self.verdicts.lock();
        if verdicts.is_empty() {
            Err(ClassifierError::Provider("script exhausted".into()))
        } else {
            Ok(verdicts.remove(0))
        }
    }
}

fn overrides(classifier: &Arc<ScriptedClassifier>) -> ApprovalOverrides {
    ApprovalOverrides {
        classifier: Some(Arc::clone(classifier) as Arc<dyn ActionClassifier>),
        escalation_timeout: None,
    }
}

fn delegate(id: &'static str, member: &str, task: &str) -> Step {
    Step::Tool {
        id,
        name: "delegate",
        args: json!({
            "member_id": member,
            "task": task,
            "result_label": "result",
            "max_text_bytes": 4000,
        }),
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

fn leak(text: String) -> &'static str {
    Box::leak(text.into_boxed_str())
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

/// The helper label of a scoped message.
fn helper_of(msg: &Msg) -> Option<(String, usize)> {
    let Msg::Scoped(ev) = msg else { return None };
    let frames: Vec<&str> = ev
        .scope_path
        .iter()
        .filter_map(|f| match f {
            StreamScopeFrame::MobMember { agent_identity, .. } => Some(agent_identity.as_str()),
            _ => None,
        })
        .collect();
    frames
        .last()
        .map(|label| ((*label).to_string(), frames.len()))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_delegated_helper_runs_behind_the_sessions_gate() {
    let ws = Workspace::new();
    std::fs::write(ws.root.join("notes.txt"), "HELPER-READ-CONTENT\n").unwrap();
    let marker = ws.sibling("pwned");
    let command = leak(format!("touch {}", marker.display()));
    let classifier = Arc::new(ScriptedClassifier::default());
    classifier.verdicts.lock().push(ClassifierVerdict::Block {
        rule: Some("D1".into()),
        reason: "touches files outside the project".into(),
    });
    let llm = ScriptedLlmClient::new(vec![
        delegate(
            "d1",
            "scout",
            "HELPER-TASK: read notes.txt and touch the marker",
        ),
        Step::Tools(vec![
            ("h1", "read_file", json!({"path": "notes.txt"})),
            ("h2", "shell", json!({ "command": command })),
        ]),
        Step::Text("HELPER-DONE: read it, the touch was blocked"),
        Step::Text("parent done"),
    ]);
    let realm = bootstrap::resolve_scope(&args_with(&ws.root, &[]))
        .unwrap()
        .realm_root();
    let mut rig = Rig::start_with(&ws.root, &llm, Start::New, overrides(&classifier)).await;
    let id = rig.first_session.clone();
    rig.send(Effect::StartTurn("USER-ASK look into the notes".into()));
    let outcome = rig.turn_finished().await;
    assert_eq!(outcome.result.expect("turn succeeds").text, "parent done");

    let recorded = llm.recorded();
    assert_eq!(recorded.len(), 4, "parent, helper ×2, parent");
    // The helper ran on the session's model, with the helper instructions
    // and the TUI's file tools, and its task as its prompt.
    assert_eq!(recorded[1].model, common::MODEL);
    assert!(recorded[1].system().contains("Working as a helper"));
    assert!(recorded[1].tools.iter().any(|t| t == "read_file"));
    assert!(recorded[1].user().contains("HELPER-TASK"));
    // The read passed the gate; the Tier-3 shell call was denied, and the
    // helper read why.
    let read = tool_result(&recorded[2].messages, "h1");
    assert!(!read.is_error, "{}", read.text_content());
    assert!(read.text_content().contains("HELPER-READ-CONTENT"));
    let denied = tool_result(&recorded[2].messages, "h2");
    assert!(denied.is_error);
    assert!(
        denied
            .text_content()
            .contains("Blocked by auto-mode safety policy: touches files outside the project"),
        "{}",
        denied.text_content()
    );
    assert!(!marker.exists(), "the denied command never ran");
    // Judged against the user's words, never the helper's task.
    let seen = classifier.seen.lock().clone();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].0, "shell");
    assert!(seen[0].1.contains("USER-ASK"), "{}", seen[0].1);
    // The helper's task is the parent's action, never the user's words.
    assert!(
        seen[0]
            .1
            .lines()
            .filter(|line| line.starts_with("[user]"))
            .all(|line| !line.contains("HELPER-TASK")),
        "{}",
        seen[0].1
    );
    // Counted for the session, and named as the helper's.
    let (counters, _) = rig.approval.auto_state();
    assert_eq!(counters.total, 1);
    saw(&mut rig, |m| {
        matches!(m, Msg::Approval(ApprovalNotice { allowed: false, tool, .. }) if tool == "[scout] shell")
    })
    .await;
    // The parent got the helper's result.
    let result = tool_result(&recorded[3].messages, "d1");
    assert!(!result.is_error, "{}", result.text_content());
    assert!(result.text_content().contains("HELPER-DONE"));

    // The UI heard the helper: running (then not), its events nested one
    // level deep and named.
    saw(&mut rig, no_helpers).await;
    let seen_msgs = rig.seen.clone();
    assert!(seen_msgs.iter().any(|m| matches!(
        m,
        Msg::Status(StatusUpdate::Helpers(h))
            if h.len() == 1 && h[0].label == "scout" && h[0].model == common::MODEL
    )));
    assert!(
        seen_msgs
            .iter()
            .filter_map(helper_of)
            .any(|(label, depth)| label == "scout" && depth == 1),
        "no scoped helper events"
    );

    // Its usage reaches the cost breakdown (and the footer total) and the
    // sidecar.
    let mut app = App::new(StatusInfo::default());
    let mut snapshots = Vec::new();
    for msg in seen_msgs {
        if matches!(msg, Msg::Scoped(_) | Msg::Status(StatusUpdate::Helpers(_))) {
            snapshots.extend(
                app.update(msg)
                    .into_iter()
                    .filter(|e| matches!(e, Effect::CostSnapshot(_))),
            );
        }
    }
    let helper = &app.status.cost.helpers["scout"][common::MODEL];
    assert_eq!((helper.input, helper.output), (20, 10), "two helper calls");
    assert_eq!(app.status.cost.per_model[common::MODEL].input, 20);
    let breakdown = app.status.cost.breakdown(&app.status.prices);
    assert!(
        breakdown.contains("scout (claude-sonnet-4-6): in 20"),
        "{breakdown}"
    );
    let last = snapshots.pop().expect("a cost snapshot");
    rig.send(last);
    rig.quit().await;
    let stored = load_cost(&realm, &id.to_string()).expect("sidecar");
    assert_eq!(stored.helpers["scout"][common::MODEL].input, 20);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_interrupt_leaves_no_running_helper() {
    let ws = Workspace::new();
    let llm = ScriptedLlmClient::new(vec![
        delegate("d1", "slow", "wait forever"),
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

/// An Anthropic-dialect gateway serving a catalogued model id. (A model id
/// only the gateway knows cannot be delegated from: upstream validates the
/// implicit delegate mob's profile model against the catalog alone.)
fn proxy_options() -> RigOptions {
    let lister: ModelLister = Arc::new(|endpoints: Vec<Endpoint>| {
        Box::pin(async move {
            endpoints
                .into_iter()
                .map(|e| {
                    let models = if e.name == "proxy" {
                        vec![ModelEntry {
                            id: common::MODEL.into(),
                            display_name: None,
                            source: ModelSource::Provider,
                            context_window: Some(50_000),
                            max_output_tokens: Some(4_096),
                        }]
                    } else {
                        Vec::new()
                    };
                    (
                        e.name,
                        Ok(EndpointListing {
                            models,
                            prices: Vec::new(),
                        }),
                    )
                })
                .collect()
        })
    });
    RigOptions {
        args: [
            "--set",
            "endpoints.proxy.dialect=anthropic",
            "--set",
            "endpoints.proxy.base_url=http://127.0.0.1:9",
            "--set",
            "endpoints.proxy.api_key=sk-proxy-test",
            "--model",
            "proxy:claude-sonnet-4-6",
        ]
        .map(str::to_string)
        .to_vec(),
        lister: Some(lister),
        ..RigOptions::default()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_configured_endpoints_helper_uses_the_same_binding() {
    let ws = Workspace::new();
    let llm = ScriptedLlmClient::new(vec![
        delegate("d1", "scout", "say hi"),
        Step::Text("hi"),
        Step::Text("done"),
    ]);
    let mut rig = Rig::start_opts(&ws.root, &llm, Start::New, proxy_options()).await;
    rig.send(Effect::StartTurn("delegate".into()));
    rig.turn_finished().await;
    let recorded = llm.recorded();
    assert_eq!(recorded[0].model, common::MODEL);
    assert_eq!(
        recorded[1].model,
        common::MODEL,
        "the helper runs on the session's model"
    );
    let result = tool_result(&recorded[2].messages, "d1");
    let value: Value = serde_json::from_str(&result.text_content()).unwrap();
    let helper_session =
        meerkat_core::SessionId::parse(value["session_id"].as_str().unwrap()).unwrap();
    let meta = rig
        .session(&helper_session)
        .await
        .session_metadata()
        .unwrap();
    assert_eq!(meta.model, common::MODEL);
    let binding = meta.auth_binding.expect("the proxy's binding");
    assert_eq!(binding.realm.as_str(), ENDPOINT_REALM);
    assert_eq!(binding.binding.as_str(), "proxy");
    rig.quit().await;
}
