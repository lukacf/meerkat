//! A cancelled runtime turn's provisional tail, on both durable persistence
//! profiles: a hard interrupt or a run stop drops the run future after a
//! round commit, so the run's last round commit is the store's provisional
//! tail. The runtime closes it as an interrupted run when the cancellation
//! commits, so the session's next save and next turn proceed over it, and a
//! restart neither resumes the run nor runs its input again.
//!
//! Each row drives the real agent, the persistent session service and the
//! runtime loop: turns are admitted through `accept_input_with_completion`,
//! and the interrupt and the stop are the runtime's own
//! (`hard_cancel_current_run`, `stop_run`).
//!
//! The operator rows start from the durable state a pre-release build of
//! 0.9.5 left: its interrupt kept the run's tail, and the next prompt stood
//! staged for a run that never committed. `resolve_held_durable_tail`
//! requeues that prompt only when the store proves it is not the tail's own.

use super::*;
use async_trait::async_trait;
use futures::{StreamExt, stream};
use meerkat_client::{LlmDoneOutcome, LlmEvent, LlmRequest};
use meerkat_core::service::{DeferredPromptPolicy, InitialTurnPolicy};
use meerkat_core::types::{SystemNoticeKind, SystemNoticeMessage};
use meerkat_core::{
    AgentError, AgentToolDispatcher, AssistantBlock, Message, Provider, RunId, SessionId,
    ToolCallView, ToolDef, ToolDispatchOutcome, ToolError, ToolResult,
};
use meerkat_runtime::completion::CompletionOutcome;
use meerkat_runtime::identifiers::LogicalRuntimeId;
use meerkat_runtime::input_state::StoredInputState;
use meerkat_runtime::store::RuntimeStore;
use meerkat_runtime::{InMemoryRuntimeStore, Input, MeerkatMachine, PromptInput};
use meerkat_session::PersistentSessionService;
use meerkat_store::{MemoryBlobStore, MemoryStore};
use serde_json::json;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::Semaphore;

pub(super) const DEADLINE: Duration = Duration::from_secs(20);

/// What the model answers to one request.
#[derive(Clone, Copy)]
pub(super) enum Step {
    /// One call of the `probe` tool.
    Tool,
    /// Text, once the test releases it.
    Held,
    Text(&'static str),
}

pub(super) struct ScriptedModel {
    steps: StdMutex<VecDeque<Step>>,
    pub(super) requests: AtomicUsize,
    pub(super) entered: Semaphore,
    pub(super) release: Semaphore,
}

impl ScriptedModel {
    pub(super) fn new() -> Self {
        Self {
            steps: StdMutex::new(VecDeque::new()),
            requests: AtomicUsize::new(0),
            entered: Semaphore::new(0),
            release: Semaphore::new(0),
        }
    }

    pub(super) fn push(&self, step: Step) {
        self.steps.lock().expect("steps").push_back(step);
    }
}

#[async_trait]
impl LlmClient for ScriptedModel {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, meerkat_client::LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, meerkat_client::LlmError>> + Send + 'a>>
    {
        let n = self.requests.fetch_add(1, Ordering::SeqCst);
        let step = self
            .steps
            .lock()
            .expect("steps")
            .pop_front()
            .unwrap_or(Step::Text("unscripted"));
        let usage = meerkat_core::TurnUsage::host_declared(
            Provider::Anthropic,
            &request.model,
            Default::default(),
        );
        let text = |text: &str| {
            vec![
                Ok(LlmEvent::TextDelta {
                    delta: text.into(),
                    meta: None,
                }),
                Ok(LlmEvent::UsageUpdate {
                    usage: usage.clone(),
                }),
                Ok(LlmEvent::Done {
                    outcome: LlmDoneOutcome::success(meerkat_core::StopReason::EndTurn),
                }),
            ]
        };
        match step {
            Step::Tool => Box::pin(stream::iter(vec![
                Ok(LlmEvent::ToolCallComplete {
                    id: format!("probe-call-{n}"),
                    name: "probe".into(),
                    args: json!({}),
                    meta: None,
                }),
                Ok(LlmEvent::UsageUpdate {
                    usage: usage.clone(),
                }),
                Ok(LlmEvent::Done {
                    outcome: LlmDoneOutcome::success(meerkat_core::StopReason::ToolUse),
                }),
            ])),
            Step::Held => {
                let events = text("held answer");
                Box::pin(
                    stream::once(async move {
                        self.entered.add_permits(1);
                        self.release
                            .acquire()
                            .await
                            .expect("release the model")
                            .forget();
                        events
                    })
                    .flat_map(stream::iter),
                )
            }
            Step::Text(answer) => Box::pin(stream::iter(text(answer))),
        }
    }

    fn provider(&self) -> Provider {
        Provider::Anthropic
    }

    async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
        Ok(())
    }
}

struct ProbeTools;

#[async_trait]
impl AgentToolDispatcher for ProbeTools {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        vec![Arc::new(ToolDef::new(
            "probe",
            "probe",
            json!({"type":"object"}),
        ))]
        .into()
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        Ok(ToolDispatchOutcome::sync_result(ToolResult::new(
            call.id.into(),
            "probe-result".into(),
            false,
        )))
    }
}

/// The ordinary factory build with the model client and the tools overridden.
pub(super) struct ScriptedBuilder {
    pub(super) factory: AgentFactory,
    pub(super) client: Arc<ScriptedModel>,
}

#[async_trait]
impl SessionAgentBuilder for ScriptedBuilder {
    type Agent = FactoryAgent;

    async fn build_agent(
        &self,
        request: &CreateSessionRequest,
        event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<FactoryAgent, SessionError> {
        let mut config = AgentBuildConfig::from_create_session_request(request, event_tx);
        config.llm_client_override = Some(self.client.clone());
        config.tool_dispatcher_override = Some(Arc::new(ProbeTools));
        let session_context = match request
            .build
            .as_ref()
            .map(|build| &build.runtime_build_mode)
        {
            Some(meerkat_core::RuntimeBuildMode::SessionOwned(bindings)) => {
                Some(Arc::clone(bindings.session_context()))
            }
            _ => None,
        };
        let factory = self.factory.clone();
        let agent = meerkat_runtime::stack_relief::relieve_caller_stack(move || async move {
            factory.build_agent(config, &Config::default()).await
        })
        .await
        .map_err(|error| SessionError::Agent(AgentError::InternalError(error.to_string())))?;
        Ok(FactoryAgent {
            agent,
            session_context,
            pending_head_canonical_boundary: None,
            acknowledged_head_canonical_boundary: None,
        })
    }
}

/// The durable stores one process start opens: a WholeBlob in-memory set
/// shared across starts, or a HeadCanonical SQLite file reopened by each.
enum Stores {
    WholeBlob {
        sessions: Arc<MemoryStore>,
        runtime: Arc<InMemoryRuntimeStore>,
        blobs: Arc<MemoryBlobStore>,
    },
    HeadCanonical {
        path: PathBuf,
    },
}

impl Stores {
    fn new(head_canonical: bool, root: &TempDir) -> Self {
        if head_canonical {
            Self::HeadCanonical {
                path: root.path().join("sessions.sqlite3"),
            }
        } else {
            Self::WholeBlob {
                sessions: Arc::new(MemoryStore::new()),
                runtime: Arc::new(InMemoryRuntimeStore::new()),
                blobs: Arc::new(MemoryBlobStore::new()),
            }
        }
    }
}

/// One process start over the stores.
pub(super) struct Start {
    pub(super) service: Arc<PersistentSessionService<ScriptedBuilder>>,
    pub(super) machine: Arc<MeerkatMachine>,
    pub(super) runtime_store: Arc<dyn RuntimeStore>,
    pub(super) head_canonical: bool,
}

impl Start {
    fn open(stores: &Stores, root: &TempDir, client: &Arc<ScriptedModel>) -> Self {
        let (sessions, runtime_store, blobs, machine): (
            Arc<dyn meerkat_store::SessionStore>,
            Arc<dyn RuntimeStore>,
            Arc<dyn meerkat_core::BlobStore>,
            Arc<MeerkatMachine>,
        ) = match stores {
            Stores::WholeBlob {
                sessions,
                runtime,
                blobs,
            } => {
                let machine = Arc::new(
                    MeerkatMachine::persistent(runtime.clone(), blobs.clone())
                        .expect("canonical machine"),
                );
                (sessions.clone(), runtime.clone(), blobs.clone(), machine)
            }
            Stores::HeadCanonical { path } => {
                let sessions: Arc<dyn meerkat_store::SessionStore> = Arc::new(
                    meerkat_store::SqliteSessionStore::open(path.clone())
                        .expect("open the sqlite session store"),
                );
                let runtime: Arc<dyn RuntimeStore> = Arc::new(
                    meerkat_runtime::store::SqliteRuntimeStore::new_head_canonical(path.clone())
                        .expect("open the head-canonical runtime store"),
                );
                let persistence = crate::PersistenceBundle::new(
                    sessions,
                    runtime,
                    Arc::new(MemoryBlobStore::new()),
                )
                .expect("construct runtime authority");
                let machine = persistence.runtime_adapter();
                let (sessions, runtime, blobs) = persistence.into_parts();
                (sessions, runtime, blobs, machine)
            }
        };
        let builder = ScriptedBuilder {
            factory: AgentFactory::new(root.path().join("sessions"))
                .session_store(sessions.clone()),
            client: client.clone(),
        };
        let service = Arc::new(
            PersistentSessionService::new(builder, 4, sessions, runtime_store.clone(), blobs)
                .with_canonical_runtime_adapter(machine.clone()),
        );
        Self {
            service,
            machine,
            runtime_store,
            head_canonical: matches!(stores, Stores::HeadCanonical { .. }),
        }
    }

    /// Materialize a session with no first turn: a new one, or the committed
    /// one of an earlier start.
    async fn materialize(&self, existing: Option<&SessionId>) -> SessionId {
        let session = match existing {
            Some(id) => self
                .service
                .load_authoritative_session(id)
                .await
                .expect("load the committed session")
                .expect("the session is committed"),
            None => meerkat_core::Session::new(),
        };
        let request = CreateSessionRequest {
            model: "claude-sonnet-4-5".into(),
            prompt: "unused".into(),
            injected_context: Vec::new(),
            system_prompt: meerkat_core::SystemPromptOverride::Inherit,
            max_tokens: None,
            event_tx: None,
            initial_turn: InitialTurnPolicy::Defer,
            deferred_prompt_policy: DeferredPromptPolicy::Discard,
            build: None,
            labels: None,
        };
        let created = Box::pin(crate::surface::materialize_session(
            &self.service,
            &self.machine,
            session,
            request,
            {
                let service = self.service.clone();
                let machine = self.machine.clone();
                move |id| crate::surface::default_persistent_executor(service, machine, id)
            },
        ))
        .await
        .expect("materialize the session and its executor");
        created.session_id
    }

    /// Admit one prompt; the completion waiter of its turn.
    async fn admit(
        &self,
        id: &SessionId,
        prompt: &str,
    ) -> tokio::task::JoinHandle<Option<CompletionOutcome>> {
        let (_, completion) = self
            .machine
            .accept_input_with_completion(id, Input::Prompt(PromptInput::new(prompt, None)))
            .await
            .expect("the runtime accepts the prompt");
        let completion = completion.expect("a completion");
        tokio::spawn(async move {
            tokio::time::timeout(DEADLINE, completion.wait())
                .await
                .ok()
                .and_then(Result::ok)
        })
    }

    /// One whole turn.
    async fn turn(&self, id: &SessionId, prompt: &str) -> Option<CompletionOutcome> {
        self.admit(id, prompt).await.await.expect("turn task")
    }

    /// The run of the store's provisional tail, if one stands.
    async fn tail_run(&self, id: &SessionId) -> Option<RunId> {
        let runtime_id = LogicalRuntimeId::for_session(id);
        if self.head_canonical {
            self.runtime_store
                .load_head_canonical_provisional_tail(&runtime_id)
                .await
                .expect("read the provisional tail")
                .map(|tail| tail.run_id().clone())
        } else {
            self.runtime_store
                .load_whole_blob_provisional_tail(&runtime_id)
                .await
                .expect("read the provisional tail")
                .map(|tail| tail.authority().run_id().clone())
        }
    }

    /// The committed transcript after the system prompt, one label per row.
    async fn committed_rows(&self, id: &SessionId) -> Vec<String> {
        let session = self
            .service
            .load_authoritative_session(id)
            .await
            .expect("read the committed session")
            .expect("the session is committed");
        session
            .messages()
            .iter()
            .filter_map(|message| match message {
                Message::System(_) => None,
                Message::User(user) => Some(format!("user {}", user.text_content())),
                Message::BlockAssistant(assistant) => Some(
                    if assistant
                        .blocks
                        .iter()
                        .any(|block| matches!(block, AssistantBlock::ToolUse { .. }))
                    {
                        "assistant tool use".to_string()
                    } else {
                        format!("assistant {}", assistant.text_blocks().collect::<String>())
                    },
                ),
                Message::ToolResults { .. } => Some("tool results".to_string()),
                Message::SystemNotice(notice) => Some(format!("notice {:?}", notice.kind)),
            })
            .collect()
    }
}

/// A started session whose first turn, run A, made one tool round (its
/// round commits stand as the provisional tail) and is held at its second
/// model call. Returns A's completion waiter and run id.
pub(super) async fn run_a_held_after_a_round_commit(
    start: &Start,
    client: &ScriptedModel,
    id: &SessionId,
) -> (tokio::task::JoinHandle<Option<CompletionOutcome>>, RunId) {
    client.push(Step::Tool);
    client.push(Step::Held);
    let turn_a = start.admit(id, "first").await;
    tokio::time::timeout(DEADLINE, client.entered.acquire())
        .await
        .expect("run A reaches its second model call")
        .expect("model gate")
        .forget();
    let run_a = start
        .tail_run(id)
        .await
        .expect("run A's round commit stands as the provisional tail");
    (turn_a, run_a)
}

/// The committed history after the cancelled run A and one more turn.
pub(super) fn history_with_closed_run_a(next_prompt: &str, next_answer: &str) -> Vec<String> {
    vec![
        "user first".to_string(),
        "assistant tool use".to_string(),
        "tool results".to_string(),
        "notice Generic".to_string(),
        format!("user {next_prompt}"),
        format!("assistant {next_answer}"),
    ]
}

async fn an_interrupt_closes_the_run_and_the_next_turn_completes(head_canonical: bool) {
    let root = TempDir::new().expect("temp");
    let stores = Stores::new(head_canonical, &root);
    let client = Arc::new(ScriptedModel::new());
    let start = Start::open(&stores, &root, &client);
    let id = start.materialize(None).await;
    let (turn_a, _run_a) = run_a_held_after_a_round_commit(&start, &client, &id).await;

    start
        .machine
        .hard_cancel_current_run(&id, "user interrupt")
        .await
        .expect("the interrupt reaches run A");
    let outcome_a = turn_a.await.expect("turn A task");
    assert!(
        matches!(outcome_a, Some(CompletionOutcome::Cancelled)),
        "run A ends cancelled: {outcome_a:?}"
    );
    assert_eq!(
        start.tail_run(&id).await,
        None,
        "the cancellation closed run A's provisional tail"
    );

    client.push(Step::Text("second answer"));
    let outcome_b = start.turn(&id, "second").await;
    assert!(
        matches!(outcome_b, Some(CompletionOutcome::Completed(ref run)) if run.text == "second answer"),
        "the next turn completes: {outcome_b:?}"
    );
    assert_eq!(
        start.committed_rows(&id).await,
        history_with_closed_run_a("second", "second answer"),
        "history keeps run A's committed round, closed as interrupted, then the next turn"
    );
    assert_eq!(start.tail_run(&id).await, None);
}

#[tokio::test]
async fn an_interrupt_after_a_round_commit_closes_the_run_and_the_next_turn_completes_whole_blob() {
    an_interrupt_closes_the_run_and_the_next_turn_completes(false).await;
}

#[tokio::test]
async fn an_interrupt_after_a_round_commit_closes_the_run_and_the_next_turn_completes_head_canonical()
 {
    an_interrupt_closes_the_run_and_the_next_turn_completes(true).await;
}

/// The next turn after the interrupt makes a tool round of its own: its round
/// commits land on the base the interrupted run's closing committed, and the
/// turn completes with both runs' rounds in history.
async fn an_interrupt_then_a_tool_round_turn_completes(head_canonical: bool) {
    let root = TempDir::new().expect("temp");
    let stores = Stores::new(head_canonical, &root);
    let client = Arc::new(ScriptedModel::new());
    let start = Start::open(&stores, &root, &client);
    let id = start.materialize(None).await;
    let (turn_a, _run_a) = run_a_held_after_a_round_commit(&start, &client, &id).await;
    start
        .machine
        .hard_cancel_current_run(&id, "user interrupt")
        .await
        .expect("the interrupt reaches run A");
    turn_a.await.expect("turn A task");
    assert_eq!(start.tail_run(&id).await, None);

    client.push(Step::Tool);
    client.push(Step::Text("second answer"));
    let outcome_b = start.turn(&id, "second").await;
    assert!(
        matches!(outcome_b, Some(CompletionOutcome::Completed(ref run)) if run.text == "second answer"),
        "the next turn, with a tool round, completes: {outcome_b:?}"
    );
    assert_eq!(
        start.committed_rows(&id).await,
        vec![
            "user first".to_string(),
            "assistant tool use".to_string(),
            "tool results".to_string(),
            "notice Generic".to_string(),
            "user second".to_string(),
            "assistant tool use".to_string(),
            "tool results".to_string(),
            "assistant second answer".to_string(),
        ],
        "history keeps run A's round, closed as interrupted, then run B's round and answer"
    );
    assert_eq!(start.tail_run(&id).await, None);
}

#[tokio::test]
async fn an_interrupt_then_a_turn_with_a_tool_round_completes_whole_blob() {
    an_interrupt_then_a_tool_round_turn_completes(false).await;
}

#[tokio::test]
async fn an_interrupt_then_a_turn_with_a_tool_round_completes_head_canonical() {
    an_interrupt_then_a_tool_round_turn_completes(true).await;
}

async fn an_interrupt_then_a_save_applies(head_canonical: bool) {
    let root = TempDir::new().expect("temp");
    let stores = Stores::new(head_canonical, &root);
    let client = Arc::new(ScriptedModel::new());
    let start = Start::open(&stores, &root, &client);
    let id = start.materialize(None).await;
    let (turn_a, _run_a) = run_a_held_after_a_round_commit(&start, &client, &id).await;
    start
        .machine
        .hard_cancel_current_run(&id, "user interrupt")
        .await
        .expect("the interrupt reaches run A");
    turn_a.await.expect("turn A task");

    let saved = {
        let _boundary = start
            .service
            .acquire_runtime_turn_finalization_guard(&id)
            .await;
        start
            .service
            .append_system_notice_under_runtime_turn_boundary(
                &id,
                SystemNoticeMessage::new(SystemNoticeKind::Mcp, "a host notice").into(),
            )
            .await
    };
    assert!(
        matches!(
            saved,
            Ok(meerkat_core::service::AppendSystemContextStatus::Applied)
        ),
        "a save after the interrupt applies: {saved:?}"
    );

    client.push(Step::Text("second answer"));
    let outcome_b = start.turn(&id, "second").await;
    assert!(
        matches!(outcome_b, Some(CompletionOutcome::Completed(_))),
        "the next turn completes: {outcome_b:?}"
    );
    let rows = start.committed_rows(&id).await;
    assert_eq!(
        rows,
        vec![
            "user first".to_string(),
            "assistant tool use".to_string(),
            "tool results".to_string(),
            "notice Generic".to_string(),
            "notice Mcp".to_string(),
            "user second".to_string(),
            "assistant second answer".to_string(),
        ]
    );
}

#[tokio::test]
async fn an_interrupt_then_a_system_notice_save_applies_whole_blob() {
    an_interrupt_then_a_save_applies(false).await;
}

#[tokio::test]
async fn an_interrupt_then_a_system_notice_save_applies_head_canonical() {
    an_interrupt_then_a_save_applies(true).await;
}

async fn a_run_stop_closes_the_run_and_the_next_turn_completes(head_canonical: bool) {
    let root = TempDir::new().expect("temp");
    let stores = Stores::new(head_canonical, &root);
    let client = Arc::new(ScriptedModel::new());
    let start = Start::open(&stores, &root, &client);
    let id = start.materialize(None).await;
    let (turn_a, run_a) = run_a_held_after_a_round_commit(&start, &client, &id).await;

    let receipt = start
        .machine
        .stop_run(&id, &run_a, "turn stop")
        .await
        .expect("the stop reaches run A");
    assert!(
        matches!(
            receipt,
            meerkat_runtime::run_stop::RunStopReceipt::Stopped { ref run_id, .. } if *run_id == run_a
        ),
        "run A is stopped: {receipt:?}"
    );
    turn_a.await.expect("turn A task");
    assert_eq!(
        start.tail_run(&id).await,
        None,
        "the stop closed run A's provisional tail"
    );

    client.push(Step::Text("second answer"));
    let outcome_b = start.turn(&id, "second").await;
    assert!(
        matches!(outcome_b, Some(CompletionOutcome::Completed(_))),
        "the next turn completes: {outcome_b:?}"
    );
    assert_eq!(
        start.committed_rows(&id).await,
        history_with_closed_run_a("second", "second answer")
    );
}

#[tokio::test]
async fn a_run_stop_after_a_round_commit_closes_the_run_and_the_next_turn_completes_whole_blob() {
    a_run_stop_closes_the_run_and_the_next_turn_completes(false).await;
}

#[tokio::test]
async fn a_run_stop_after_a_round_commit_closes_the_run_and_the_next_turn_completes_head_canonical()
{
    a_run_stop_closes_the_run_and_the_next_turn_completes(true).await;
}

async fn a_restart_resumes_and_requeues_nothing_of_the_interrupted_run(head_canonical: bool) {
    let root = TempDir::new().expect("temp");
    let stores = Stores::new(head_canonical, &root);
    let client = Arc::new(ScriptedModel::new());
    let id = {
        let start = Start::open(&stores, &root, &client);
        let id = start.materialize(None).await;
        let (turn_a, _run_a) = run_a_held_after_a_round_commit(&start, &client, &id).await;
        start
            .machine
            .hard_cancel_current_run(&id, "user interrupt")
            .await
            .expect("the interrupt reaches run A");
        turn_a.await.expect("turn A task");
        start.service.try_shutdown().await.expect("shutdown");
        id
    };

    let start = Start::open(&stores, &root, &client);
    start.materialize(Some(&id)).await;
    assert_eq!(start.tail_run(&id).await, None, "no tail stands");
    assert_eq!(
        start.machine.resumable_run_id(&id).await,
        None,
        "the registration marks no run resumable"
    );
    let inputs = start
        .runtime_store
        .load_input_states(&LogicalRuntimeId::for_session(&id))
        .await
        .expect("read the input rows");
    for row in &inputs {
        let meerkat_runtime::store::InputStateRow::Decoded(stored) = row else {
            panic!("an input row decodes: {row:?}");
        };
        let last = stored.state.history.last().map(|entry| entry.to);
        assert!(
            matches!(
                last,
                Some(
                    meerkat_runtime::input_state::InputLifecycleState::Consumed
                        | meerkat_runtime::input_state::InputLifecycleState::Superseded
                        | meerkat_runtime::input_state::InputLifecycleState::Coalesced
                        | meerkat_runtime::input_state::InputLifecycleState::Abandoned
                )
            ),
            "no input of the interrupted run waits to run again: {:?}",
            stored.state.history.last()
        );
    }

    let requests_before = client.requests.load(Ordering::SeqCst);
    client.push(Step::Text("after the restart"));
    let outcome = start.turn(&id, "again").await;
    assert!(
        matches!(outcome, Some(CompletionOutcome::Completed(ref run)) if run.text == "after the restart"),
        "the next turn completes: {outcome:?}"
    );
    assert_eq!(
        client.requests.load(Ordering::SeqCst),
        requests_before + 1,
        "only the new turn calls the model"
    );
    assert_eq!(
        start.committed_rows(&id).await,
        history_with_closed_run_a("again", "after the restart"),
        "run A is closed once, and its input does not run again"
    );
}

#[tokio::test]
async fn a_restart_after_an_interrupt_resumes_and_requeues_nothing_whole_blob() {
    a_restart_resumes_and_requeues_nothing_of_the_interrupted_run(false).await;
}

#[tokio::test]
async fn a_restart_after_an_interrupt_resumes_and_requeues_nothing_head_canonical() {
    a_restart_resumes_and_requeues_nothing_of_the_interrupted_run(true).await;
}

// ---------------------------------------------------------------------------
// A pre-release interrupt's held tail, and the operator verb that resolves it.
// ---------------------------------------------------------------------------

/// How the fixture binds the prompt it strands beside run A's tail.
#[derive(Clone, Copy)]
enum Strand {
    /// Staged for a run that never started: the row a pre-release build of
    /// 0.9.5 left for the turn after its interrupt.
    ForANewRun,
    /// Staged for the session's earlier run, which committed: a boundary
    /// receipt names that run.
    ForACommittedRun,
    /// Queued and bound to no run.
    Unattributed,
}

/// The durable state a pre-release interrupt left, every host stopped.
struct PreReleaseWedge {
    id: SessionId,
    run_a: RunId,
    /// Run A's own input row as the store held it while run A ran.
    run_a_row: StoredInputState,
    /// The prompt stranded beside run A's tail.
    second: meerkat_core::InputId,
}

impl Stores {
    fn head_canonical(&self) -> bool {
        matches!(self, Self::HeadCanonical { .. })
    }

    /// The runtime store as an operator opens it, with no host running.
    fn operator_store(&self) -> Arc<dyn RuntimeStore> {
        match self {
            Self::WholeBlob { runtime, .. } => runtime.clone(),
            Self::HeadCanonical { path } => Arc::new(
                meerkat_runtime::store::SqliteRuntimeStore::open_existing(path.clone())
                    .expect("open the runtime database"),
            ),
        }
    }
}

/// The session's input rows, decoded.
async fn input_rows(store: &dyn RuntimeStore, id: &SessionId) -> Vec<StoredInputState> {
    store
        .load_input_states(&LogicalRuntimeId::for_session(id))
        .await
        .expect("read the input rows")
        .into_iter()
        .map(|row| match row {
            meerkat_runtime::store::InputStateRow::Decoded(stored) => *stored,
            other => panic!("an input row decodes: {other:?}"),
        })
        .collect()
}

/// Write one exact input row, as a store holds it.
async fn write_input_row(store: &dyn RuntimeStore, id: &SessionId, row: StoredInputState) {
    let record =
        meerkat_runtime::input_state::InputStatePersistenceRecord::stored_row_for_test(row)
            .expect("the row passes the stored-seed authority");
    store
        .persist_input_state(&LogicalRuntimeId::for_session(id), &record)
        .await
        .expect("write the input row");
}

/// The row of `input` the fixture strands: run A's admission facts (policy,
/// semantics, durability, destination), the next admission sequence, and
/// staged for `bound_run`, or queued and bound to no run.
fn stranded_row(
    run_a_row: &StoredInputState,
    input: &Input,
    bound_run: Option<RunId>,
) -> StoredInputState {
    use meerkat_runtime::input_state::{
        InputLifecycleState as Phase, InputStateHistoryEntry, InputStateSeed,
    };
    let mut row = StoredInputState::accepted_for_test(input).expect("an accepted row");
    row.state.persisted_input = Some(input.clone());
    row.state.durability = run_a_row.state.durability;
    row.state.policy = run_a_row.state.policy.clone();
    row.state.runtime_semantics = run_a_row.state.runtime_semantics;
    row.state.scoped_destination = run_a_row.state.scoped_destination.clone();
    let entry = |from, to, reason: String| InputStateHistoryEntry {
        timestamp: chrono::Utc::now(),
        from,
        to,
        reason: Some(reason),
    };
    row.state.history = vec![entry(
        Phase::Accepted,
        Phase::Queued,
        "QueueAccepted".to_string(),
    )];
    let (phase, attempt_count) = match &bound_run {
        Some(run) => {
            row.state.history.push(entry(
                Phase::Queued,
                Phase::Staged,
                format!("StageForRun({run})"),
            ));
            (Phase::Staged, 1)
        }
        None => (Phase::Queued, 0),
    };
    row.seed = InputStateSeed {
        phase,
        last_run_id: bound_run,
        last_boundary_sequence: None,
        terminal_outcome: None,
        attempt_count,
        admission_sequence: run_a_row
            .seed
            .admission_sequence
            .map(|sequence| sequence + 1),
        recovery_lane: Some(meerkat_core::types::HandlingMode::Queue),
    };
    row
}

/// The durable state a pre-release build of 0.9.5 left: its interrupt kept
/// run A's round commit as the tail, run A's input ended, and the next
/// prompt, `second`, stood staged for a run that never committed. The
/// runtime no longer writes that row, so the fixture writes it (bound as
/// `strand` says) while run A is held. The interrupt then ends run A, and the
/// cancelled run's settle holds A's tail beside the row, as ordinary recovery
/// held the pre-release one.
async fn pre_release_wedge(
    stores: &Stores,
    root: &TempDir,
    client: &Arc<ScriptedModel>,
    strand: Strand,
) -> PreReleaseWedge {
    let start = Start::open(stores, root, client);
    let id = start.materialize(None).await;
    let committed_run = match strand {
        Strand::ForACommittedRun => {
            client.push(Step::Text("zeroth answer"));
            let outcome = start.turn(&id, "zeroth").await;
            assert!(
                matches!(outcome, Some(CompletionOutcome::Completed(_))),
                "the earlier run commits: {outcome:?}"
            );
            input_rows(start.runtime_store.as_ref(), &id)
                .await
                .into_iter()
                .find_map(|row| row.seed.last_run_id)
        }
        Strand::ForANewRun | Strand::Unattributed => None,
    };
    let (turn_a, run_a) = run_a_held_after_a_round_commit(&start, client, &id).await;
    let run_a_row = input_rows(start.runtime_store.as_ref(), &id)
        .await
        .into_iter()
        .find(|row| row.seed.last_run_id.as_ref() == Some(&run_a))
        .expect("run A's own input row");
    let bound_run = match strand {
        Strand::ForANewRun => Some(RunId::new()),
        Strand::ForACommittedRun => {
            Some(committed_run.expect("the earlier run's input names its run"))
        }
        Strand::Unattributed => None,
    };
    let second = Input::Prompt(PromptInput::new("second", None));
    write_input_row(
        start.runtime_store.as_ref(),
        &id,
        stranded_row(&run_a_row, &second, bound_run),
    )
    .await;
    start
        .machine
        .hard_cancel_current_run(&id, "user interrupt")
        .await
        .expect("the interrupt reaches run A");
    turn_a.await.expect("turn A task");
    assert_eq!(
        start.tail_run(&id).await,
        Some(run_a.clone()),
        "run A's tail stays held beside the stranded prompt"
    );
    start.service.try_shutdown().await.expect("shutdown");
    PreReleaseWedge {
        id,
        run_a,
        run_a_row,
        second: second.id().clone(),
    }
}

/// Every durable fact the operator verb could write, as one comparable
/// value: the committed authority, the tail, every input row with the input
/// set's revision and token, and the lifecycle row.
async fn durable_witness(store: &dyn RuntimeStore, id: &SessionId, head_canonical: bool) -> String {
    let runtime_id = LogicalRuntimeId::for_session(id);
    let authority = store
        .load_session_boundary_authority(&runtime_id)
        .await
        .expect("read the committed authority");
    let tail = if head_canonical {
        format!(
            "{:?}",
            store
                .load_head_canonical_provisional_tail(&runtime_id)
                .await
                .expect("read the provisional tail")
        )
    } else {
        format!(
            "{:?}",
            store
                .load_whole_blob_provisional_tail(&runtime_id)
                .await
                .expect("read the provisional tail")
        )
    };
    let (versioned, revision, token) = store
        .load_input_states_with_versions(&runtime_id)
        .await
        .expect("read the versioned input rows")
        .into_parts();
    let versioned: Vec<String> = versioned
        .iter()
        .map(|(row, digest)| format!("{} {digest}", row.state.input_id))
        .collect();
    let rows = store
        .load_input_states(&runtime_id)
        .await
        .expect("read the input rows");
    let lifecycle = store
        .observe_machine_lifecycle(&runtime_id)
        .await
        .expect("read the lifecycle row");
    format!(
        "{authority:?}\n{tail}\n{} {token} {versioned:?}\n{rows:?}\n{lifecycle:?}",
        revision.store_generation()
    )
}

/// A pre-release interrupt left run A's tail held beside the next prompt,
/// staged for a run that never committed. The operator's dry run names that
/// shape and the prompt it requeues and writes nothing; the repair closes
/// run A as interrupted and requeues the prompt; a second repair finds no
/// tail. The next start runs the prompt exactly once.
async fn an_operator_requeues_the_prompt_stranded_beside_a_pre_release_interrupt(
    head_canonical: bool,
) {
    use meerkat_runtime::input_state::InputLifecycleState as Phase;
    use meerkat_runtime::recovery::{
        HeldTailRepairVerdict, HeldTailResolution, resolve_held_durable_tail,
    };

    let root = TempDir::new().expect("temp");
    let stores = Stores::new(head_canonical, &root);
    let client = Arc::new(ScriptedModel::new());
    let wedge = pre_release_wedge(&stores, &root, &client, Strand::ForANewRun).await;
    let id = &wedge.id;

    let store = stores.operator_store();
    let before = durable_witness(store.as_ref(), id, head_canonical).await;
    let HeldTailResolution::StrandedInputs(plan) =
        resolve_held_durable_tail(store.as_ref(), id, false)
            .await
            .expect("the dry run reads the held tail")
    else {
        panic!("the tail is held beside a stranded prompt");
    };
    assert_eq!(plan.verdict, HeldTailRepairVerdict::WouldRepair);
    assert_eq!(plan.run_id, wedge.run_a);
    assert_eq!(plan.requeued_inputs, vec![wedge.second.clone()]);
    assert!(
        plan.terminalized_inputs.is_empty(),
        "run A's own input has ended"
    );
    assert!(
        plan.closed_calls.is_empty(),
        "run A's round answered its call"
    );
    assert!(plan.runtime_quiescent);
    assert_eq!(
        durable_witness(store.as_ref(), id, head_canonical).await,
        before,
        "the dry run writes nothing"
    );

    let HeldTailResolution::StrandedInputs(applied) =
        resolve_held_durable_tail(store.as_ref(), id, true)
            .await
            .expect("the repair applies")
    else {
        panic!("the tail is still held beside the stranded prompt when applied");
    };
    assert!(
        matches!(applied.verdict, HeldTailRepairVerdict::Repaired { .. }),
        "{:?}",
        applied.verdict
    );
    assert_eq!(applied.candidate_id, plan.candidate_id);
    assert_eq!(applied.requeued_inputs, vec![wedge.second.clone()]);
    let rows = input_rows(store.as_ref(), id).await;
    let second = rows
        .iter()
        .find(|row| row.state.input_id == wedge.second)
        .expect("the stranded prompt's row");
    assert_eq!(second.seed.phase, Phase::Queued);
    assert_eq!(
        second.seed.last_run_id, None,
        "the prompt is bound to no run"
    );
    assert!(
        second.state.persisted_input.is_some(),
        "the prompt keeps its content"
    );
    let run_a_input = rows
        .iter()
        .find(|row| row.state.input_id == wedge.run_a_row.state.input_id)
        .expect("run A's own input row");
    assert_eq!(
        run_a_input.seed.phase,
        Phase::Abandoned,
        "run A's own input stays ended"
    );
    assert_eq!(
        resolve_held_durable_tail(store.as_ref(), id, true)
            .await
            .expect("a second repair reads the store"),
        HeldTailResolution::NoTail,
        "a second repair finds no tail"
    );
    drop(store);

    let start = Start::open(&stores, &root, &client);
    let requests_before = client.requests.load(Ordering::SeqCst);
    client.push(Step::Text("second answer"));
    start.materialize(Some(id)).await;
    let deadline = tokio::time::Instant::now() + DEADLINE;
    loop {
        let rows = start.committed_rows(id).await;
        if rows.last().map(String::as_str) == Some("assistant second answer") {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the requeued prompt runs after the start: {rows:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(
        client.requests.load(Ordering::SeqCst),
        requests_before + 1,
        "the requeued prompt calls the model once"
    );
    assert_eq!(
        start.committed_rows(id).await,
        history_with_closed_run_a("second", "second answer"),
        "run A is closed once, and the requeued prompt runs once after it"
    );
    client.push(Step::Text("third answer"));
    let outcome = start.turn(id, "third").await;
    assert!(
        matches!(outcome, Some(CompletionOutcome::Completed(ref run)) if run.text == "third answer"),
        "the next turn completes: {outcome:?}"
    );
    assert_eq!(
        client.requests.load(Ordering::SeqCst),
        requests_before + 2,
        "only the new turn calls the model again"
    );
}

#[tokio::test]
async fn an_operator_requeues_the_prompt_stranded_beside_a_pre_release_interrupt_whole_blob() {
    an_operator_requeues_the_prompt_stranded_beside_a_pre_release_interrupt(false).await;
}

#[tokio::test]
async fn an_operator_requeues_the_prompt_stranded_beside_a_pre_release_interrupt_head_canonical() {
    an_operator_requeues_the_prompt_stranded_beside_a_pre_release_interrupt(true).await;
}

/// The shapes beside an interrupted tail the operator verb cannot prove
/// stranded, each held for its inputs and refused, typed, with nothing
/// written. A run the stranded prompt names cannot hold a tail of its own:
/// the store keeps one tail per session, and run A's holds it.
#[derive(Clone, Copy)]
enum Unproven {
    /// The tail's run cursor binds the row staged for another run.
    InTheTailsCursor,
    /// The stranded prompt names a run that committed.
    ForACommittedRun,
    /// The stranded prompt is bound to no run.
    Unattributed,
}

async fn the_operator_refuses_a_stranded_shape_it_cannot_prove(
    head_canonical: bool,
    unproven: Unproven,
) {
    use meerkat_runtime::recovery::{HeldTailResolution, resolve_held_durable_tail};

    let root = TempDir::new().expect("temp");
    let stores = Stores::new(head_canonical, &root);
    let client = Arc::new(ScriptedModel::new());
    let strand = match unproven {
        Unproven::InTheTailsCursor => Strand::ForANewRun,
        Unproven::ForACommittedRun => Strand::ForACommittedRun,
        Unproven::Unattributed => Strand::Unattributed,
    };
    let wedge = pre_release_wedge(&stores, &root, &client, strand).await;
    let id = &wedge.id;
    let store = stores.operator_store();
    if let Unproven::InTheTailsCursor = unproven {
        // Run A's own input, which its cursor binds, staged again for a run
        // that never committed.
        let mut row = wedge.run_a_row.clone();
        row.seed.last_run_id = Some(RunId::new());
        write_input_row(store.as_ref(), id, row).await;
    }

    let before = durable_witness(store.as_ref(), id, head_canonical).await;
    for apply in [false, true] {
        let resolution = resolve_held_durable_tail(store.as_ref(), id, apply)
            .await
            .expect("the operator reads the held tail");
        let HeldTailResolution::HeldOtherwise { reason } = &resolution else {
            panic!("the tail stays held for its inputs (apply {apply}): {resolution:?}");
        };
        assert!(reason.contains("input rows"), "{reason}");
        assert_eq!(
            durable_witness(store.as_ref(), id, head_canonical).await,
            before,
            "nothing is written (apply {apply})"
        );
    }
}

#[tokio::test]
async fn the_operator_refuses_a_row_of_the_tails_cursor_staged_for_another_run_whole_blob() {
    the_operator_refuses_a_stranded_shape_it_cannot_prove(false, Unproven::InTheTailsCursor).await;
}

#[tokio::test]
async fn the_operator_refuses_a_row_of_the_tails_cursor_staged_for_another_run_head_canonical() {
    the_operator_refuses_a_stranded_shape_it_cannot_prove(true, Unproven::InTheTailsCursor).await;
}

#[tokio::test]
async fn the_operator_refuses_a_prompt_staged_for_a_run_that_committed_whole_blob() {
    the_operator_refuses_a_stranded_shape_it_cannot_prove(false, Unproven::ForACommittedRun).await;
}

#[tokio::test]
async fn the_operator_refuses_a_prompt_staged_for_a_run_that_committed_head_canonical() {
    the_operator_refuses_a_stranded_shape_it_cannot_prove(true, Unproven::ForACommittedRun).await;
}

#[tokio::test]
async fn the_operator_refuses_an_unattributed_prompt_beside_the_tail_whole_blob() {
    the_operator_refuses_a_stranded_shape_it_cannot_prove(false, Unproven::Unattributed).await;
}

#[tokio::test]
async fn the_operator_refuses_an_unattributed_prompt_beside_the_tail_head_canonical() {
    the_operator_refuses_a_stranded_shape_it_cannot_prove(true, Unproven::Unattributed).await;
}
