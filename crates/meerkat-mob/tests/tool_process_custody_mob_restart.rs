//! Abrupt host death mid-tool on a mob member, recovered by mob resume.
//!
//! A child host process creates a persistent mob whose worker's scripted
//! model calls the shell tool with a delayed file effect, starts a worker
//! turn, and is SIGKILLed while the tool runs. The next host incarnation
//! (this test process) resumes the mob. Reviving the worker must settle the
//! member session's process custody before it serves: kill the orphaned
//! tool, settle the interrupted input instead of replaying it (a replay would
//! reach the model without the notice and call the tool again), and record
//! the typed notice, preceded by the interrupted request, so the worker's
//! next turn sees both exactly once.

#![cfg(any(target_os = "linux", target_os = "macos"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use meerkat::{AgentFactory, Config, FactoryAgentBuilder, PersistentSessionService};
use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
use meerkat_core::Message;
use meerkat_core::tool_process::InterruptedToolRunDisposition;
use meerkat_core::types::{HandlingMode, SystemNoticeBlock, SystemNoticeKind};
use meerkat_mob::{
    AgentIdentity, MemberTurnOptions, MobBuilder, MobDefinition, MobId, MobRuntimeMode, MobStorage,
    Profile, ProfileBinding, ProfileName, SpawnMemberSpec, ToolConfig,
};

const PHASE_ENV: &str = "MEERKAT_CUSTODY_MOB_PHASE";
const ROOT_ENV: &str = "MEERKAT_CUSTODY_MOB_ROOT";
const CHILD_TEST: &str = "mob_custody_host_child";
const TOOL_CALL_ID: &str = "call-mob-crash";
const WORKER: &str = "w-1";
const INTERRUPTED_PROMPT: &str = "create the effect file";
const NEXT_PROMPT: &str = "what happened?";

/// Scripted model: on a fresh request (no tool results, no interrupted-run
/// notice) it calls the shell tool; otherwise it answers.
struct ScriptedShellClient {
    command: String,
    tool_calls: AtomicUsize,
    requests_seeing_notice: AtomicUsize,
}

impl ScriptedShellClient {
    fn new(command: String) -> Self {
        Self {
            command,
            tool_calls: AtomicUsize::new(0),
            requests_seeing_notice: AtomicUsize::new(0),
        }
    }
}

#[async_trait::async_trait]
impl LlmClient for ScriptedShellClient {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> std::pin::Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>>
    {
        let sees_notice = request.messages.iter().any(|message| {
            matches!(
                message,
                Message::SystemNotice(notice) if notice.kind == SystemNoticeKind::ToolProcessRecovery
            )
        });
        if sees_notice {
            self.requests_seeing_notice.fetch_add(1, Ordering::SeqCst);
        }
        let answered = sees_notice
            || request
                .messages
                .iter()
                .any(|message| matches!(message, Message::ToolResults { .. }));
        let mut events = Vec::new();
        let stop_reason = if answered {
            events.push(LlmEvent::TextDelta {
                delta: "noted".to_string(),
                meta: None,
            });
            meerkat_core::StopReason::EndTurn
        } else {
            self.tool_calls.fetch_add(1, Ordering::SeqCst);
            events.push(LlmEvent::ToolCallComplete {
                id: TOOL_CALL_ID.to_string(),
                name: "shell".to_string(),
                args: serde_json::json!({ "command": self.command, "timeout_secs": 60 }),
                meta: None,
            });
            meerkat_core::StopReason::ToolUse
        };
        events.push(LlmEvent::UsageUpdate {
            usage: meerkat_core::TurnUsage::host_declared(
                meerkat_core::Provider::OpenAI,
                &request.model,
                meerkat_core::Usage::default(),
            ),
        });
        events.push(LlmEvent::Done {
            outcome: LlmDoneOutcome::Success { stop_reason },
        });
        Box::pin(futures::stream::iter(events.into_iter().map(Ok)))
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::OpenAI
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

fn tool_command(root: &Path) -> String {
    format!(
        "echo started > '{}'; sleep 2; echo effect >> '{}'",
        root.join("started.fifo").display(),
        root.join("effect").display()
    )
}

fn mkfifo(path: &Path) {
    let status = std::process::Command::new("mkfifo")
        .arg(path)
        .status()
        .expect("run mkfifo");
    assert!(status.success(), "mkfifo {}", path.display());
}

async fn build_service(
    root: &Path,
    client: Arc<ScriptedShellClient>,
) -> Arc<PersistentSessionService<FactoryAgentBuilder>> {
    let (_manifest, persistence) = meerkat::open_realm_persistence_in(
        root,
        "custody-mob-realm",
        Some(meerkat_store::RealmBackend::Sqlite),
        Some(meerkat_store::RealmOrigin::Explicit),
    )
    .await
    .expect("open realm persistence");
    let project = root.join("project");
    std::fs::create_dir_all(&project).expect("project dir");
    let factory = AgentFactory::new(root.join("sessions"))
        .runtime_root(root.join("realm"))
        .project_root(&project)
        .builtins(true)
        .shell(true)
        .comms(true);
    let mut config = Config::default();
    config.shell.program = "sh".to_string();
    config.shell.security_mode = meerkat_core::types::SecurityMode::Unrestricted;
    let mut builder = FactoryAgentBuilder::new(factory, config);
    builder.default_llm_client = Some(client);
    let (service, _adapter) =
        meerkat::surface::build_runtime_backed_service(builder, 8, persistence);
    Arc::new(service)
}

fn mob_definition(mob_id: &str) -> MobDefinition {
    let mut profiles = BTreeMap::new();
    profiles.insert(
        ProfileName::from("worker"),
        ProfileBinding::Inline(Box::new(Profile {
            model_fallback: None,
            model: "gpt-5.4".to_string(),
            provider: None,
            self_hosted_server_id: None,
            image_generation_provider: None,
            auto_compact_threshold: None,
            resume_overrides: Vec::new(),
            skills: vec![],
            tools: ToolConfig {
                shell: true,
                comms: true,
                ..Default::default()
            },
            peer_description: "Runs shell tools".to_string(),
            external_addressable: true,
            backend: None,
            runtime_mode: MobRuntimeMode::TurnDriven,
            max_inline_peer_notifications: None,
            output_schema: None,
            provider_params: None,
        })),
    );
    let mut definition = MobDefinition::explicit(MobId::from(mob_id.to_string()));
    definition.profiles = profiles;
    definition
}

/// The first host: create the mob, spawn the worker, record its session,
/// start a worker turn that runs the delayed-effect shell tool, and wait to
/// be killed.
#[tokio::test(flavor = "multi_thread")]
async fn mob_custody_host_child() {
    let Some(root) = std::env::var_os(ROOT_ENV).map(PathBuf::from) else {
        return;
    };
    if std::env::var_os(PHASE_ENV).is_none() {
        return;
    }
    let client = Arc::new(ScriptedShellClient::new(tool_command(&root)));
    let service = build_service(&root, Arc::clone(&client)).await;
    let storage = MobStorage::persistent(&root.join("mob.db")).expect("persistent mob storage");
    let mob_id = std::fs::read_to_string(root.join("mob-id")).expect("mob id");
    let handle = MobBuilder::new(mob_definition(mob_id.trim()), storage)
        .with_session_service(service)
        .with_default_llm_client(client)
        .create()
        .await
        .expect("create persistent mob");
    handle
        .spawn_spec(SpawnMemberSpec::new("worker", AgentIdentity::from(WORKER)))
        .await
        .expect("spawn worker");
    let session_id = handle
        .resolve_bridge_session_id(&AgentIdentity::from(WORKER))
        .await
        .expect("worker session id");
    std::fs::write(root.join("session-id"), format!("{session_id}\n")).expect("session id");
    let _turn = handle
        .member(&AgentIdentity::from(WORKER))
        .await
        .expect("worker handle")
        .start_turn(
            INTERRUPTED_PROMPT,
            HandlingMode::Queue,
            MemberTurnOptions::default(),
            None,
        )
        .await
        .expect("start worker turn");
    // The turn blocks in the tool until the parent kills this process.
    std::future::pending::<()>().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn mob_member_interrupted_by_host_death_is_settled_not_replayed_on_resume() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    mkfifo(&root.join("started.fifo"));
    let mob_id = format!("custody-mob-{}", meerkat_core::time_compat::new_uuid_v7());
    std::fs::write(root.join("mob-id"), &mob_id).expect("mob id");

    let mut host = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", CHILD_TEST, "--nocapture", "--test-threads=1"])
        .env(PHASE_ENV, "host")
        .env(ROOT_ENV, root)
        .env("MEERKAT_DISABLE_GRAPH_DECODE_MEMO", "1")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn host");
    // Block (no polling) until the tool itself reports it is running.
    let fifo = root.join("started.fifo");
    let started = tokio::time::timeout(
        Duration::from_secs(180),
        tokio::task::spawn_blocking(move || std::fs::read_to_string(fifo)),
    )
    .await;
    // Abrupt host death mid-tool.
    host.kill().unwrap();
    host.wait().unwrap();
    let started = started
        .expect("the worker's shell tool never started")
        .unwrap()
        .unwrap();
    assert_eq!(started.trim(), "started");
    let session_id = meerkat::SessionId::parse(
        std::fs::read_to_string(root.join("session-id"))
            .expect("session id")
            .trim(),
    )
    .expect("valid session id");

    // Next host: resume the mob and give the revived worker a real turn.
    let client = Arc::new(ScriptedShellClient::new(tool_command(root)));
    let service = build_service(root, Arc::clone(&client)).await;
    let storage = MobStorage::persistent(&root.join("mob.db")).expect("reopen mob storage");
    let handle = MobBuilder::for_resume(storage)
        .with_session_service(Arc::clone(&service) as Arc<dyn meerkat_mob::MobSessionService>)
        .with_default_llm_client(Arc::clone(&client) as Arc<dyn LlmClient>)
        .notify_orchestrator_on_resume(false)
        .resume()
        .await
        .expect("mob resume after host death");
    tokio::time::timeout(Duration::from_secs(60), async {
        handle
            .member(&AgentIdentity::from(WORKER))
            .await
            .expect("revived worker handle")
            .start_turn(
                NEXT_PROMPT,
                HandlingMode::Queue,
                MemberTurnOptions::default(),
                None,
            )
            .await
            .expect("start worker turn")
            .wait()
            .await
            .expect("worker turn completes");
    })
    .await
    .expect("worker turn completes in time");

    assert_eq!(
        client.tool_calls.load(Ordering::SeqCst),
        0,
        "the interrupted worker input was replayed"
    );
    assert_eq!(
        client.requests_seeing_notice.load(Ordering::SeqCst),
        1,
        "the worker's next turn sees the interrupted-run notice"
    );
    let session = service
        .load_authoritative_session(&session_id)
        .await
        .expect("load worker session")
        .expect("worker session");
    let prompts = session
        .messages()
        .iter()
        .filter(|message| {
            matches!(message, Message::User(user) if user.text_content().contains(INTERRUPTED_PROMPT))
        })
        .count();
    assert_eq!(prompts, 1, "the interrupted request appears exactly once");
    let blocks: Vec<&SystemNoticeBlock> = session
        .messages()
        .iter()
        .filter_map(|message| match message {
            Message::SystemNotice(notice)
                if notice.kind == SystemNoticeKind::ToolProcessRecovery =>
            {
                Some(notice.blocks.iter())
            }
            _ => None,
        })
        .flatten()
        .collect();
    assert_eq!(blocks.len(), 1, "exactly one interrupted-run notice");
    assert!(
        matches!(
            blocks[0],
            SystemNoticeBlock::ToolProcessInterrupted {
                tool_call_id: Some(id),
                disposition: InterruptedToolRunDisposition::InputsSettled { inputs: 1 },
                ..
            } if id == TOOL_CALL_ID
        ),
        "{:?}",
        blocks[0]
    );
    // The killed tool's delayed effect never happens.
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(
        !root.join("effect").exists(),
        "the interrupted tool's effect happened"
    );
    handle.shutdown().await.expect("shutdown resumed mob");
}
