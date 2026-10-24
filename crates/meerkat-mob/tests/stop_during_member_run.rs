//! A mob Stop while a turn-driven member's run is in flight.
//!
//! Stop pauses the mob: it holds every member's run starts, and it leaves a
//! turn-driven member's in-flight run running to finish normally
//! (`MemberStopRun::LeftRunning`). That run must commit: the member stays
//! registered, and after Resume nothing runs again. A run that failed at its
//! checkpoint instead tore the member down, and Resume then replayed its
//! input, tool calls included.
//!
//! One row per runtime persistence profile: a Jsonl realm checkpoints
//! WholeBlobV1, a Sqlite realm HeadCanonicalV1.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use futures::StreamExt as _;
use meerkat::{AgentFactory, Config, FactoryAgentBuilder, PersistentSessionService};
use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
use meerkat_core::types::HandlingMode;
use meerkat_core::{AgentEvent, Message};
use meerkat_mob::{
    AgentIdentity, MemberRunStarts, MemberStopOutcome, MemberStopRun, MemberTurnOptions,
    MobBuilder, MobDefinition, MobId, MobRuntimeMode, MobStorage, Profile, ProfileBinding,
    ProfileName, SpawnMemberSpec, ToolConfig,
};

const WORKER: &str = "stop-mid-run-worker";
const FIRST_PROMPT: &str = "count once while the mob stops";
const NEXT_PROMPT: &str = "a later turn after resume";
const STEP: Duration = Duration::from_secs(30);

/// Scripted model:
/// - a request whose newest input carries `FIRST_PROMPT` calls the shell
///   tool, which appends one line to the counter file; the first such
///   request waits for the test before it answers;
/// - a request carrying a tool result ends the turn: one answer to the
///   first prompt;
/// - any other request ends the turn at once.
struct ScriptedClient {
    counter: PathBuf,
    first_gate_taken: AtomicBool,
    first_entered: tokio::sync::Notify,
    release_first: tokio::sync::Notify,
    tool_call_requests: AtomicUsize,
    answers: AtomicUsize,
}

impl ScriptedClient {
    fn new(counter: PathBuf) -> Self {
        Self {
            counter,
            first_gate_taken: AtomicBool::new(false),
            first_entered: tokio::sync::Notify::new(),
            release_first: tokio::sync::Notify::new(),
            tool_call_requests: AtomicUsize::new(0),
            answers: AtomicUsize::new(0),
        }
    }
}

/// Whether the input after the newest assistant message carries `needle`.
fn newest_input_mentions(messages: &[Message], needle: &str) -> bool {
    messages
        .iter()
        .rev()
        .take_while(|message| !matches!(message, Message::BlockAssistant(_)))
        .filter(|message| !matches!(message, Message::System(_)))
        .any(|message| {
            serde_json::to_string(message)
                .expect("message renders")
                .contains(needle)
        })
}

fn done_events(
    request: &LlmRequest,
    mut events: Vec<LlmEvent>,
    stop: meerkat_core::StopReason,
) -> Vec<LlmEvent> {
    events.push(LlmEvent::UsageUpdate {
        usage: meerkat_core::TurnUsage::host_declared(
            meerkat_core::Provider::OpenAI,
            &request.model,
            meerkat_core::Usage::default(),
        ),
    });
    events.push(LlmEvent::Done {
        outcome: LlmDoneOutcome::success(stop),
    });
    events
}

fn text_reply(request: &LlmRequest, text: &str) -> Vec<LlmEvent> {
    done_events(
        request,
        vec![LlmEvent::TextDelta {
            delta: text.to_string(),
            meta: None,
        }],
        meerkat_core::StopReason::EndTurn,
    )
}

fn counting_call(request: &LlmRequest, index: usize, counter: &Path) -> Vec<LlmEvent> {
    done_events(
        request,
        vec![LlmEvent::ToolCallComplete {
            id: format!("call-stop-mid-run-{index}"),
            name: "shell".to_string(),
            args: serde_json::json!({
                "command": format!("printf 'x\\n' >> '{}'", counter.display()),
                "timeout_secs": 30,
            }),
            meta: None,
        }],
        meerkat_core::StopReason::ToolUse,
    )
}

#[async_trait::async_trait]
impl LlmClient for ScriptedClient {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> std::pin::Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>>
    {
        if matches!(request.messages.last(), Some(Message::ToolResults { .. })) {
            self.answers.fetch_add(1, Ordering::SeqCst);
            return Box::pin(futures::stream::iter(
                text_reply(request, "counted").into_iter().map(Ok),
            ));
        }
        if !newest_input_mentions(&request.messages, FIRST_PROMPT) {
            return Box::pin(futures::stream::iter(
                text_reply(request, "ok").into_iter().map(Ok),
            ));
        }
        let index = self.tool_call_requests.fetch_add(1, Ordering::SeqCst) + 1;
        let events = counting_call(request, index, &self.counter);
        if !self.first_gate_taken.swap(true, Ordering::SeqCst) {
            return Box::pin(
                futures::stream::once(async move {
                    self.first_entered.notify_one();
                    self.release_first.notified().await;
                })
                .flat_map(move |()| futures::stream::iter(events.clone().into_iter().map(Ok))),
            );
        }
        Box::pin(futures::stream::iter(events.into_iter().map(Ok)))
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::OpenAI
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

async fn build_service(
    root: &Path,
    backend: meerkat_store::RealmBackend,
    client: Arc<ScriptedClient>,
) -> (
    Arc<PersistentSessionService<FactoryAgentBuilder>>,
    Arc<meerkat_runtime::MeerkatMachine>,
) {
    let (_manifest, persistence) = meerkat::open_realm_persistence_in(
        root,
        "stop-mid-run-realm",
        Some(backend),
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
    meerkat::surface::build_runtime_backed_service_with_default_reconfigure_host(
        builder,
        8,
        persistence,
        root.join("config-state.json"),
    )
    .expect("compose the runtime-backed service")
}

fn mob_definition() -> MobDefinition {
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
            peer_description: "worker".to_string(),
            external_addressable: true,
            backend: None,
            runtime_mode: MobRuntimeMode::TurnDriven,
            max_inline_peer_notifications: None,
            output_schema: None,
            provider_params: None,
            durable: false,
        })),
    );
    let mut definition = MobDefinition::explicit(MobId::from(format!(
        "stop-mid-run-{}",
        uuid::Uuid::new_v4().simple()
    )));
    definition.profiles = profiles;
    definition
}

/// How the in-flight run ended while the mob was Stopped.
#[derive(Debug, PartialEq, Eq)]
enum RunEnd {
    Completed,
    Failed(String),
    TurnEndedWithoutTerminalEvent,
    NoTerminalEvent,
}

/// What one Stop-during-run scenario observed.
#[derive(Debug, PartialEq, Eq)]
struct Observed {
    stop_outcome: Option<MemberStopOutcome>,
    run_end_before_resume: RunEnd,
    registered_after_run: bool,
    resume: Result<(), String>,
    later_turn: Result<(), String>,
    first_turn: Result<(), String>,
    tool_executions: usize,
    tool_call_requests: usize,
    first_prompt_answers: usize,
    committed_tool_results: usize,
}

async fn stop_during_a_member_run(backend: meerkat_store::RealmBackend) {
    let temp = tempfile::tempdir().expect("temp dir");
    let counter = temp.path().join("project").join("tool-count.txt");
    let client = Arc::new(ScriptedClient::new(counter.clone()));
    let (service, machine) = build_service(temp.path(), backend, Arc::clone(&client)).await;
    let storage = MobStorage::persistent(temp.path().join("mob.db")).expect("mob storage");
    let handle = MobBuilder::new(mob_definition(), storage)
        .with_session_service(service.clone())
        .with_default_llm_client(client.clone())
        .create()
        .await
        .expect("create persistent mob");
    let worker = AgentIdentity::from(WORKER);
    handle
        .spawn_spec(SpawnMemberSpec::new("worker", worker.clone()))
        .await
        .expect("spawn member");
    let session_id = handle
        .resolve_bridge_session_id(&worker)
        .await
        .expect("worker session");
    let member = handle.member(&worker).await.expect("worker handle");

    // The member's run is in flight: its first provider request waits.
    let (events_tx, mut events_rx) = tokio::sync::mpsc::channel(256);
    let first = member
        .start_turn(
            FIRST_PROMPT,
            HandlingMode::Queue,
            MemberTurnOptions::default(),
            Some(events_tx),
        )
        .await
        .expect("start the first turn");
    let (first_done_tx, mut first_done_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let _ = first_done_tx.send(first.wait().await.map(|_| ()));
    });
    tokio::time::timeout(STEP, client.first_entered.notified())
        .await
        .expect("the first provider request is in flight");
    let run_id = tokio::time::timeout(STEP, async {
        loop {
            let envelope = events_rx.recv().await.expect("first turn events");
            if let AgentEvent::RunStarted { identity, .. } = envelope.payload
                && let Some(run_id) = identity.run_id
            {
                return run_id;
            }
        }
    })
    .await
    .expect("the run reports its id");

    // Stop returns while the run is still inside its provider request.
    let report = tokio::time::timeout(STEP, handle.stop())
        .await
        .expect("stop returns while the run is in flight")
        .expect("stop");

    // The run goes on: its tool runs, and the run ends while Stopped.
    client.release_first.notify_one();
    let mut first_turn = None;
    let run_end_before_resume = tokio::time::timeout(STEP, async {
        loop {
            tokio::select! {
                envelope = events_rx.recv() => match envelope.map(|envelope| envelope.payload) {
                    Some(AgentEvent::RunCompleted { .. }) => return RunEnd::Completed,
                    Some(AgentEvent::RunFailed { error_report, .. }) => {
                        return RunEnd::Failed(error_report.message);
                    }
                    Some(_) => {}
                    None => return RunEnd::TurnEndedWithoutTerminalEvent,
                },
                done = &mut first_done_rx, if first_turn.is_none() => {
                    first_turn = Some(done.expect("first turn outcome"));
                }
            }
        }
    })
    .await
    .unwrap_or(RunEnd::NoTerminalEvent);
    let registered_after_run = if run_end_before_resume == RunEnd::Completed {
        machine.contains_session(&session_id).await
    } else {
        // A failed run's teardown unregisters the member asynchronously.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            let registered = machine.contains_session(&session_id).await;
            if !registered || tokio::time::Instant::now() >= deadline {
                break registered;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    };

    let resume = tokio::time::timeout(STEP, handle.resume())
        .await
        .expect("resume returns")
        .map_err(|error| error.to_string());
    // A later turn runs after anything the Stop left queued, so once it is
    // answered a replay of the first input would have run too.
    let later_turn = match member
        .start_turn(
            NEXT_PROMPT,
            HandlingMode::Queue,
            MemberTurnOptions::default(),
            None,
        )
        .await
    {
        Ok(turn) => tokio::time::timeout(STEP, turn.wait())
            .await
            .expect("the later turn ends")
            .map(|_| ())
            .map_err(|error| error.to_string()),
        Err(error) => Err(error.to_string()),
    };
    let first_turn = match first_turn {
        Some(done) => done,
        None => tokio::time::timeout(STEP, first_done_rx)
            .await
            .expect("the first turn ends")
            .expect("first turn outcome"),
    }
    .map_err(|error| error.to_string());

    let committed = meerkat_core::service::SessionServiceHistoryExt::read_history(
        service.as_ref(),
        &session_id,
        meerkat_core::SessionHistoryQuery::default(),
    )
    .await
    .expect("read the member's history");
    let observed = Observed {
        stop_outcome: report.members.get(&worker).cloned(),
        run_end_before_resume,
        registered_after_run,
        resume,
        later_turn,
        first_turn,
        tool_executions: std::fs::read_to_string(&counter)
            .map(|text| text.lines().count())
            .unwrap_or(0),
        tool_call_requests: client.tool_call_requests.load(Ordering::SeqCst),
        first_prompt_answers: client.answers.load(Ordering::SeqCst),
        committed_tool_results: committed
            .messages
            .iter()
            .filter(|message| matches!(message, Message::ToolResults { .. }))
            .count(),
    };
    assert_eq!(
        observed,
        Observed {
            stop_outcome: Some(MemberStopOutcome {
                run: MemberStopRun::LeftRunning { run_id },
                starts: MemberRunStarts::Held,
            }),
            run_end_before_resume: RunEnd::Completed,
            registered_after_run: true,
            resume: Ok(()),
            later_turn: Ok(()),
            first_turn: Ok(()),
            tool_executions: 1,
            tool_call_requests: 1,
            first_prompt_answers: 1,
            committed_tool_results: 1,
        },
        "{backend:?}: the run in flight at the Stop commits, the member stays registered, \
         and nothing runs again after Resume"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stop_during_a_member_run_lets_the_run_commit_whole_blob() {
    stop_during_a_member_run(meerkat_store::RealmBackend::Jsonl).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stop_during_a_member_run_lets_the_run_commit_head_canonical() {
    stop_during_a_member_run(meerkat_store::RealmBackend::Sqlite).await;
}
