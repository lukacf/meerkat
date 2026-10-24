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
