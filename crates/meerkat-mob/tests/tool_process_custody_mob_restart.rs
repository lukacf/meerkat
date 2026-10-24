//! Abrupt host death mid-tool on a mob member, recovered by mob resume.
//!
//! A child host process creates a persistent mob whose worker's scripted
//! model calls the shell tool with a delayed file effect, starts a worker
//! turn, and is SIGKILLed while the tool runs. The worker's run is open at
//! its committed round (the round commit precedes the tool), so the next host
//! incarnation (this test process) resumes it when it revives the worker.
//! Reviving the worker must settle the member session's process custody
//! before it serves (kill the orphaned tool, so its delayed effect never
//! happens), and the resumed round must answer the interrupted call with an
//! outcome-uncertain result instead of running it again. The run then ends
//! once, without an interrupted-run notice: the call's own result tells the
//! model.
//!
//! The scripted tool appends a line to `runs` on every execution and signals
//! `started.fifo` on its first execution only, so a call that runs again
//! shows as a second line (and as its effect) instead of blocking on the
//! FIFO.

#![cfg(any(target_os = "linux", target_os = "macos"))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::disallowed_methods
)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use meerkat::{AgentFactory, Config, FactoryAgentBuilder, PersistentSessionService};
use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
use meerkat_core::Message;
use meerkat_core::types::{HandlingMode, SystemNoticeKind};
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
const ANSWER: &str = "noted";
/// Bound on the host child reaching the tool; a child that exits first fails
/// the row at once.
const CHILD_BOUND: Duration = Duration::from_secs(180);

/// Scripted model: on a fresh request (no tool results, no interrupted-run
/// notice) it calls the shell tool; otherwise it answers.
struct ScriptedShellClient {
    command: String,
    tool_calls: AtomicUsize,
    requests: AtomicUsize,
    requests_seeing_notice: AtomicUsize,
}

impl ScriptedShellClient {
    fn new(command: String) -> Self {
        Self {
            command,
            tool_calls: AtomicUsize::new(0),
            requests: AtomicUsize::new(0),
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
        self.requests.fetch_add(1, Ordering::SeqCst);
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
                delta: ANSWER.to_string(),
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
            outcome: LlmDoneOutcome::success(stop_reason),
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

/// The scripted tool: one `runs` line per execution, the FIFO signal on the
/// first execution only, then the delayed effect.
fn tool_command(root: &Path) -> String {
    format!(
        "echo $$ >> '{pids}'; echo ran >> '{runs}'; if [ ! -e '{once}' ]; then : > '{once}'; echo started > '{fifo}'; fi; sleep 2; echo effect >> '{effect}'",
        pids = root.join("pids").display(),
        runs = root.join("runs").display(),
        once = root.join("started-once").display(),
        fifo = root.join("started.fifo").display(),
        effect = root.join("effect").display()
    )
}

/// Bound on the scripted tool's shells exiting after recovery killed them.
const PROCESS_EXIT_BOUND: Duration = Duration::from_secs(10);

/// Whether a process with this pid exists (`kill -0`; a zombie counts until
/// it is reaped).
fn process_exists(pid: u32) -> bool {
    std::process::Command::new("sh")
        .args(["-c", &format!("kill -0 {pid}")])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// The scripted tool's delayed effect can no longer happen: every shell the
/// tool started has exited (recovery killed the interrupted one, which
/// writes the effect only after its sleep). Waits a bounded time for exits
/// still being reaped; a shell still running fails the row.
async fn assert_tool_processes_gone(root: &Path) {
    let pids = std::fs::read_to_string(root.join("pids")).unwrap_or_default();
    assert!(
        !pids.trim().is_empty(),
        "the scripted tool recorded no process"
    );
    let started = std::time::Instant::now();
    for pid in pids.lines() {
        let pid: u32 = pid.trim().parse().expect("a recorded pid");
        while process_exists(pid) {
            assert!(
                started.elapsed() < PROCESS_EXIT_BOUND,
                "the interrupted tool's shell {pid} is still running"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

/// How many times the scripted tool has run.
fn tool_runs(root: &Path) -> usize {
    std::fs::read_to_string(root.join("runs"))
        .map(|runs| runs.lines().count())
        .unwrap_or(0)
}

/// Wait for the tool's line in `started.fifo`. A host that exits first, or
/// never starts the tool within the bound, fails the row at once with its
/// log.
async fn tool_started(root: &Path, host: &mut std::process::Child) -> String {
    let fifo = root.join("started.fifo");
    // A plain thread, not a runtime blocking task, so a read that never ends
    // cannot hold the test runtime's shutdown.
    let reader = std::thread::spawn({
        let fifo = fifo.clone();
        move || std::fs::read_to_string(fifo)
    });
    let started = std::time::Instant::now();
    loop {
        if reader.is_finished() {
            return reader
                .join()
                .expect("join the FIFO reader")
                .expect("read the FIFO");
        }
        let failure = match host.try_wait().expect("poll the host") {
            Some(status) => Some(format!("exited before its tool started ({status})")),
            None if started.elapsed() > CHILD_BOUND => Some("never started its tool".to_string()),
            None => None,
        };
        if let Some(failure) = failure {
            let _ = host.kill();
            let _ = host.wait();
            // End the blocked read: a writer that opens and closes the FIFO
            // gives it end of file. Detached, since its open blocks when the
            // reader has already finished.
            drop(std::thread::spawn(move || {
                std::fs::OpenOptions::new().write(true).open(fifo)
            }));
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while !reader.is_finished() && std::time::Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            if reader.is_finished() {
                let line = reader
                    .join()
                    .expect("join the FIFO reader")
                    .unwrap_or_default();
                if !line.is_empty() {
                    return line;
                }
            }
            panic!(
                "the host {failure}: {}",
                std::fs::read_to_string(root.join("host.log")).unwrap_or_default()
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
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
        meerkat::surface::build_runtime_backed_service(builder, 8, persistence)
            .expect("compose the runtime-backed service");
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
            durable: false,
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
    let storage = MobStorage::persistent(root.join("mob.db")).expect("persistent mob storage");
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

    let log = std::fs::File::create(root.join("host.log")).expect("host log");
    let mut host = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", CHILD_TEST, "--nocapture", "--test-threads=1"])
        .env(PHASE_ENV, "host")
        .env(ROOT_ENV, root)
        .env("MEERKAT_DISABLE_GRAPH_DECODE_MEMO", "1")
        .stdout(log.try_clone().expect("host log"))
        .stderr(log)
        .spawn()
        .expect("spawn host");
    // The tool itself reports it is running.
    let started = tool_started(root, &mut host).await;
    // Abrupt host death mid-tool.
    host.kill().unwrap();
    host.wait().unwrap();
    assert_eq!(started.trim(), "started");
    assert_eq!(tool_runs(root), 1, "the first host ran the tool once");
    let session_id = meerkat::SessionId::parse(
        std::fs::read_to_string(root.join("session-id"))
            .expect("session id")
            .trim(),
    )
    .expect("valid session id");

    // Next host: resume the mob, which revives the worker and resumes its
    // open run, then give the worker a real turn, queued behind that run.
    let client = Arc::new(ScriptedShellClient::new(tool_command(root)));
    let service = build_service(root, Arc::clone(&client)).await;
    let storage = MobStorage::persistent(root.join("mob.db")).expect("reopen mob storage");
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
    .unwrap_or_else(|_| {
        panic!(
            "worker turn completes in time (the interrupted call ran {} times)",
            tool_runs(root)
        )
    });

    assert_eq!(tool_runs(root), 1, "the interrupted call ran again");
    assert_eq!(
        client.tool_calls.load(Ordering::SeqCst),
        0,
        "the model never asked for the call again"
    );
    assert_eq!(
        client.requests.load(Ordering::SeqCst),
        2,
        "two model calls: the resumed run's next one and the new turn's"
    );
    assert_eq!(
        client.requests_seeing_notice.load(Ordering::SeqCst),
        0,
        "the call's own result tells the model: no interrupted-run notice"
    );
    let session = service
        .load_authoritative_session(&session_id)
        .await
        .expect("load worker session")
        .expect("worker session");
    let messages = session.messages();
    let prompts = messages
        .iter()
        .filter(|message| {
            matches!(message, Message::User(user) if user.text_content().contains(INTERRUPTED_PROMPT))
        })
        .count();
    assert_eq!(prompts, 1, "the interrupted request appears exactly once");
    let calls = messages
        .iter()
        .filter(|message| {
            matches!(message, Message::BlockAssistant(answer)
                if answer.tool_calls().any(|call| call.id == TOOL_CALL_ID))
        })
        .count();
    assert_eq!(calls, 1, "the model asked for the call exactly once");
    let results: Vec<String> = messages
        .iter()
        .filter_map(|message| match message {
            Message::ToolResults { results, .. } => Some(results),
            _ => None,
        })
        .flatten()
        .filter(|result| result.tool_use_id == TOOL_CALL_ID)
        .map(meerkat_core::ToolResult::text_content)
        .collect();
    assert_eq!(
        results.len(),
        1,
        "the call has exactly one result: {results:?}"
    );
    assert!(
        results[0].contains("outcome_uncertain") && results[0].contains("It was not run again"),
        "the interrupted call is answered outcome-uncertain: {}",
        results[0]
    );
    assert!(
        !messages.iter().any(|message| matches!(
            message,
            Message::SystemNotice(notice) if notice.kind == SystemNoticeKind::ToolProcessRecovery
        )),
        "no interrupted-run notice"
    );
    let answers = messages
        .iter()
        .filter(|message| {
            matches!(message, Message::BlockAssistant(answer)
                if answer.text_blocks().collect::<String>() == ANSWER)
        })
        .count();
    assert_eq!(
        answers, 2,
        "the resumed run and the new turn answered once each"
    );
    // The killed tool's delayed effect never happens: its shell is gone.
    assert_tool_processes_gone(root).await;
    assert!(
        !root.join("effect").exists(),
        "the interrupted tool's effect happened"
    );
    assert_eq!(tool_runs(root), 1, "the interrupted call ran again");
    handle.shutdown().await.expect("shutdown resumed mob");
}
