//! The shell under `--confinement`, end to end through the real
//! runtime-backed service with a scripted LLM: the workspace mode lets a
//! command write the workspace but not outside it and keeps the network;
//! the strict mode denies IP network access. An interrupted shell call runs
//! in durable process custody and leaves no process behind.
//!
//! The confined cases need the platform backend (Linux: unprivileged user
//! namespaces; macOS: Seatbelt). Each case first probes the backend directly
//! (`meerkat_sandbox` compiles the mode's requirement and launches a
//! confined `/bin/true`); where that fails, as in containers or on Ubuntu
//! without unprivileged user namespaces, the case prints a `SKIPPED:` line
//! with the reason and returns. Apart from the markers the commands echo,
//! nothing is decided from a tool's output text.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::collections::BTreeMap;
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
use meerkat_tui::bootstrap::confinement::{HostPaths, lower};
use meerkat_tui::cli::ConfinementArg;
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

/// Why this host cannot run a launch confined by `mode` over `workspace`,
/// probed directly: the requirement is compiled by the sandbox backend and a
/// confined `/bin/true` must exit successfully. `None` when it can.
#[cfg(any(target_os = "linux", target_os = "macos"))]
async fn sandbox_unavailable(workspace: &Path, mode: ConfinementArg) -> Option<String> {
    let requirement = match lower(mode, workspace, &HostPaths::detect()) {
        Ok(Some(requirement)) => requirement,
        Ok(None) => return None,
        Err(err) => return Some(format!("the requirement is invalid: {err}")),
    };
    let launch = match meerkat_sandbox::ProcessLaunchSpec::new(
        "/bin/true".into(),
        Vec::new(),
        workspace.to_path_buf(),
        BTreeMap::new(),
    ) {
        Ok(launch) => launch,
        Err(refusal) => return Some(format!("the probe launch is invalid: {refusal}")),
    };
    let prepared = match meerkat_sandbox::prepare(&requirement, launch) {
        Ok(prepared) => prepared,
        Err(refusal) => return Some(format!("the backend refuses the requirement: {refusal}")),
    };
    let mut child = match prepared.spawn() {
        Ok(child) => child,
        Err(err) => return Some(format!("a confined launch cannot start: {err}")),
    };
    match child.wait().await {
        Ok(status) if status.success() => None,
        Ok(status) => Some(format!("a confined /bin/true exited with {status}")),
        Err(err) => Some(format!("a confined launch cannot be awaited: {err}")),
    }
}

/// No sandbox backend on this platform.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
async fn sandbox_unavailable(_workspace: &Path, _mode: ConfinementArg) -> Option<String> {
    Some("this platform has no sandbox backend".to_string())
}

/// Start a rig whose shell is allowed without review, under `mode`; `None`
/// (after printing a `SKIPPED:` line) when this host cannot run the mode.
async fn confined_rig(ws: &Workspace, llm: &Arc<ScriptedLlmClient>, mode: &str) -> Option<Rig> {
    let arg = match mode {
        "strict" => ConfinementArg::Strict,
        _ => ConfinementArg::Workspace,
    };
    if let Some(reason) = sandbox_unavailable(&ws.root, arg).await {
        println!("SKIPPED: --confinement={mode} is unavailable on this host: {reason}");
        return None;
    }
    let options = allow_shell(vec![format!("--confinement={mode}")]);
    match Rig::try_start_opts(&ws.root, llm, Start::New, options).await {
        Ok(rig) => Some(rig),
        Err(err @ TuiError::Confinement(_)) => {
            panic!("the probe launched confined, but bootstrap refused the mode: {err}")
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

/// A confined command gets rkat-tui's curated environment: `HOME` and
/// `PATH` as rkat-tui has them (so `~/.cargo/bin` tools resolve), and none
/// of the rest (the test runner's `CARGO_MANIFEST_DIR` stands in for a
/// provider API key: set in rkat-tui's environment, not on the allowlist).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn workspace_mode_passes_the_curated_environment() {
    let ws = Workspace::new();
    let command = "echo \"HOME=[$HOME]\"; echo \"PATH=[$PATH]\"; echo \"OUTSIDE=[${CARGO_MANIFEST_DIR:-unset}]\"; echo END";
    let Some(output) = run_confined(&ws, "workspace", command).await else {
        return;
    };
    assert!(output.contains("END"), "{output}");
    if let Ok(home) = std::env::var("HOME") {
        assert!(output.contains(&format!("HOME=[{home}]")), "{output}");
    }
    if let Ok(path) = std::env::var("PATH") {
        assert!(output.contains(&format!("PATH=[{path}]")), "{output}");
    }
    if std::env::var_os("CARGO_MANIFEST_DIR").is_some() {
        assert!(output.contains("OUTSIDE=[unset]"), "{output}");
    }
}

/// Name resolution inside workspace mode, with this host's real system
/// paths (`HostPaths::detect`): `localhost` resolves from the granted
/// `/etc/hosts`, and a DNS name resolves through the stub resolver named in
/// a granted `/etc/resolv.conf`, a regular file or a symbolic link
/// (systemd-resolved: the sandbox binds the link's target at
/// `/etc/resolv.conf`). The DNS half prints `SKIPPED:` only when the host
/// itself cannot resolve the name (no network).
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn workspace_mode_resolves_localhost_and_dns_names() {
    use std::net::ToSocketAddrs as _;
    let ws = Workspace::new();
    if let Some(reason) = sandbox_unavailable(&ws.root, ConfinementArg::Workspace).await {
        println!("SKIPPED: --confinement=workspace is unavailable on this host: {reason}");
        return;
    }
    let host = HostPaths::detect();
    let dns_skip = if !Path::new("/etc/resolv.conf").is_file() {
        Some("this host has no /etc/resolv.conf".to_string())
    } else if ("example.com", 80).to_socket_addrs().is_err() {
        Some("the host itself cannot resolve example.com".to_string())
    } else {
        None
    };
    let llm = ScriptedLlmClient::new(vec![
        Step::Tool {
            id: "sh",
            name: "shell",
            args: json!({
                "command": "getent hosts localhost && echo LOCAL-OK; getent hosts example.com && echo DNS-OK; echo END"
            }),
        },
        Step::Text("done"),
    ]);
    let options = RigOptions {
        host_paths: Some(host),
        ..allow_shell(vec!["--confinement=workspace".to_string()])
    };
    let mut rig = Rig::try_start_opts(&ws.root, &llm, Start::New, options)
        .await
        .unwrap_or_else(|err| panic!("bootstrap: {err}"));
    rig.send(Effect::StartTurn("resolve".into()));
    rig.turn_finished().await;
    let output = tool_text(&llm.requests()[1], "sh");
    rig.quit().await;
    assert!(output.contains("END"), "{output}");
    assert!(
        output.contains("LOCAL-OK"),
        "localhost did not resolve: {output}"
    );
    match dns_skip {
        Some(reason) => println!("SKIPPED: DNS inside the sandbox: {reason}"),
        None => assert!(
            output.contains("DNS-OK"),
            "example.com did not resolve: {output}"
        ),
    }
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_confinement_below_a_realm_policy_is_a_typed_startup_error() {
    let ws = Workspace::new();
    let realm = meerkat_tui::bootstrap::resolve_scope(&common::args_for(&ws.root))
        .unwrap()
        .realm_root();
    std::fs::create_dir_all(&realm).unwrap();
    std::fs::write(
        realm.join("config.toml"),
        "[confinement.shell]\nmode = \"workspace\"\nnetwork = \"unrestricted\"\n",
    )
    .unwrap();
    let llm = ScriptedLlmClient::new(Vec::new());
    // `--confinement=off`, the same as `--no-confinement`.
    let options = allow_shell(vec!["--confinement=off".to_string()]);
    let Err(err) = Rig::try_start_opts(&ws.root, &llm, Start::New, options).await else {
        panic!("--no-confinement started below the realm's policy");
    };
    assert!(
        matches!(
            &err,
            TuiError::Confinement(meerkat_tui::ConfinementError::BelowRealmFloor { policy, .. })
                if policy.contains("workspace preset")
        ),
        "{err}"
    );
}
