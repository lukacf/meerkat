//! The shell under `--confinement`, end to end through the real
//! runtime-backed service with a scripted LLM: the workspace mode lets a
//! command write the workspace but not outside it and keeps the network;
//! the strict mode denies IP network access. An interrupted shell call runs
//! in durable process custody and leaves no process behind.
//!
//! The confined cases need the platform backend (Linux: unprivileged user
//! namespaces; macOS: Seatbelt). Where it is unavailable, as in containers
//! without user namespaces, bootstrap refuses the mode and the case is
//! skipped with the reason printed.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use common::{Rig, RigOptions, ScriptedLlmClient, Step, Workspace, within};
use meerkat_core::{AgentEvent, Message};
use meerkat_tui::TuiError;
use meerkat_tui::approval::classifier::{
    ActionClassifier, ClassifierError, ClassifierInput, ClassifierVerdict,
};
use meerkat_tui::bootstrap::ApprovalOverrides;
use meerkat_tui::runner::Start;
use meerkat_tui::ui::Effect;
use serde_json::json;

/// Allows every reviewed call: these tests are about the sandbox under the
/// approval gate, not the gate.
struct AllowAll;

#[async_trait]
impl ActionClassifier for AllowAll {
    async fn classify(
        &self,
        _input: ClassifierInput<'_>,
    ) -> Result<ClassifierVerdict, ClassifierError> {
        Ok(ClassifierVerdict::Allow)
    }
}

/// Options that run the shell without review under `args`.
fn allow_shell(args: Vec<String>) -> RigOptions {
    RigOptions {
        args,
        approval: ApprovalOverrides {
            classifier: Some(Arc::new(AllowAll)),
            ..ApprovalOverrides::default()
        },
        ..RigOptions::default()
    }
}

fn leak(text: String) -> &'static str {
    Box::leak(text.into_boxed_str())
}

/// The text of tool result `id` in `messages`.
fn tool_text(messages: &[Message], id: &str) -> String {
    messages
        .iter()
        .find_map(|m| match m {
            Message::ToolResults { results, .. } => results
                .iter()
                .find(|r| r.tool_use_id == id)
                .map(meerkat_core::types::ToolResult::text_content),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no result for {id}"))
}

/// Start a rig whose shell is allowed without review, under `mode`; `None`
/// when this host cannot enforce the mode (the reason is printed).
async fn confined_rig(ws: &Workspace, llm: &Arc<ScriptedLlmClient>, mode: &str) -> Option<Rig> {
    let options = allow_shell(vec![format!("--confinement={mode}")]);
    match Rig::try_start_opts(&ws.root, llm, Start::New, options).await {
        Ok(rig) => Some(rig),
        Err(TuiError::Confinement(reason)) => {
            eprintln!("skipped: --confinement={mode} is unavailable on this host: {reason}");
            None
        }
        Err(other) => panic!("bootstrap: {other}"),
    }
}

/// Run one shell call `command` in a fresh confined session and return its
/// output; `None` when the mode is unavailable here.
async fn run_confined(ws: &Workspace, mode: &str, command: &'static str) -> Option<String> {
    let llm = ScriptedLlmClient::new(vec![
        Step::Tool {
            id: "sh",
            name: "shell",
            args: json!({ "command": command }),
        },
        Step::Text("done"),
    ]);
    let mut rig = confined_rig(ws, &llm, mode).await?;
    rig.send(Effect::StartTurn("run it".into()));
    rig.turn_finished().await;
    let text = tool_text(&llm.requests()[1], "sh");
    rig.quit().await;
    if text.contains("confinement_refused") || text.contains("confinement is unsupported") {
        eprintln!("skipped: the backend refused the confined launch: {text}");
        return None;
    }
    Some(text)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn workspace_mode_confines_writes_to_the_workspace() {
    let ws = Workspace::new();
    let outside = ws.sibling("outside");
    std::fs::create_dir_all(&outside).unwrap();
    let command = leak(format!(
        "echo in > inside.txt; echo out > {}/outside.txt; echo END",
        outside.display()
    ));
    let Some(output) = run_confined(&ws, "workspace", command).await else {
        return;
    };
    assert!(output.contains("END"), "{output}");
    assert!(ws.root.join("inside.txt").exists(), "{output}");
    assert!(
        !outside.join("outside.txt").exists(),
        "a confined command wrote outside the workspace: {output}"
    );
}

/// Accept connections on a loopback port until the test ends.
async fn listener() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            drop(socket);
        }
    });
    port
}

fn connect_command(port: u16) -> &'static str {
    leak(format!(
        "if bash -c 'exec 3<>/dev/tcp/127.0.0.1/{port}' 2>/dev/null; then echo NET-OPEN; else echo NET-DENIED; fi"
    ))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn strict_mode_denies_the_network_and_workspace_mode_allows_it() {
    let port = listener().await;
    let ws = Workspace::new();
    let Some(strict) = run_confined(&ws, "strict", connect_command(port)).await else {
        return;
    };
    assert!(strict.contains("NET-DENIED"), "{strict}");
    let ws = Workspace::new();
    let Some(open) = run_confined(&ws, "workspace", connect_command(port)).await else {
        return;
    };
    assert!(open.contains("NET-OPEN"), "{open}");
}

/// Whether process `pid` is alive (not reaped and not a zombie).
fn alive(pid: &str) -> bool {
    std::fs::read_to_string(Path::new("/proc").join(pid).join("stat"))
        .ok()
        .and_then(|stat| {
            stat.rsplit_once(')')
                .and_then(|(_, rest)| rest.split_whitespace().next().map(str::to_string))
        })
        .is_some_and(|state| state != "Z")
}

#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_interrupted_shell_call_leaves_custody_records_and_no_process() {
    let ws = Workspace::new();
    let pid_file = ws.root.join("pid");
    let command = leak(format!("echo $$ > {}; exec sleep 60", pid_file.display()));
    let llm = ScriptedLlmClient::new(vec![Step::Tool {
        id: "sleep",
        name: "shell",
        args: json!({ "command": command, "timeout_secs": 120 }),
    }]);
    let options = allow_shell(Vec::new());
    let mut rig = Rig::start_opts(&ws.root, &llm, Start::New, options).await;
    let session = rig.first_session.clone();
    rig.send(Effect::StartTurn("sleep".into()));
    let pid = within(async {
        loop {
            if let Ok(text) = std::fs::read_to_string(&pid_file)
                && !text.trim().is_empty()
            {
                return text.trim().to_string();
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    assert!(alive(&pid), "the shell process {pid} should be running");
    rig.send(Effect::Interrupt);
    let outcome = rig.turn_finished().await;
    assert!(outcome.result.is_err(), "{outcome:?}");
    within(async {
        while alive(&pid) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    // The call ran in the session's durable process custody.
    let custody = meerkat_tui::bootstrap::resolve_scope(&common::args_for(&ws.root))
        .unwrap()
        .realm_root()
        .join(meerkat_tools::builtin::shell::PROCESS_CUSTODY_DIR);
    assert!(custody.is_dir(), "{}", custody.display());
    let has_session_scope = walk(&custody)
        .iter()
        .any(|path| path.to_string_lossy().contains(&session.to_string()));
    assert!(has_session_scope, "no custody records for {session}");
    assert!(rig.events.lock().iter().any(|event| matches!(
        event,
        AgentEvent::ToolExecutionStarted { id, .. } if id == "sleep"
    )));
    rig.quit().await;
}

fn walk(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(next) = pending.pop() {
        for entry in std::fs::read_dir(&next).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path.clone());
            }
            out.push(path);
        }
    }
    out
}
