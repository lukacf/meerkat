// Most of these factories keep sessions in meerkat-store's in-memory store.
#![cfg(all(not(target_arch = "wasm32"), feature = "memory-store"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! A stock host's resolved OS process confinement installed on its factory:
//! every launch kind takes the host's requirement, a launch the backend
//! cannot confine is refused and never runs unconfined, and the default
//! leaves the factory as the trusted host.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use meerkat::{
    AgentBuildConfig, AgentFactory, HostConfinement, LlmDoneOutcome, LlmEvent, LlmRequest,
};
use meerkat_client::LlmClient;
use meerkat_core::confinement::{
    ConfinementRefusal, ConfinementSpec, ExecutionConfinement, FilesystemAccess, IpNetworkAccess,
    PathAccess, PlatformBaseline,
};
use meerkat_core::{
    AgentEvent, AgentToolDispatcher, Config, ConfinementConfig, CustomConfinement,
    HookAdapterConfig, HookCapability, HookEntryConfig, HookFailureReason, HookId, HookPoint,
    LaunchConfinement, Message, Provider, ToolCallView, ToolDef, ToolDispatchOutcome, ToolError,
    ToolResult,
};
use meerkat_tools::builtin::shell::ShellConfinement;
use serde_json::json;

/// Calls `tool` once with `args`, then ends the run with "continued".
struct ScriptedClient {
    tool: &'static str,
    args: serde_json::Value,
    requests: Mutex<Vec<Vec<Message>>>,
}

impl ScriptedClient {
    fn new(tool: &'static str, args: serde_json::Value) -> Self {
        Self {
            tool,
            args,
            requests: Mutex::new(Vec::new()),
        }
    }

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
            assert!(
                request.tools.iter().any(|tool| tool.name == self.tool),
                "the model sees {}",
                self.tool
            );
            (
                LlmEvent::ToolCallComplete {
                    id: "confined-call".into(),
                    name: self.tool.into(),
                    args: self.args.clone(),
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
            "A tool with a command-hook prerequisite",
            json!({"type":"object"}),
        ))]
        .into()
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(ToolResult::new(call.id.into(), "tool completed".into(), false).into())
    }
}

/// A requirement over `root` that no platform backend enforces: descendant
/// termination is unsupported everywhere, so every confined launch is
/// refused before its program runs, on any OS.
fn unenforceable(root: &Path) -> ExecutionConfinement {
    ConfinementSpec {
        baseline: PlatformBaseline::CommandRuntimeV1,
        read: FilesystemAccess::Paths(vec![PathAccess::Subtree(root.into())]),
        write: FilesystemAccess::Paths(vec![PathAccess::Subtree(root.into())]),
        deny_read: vec![],
        deny_write: vec![],
        network: IpNetworkAccess::Denied,
        unix_connect: vec![],
        require_descendant_termination: true,
    }
    .try_into()
    .unwrap()
}

/// The host confinement a `[confinement.all]` custom selection resolves to.
fn host_confinement(root: &Path) -> HostConfinement {
    HostConfinement::resolve(
        &ConfinementConfig::for_all(LaunchConfinement::Custom(CustomConfinement {
            working_directory: root.to_path_buf(),
            requirement: unenforceable(root),
        })),
        None,
    )
    .unwrap()
}

fn confined_factory(root: &Path) -> AgentFactory {
    AgentFactory::new(root.join("sessions"))
        .session_store(Arc::new(meerkat_store::MemoryStore::new()))
        .runtime_root(root.join("runtime"))
        .project_root(root)
        .builtins(false)
        .shell(true)
        .with_host_confinement(&host_confinement(root))
}

fn assert_confinement_refused(text: &str) {
    assert_refused_with(text, ConfinementRefusal::UnsupportedRequirement);
}

fn assert_refused_with(text: &str, refusal: ConfinementRefusal) {
    let body: serde_json::Value = serde_json::from_str(text)
        .unwrap_or_else(|error| panic!("typed refusal body ({error}): {text}"));
    assert_eq!(body["error"], "confinement_refused", "{text}");
    assert_eq!(body["data"]["refusal"], json!(refusal), "{text}");
}

/// A host whose every launch kind writes `writable` (and nothing protects
/// any part of it): the convention files under it are not trusted.
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

/// A project `config.toml` stating `entries` as its hooks.
fn write_project_hooks(project: &Path, entries: Vec<HookEntryConfig>) {
    #[derive(serde::Serialize)]
    struct ProjectConfig {
        hooks: meerkat_core::HooksConfig,
    }
    std::fs::create_dir_all(project.join(".rkat")).unwrap();
    std::fs::write(
        project.join(".rkat").join("config.toml"),
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

/// A PreTool command guardrail whose only effect, if it ever ran, is to
/// create `marker`.
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

#[test]
fn installs_the_requirement_for_every_launch_kind() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let factory = confined_factory(&root);
    assert_eq!(
        factory.shell_confinement(),
        &ShellConfinement::Required {
            requirement: unenforceable(&root)
        }
    );
    #[cfg(feature = "mcp")]
    assert_eq!(
        factory.mcp_stdio_launch_profile().capabilities().err(),
        Some(ConfinementRefusal::UnsupportedRequirement)
    );
}

#[test]
fn the_trusted_host_default_leaves_the_factory_unconfined() {
    let factory = AgentFactory::minimal().with_host_confinement(&HostConfinement::trusted_host());
    assert_eq!(factory.shell_confinement(), &ShellConfinement::TrustedHost);
    #[cfg(feature = "mcp")]
    assert!(matches!(
        factory.mcp_stdio_launch_profile().capabilities(),
        Ok(None)
    ));
}

#[test]
fn installing_the_trusted_host_never_downgrades_an_installed_requirement() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let factory = confined_factory(&root).with_host_confinement(&HostConfinement::trusted_host());
    assert_eq!(
        factory.shell_confinement(),
        &ShellConfinement::Required {
            requirement: unenforceable(&root)
        }
    );
    #[cfg(feature = "mcp")]
    assert_eq!(
        factory.mcp_stdio_launch_profile().capabilities().err(),
        Some(ConfinementRefusal::UnsupportedRequirement)
    );
}

#[cfg(unix)]
#[tokio::test]
async fn a_refused_shell_launch_never_runs_unconfined() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let marker = root.join("shell-entered");
    let client = Arc::new(ScriptedClient::new(
        "shell",
        json!({"command": format!("printf entered > '{}'", marker.display())}),
    ));
    let mut build = AgentBuildConfig::new("claude-sonnet-4-5");
    build.llm_client_override = Some(client.clone());
    let mut agent = tokio::time::timeout(
        Duration::from_secs(15),
        confined_factory(&root).build_agent(build, &Config::default()),
    )
    .await
    .unwrap()
    .unwrap();
    let result = tokio::time::timeout(
        Duration::from_secs(15),
        agent.run("Run the command".to_string().into()),
    )
    .await
    .unwrap()
    .unwrap();

    assert_eq!(result.text, "continued");
    let results = client.returned_results();
    assert_eq!(results.len(), 1);
    assert!(results[0].0, "the refused launch is an error result");
    assert_confinement_refused(&results[0].1);
    assert!(!marker.exists(), "the refused command never ran");
}

#[cfg(unix)]
#[tokio::test]
async fn a_refused_command_hook_launch_never_runs_unconfined() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let marker = root.join("hook-entered");
    let mut config = Config::default();
    config.hooks.entries.push(HookEntryConfig {
        id: HookId::new("confined-command-hook"),
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
    });
    let client = Arc::new(ScriptedClient::new("guarded_tool", json!({})));
    let tool = Arc::new(GuardedTool::default());
    let (tx, mut rx) = tokio::sync::mpsc::channel(256);
    let mut build = AgentBuildConfig::new("claude-sonnet-4-5");
    build.llm_client_override = Some(client.clone());
    build.external_tools = Some(tool.clone());
    build.event_tx = Some(tx);
    let factory = confined_factory(&root).shell(false);
    let mut agent =
        tokio::time::timeout(Duration::from_secs(15), factory.build_agent(build, &config))
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
    assert_eq!(
        tool.0.load(Ordering::SeqCst),
        0,
        "the refused hook blocks its tool"
    );
    let results = client.returned_results();
    assert_eq!(results.len(), 1);
    assert!(results[0].0);
    assert_confinement_refused(&results[0].1);
    let mut refusals = 0;
    while let Ok(event) = rx.try_recv() {
        if let AgentEvent::HookLaunchRefused {
            hook_id, reason, ..
        } = event
        {
            assert_eq!(hook_id, HookId::new("confined-command-hook"));
            assert_eq!(
                reason,
                HookFailureReason::ConfinementRefused {
                    refusal: ConfinementRefusal::UnsupportedRequirement
                }
            );
            refusals += 1;
        }
    }
    assert_eq!(refusals, 1);
    assert!(!marker.exists(), "the refused hook never ran");
}

/// A hook from a project `config.toml` that a confined process can write is
/// not trusted: it keeps its slot and every invocation is refused with the
/// typed untrusted-source refusal, so its guarded tool is blocked with model
/// feedback, the hook never runs, and the same run continues.
#[cfg(unix)]
#[tokio::test]
async fn a_hook_from_an_untrusted_project_config_is_refused_at_every_invocation() {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().canonicalize().unwrap();
    let project = base.join("project");
    let marker = base.join("project-hook-entered");
    write_project_hooks(&project, vec![marker_guardrail("project-guard", &marker)]);
    let host = writing_host(&project);
    assert!(host.untrusted_source(&project.join(".rkat").join("config.toml")));

    let client = Arc::new(ScriptedClient::new("guarded_tool", json!({})));
    let tool = Arc::new(GuardedTool::default());
    let (tx, mut rx) = tokio::sync::mpsc::channel(256);
    let mut build = AgentBuildConfig::new("claude-sonnet-4-5");
    build.llm_client_override = Some(client.clone());
    build.external_tools = Some(tool.clone());
    build.event_tx = Some(tx);
    let factory = AgentFactory::new(base.join("sessions"))
        .session_store(Arc::new(meerkat_store::MemoryStore::new()))
        .runtime_root(base.join("runtime"))
        .project_root(&project)
        .builtins(false)
        .shell(false)
        .with_host_confinement(&host);
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
    assert_eq!(
        tool.0.load(Ordering::SeqCst),
        0,
        "the refused hook blocks its tool"
    );
    let results = client.returned_results();
    assert_eq!(results.len(), 1);
    assert!(results[0].0);
    assert_refused_with(
        &results[0].1,
        ConfinementRefusal::UntrustedConfigurationSource,
    );
    let mut refusals = 0;
    while let Ok(event) = rx.try_recv() {
        if let AgentEvent::HookLaunchRefused {
            hook_id, reason, ..
        } = event
        {
            assert_eq!(hook_id, HookId::new("project-guard"));
            assert_eq!(
                reason,
                HookFailureReason::ConfinementRefused {
                    refusal: ConfinementRefusal::UntrustedConfigurationSource
                }
            );
            refusals += 1;
        }
    }
    assert_eq!(refusals, 1);
    assert!(!marker.exists(), "the refused hook never ran");
}

#[cfg(all(unix, feature = "mcp"))]
#[tokio::test]
async fn a_refused_mcp_stdio_launch_never_runs_unconfined() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let marker = root.join("mcp-entered");
    let client = Arc::new(ScriptedClient::new("guarded_tool", json!({})));
    let mut build = AgentBuildConfig::new("claude-sonnet-4-5");
    build.llm_client_override = Some(client.clone());
    build.external_tools = Some(Arc::new(GuardedTool::default()));
    build.wait_for_mcp = true;
    build.mcp_servers = vec![meerkat_core::McpServerConfig::stdio(
        "confined-server",
        "/bin/sh",
        vec!["-c".into(), "printf entered > \"$MARKER\"".into()],
        HashMap::from([("MARKER".into(), marker.display().to_string())]),
    )];
    let factory = confined_factory(&root).shell(false);
    let mut agent = tokio::time::timeout(
        Duration::from_secs(15),
        factory.build_agent(build, &Config::default()),
    )
    .await
    .unwrap()
    .unwrap();
    let result = tokio::time::timeout(
        Duration::from_secs(15),
        agent.run("Use the available tool".to_string().into()),
    )
    .await
    .unwrap()
    .unwrap();

    assert_eq!(result.text, "continued");
    assert!(!marker.exists(), "the refused server never started");
}

/// A host that confines every launch kind does not trust a skills directory
/// one of its confined processes could change: the project skills directory
/// under a write grant is not loaded, so a skill planted there never reaches
/// a session. The same directory outside every grant is loaded.
#[cfg(all(unix, feature = "skills"))]
#[tokio::test]
async fn skills_a_confined_process_can_write_are_not_loaded() {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().canonicalize().unwrap();
    let project = base.join("project");
    let skill = project.join(".rkat").join("skills").join("planted-skill");
    std::fs::create_dir_all(&skill).unwrap();
    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: planted-skill\ndescription: A skill a confined process planted\n---\n\n# Planted",
    )
    .unwrap();
    let skill_names = |host: HostConfinement| {
        let base = base.clone();
        let project = project.clone();
        async move {
            let factory = AgentFactory::new(base.join("sessions"))
                .session_store(Arc::new(meerkat_store::MemoryStore::new()))
                .runtime_root(base.join("runtime"))
                .project_root(&project)
                .context_root(project.clone())
                .with_host_confinement(&host);
            let runtime = factory
                .build_skill_runtime(&Config::default())
                .await
                .unwrap()
                .expect("a skill runtime");
            runtime
                .list_skills(&meerkat_core::skills::SkillFilter::default())
                .await
                .unwrap()
                .into_iter()
                .map(|skill| skill.name)
                .collect::<Vec<_>>()
        }
    };

    let untrusted = skill_names(writing_host(&project)).await;
    assert!(
        !untrusted.iter().any(|name| name == "planted-skill"),
        "a skills directory a confined process can write is not loaded: {untrusted:?}"
    );
    let trusted = skill_names(writing_host(&base.join("elsewhere"))).await;
    assert!(
        trusted.iter().any(|name| name == "planted-skill"),
        "a skills directory outside every grant is loaded: {trusted:?}"
    );
}

/// A stdio skill source runs under the host's `mcp_stdio` confinement, as a
/// local MCP stdio server does: a launch the platform cannot confine as
/// required is refused, and the source's command never runs as the host.
#[cfg(all(unix, feature = "skills"))]
#[tokio::test]
async fn a_stdio_skill_source_launches_under_the_mcp_stdio_confinement() {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().canonicalize().unwrap();
    let marker = base.join("stdio-skill-source-ran");
    let mut config = Config::default();
    config
        .skills
        .repositories
        .push(meerkat_core::skills_config::SkillRepositoryConfig {
            name: "stdio-source".into(),
            source_uuid: meerkat_core::skills::SourceUuid::from_uuid(uuid::Uuid::new_v4()),
            transport: meerkat_core::skills_config::SkillRepoTransport::Stdio {
                command: "/bin/sh".into(),
                args: vec![
                    "-c".into(),
                    format!(
                        "printf entered > '{}'; cat >/dev/null; printf '{{}}'",
                        marker.display()
                    ),
                ],
                cwd: None,
                env: Default::default(),
                timeout_seconds: 5,
            },
        });
    let factory = AgentFactory::new(base.join("sessions"))
        .session_store(Arc::new(meerkat_store::MemoryStore::new()))
        .runtime_root(base.join("runtime"))
        .project_root(&base)
        .with_host_confinement(&host_confinement(&base));
    let runtime = factory
        .build_skill_runtime(&config)
        .await
        .unwrap()
        .expect("a skill runtime");
    let _ = runtime
        .list_skills(&meerkat_core::skills::SkillFilter::default())
        .await;
    assert!(
        !marker.exists(),
        "the stdio skill source never ran as the host"
    );
}

/// A skill that a trusted skills directory links to from elsewhere is
/// judged where the link leads: a link into a path a confined process can
/// write is not followed, so that process cannot supply the skill through
/// it. The same kind of link to a path no confined process can write is
/// followed.
#[cfg(all(unix, feature = "skills"))]
#[tokio::test]
async fn a_skill_linked_out_of_a_trusted_skills_directory_is_judged_where_it_leads() {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().canonicalize().unwrap();
    let project = base.join("project");
    let skills = project.join(".rkat").join("skills");
    std::fs::create_dir_all(&skills).unwrap();
    let writable = base.join("writable");
    let elsewhere = base.join("elsewhere");
    for (root, name) in [(&writable, "linked-skill"), (&elsewhere, "kept-skill")] {
        let skill = root.join(name);
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(
            skill.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: A linked skill\n---\n\n# Linked"),
        )
        .unwrap();
        std::os::unix::fs::symlink(&skill, skills.join(name)).unwrap();
    }
    let factory = AgentFactory::new(base.join("sessions"))
        .session_store(Arc::new(meerkat_store::MemoryStore::new()))
        .runtime_root(base.join("runtime"))
        .project_root(&project)
        .context_root(project.clone())
        .with_host_confinement(&writing_host(&writable));
    let runtime = factory
        .build_skill_runtime(&Config::default())
        .await
        .unwrap()
        .expect("a skill runtime");
    let names: Vec<String> = runtime
        .list_skills(&meerkat_core::skills::SkillFilter::default())
        .await
        .unwrap()
        .into_iter()
        .map(|skill| skill.name)
        .collect();
    assert!(
        !names.iter().any(|name| name == "linked-skill"),
        "a link into a write grant is not followed: {names:?}"
    );
    assert!(
        names.iter().any(|name| name == "kept-skill"),
        "a link outside every grant is followed: {names:?}"
    );
}

/// The credential store is judged with the links in it: a store whose
/// directory is outside every grant but holds a link into a path a
/// confined process can write (even one that does not exist yet) refuses
/// the start. A store without such links is admitted.
#[cfg(unix)]
#[test]
fn a_link_in_the_credential_store_to_a_writable_path_refuses_the_start() {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().canonicalize().unwrap();
    let writable = base.join("writable");
    let store = base.join("store");
    let clean = base.join("clean");
    for dir in [&writable, &store, &clean] {
        std::fs::create_dir_all(dir).unwrap();
    }
    std::fs::write(writable.join("token.json"), "{}").unwrap();
    std::fs::write(clean.join("token.json"), "{}").unwrap();
    let host = writing_host(&writable);

    std::os::unix::fs::symlink(writable.join("token.json"), store.join("token.json")).unwrap();
    let refused = host.admit_host_inputs(&[(meerkat::HostInput::CredentialStore, store.clone())]);
    assert!(
        matches!(
            refused,
            Err(meerkat::HostConfinementError::AdministrationInputWritable {
                input: meerkat::HostInput::CredentialStore,
                ..
            })
        ),
        "{refused:?}"
    );

    std::fs::remove_file(store.join("token.json")).unwrap();
    std::os::unix::fs::symlink(writable.join("later.json"), store.join("token.json")).unwrap();
    assert!(
        host.admit_host_inputs(&[(meerkat::HostInput::CredentialStore, store)])
            .is_err(),
        "a dangling link into a write grant refuses the start"
    );
    host.admit_host_inputs(&[(meerkat::HostInput::CredentialStore, clean)])
        .expect("a store without links into a grant is admitted");
}

/// A credential command is a host launch exemption: the host runs it as
/// itself, outside its OS process confinement, so it refuses one whose
/// program or working directory a confined process could change, before
/// the program runs. The same command outside every grant runs.
#[cfg(all(unix, feature = "openai"))]
#[tokio::test]
async fn a_credential_command_a_confined_process_could_change_is_refused() {
    use std::collections::BTreeMap;
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().canonicalize().unwrap();
    let writable = base.join("writable");
    let elsewhere = base.join("elsewhere");
    for dir in [&writable, &elsewhere] {
        std::fs::create_dir_all(dir).unwrap();
    }
    let marker = base.join("helper-ran");
    let script = format!(
        "#!/bin/sh\nprintf ran > '{}'\nprintf sk-from-helper\n",
        marker.display()
    );
    for dir in [&writable, &elsewhere] {
        let helper = dir.join("helper.sh");
        std::fs::write(&helper, &script).unwrap();
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let host = writing_host(&writable);
    let sh_writing_marker = vec![
        "-c".to_string(),
        format!("printf ran > '{}'; printf sk-from-helper", marker.display()),
    ];

    for (program, args, cwd, env) in [
        // The program lives under a write grant.
        (
            writable.join("helper.sh"),
            Vec::new(),
            None,
            BTreeMap::new(),
        ),
        // Its working directory does.
        (
            Path::new("/bin/sh").to_path_buf(),
            sh_writing_marker.clone(),
            Some(writable.clone()),
            BTreeMap::new(),
        ),
        // A bare program name found through a PATH directory under a grant.
        (
            Path::new("helper.sh").to_path_buf(),
            Vec::new(),
            None,
            BTreeMap::from([("PATH".to_string(), writable.display().to_string())]),
        ),
    ] {
        let refused = preflight_command(&base, &host, program.clone(), args, cwd, env)
            .await
            .expect_err("a credential command a confined process could change is refused");
        assert!(
            refused.contains("confined process"),
            "{program:?}: {refused}"
        );
        assert!(
            !marker.exists(),
            "{program:?}: the refused command never ran"
        );
    }

    preflight_command(
        &base,
        &host,
        elsewhere.join("helper.sh"),
        Vec::new(),
        Some(elsewhere.clone()),
        BTreeMap::new(),
    )
    .await
    .expect("a credential command outside every grant runs");
    assert!(marker.exists(), "the trusted command ran");
}

/// Preflight an OpenAI binding whose credential is the command `program`
/// on a factory installing `host`.
#[cfg(all(unix, feature = "openai"))]
async fn preflight_command(
    base: &Path,
    host: &HostConfinement,
    program: std::path::PathBuf,
    args: Vec<String>,
    cwd: Option<std::path::PathBuf>,
    env: std::collections::BTreeMap<String, String>,
) -> Result<(), String> {
    use meerkat_core::{
        AuthProfileConfig, BackendProfileConfig, CredentialSourceSpec, ProviderBindingConfig,
        RealmConfigSection, SessionLlmIdentity,
    };
    let factory = AgentFactory::new(base.join("sessions")).with_host_confinement(host);
    let mut section = RealmConfigSection::default();
    section.backend.insert(
        "openai_api".to_string(),
        BackendProfileConfig {
            provider: "openai".to_string(),
            backend_kind: "openai_api".to_string(),
            base_url: None,
            options: serde_json::Value::Null,
            server: None,
        },
    );
    section.auth.insert(
        "command_token".to_string(),
        AuthProfileConfig {
            provider: "openai".to_string(),
            auth_method: "api_key".to_string(),
            source: CredentialSourceSpec::Command {
                program,
                args,
                cwd,
                env,
                timeout_ms: 5_000,
                refresh_interval_ms: None,
            },
            constraints: Default::default(),
            metadata_defaults: Default::default(),
        },
    );
    section.binding.insert(
        "command_openai".to_string(),
        ProviderBindingConfig {
            backend_profile: "openai_api".to_string(),
            auth_profile: "command_token".to_string(),
            credential_account: None,
            default_model: None,
            policy: Default::default(),
            provider_default: true,
        },
    );
    let realm = meerkat_core::mob_realm_id("helper").unwrap();
    let mut config = Config::default();
    config.realm.insert(realm.as_str().to_string(), section);
    // The binding is named, so no other credential (such as one in the
    // process environment) stands in for a refused command.
    let identity = SessionLlmIdentity {
        model: "gpt-5.4".to_string(),
        provider: Provider::OpenAI,
        self_hosted_server_id: None,
        provider_params: None,
        auth_binding: Some(meerkat_core::AuthBindingRef {
            realm: realm.clone(),
            binding: meerkat_core::BindingId::parse("command_openai").unwrap(),
            profile: None,
            origin: meerkat_core::BindingOrigin::Configured,
        }),
    };
    factory
        .preflight_llm_identity(
            &config,
            &identity,
            &std::collections::BTreeMap::new(),
            Some(&realm),
            Some(meerkat_runtime::MeerkatMachine::ephemeral().generated_auth_lease_handle()),
        )
        .await
        .map_err(|error| error.to_string())
}

/// A credential command runs as the host with a `PATH` from which every
/// directory a confined process could change is gone, so an interpreter or
/// helper it starts by name (here through `#!/usr/bin/env`) is never one a
/// confined process planted, even when its configured `PATH` names one.
#[cfg(all(unix, feature = "openai"))]
#[tokio::test]
async fn a_credential_command_never_runs_an_interpreter_planted_on_its_path() {
    use std::collections::BTreeMap;
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().canonicalize().unwrap();
    let writable = base.join("writable");
    let trusted = base.join("trusted");
    for dir in [&writable, &trusted] {
        std::fs::create_dir_all(dir).unwrap();
    }
    let planted_ran = base.join("planted-interpreter-ran");
    let interpreter = writable.join("meerkat-test-interpreter");
    std::fs::write(
        &interpreter,
        format!(
            "#!/bin/sh\nprintf ran > '{}'\nprintf sk-from-planted\n",
            planted_ran.display()
        ),
    )
    .unwrap();
    let helper = trusted.join("helper");
    std::fs::write(&helper, "#!/usr/bin/env meerkat-test-interpreter\n").unwrap();
    for program in [&interpreter, &helper] {
        std::fs::set_permissions(program, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    let result = preflight_command(
        &base,
        &writing_host(&writable),
        helper,
        Vec::new(),
        Some(trusted.clone()),
        BTreeMap::from([(
            "PATH".to_string(),
            format!("{}:/usr/bin:/bin", writable.display()),
        )]),
    )
    .await;
    assert!(
        !planted_ran.exists(),
        "the planted interpreter never ran: {result:?}"
    );
}

/// A host launch exemption never hands the program it runs a module or
/// library path a confined process could change: here a credential
/// command whose configured `PYTHONPATH` points into a write grant sees
/// none, so a module planted there is never loaded.
#[cfg(all(unix, feature = "openai"))]
#[tokio::test]
async fn a_credential_command_is_not_handed_a_module_path_a_confined_process_could_change() {
    use std::collections::BTreeMap;
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().canonicalize().unwrap();
    let writable = base.join("writable");
    let trusted = base.join("trusted");
    for dir in [&writable, &trusted] {
        std::fs::create_dir_all(dir).unwrap();
    }
    let seen = base.join("module-path-seen");
    let helper = trusted.join("helper");
    std::fs::write(
        &helper,
        format!(
            "#!/bin/sh\nprintf '%s' \"${{PYTHONPATH-unset}}\" > '{}'\nprintf sk-from-helper\n",
            seen.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755)).unwrap();

    preflight_command(
        &base,
        &writing_host(&writable),
        helper,
        Vec::new(),
        Some(trusted.clone()),
        BTreeMap::from([("PYTHONPATH".to_string(), writable.display().to_string())]),
    )
    .await
    .expect("the command runs");
    assert_eq!(std::fs::read_to_string(&seen).unwrap(), "unset");
}

/// A credential command without a working directory of its own runs, under
/// a confining host, from the user's home directory rather than the host's
/// working directory (which a confined process may be able to change), so
/// it is neither refused nor exposed by default.
#[cfg(all(unix, feature = "openai"))]
#[tokio::test]
async fn a_credential_command_without_a_working_directory_runs_from_a_protected_one() {
    use std::collections::BTreeMap;
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().canonicalize().unwrap();
    let helper_dir = base.join("helper");
    std::fs::create_dir_all(&helper_dir).unwrap();
    let ran_in = base.join("helper-ran-in");
    let helper = helper_dir.join("helper.sh");
    std::fs::write(
        &helper,
        format!(
            "#!/bin/sh\npwd -P > '{}'\nprintf sk-from-helper\n",
            ran_in.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755)).unwrap();
    // A confined process can write the host's working directory.
    let host = writing_host(&std::env::current_dir().unwrap().canonicalize().unwrap());

    preflight_command(&base, &host, helper, Vec::new(), None, BTreeMap::new())
        .await
        .expect("the command runs from a directory no confined process can change");
    let home = std::path::PathBuf::from(std::env::var_os("HOME").expect("HOME is set"))
        .canonicalize()
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(&ran_in).unwrap().trim(),
        home.display().to_string()
    );
}

/// A credential command the host refuses ends the binding's resolution with
/// that refusal: no other credential (another realm's binding, the global
/// realm's, or the process environment's) silently stands in for it.
#[cfg(all(unix, feature = "openai"))]
#[tokio::test]
async fn a_refused_credential_command_is_not_replaced_by_another_credential() {
    use meerkat_core::{
        AuthProfileConfig, BackendProfileConfig, CredentialSourceSpec, ProviderBindingConfig,
        RealmConfigSection, SessionLlmIdentity,
    };
    use std::collections::BTreeMap;
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().canonicalize().unwrap();
    let writable = base.join("writable");
    std::fs::create_dir_all(&writable).unwrap();
    let helper = writable.join("helper.sh");
    std::fs::write(&helper, "#!/bin/sh\nprintf sk-from-helper\n").unwrap();
    std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755)).unwrap();

    let section = |source: CredentialSourceSpec| {
        let mut section = RealmConfigSection::default();
        section.backend.insert(
            "openai_api".to_string(),
            BackendProfileConfig {
                provider: "openai".to_string(),
                backend_kind: "openai_api".to_string(),
                base_url: None,
                options: serde_json::Value::Null,
                server: None,
            },
        );
        section.auth.insert(
            "token".to_string(),
            AuthProfileConfig {
                provider: "openai".to_string(),
                auth_method: "api_key".to_string(),
                source,
                constraints: Default::default(),
                metadata_defaults: Default::default(),
            },
        );
        section.binding.insert(
            "openai".to_string(),
            ProviderBindingConfig {
                backend_profile: "openai_api".to_string(),
                auth_profile: "token".to_string(),
                credential_account: None,
                default_model: None,
                policy: Default::default(),
                provider_default: true,
            },
        );
        section
    };
    let realm = meerkat_core::mob_realm_id("helper").unwrap();
    let mut config = Config::default();
    config.realm.insert(
        realm.as_str().to_string(),
        section(CredentialSourceSpec::Command {
            program: helper,
            args: Vec::new(),
            cwd: Some(base.clone()),
            env: BTreeMap::new(),
            timeout_ms: 5_000,
            refresh_interval_ms: None,
        }),
    );
    config.realm.insert(
        "global".to_string(),
        section(CredentialSourceSpec::InlineSecret {
            secret: "sk-global-stand-in".into(),
        }),
    );
    let identity = SessionLlmIdentity {
        model: "gpt-5.4".to_string(),
        provider: Provider::OpenAI,
        self_hosted_server_id: None,
        provider_params: None,
        auth_binding: None,
    };
    let refused = AgentFactory::new(base.join("sessions"))
        .with_host_confinement(&writing_host(&writable))
        .preflight_llm_identity(
            &config,
            &identity,
            &BTreeMap::new(),
            Some(&realm),
            Some(meerkat_runtime::MeerkatMachine::ephemeral().generated_auth_lease_handle()),
        )
        .await
        .expect_err("the refused command is not replaced by the global realm's credential");
    assert!(
        refused.to_string().contains("confined process"),
        "{refused}"
    );
}

// ---- The in-process file tools and the engine's own files ----

/// A factory with the built-in tools and no shell, as a host's default tool
/// preset builds it, working in `root`.
fn file_tool_factory(root: &Path) -> AgentFactory {
    AgentFactory::new(root.join("sessions"))
        .session_store(Arc::new(meerkat_store::MemoryStore::new()))
        .runtime_root(root.join("runtime"))
        .project_root(root)
        .builtins(true)
        .shell(false)
}

/// One run in which the scripted model calls `apply_patch` once with
/// `patch`: whether the result it got back is an error, and its text.
async fn apply_patch_through(factory: AgentFactory, patch: String) -> (bool, String) {
    let client = Arc::new(ScriptedClient::new(
        "apply_patch",
        json!({ "patch": patch }),
    ));
    let mut build = AgentBuildConfig::new("claude-sonnet-4-5");
    build.llm_client_override = Some(client.clone());
    let mut agent = tokio::time::timeout(
        Duration::from_secs(15),
        factory.build_agent(build, &Config::default()),
    )
    .await
    .unwrap()
    .unwrap();
    tokio::time::timeout(
        Duration::from_secs(15),
        agent.run("Apply the patch".to_string().into()),
    )
    .await
    .unwrap()
    .unwrap();
    let mut results = client.returned_results();
    assert_eq!(results.len(), 1, "one apply_patch result: {results:?}");
    results.remove(0)
}

fn add_file_patch(path: &str) -> String {
    format!("*** Begin Patch\n*** Add File: {path}\n+planted\n*** End Patch")
}

/// The call came back to the model as the refusal of a write into the
/// engine's own files, an ordinary tool error.
fn assert_engine_file_refusal(result: &(bool, String), case: &str) {
    assert!(
        result.0 && result.1.contains("engine's own directory"),
        "{case}: expected the engine directory refusal as a tool error, got {result:?}"
    );
}

/// A host that declares nothing still keeps `<project root>/.rkat` from the
/// file tools: a session with the built-in tools and no shell cannot write
/// the project's MCP servers, hooks, skills or a realm kept there, and an
/// ordinary project file is still written.
#[tokio::test]
async fn file_tools_refuse_the_engine_directory_without_host_cooperation() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    for path in [
        ".rkat/mcp.toml",
        ".rkat/config.toml",
        ".rkat/skills/planted/SKILL.md",
        ".rkat/realms/ws-0000000000000000/config.toml",
    ] {
        let result = apply_patch_through(file_tool_factory(&root), add_file_patch(path)).await;
        assert_engine_file_refusal(&result, path);
        assert!(!root.join(".rkat").exists(), "{path}: .rkat was created");
    }
    let result = apply_patch_through(file_tool_factory(&root), add_file_patch("src/lib.rs")).await;
    assert!(!result.0, "an ordinary project file is written: {result:?}");
    assert_eq!(
        std::fs::read_to_string(root.join("src/lib.rs")).unwrap(),
        "planted\n"
    );
}

/// The lists a stock host assembles for a project whose own state and
/// inputs all lie inside the project root, outside `.rkat`: the realm state
/// root (`--state-root ./state`), the user config root (its document and
/// `mcp.toml`), a context root below the project root (its `mcp.toml`,
/// hooks and skills), and a file credential store.
fn declared_in_project(root: &Path) -> AgentFactory {
    let mut inputs = HostConfinement::host_inputs(
        &root.join("state"),
        Some(&root.join("home/.rkat/config.toml")),
    );
    inputs.push((
        meerkat::HostInput::CredentialStore,
        root.join("credentials"),
    ));
    let realm_documents = HostConfinement::realm_documents(&root.join("state/realm-1"));
    let convention_sources =
        HostConfinement::convention_sources(Some(&root.join("service")), Some(&root.join("home")));
    file_tool_factory(root).with_protected_host_inputs(
        &inputs,
        &realm_documents,
        &convention_sources,
    )
}

/// Every path a host declares as its own state or input inside the project
/// root is refused by the file tools, with everything under it, existing or
/// not; a file beside each of them is still written.
#[tokio::test]
async fn file_tools_refuse_every_declared_host_input_inside_the_project() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    for (case, path) in [
        (
            "a realm config document under the state root",
            "state/realm-1/config.toml",
        ),
        (
            "another realm under the state root",
            "state/parent/config.toml",
        ),
        ("a store under the state root", "state/realm-1/tasks.db"),
        ("the user config document", "home/.rkat/config.toml"),
        ("the user MCP file", "home/.rkat/mcp.toml"),
        (
            "the MCP file of a context root below the project root",
            "service/.rkat/mcp.toml",
        ),
        (
            "the hooks of that context root",
            "service/.rkat/config.toml",
        ),
        (
            "a skill of that context root",
            "service/.rkat/skills/planted/SKILL.md",
        ),
        ("the credential store", "credentials/provider/token.json"),
        (
            "the engine directory, as without a declaration",
            ".rkat/mcp.toml",
        ),
    ] {
        let result = apply_patch_through(declared_in_project(&root), add_file_patch(path)).await;
        assert_engine_file_refusal(&result, case);
        assert!(!root.join(path).exists(), "{case}: {path} was written");
    }
    for path in [
        "state.md",
        "home/notes.txt",
        "home/.rkat/notes.txt",
        "service/src/main.rs",
        "credentials.example",
    ] {
        let result = apply_patch_through(declared_in_project(&root), add_file_patch(path)).await;
        assert!(!result.0, "{path} is the project's own: {result:?}");
        assert!(root.join(path).is_file(), "{path} was written");
    }
}

/// A server started without a context root roots its tools in the realm
/// directory itself. The realm's own documents are refused there; the
/// state root as a whole contains the project root and is not applied, so
/// an ordinary file is written as before. The stores beside the documents
/// stay writable in this layout (a store file is written below): that is
/// the stated limit of the rule there, and a host that wants them out of
/// reach gives its tools a context root.
#[tokio::test]
async fn file_tools_keep_the_realm_documents_where_the_realm_is_the_project_root() {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().canonicalize().unwrap();
    let state_root = base.join("realms");
    let realm = state_root.join("realm-1");
    std::fs::create_dir_all(&realm).unwrap();
    std::fs::write(realm.join("config.toml"), "kept = true\n").unwrap();
    let factory = || {
        let mut inputs =
            HostConfinement::host_inputs(&state_root, Some(&base.join("user/.rkat/config.toml")));
        inputs.push((
            meerkat::HostInput::CredentialStore,
            base.join("credentials"),
        ));
        AgentFactory::new(realm.clone())
            .session_store(Arc::new(meerkat_store::MemoryStore::new()))
            .runtime_root(realm.clone())
            .project_root(realm.clone())
            .builtins(true)
            .shell(false)
            .with_protected_host_inputs(
                &inputs,
                &HostConfinement::realm_documents(&realm),
                &HostConfinement::convention_sources(None, Some(&base.join("user"))),
            )
    };
    let update = "*** Begin Patch\n*** Update File: config.toml\n@@\n-kept = true\n+kept = false\n*** End Patch";
    let result = apply_patch_through(factory(), update.to_string()).await;
    assert_engine_file_refusal(&result, "the realm config document");
    assert_eq!(
        std::fs::read_to_string(realm.join("config.toml")).unwrap(),
        "kept = true\n"
    );
    for (case, path) in [
        ("the config generation sidecar", "config_state.json"),
        ("the realm manifest", "realm_manifest.json"),
        ("the grant owner store", "authorization/grant-owner.jsonl"),
        ("the hooks read from the project root", ".rkat/config.toml"),
        (
            "the skills read from the project root",
            ".rkat/skills/planted/SKILL.md",
        ),
        ("an MCP file at the project root", ".rkat/mcp.toml"),
    ] {
        let result = apply_patch_through(factory(), add_file_patch(path)).await;
        assert_engine_file_refusal(&result, case);
        assert!(!realm.join(path).exists(), "{case}: {path} was written");
    }
    for path in ["out.txt", "notes/config.toml", "tasks.db"] {
        let result = apply_patch_through(factory(), add_file_patch(path)).await;
        assert!(!result.0, "{path} is written as before: {result:?}");
        assert!(realm.join(path).is_file(), "{path} was written");
    }
}
