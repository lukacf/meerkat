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
    HookSourceTrust, HostConfinement, LlmDoneOutcome, LlmEvent, LlmRequest,
};
use meerkat_client::LlmClient;
use meerkat_core::confinement::{
    ConfinementRefusal, ConfinementSpec, ExecutionConfinement, FilesystemAccess, IpNetworkAccess,
    PathAccess, PlatformBaseline,
};
use meerkat_core::{
    AgentEvent, AgentToolDispatcher, Config, ConfinementConfig, CustomConfinement,
    HookAdapterConfig, HookCapability, HookEntryConfig, HookFailureReason, HookId, HookPoint,
    HookRunOverrides, LaunchConfinement, Message, Provider, ToolCallView, ToolDef,
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

/// A host's trust record: the sources the host checked, each as the path and
/// the digest of the content it judged. A source is trusted only when both
/// match a record, so any other source, an unknown path included, is
/// untrusted. Every question the factory asks is kept.
struct RecordedTrust {
    records: Vec<(PathBuf, HookSourceDigest)>,
    asked: Mutex<Vec<(PathBuf, HookSourceDigest)>>,
}

impl RecordedTrust {
    fn new(records: impl IntoIterator<Item = (PathBuf, HookSourceDigest)>) -> Arc<Self> {
        Arc::new(Self {
            records: records.into_iter().collect(),
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
        if self
            .records
            .iter()
            .any(|(path, digest)| path == source.path && *digest == source.sha256)
        {
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
    write_hooks(&project_file(project), entries);
}

/// Writes the convention `config.toml` at `file` stating `entries` as its
/// hooks.
fn write_hooks(file: &Path, entries: Vec<HookEntryConfig>) {
    #[derive(serde::Serialize)]
    struct ProjectConfig {
        hooks: meerkat_core::HooksConfig,
    }
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(
        file,
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
    /// The run's own result: its final text, or the error that ended it.
    outcome: Result<String, String>,
}

/// A factory over `project`, with a user config root, a trust and host
/// confinement when they are given.
fn factory_for(
    base: &Path,
    project: &Path,
    user_root: Option<&Path>,
    trust: Option<Arc<dyn HookSourceTrust>>,
    host: Option<&HostConfinement>,
) -> AgentFactory {
    let mut factory = AgentFactory::new(base.join("sessions"))
        .session_store(Arc::new(meerkat_store::MemoryStore::new()))
        .runtime_root(base.join("runtime"))
        .project_root(project)
        .builtins(false)
        .shell(false);
    if let Some(user_root) = user_root {
        factory = factory.user_config_root(user_root);
    }
    if let Some(trust) = trust {
        factory = factory.with_hook_source_trust(trust);
    }
    if let Some(host) = host {
        factory = factory.with_host_confinement(host);
    }
    factory
}

/// Builds an agent from `factory`, with `hooks_override` as its run
/// overrides, and runs one turn that calls the guarded tool.
async fn run_on(factory: &AgentFactory, hooks_override: HookRunOverrides) -> Run {
    let client = Arc::new(ScriptedClient::default());
    let tool = Arc::new(GuardedTool::default());
    let (tx, mut rx) = tokio::sync::mpsc::channel(256);
    let mut build = AgentBuildConfig::new("claude-sonnet-4-5");
    build.llm_client_override = Some(client.clone());
    build.external_tools = Some(tool.clone());
    build.event_tx = Some(tx);
    build.hooks_override = hooks_override;
    let mut agent = tokio::time::timeout(
        Duration::from_secs(15),
        factory.build_agent(build, &Config::default()),
    )
    .await
    .unwrap()
    .unwrap();
    let outcome = tokio::time::timeout(
        Duration::from_secs(15),
        agent.run("Attempt the guarded operation".to_string().into()),
    )
    .await
    .unwrap()
    .map(|result| result.text)
    .map_err(|error| error.to_string());
    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }
    Run {
        client,
        tool,
        events,
        outcome,
    }
}

/// Builds an agent over `project` (with `trust` installed, if any) and runs
/// one turn that calls the guarded tool.
async fn build_and_run(base: &Path, project: &Path, trust: Option<Arc<RecordedTrust>>) -> Run {
    build_and_run_with(
        base,
        project,
        None,
        trust.map(|trust| trust as Arc<dyn HookSourceTrust>),
    )
    .await
}

/// [`build_and_run`] with a user config root (if any) and any trust.
async fn build_and_run_with(
    base: &Path,
    project: &Path,
    user_root: Option<&Path>,
    trust: Option<Arc<dyn HookSourceTrust>>,
) -> Run {
    build_and_run_with_host(base, project, user_root, trust, None).await
}

/// [`build_and_run_with`], with host confinement installed when `host` is
/// given. The run completes.
async fn build_and_run_with_host(
    base: &Path,
    project: &Path,
    user_root: Option<&Path>,
    trust: Option<Arc<dyn HookSourceTrust>>,
    host: Option<&HostConfinement>,
) -> Run {
    let run = run_on(
        &factory_for(base, project, user_root, trust, host),
        HookRunOverrides::default(),
    )
    .await;
    assert_eq!(run.outcome.as_deref(), Ok("continued"));
    run
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
    let trust = RecordedTrust::new([(project_file(&project), host_digest(&project))]);

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
    let trust = RecordedTrust::new([(project_file(&project), host_digest(&project))]);

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
    let trust = RecordedTrust::new([(project_file(&project), HookSourceDigest::of(b"[hooks]\n"))]);

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

/// Counts the factory's questions per file and trusts only `trusted`, so
/// the project file, and any other path, is untrusted.
struct CountingTrust {
    trusted: Option<PathBuf>,
    asked: Mutex<HashMap<PathBuf, usize>>,
}

impl HookSourceTrust for CountingTrust {
    fn decide(&self, source: HookSource<'_>) -> HookSourceDecision {
        *self
            .asked
            .lock()
            .unwrap()
            .entry(source.path.to_path_buf())
            .or_default() += 1;
        if self.trusted.as_deref() == Some(source.path) {
            HookSourceDecision::Trusted
        } else {
            HookSourceDecision::Untrusted
        }
    }
}

/// Each convention file is decided once per build, however many hooks it
/// states, and no hook of the untrusted project file runs.
#[tokio::test]
async fn each_hook_source_is_decided_once_per_build() {
    let (_temp, base, project) = fixture();
    let user_root = base.join("user");
    let user_file = user_root.join(".rkat").join("config.toml");
    let project_markers: Vec<_> = (0..3)
        .map(|n| base.join(format!("project-hook-{n}-entered")))
        .collect();
    write_project_hooks(
        &project,
        project_markers
            .iter()
            .enumerate()
            .map(|(n, marker)| marker_guardrail(&format!("project-{n}"), marker))
            .collect(),
    );
    write_hooks(
        &user_file,
        (0..2)
            .map(|n| {
                marker_guardrail(
                    &format!("user-{n}"),
                    &base.join(format!("user-hook-{n}-entered")),
                )
            })
            .collect(),
    );
    let trust = Arc::new(CountingTrust {
        trusted: Some(user_file.clone()),
        asked: Mutex::new(HashMap::new()),
    });

    let run = build_and_run_with(
        &base,
        &project,
        Some(&user_root),
        Some(Arc::clone(&trust) as Arc<dyn HookSourceTrust>),
    )
    .await;

    assert_eq!(
        *trust.asked.lock().unwrap(),
        HashMap::from([(project_file(&project), 1), (user_file, 1)]),
        "each file is decided exactly once"
    );
    assert!(project_markers.iter().all(|marker| !marker.exists()));
    assert_eq!(run.tool.0.load(Ordering::SeqCst), 0, "its tool is blocked");
    // The first refused guardrail ends the dispatch, so only it reports;
    // the sdk unit test `untrusted_entries_decides_each_file_once` pins
    // that all three are in the refused set.
    let refused = refused_hooks(&run.events);
    assert!(!refused.is_empty());
    assert!(
        refused.iter().all(|(id, _)| id.0.starts_with("project-")),
        "only project hooks are refused: {refused:?}"
    );
}

/// A guarded host whose confined shell could write the project file.
#[cfg(unix)]
fn writing_host(writable: &Path) -> HostConfinement {
    let requirement: ExecutionConfinement = ConfinementSpec {
        baseline: PlatformBaseline::CommandRuntimeV1,
        read: FilesystemAccess::Paths(vec![PathAccess::Subtree(writable.into())]),
        write: FilesystemAccess::Paths(vec![PathAccess::Subtree(writable.into())]),
        deny_read: vec![],
        deny_write: vec![],
        network: IpNetworkAccess::Denied,
        unix_connect: vec![],
        require_descendant_termination: false,
    }
    .try_into()
    .unwrap();
    HostConfinement::resolve(
        &ConfinementConfig::for_all(LaunchConfinement::Custom(CustomConfinement {
            working_directory: writable.to_path_buf(),
            requirement,
        })),
        None,
    )
    .unwrap()
}

/// With host confinement installed beside the trust, a file confinement
/// refuses on its own is still decided by the trust, once: both policies
/// see every source the build read, so an auditing or stateful trust
/// observes each file whatever confinement said.
#[cfg(unix)]
#[tokio::test]
async fn confinement_refusing_a_file_does_not_skip_the_trust() {
    let (_temp, base, project) = fixture();
    let marker = base.join("project-hook-entered");
    write_project_hooks(&project, vec![marker_guardrail("project-0", &marker)]);
    let host = writing_host(&project);
    assert!(
        host.untrusted_source(&project_file(&project)),
        "confinement refuses the project file on its own"
    );
    let trust = Arc::new(CountingTrust {
        trusted: None,
        asked: Mutex::new(HashMap::new()),
    });

    let run = build_and_run_with_host(
        &base,
        &project,
        None,
        Some(Arc::clone(&trust) as Arc<dyn HookSourceTrust>),
        Some(&host),
    )
    .await;

    assert_eq!(
        *trust.asked.lock().unwrap(),
        HashMap::from([(project_file(&project), 1)]),
        "the trust is asked once although confinement already refused the file"
    );
    assert!(!marker.exists(), "the refused hook never ran");
    assert_eq!(run.tool.0.load(Ordering::SeqCst), 0, "its tool is blocked");
    assert_eq!(
        refused_hooks(&run.events),
        vec![(
            HookId::new("project-0"),
            HookFailureReason::ConfinementRefused {
                refusal: ConfinementRefusal::UntrustedConfigurationSource
            }
        )]
    );
}

fn untrusted_source() -> HookFailureReason {
    HookFailureReason::ConfinementRefused {
        refusal: ConfinementRefusal::UntrustedConfigurationSource,
    }
}

/// A run override is the caller's own input, which the source trust does
/// not judge: it may disable an untrusted project entry and supply its own
/// entry under the same id, and that entry runs. The untrusted entry never
/// does.
#[tokio::test]
async fn a_run_override_replacing_an_untrusted_entry_runs() {
    let (_temp, base, project) = fixture();
    let planted = base.join("planted-hook-entered");
    let replacement = base.join("replacement-hook-entered");
    write_project_hooks(&project, vec![marker_guardrail("shared", &planted)]);
    let trust = RecordedTrust::new([]);
    let factory = factory_for(
        &base,
        &project,
        None,
        Some(Arc::clone(&trust) as Arc<dyn HookSourceTrust>),
        None,
    );

    let run = run_on(
        &factory,
        HookRunOverrides {
            entries: vec![marker_guardrail("shared", &replacement)],
            disable: vec![HookId::new("shared")],
        },
    )
    .await;

    assert_eq!(run.outcome.as_deref(), Ok("continued"));
    assert_eq!(trust.asked().len(), 1, "the project file is decided");
    assert!(
        refused_hooks(&run.events).is_empty(),
        "the replacement is not refused: {:?}",
        refused_hooks(&run.events)
    );
    assert!(replacement.exists(), "the caller's replacement ran");
    assert!(!planted.exists(), "the untrusted entry never ran");
    assert_eq!(
        run.tool.0.load(Ordering::SeqCst),
        1,
        "the replacement let the tool run"
    );
}

/// The untrusted entry never runs, whatever the run overrides do: with none
/// it is refused; beside an unrelated override entry it is still refused;
/// disabled, it does not run; an override entry of its id that does not
/// disable it is the typed duplicate-id configuration error. A trusted user
/// file's entry runs wherever the run proceeds.
#[tokio::test]
async fn an_untrusted_entry_never_runs_whatever_the_run_overrides() {
    for case in ["none", "unrelated", "disabled", "duplicate"] {
        let (_temp, base, project) = fixture();
        let user_root = base.join("user");
        let user_file = user_root.join(".rkat").join("config.toml");
        let planted = base.join("planted-hook-entered");
        let user_marker = base.join("user-hook-entered");
        let other = base.join("other-hook-entered");
        write_project_hooks(&project, vec![marker_guardrail("shared", &planted)]);
        write_hooks(
            &user_file,
            vec![HookEntryConfig {
                priority: -1,
                ..marker_guardrail("user-own", &user_marker)
            }],
        );
        let trust = RecordedTrust::new([(
            user_file.clone(),
            HookSourceDigest::of(&std::fs::read(&user_file).unwrap()),
        )]);
        let factory = factory_for(
            &base,
            &project,
            Some(&user_root),
            Some(trust as Arc<dyn HookSourceTrust>),
            None,
        );
        let overrides = match case {
            "none" => HookRunOverrides::default(),
            "unrelated" => HookRunOverrides {
                entries: vec![marker_guardrail("other", &other)],
                disable: Vec::new(),
            },
            "disabled" => HookRunOverrides {
                entries: Vec::new(),
                disable: vec![HookId::new("shared")],
            },
            _ => HookRunOverrides {
                entries: vec![marker_guardrail("shared", &other)],
                disable: Vec::new(),
            },
        };

        let run = run_on(&factory, overrides).await;

        assert!(!planted.exists(), "{case}: the untrusted entry never ran");
        match case {
            "none" | "unrelated" => {
                assert_eq!(run.outcome.as_deref(), Ok("continued"), "{case}");
                assert_eq!(
                    refused_hooks(&run.events),
                    vec![(HookId::new("shared"), untrusted_source())],
                    "{case}"
                );
                assert_eq!(
                    run.tool.0.load(Ordering::SeqCst),
                    0,
                    "{case}: its tool stays blocked"
                );
                assert!(user_marker.exists(), "{case}: the trusted entry ran");
            }
            "disabled" => {
                assert_eq!(run.outcome.as_deref(), Ok("continued"), "{case}");
                assert!(refused_hooks(&run.events).is_empty(), "{case}");
                assert_eq!(run.tool.0.load(Ordering::SeqCst), 1, "{case}");
                assert!(user_marker.exists(), "{case}: the trusted entry ran");
            }
            _ => {
                let error = run
                    .outcome
                    .expect_err("an override of the untrusted entry's id that keeps it is refused");
                assert!(error.contains("duplicate hook id"), "{case}: {error}");
                assert!(!other.exists(), "{case}");
                assert_eq!(run.tool.0.load(Ordering::SeqCst), 0, "{case}");
            }
        }
    }
}
