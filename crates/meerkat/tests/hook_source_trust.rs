// These factories keep sessions in meerkat-store's in-memory store.
#![cfg(all(unix, feature = "memory-store"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! The host's trust over the convention hook sources binds the content the
//! factory consumes: the factory asks the host about the exact bytes it read
//! and parsed, so a hook written into the project `.rkat/config.toml` after
//! the host checked the file is refused before it can run.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use meerkat::{
    AgentBuildConfig, AgentFactory, HookSource, HookSourceDecision, HookSourceDigest,
    HookSourceTrust, LlmDoneOutcome, LlmEvent, LlmRequest,
};
use meerkat_client::LlmClient;
use meerkat_core::confinement::ConfinementRefusal;
use meerkat_core::{
    AgentEvent, AgentToolDispatcher, Config, HookAdapterConfig, HookCapability, HookEntryConfig,
    HookFailureReason, HookId, HookPoint, Message, Provider, ToolCallView, ToolDef,
    ToolDispatchOutcome, ToolError, ToolResult,
};
use serde_json::json;

/// Calls `guarded_tool` once, then ends the run with "continued".
#[derive(Default)]
struct ScriptedClient {
    requests: Mutex<Vec<Vec<Message>>>,
}

impl ScriptedClient {
    /// The tool results the model received on its second request.
    fn returned_results(&self) -> Vec<(bool, String)> {
        let requests = self.requests.lock().unwrap();
        assert_eq!(requests.len(), 2, "the run reaches the next model turn");
        requests[1]
            .iter()
            .filter_map(|message| match message {
                Message::ToolResults { results, .. } => Some(results.as_slice()),
                _ => None,
            })
            .flatten()
            .map(|result| (result.is_error, result.text_content()))
            .collect()
    }
}

#[async_trait::async_trait]
impl LlmClient for ScriptedClient {
    fn project_replay_messages(
        &self,
        messages: &[Message],
    ) -> Result<Vec<Message>, meerkat_client::LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(&'a self, request: &'a LlmRequest) -> meerkat_llm_core::LlmStream<'a> {
        let first = {
            let mut requests = self.requests.lock().unwrap();
            let first = requests.is_empty();
            requests.push(request.messages.clone());
            first
        };
        let (event, stop_reason) = if first {
            (
                LlmEvent::ToolCallComplete {
                    id: "guarded-call".into(),
                    name: "guarded_tool".into(),
                    args: json!({}),
                    meta: None,
                },
                meerkat_core::StopReason::ToolUse,
            )
        } else {
            (
                LlmEvent::TextDelta {
                    delta: "continued".into(),
                    meta: None,
                },
                meerkat_core::StopReason::EndTurn,
            )
        };
        Box::pin(futures::stream::iter(vec![
            Ok(event),
            Ok(LlmEvent::UsageUpdate {
                usage: meerkat_core::TurnUsage::host_declared(
                    Provider::Other,
                    &request.model,
                    meerkat_core::Usage::default(),
                ),
            }),
            Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::success(stop_reason),
            }),
        ]))
    }

    fn provider(&self) -> Provider {
        Provider::Other
    }

    async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
        Ok(())
    }
}

#[derive(Default)]
struct GuardedTool(AtomicUsize);

#[async_trait::async_trait]
impl AgentToolDispatcher for GuardedTool {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        vec![Arc::new(ToolDef::new(
            "guarded_tool",
            "A tool a PreTool hook guards",
            json!({"type":"object"}),
        ))]
        .into()
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(ToolResult::new(call.id.into(), "tool completed".into(), false).into())
    }
}

/// A host's trust record: the project config is trusted when the digest it
/// recorded is the content's (rkat-tui's `HostTrustRecord` shape); any other
/// source is the user's own. Every question the factory asks is kept.
struct RecordedTrust {
    project_file: PathBuf,
    recorded: HookSourceDigest,
    asked: Mutex<Vec<(PathBuf, HookSourceDigest)>>,
}

impl RecordedTrust {
    fn new(project_file: PathBuf, recorded: HookSourceDigest) -> Arc<Self> {
        Arc::new(Self {
            project_file,
            recorded,
            asked: Mutex::new(Vec::new()),
        })
    }

    fn asked(&self) -> Vec<(PathBuf, HookSourceDigest)> {
        self.asked.lock().unwrap().clone()
    }
}

impl HookSourceTrust for RecordedTrust {
    fn decide(&self, source: HookSource<'_>) -> HookSourceDecision {
        self.asked
            .lock()
            .unwrap()
            .push((source.path.to_path_buf(), source.sha256));
        if source.path != self.project_file || source.sha256 == self.recorded {
            HookSourceDecision::Trusted
        } else {
            HookSourceDecision::Untrusted
        }
    }
}

fn project_file(project: &Path) -> PathBuf {
    project.join(".rkat").join("config.toml")
}

/// Writes the project `config.toml` stating `entries` as its hooks.
fn write_project_hooks(project: &Path, entries: Vec<HookEntryConfig>) {
    #[derive(serde::Serialize)]
    struct ProjectConfig {
        hooks: meerkat_core::HooksConfig,
    }
    std::fs::create_dir_all(project.join(".rkat")).unwrap();
    std::fs::write(
        project_file(project),
        toml::to_string(&ProjectConfig {
            hooks: meerkat_core::HooksConfig {
                entries,
                ..Default::default()
            },
        })
        .unwrap(),
    )
    .unwrap();
}

/// What the host computes over the file when it checks it.
fn host_digest(project: &Path) -> HookSourceDigest {
    HookSourceDigest::of(&std::fs::read(project_file(project)).unwrap())
}

/// A PreTool command guardrail whose only effect, when it runs, is to create
/// `marker`.
fn marker_guardrail(id: &str, marker: &Path) -> HookEntryConfig {
    HookEntryConfig {
        id: HookId::new(id),
        point: HookPoint::PreToolExecution,
        capability: HookCapability::Guardrail,
        timeout_ms: Some(5_000),
        runtime: HookAdapterConfig::command(
            "/bin/sh",
            vec![
                "-c".into(),
                "printf entered > \"$1\"; cat >/dev/null; printf '{}'".into(),
                "hook-probe".into(),
                marker.display().to_string(),
            ],
            HashMap::new(),
        ),
        ..Default::default()
    }
}

struct Run {
    client: Arc<ScriptedClient>,
    tool: Arc<GuardedTool>,
    events: Vec<AgentEvent>,
}

/// Builds an agent over `project` (with `trust` installed, if any) and runs
/// one turn that calls the guarded tool.
async fn build_and_run(base: &Path, project: &Path, trust: Option<Arc<RecordedTrust>>) -> Run {
    let client = Arc::new(ScriptedClient::default());
    let tool = Arc::new(GuardedTool::default());
    let (tx, mut rx) = tokio::sync::mpsc::channel(256);
    let mut build = AgentBuildConfig::new("claude-sonnet-4-5");
    build.llm_client_override = Some(client.clone());
    build.external_tools = Some(tool.clone());
    build.event_tx = Some(tx);
    let mut factory = AgentFactory::new(base.join("sessions"))
        .session_store(Arc::new(meerkat_store::MemoryStore::new()))
        .runtime_root(base.join("runtime"))
        .project_root(project)
        .builtins(false)
        .shell(false);
    if let Some(trust) = trust {
        factory = factory.with_hook_source_trust(trust);
    }
    let mut agent = tokio::time::timeout(
        Duration::from_secs(15),
        factory.build_agent(build, &Config::default()),
    )
    .await
    .unwrap()
    .unwrap();
    let result = tokio::time::timeout(
        Duration::from_secs(15),
        agent.run("Attempt the guarded operation".to_string().into()),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(result.text, "continued");
    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }
    Run {
        client,
        tool,
        events,
    }
}

fn refused_hooks(events: &[AgentEvent]) -> Vec<(HookId, HookFailureReason)> {
    events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::HookLaunchRefused {
                hook_id, reason, ..
            } => Some((hook_id.clone(), reason.clone())),
            _ => None,
        })
        .collect()
}

fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().canonicalize().unwrap();
    let project = base.join("project");
    (temp, base, project)
}

/// The reproducer: the host checks the project config (no hooks) and
/// records its digest; a hook is written into the file before the factory
/// reads it. The factory asks the host about the bytes it read, the host
/// does not trust them, and the added hook is refused: it never runs and
/// its guarded tool is blocked.
#[tokio::test]
async fn a_hook_written_after_the_hosts_check_never_runs() {
    let (_temp, base, project) = fixture();
    let marker = base.join("planted-hook-entered");
    write_project_hooks(&project, vec![]);
    let trust = RecordedTrust::new(project_file(&project), host_digest(&project));

    // The interval between the host's check and the factory's read.
    write_project_hooks(&project, vec![marker_guardrail("planted", &marker)]);
    let consumed = host_digest(&project);

    let run = build_and_run(&base, &project, Some(Arc::clone(&trust))).await;

    assert!(!marker.exists(), "the planted hook never ran");
    assert_eq!(run.tool.0.load(Ordering::SeqCst), 0, "its tool is blocked");
    assert_eq!(
        trust.asked(),
        vec![(project_file(&project), consumed)],
        "the host is asked about the content the factory read"
    );
    let results = run.client.returned_results();
    assert_eq!(results.len(), 1);
    assert!(results[0].0);
    let body: serde_json::Value = serde_json::from_str(&results[0].1).unwrap();
    assert_eq!(body["error"], "confinement_refused", "{}", results[0].1);
    assert_eq!(
        body["data"]["refusal"],
        json!(ConfinementRefusal::UntrustedConfigurationSource)
    );
    assert_eq!(
        refused_hooks(&run.events),
        vec![(
            HookId::new("planted"),
            HookFailureReason::ConfinementRefused {
                refusal: ConfinementRefusal::UntrustedConfigurationSource
            }
        )]
    );
}

/// Content the host trusted, read unchanged: its hooks run as before.
#[tokio::test]
async fn a_hook_whose_content_the_host_trusted_runs() {
    let (_temp, base, project) = fixture();
    let marker = base.join("trusted-hook-entered");
    write_project_hooks(&project, vec![marker_guardrail("trusted", &marker)]);
    let trust = RecordedTrust::new(project_file(&project), host_digest(&project));

    let run = build_and_run(&base, &project, Some(Arc::clone(&trust))).await;

    assert!(marker.exists(), "the trusted hook ran");
    assert_eq!(run.tool.0.load(Ordering::SeqCst), 1);
    assert!(refused_hooks(&run.events).is_empty());
    assert_eq!(trust.asked().len(), 1);
}

/// A digest recorded for a project config that is absent when the factory
/// reads: the file states no hooks, the host is not asked, and the build
/// runs without hooks.
#[tokio::test]
async fn an_absent_hook_source_is_not_asked_about() {
    let (_temp, base, project) = fixture();
    std::fs::create_dir_all(&project).unwrap();
    let trust = RecordedTrust::new(project_file(&project), HookSourceDigest::of(b"[hooks]\n"));

    let run = build_and_run(&base, &project, Some(Arc::clone(&trust))).await;

    assert!(trust.asked().is_empty());
    assert_eq!(run.tool.0.load(Ordering::SeqCst), 1);
    assert!(refused_hooks(&run.events).is_empty());
}

/// Without a trust installed nothing changes: the project config's hooks run
/// as the host, including one written after a check the host made on its
/// own before the build (the defect a pre-build check alone leaves open).
#[tokio::test]
async fn without_a_trust_the_project_hooks_run_as_before() {
    let (_temp, base, project) = fixture();
    let marker = base.join("unguarded-hook-entered");
    write_project_hooks(&project, vec![]);
    let _checked_before_the_build = host_digest(&project);
    write_project_hooks(&project, vec![marker_guardrail("planted", &marker)]);

    let run = build_and_run(&base, &project, None).await;

    assert!(marker.exists(), "the project hook ran");
    assert_eq!(run.tool.0.load(Ordering::SeqCst), 1);
    assert!(refused_hooks(&run.events).is_empty());
}
