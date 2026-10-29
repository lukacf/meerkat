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
//! The inputs beside the closed run follow the tail's run cursor: the inputs
//! it binds end with the run, a steer its round commit listed included, and
//! every other input (one queued behind the run, a steer that never joined,
//! or one that joined after the last round commit) runs once after it.
//!
//! The operator rows start from the durable state a pre-release build of
//! 0.9.5 left: its interrupt kept the run's tail, and the next input stood
//! bound to a run that never committed. `resolve_held_durable_tail`
//! requeues that input only when the store proves it is not the tail's own,
//! and consumes with the tail an input the tail's cursor binds.

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
use meerkat_runtime::completion::{CompletionOutcome, CompletionWaitError};
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
    /// One call of the `probe` tool, once the test releases it.
    HeldTool,
    Text(&'static str),
}

pub(super) struct ScriptedModel {
    steps: StdMutex<VecDeque<Step>>,
    pub(super) requests: AtomicUsize,
    pub(super) entered: Semaphore,
    pub(super) release: Semaphore,
    /// When armed, the next `probe` call holds until the test releases it.
    tool_gate_armed: std::sync::atomic::AtomicBool,
    tool_entered: Semaphore,
    tool_release: Semaphore,
    /// Builds the builder refuses, from the next one on: a session whose
    /// agent build fails is refused by its materialization.
    refused_builds: AtomicUsize,
    /// The turn-boundary state of the agent the builder built last: the
    /// session's live agent.
    boundary: StdMutex<Option<meerkat_core::TransientTurnContextStateHandle>>,
}

impl ScriptedModel {
    pub(super) fn new() -> Self {
        Self {
            steps: StdMutex::new(VecDeque::new()),
            requests: AtomicUsize::new(0),
            entered: Semaphore::new(0),
            release: Semaphore::new(0),
            tool_gate_armed: std::sync::atomic::AtomicBool::new(false),
            tool_entered: Semaphore::new(0),
            tool_release: Semaphore::new(0),
            refused_builds: AtomicUsize::new(0),
            boundary: StdMutex::new(None),
        }
    }

    pub(super) fn push(&self, step: Step) {
        self.steps.lock().expect("steps").push_back(step);
    }

    /// Hold the next `probe` call until the test releases it.
    fn hold_next_tool_call(&self) {
        self.tool_gate_armed.store(true, Ordering::SeqCst);
    }

    /// Refuse the next agent build.
    fn refuse_next_build(&self) {
        self.refused_builds.fetch_add(1, Ordering::SeqCst);
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
            Step::HeldTool => {
                let events = vec![
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
                ];
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

struct ProbeTools {
    model: Arc<ScriptedModel>,
}

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
        if self.model.tool_gate_armed.swap(false, Ordering::SeqCst) {
            self.model.tool_entered.add_permits(1);
            self.model
                .tool_release
                .acquire()
                .await
                .expect("release the tool")
                .forget();
        }
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

    fn session_comms_identity_root(&self) -> meerkat_core::SessionCommsIdentityRoot {
        self.factory.session_comms_identity_root()
    }

    async fn build_agent(
        &self,
        request: &CreateSessionRequest,
        event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<FactoryAgent, SessionError> {
        if self
            .client
            .refused_builds
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |refused| {
                refused.checked_sub(1)
            })
            .is_ok()
        {
            return Err(SessionError::Agent(AgentError::InternalError(
                "the scripted build is refused".to_string(),
            )));
        }
        let mut config = AgentBuildConfig::from_create_session_request(request, event_tx);
        config.llm_client_override = Some(self.client.clone());
        config.tool_dispatcher_override = Some(Arc::new(ProbeTools {
            model: self.client.clone(),
        }));
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
        *self.client.boundary.lock().expect("boundary") =
            Some(agent.transient_turn_context_state());
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
        let created = Box::pin(crate::surface::materialize_session(
            &self.service,
            &self.machine,
            session,
            deferred_create_request(),
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

    /// Resolve the provisional tail the store holds before this start
    /// attaches the session, as a host does before its first committed read
    /// (`prepare_cold_attach`; a HeadCanonical tail only).
    async fn cold_attach(&self, id: &SessionId) {
        let claim = self
            .service
            .grant_session_hosting(id)
            .expect("the hosting claim");
        self.service
            .prepare_cold_attach(id, claim)
            .await
            .expect("the cold attach");
    }

    /// Restart the committed session `id` as a host does: the cold attach,
    /// the committed read and the materialization. Answers the first
    /// refusal, typed.
    async fn restart(&self, id: &SessionId) -> Result<(), SessionError> {
        let claim = self.service.grant_session_hosting(id)?;
        self.service.prepare_cold_attach(id, claim).await?;
        let session = self
            .service
            .load_authoritative_session(id)
            .await?
            .expect("the session is committed");
        match Box::pin(crate::surface::materialize_session(
            &self.service,
            &self.machine,
            session,
            deferred_create_request(),
            {
                let service = self.service.clone();
                let machine = self.machine.clone();
                move |id| crate::surface::default_persistent_executor(service, machine, id)
            },
        ))
        .await
        {
            Ok(_) => Ok(()),
            Err(crate::surface::SurfaceRuntimeMaterializeError::Session(error)) => Err(error),
            Err(error) => panic!("a restart is refused only as a session error: {error:?}"),
        }
    }

    /// Admit one prompt; the completion waiter of its turn.
    async fn admit(
        &self,
        id: &SessionId,
        prompt: &str,
    ) -> tokio::task::JoinHandle<Option<CompletionOutcome>> {
        self.admit_input(id, Input::Prompt(PromptInput::new(prompt, None)))
            .await
    }

    /// Admit one input; the completion waiter of its turn.
    async fn admit_input(
        &self,
        id: &SessionId,
        input: Input,
    ) -> tokio::task::JoinHandle<Option<CompletionOutcome>> {
        let waiter = self.admit_input_waiter(id, input).await;
        tokio::spawn(async move { waiter.await.expect("waiter task").and_then(Result::ok) })
    }

    /// Admit one input; its completion waiter's own answer, `None` when it
    /// does not answer within the deadline.
    async fn admit_input_waiter(
        &self,
        id: &SessionId,
        input: Input,
    ) -> tokio::task::JoinHandle<Option<Result<CompletionOutcome, CompletionWaitError>>> {
        let (_, completion) = self
            .machine
            .accept_input_with_completion(id, input)
            .await
            .expect("the runtime accepts the input");
        let completion = completion.expect("a completion");
        tokio::spawn(async move { tokio::time::timeout(DEADLINE, completion.wait()).await.ok() })
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
        transcript_rows(&session)
    }

    /// Wait until the runtime of `id` runs nothing and holds no input it has
    /// not ended, in its own ledger and in the store: whatever this start
    /// still held to run has run to its end, so a second run of an input
    /// shows in the committed history.
    async fn wait_runtime_settled(&self, id: &SessionId) {
        use meerkat_runtime::SessionServiceRuntimeExt as _;
        let deadline = tokio::time::Instant::now() + DEADLINE;
        loop {
            let state = self
                .machine
                .runtime_state(id)
                .await
                .expect("read the runtime state");
            let mut open = Vec::new();
            for row in input_rows(self.runtime_store.as_ref(), id).await {
                let live = self
                    .machine
                    .input_state(id, &row.state.input_id)
                    .await
                    .expect("read an input's live state");
                let ended_live = live
                    .as_ref()
                    .is_none_or(|live| live.seed.terminal_outcome.is_some());
                if !ended_live || row.seed.terminal_outcome.is_none() {
                    open.push((row.state.input_id.clone(), row.seed.phase));
                }
            }
            if open.is_empty()
                && matches!(
                    state,
                    meerkat_runtime::RuntimeState::Idle | meerkat_runtime::RuntimeState::Attached
                )
            {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "the runtime never settled: {state:?}, open inputs {open:?}"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// The committed transcript once `last` is its last row. A run that
    /// commits while this polls makes a committed read conflict for a moment
    /// (a HeadCanonical end-of-run save lands before its boundary), so a read
    /// that fails is read again.
    async fn committed_rows_ending_with(&self, id: &SessionId, last: &str) -> Vec<String> {
        let deadline = tokio::time::Instant::now() + DEADLINE;
        let mut seen = None;
        loop {
            if let Ok(Some(session)) = self.service.load_authoritative_session(id).await {
                let rows = transcript_rows(&session);
                if rows.last().map(String::as_str) == Some(last) {
                    return rows;
                }
                seen = Some(rows);
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "the committed transcript never ends with {last:?}: {seen:?}"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

/// A create request with no first turn, as every start here materializes.
fn deferred_create_request() -> CreateSessionRequest {
    CreateSessionRequest {
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
    }
}

/// One label per transcript row after the system prompt.
fn transcript_rows(session: &meerkat_core::Session) -> Vec<String> {
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
    NewRunPrompt,
    /// Staged for the session's earlier run, which committed: a boundary
    /// receipt names that run.
    CommittedRunPrompt,
    /// A steer queued again in its lane, still naming a run that never
    /// committed: it joined that run at a model boundary and the join was
    /// discarded when the run failed.
    NewRunDiscardedSteer,
    /// A peer message staged for a run that never started.
    NewRunPeerMessage,
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

/// Stamp `row` with `input`'s own admission policy and semantics, as the
/// runtime admitted it (`runtime_idle`: whether no run was running then).
fn stamp_own_admission(row: &mut StoredInputState, input: &Input, runtime_idle: bool) {
    let policy = meerkat_runtime::DefaultPolicyTable::resolve(input, runtime_idle);
    row.state.policy = Some(meerkat_runtime::input_state::PolicySnapshot {
        version: policy.policy_version,
        decision: policy,
    });
    row.state.runtime_semantics = Some(
        meerkat_runtime::ingress_types::RuntimeInputSemantics::try_from_generated_admission(
            input,
            runtime_idle,
        )
        .expect("generated admission semantics"),
    );
}

/// Recast a row staged for a run as a steer that joined that run at a model
/// boundary while it ran, and whose join was discarded when the run failed:
/// queued again in the steer lane, still naming the run.
fn as_discarded_join(row: &mut StoredInputState, input: &Input) {
    use meerkat_runtime::input_state::{InputLifecycleState as Phase, InputStateHistoryEntry};
    stamp_own_admission(row, input, false);
    let run = row.seed.last_run_id.clone().expect("the joined run");
    let entry = |from, to, reason: String| InputStateHistoryEntry {
        timestamp: chrono::Utc::now(),
        from,
        to,
        reason: Some(reason),
    };
    row.state.history = vec![
        entry(Phase::Accepted, Phase::Queued, "QueueAccepted".to_string()),
        entry(
            Phase::Queued,
            Phase::Staged,
            format!("JoinLiveBoundaryDurableAppend({run})"),
        ),
        entry(
            Phase::Staged,
            Phase::Queued,
            "ResolveLiveBoundaryDurableAppendJoin(AppliedDiscarded)".to_string(),
        ),
    ];
    row.seed.phase = Phase::Queued;
    row.seed.recovery_lane = Some(meerkat_core::types::HandlingMode::Steer);
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
        Strand::CommittedRunPrompt => {
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
        Strand::NewRunPrompt | Strand::NewRunDiscardedSteer | Strand::NewRunPeerMessage => None,
    };
    let (turn_a, run_a) = run_a_held_after_a_round_commit(&start, client, &id).await;
    let run_a_row = input_rows(start.runtime_store.as_ref(), &id)
        .await
        .into_iter()
        .find(|row| row.seed.last_run_id.as_ref() == Some(&run_a))
        .expect("run A's own input row");
    let bound_run = match strand {
        Strand::NewRunPrompt | Strand::NewRunDiscardedSteer | Strand::NewRunPeerMessage => {
            Some(RunId::new())
        }
        Strand::CommittedRunPrompt => {
            Some(committed_run.expect("the earlier run's input names its run"))
        }
    };
    let second = match strand {
        Strand::NewRunDiscardedSteer => steer("second"),
        Strand::NewRunPeerMessage => peer_message("second"),
        Strand::NewRunPrompt | Strand::CommittedRunPrompt => {
            Input::Prompt(PromptInput::new("second", None))
        }
    };
    let mut row = stranded_row(&run_a_row, &second, bound_run);
    match strand {
        Strand::NewRunDiscardedSteer => as_discarded_join(&mut row, &second),
        Strand::NewRunPeerMessage => stamp_own_admission(&mut row, &second, true),
        Strand::NewRunPrompt | Strand::CommittedRunPrompt => {}
    }
    write_input_row(start.runtime_store.as_ref(), &id, row).await;
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

/// A pre-release interrupt left run A's tail held beside the next input,
/// bound to a run that never committed (`strand`). The operator's dry run
/// names that shape and the input it requeues and writes nothing; the repair
/// closes run A as interrupted and requeues the input; a second repair finds
/// no tail. The next start runs the input exactly once (it lands as
/// `input_row`).
///
/// With `restart_first`, a host restarts the session before the operator
/// acts. The restart is refused, typed, as held for recovery, and its
/// rollback leaves the stranded input as the store held it, not ended, so
/// the hold stands and the repair finds and requeues the input. RED
/// before: the refused restart's rollback stopped the runtime with the
/// Stop disposition, which ended the input as stopped; the hold went with
/// it, and the next start never ran the input.
async fn an_operator_requeues_the_input_stranded_beside_a_pre_release_interrupt(
    head_canonical: bool,
    strand: Strand,
    input_row: &str,
    restart_first: bool,
) {
    use meerkat_runtime::input_state::InputLifecycleState as Phase;
    use meerkat_runtime::recovery::{
        HeldTailRepairVerdict, HeldTailResolution, resolve_held_durable_tail,
    };

    let root = TempDir::new().expect("temp");
    let stores = Stores::new(head_canonical, &root);
    let client = Arc::new(ScriptedModel::new());
    let wedge = pre_release_wedge(&stores, &root, &client, strand).await;
    let id = &wedge.id;

    if restart_first {
        let start = Start::open(&stores, &root, &client);
        let refused = start.restart(id).await;
        assert!(
            matches!(
                refused,
                Err(SessionError::DurableTailHeldForRecovery { .. })
            ),
            "a restart before the repair is refused as held for recovery: {refused:?}"
        );
        start.service.try_shutdown().await.expect("shutdown");
        drop(start);
        let store = stores.operator_store();
        let second = input_rows(store.as_ref(), id)
            .await
            .into_iter()
            .find(|row| row.state.input_id == wedge.second)
            .expect("the stranded input's row");
        assert_eq!(
            second.seed.terminal_outcome, None,
            "the refused restart leaves the stranded input open: {:?}",
            second.state.history
        );
    }

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
    let rows = start
        .committed_rows_ending_with(id, "assistant second answer")
        .await;
    assert_eq!(
        client.requests.load(Ordering::SeqCst),
        requests_before + 1,
        "the requeued input calls the model once"
    );
    let mut expected = history_with_closed_run_a("second", "second answer");
    expected[4] = input_row.to_string();
    assert_eq!(
        rows, expected,
        "run A is closed once, and the requeued input runs once after it"
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
    an_operator_requeues_the_input_stranded_beside_a_pre_release_interrupt(
        false,
        Strand::NewRunPrompt,
        "user second",
        false,
    )
    .await;
}

#[tokio::test]
async fn an_operator_requeues_the_prompt_stranded_beside_a_pre_release_interrupt_head_canonical() {
    an_operator_requeues_the_input_stranded_beside_a_pre_release_interrupt(
        true,
        Strand::NewRunPrompt,
        "user second",
        false,
    )
    .await;
}

#[tokio::test]
async fn an_operator_requeues_a_discarded_steer_beside_a_pre_release_interrupt_whole_blob() {
    an_operator_requeues_the_input_stranded_beside_a_pre_release_interrupt(
        false,
        Strand::NewRunDiscardedSteer,
        "user second",
        false,
    )
    .await;
}

#[tokio::test]
async fn an_operator_requeues_a_discarded_steer_beside_a_pre_release_interrupt_head_canonical() {
    an_operator_requeues_the_input_stranded_beside_a_pre_release_interrupt(
        true,
        Strand::NewRunDiscardedSteer,
        "user second",
        false,
    )
    .await;
}

#[tokio::test]
async fn an_operator_requeues_a_peer_message_stranded_beside_an_interrupt_whole_blob() {
    an_operator_requeues_the_input_stranded_beside_a_pre_release_interrupt(
        false,
        Strand::NewRunPeerMessage,
        "notice Comms",
        false,
    )
    .await;
}

#[tokio::test]
async fn an_operator_requeues_a_peer_message_stranded_beside_an_interrupt_head_canonical() {
    an_operator_requeues_the_input_stranded_beside_a_pre_release_interrupt(
        true,
        Strand::NewRunPeerMessage,
        "notice Comms",
        false,
    )
    .await;
}

#[tokio::test]
async fn a_restart_before_the_operator_keeps_the_stranded_prompt_whole_blob() {
    an_operator_requeues_the_input_stranded_beside_a_pre_release_interrupt(
        false,
        Strand::NewRunPrompt,
        "user second",
        true,
    )
    .await;
}

#[tokio::test]
async fn a_restart_before_the_operator_keeps_the_discarded_steer_whole_blob() {
    an_operator_requeues_the_input_stranded_beside_a_pre_release_interrupt(
        false,
        Strand::NewRunDiscardedSteer,
        "user second",
        true,
    )
    .await;
}

/// HeadCanonical twin: the cold attach refuses the restart, typed, as held
/// for recovery (before any committed read), and the hold stands for the
/// operator.
#[tokio::test]
async fn a_restart_before_the_operator_keeps_the_stranded_prompt_head_canonical() {
    an_operator_requeues_the_input_stranded_beside_a_pre_release_interrupt(
        true,
        Strand::NewRunPrompt,
        "user second",
        true,
    )
    .await;
}

#[tokio::test]
async fn a_restart_before_the_operator_keeps_the_discarded_steer_head_canonical() {
    an_operator_requeues_the_input_stranded_beside_a_pre_release_interrupt(
        true,
        Strand::NewRunDiscardedSteer,
        "user second",
        true,
    )
    .await;
}

/// A prompt an earlier process left queued, with no tail standing. A
/// restart whose agent build fails is refused before the session serves,
/// and its rollback keeps the prompt as the store holds it: not ended, still
/// queued and with no requeue mark. The next start runs it exactly once.
/// RED before: the refused restart's rollback stopped the runtime with the
/// Stop disposition, which ended the recovered prompt as stopped.
async fn a_refused_restart_keeps_a_recovered_prompt(head_canonical: bool) {
    use meerkat_runtime::input_state::InputLifecycleState as Phase;

    let root = TempDir::new().expect("temp");
    let stores = Stores::new(head_canonical, &root);
    let client = Arc::new(ScriptedModel::new());
    let id = {
        let start = Start::open(&stores, &root, &client);
        let id = start.materialize(None).await;
        client.push(Step::Text("first answer"));
        let outcome = start.turn(&id, "first").await;
        assert!(
            matches!(outcome, Some(CompletionOutcome::Completed(_))),
            "the first turn commits: {outcome:?}"
        );
        start.service.try_shutdown().await.expect("shutdown");
        id
    };
    let second = {
        let store = stores.operator_store();
        let first_row = input_rows(store.as_ref(), &id)
            .await
            .into_iter()
            .next()
            .expect("the first turn's row");
        let row = stranded_row(
            &first_row,
            &Input::Prompt(PromptInput::new("second", None)),
            None,
        );
        write_input_row(store.as_ref(), &id, row.clone()).await;
        row.state.input_id
    };

    client.refuse_next_build();
    {
        let start = Start::open(&stores, &root, &client);
        let refused = start.restart(&id).await;
        assert!(
            matches!(refused, Err(SessionError::Agent(_))),
            "the restart is refused by its agent build: {refused:?}"
        );
        start.service.try_shutdown().await.expect("shutdown");
    }
    {
        let store = stores.operator_store();
        let row = input_rows(store.as_ref(), &id)
            .await
            .into_iter()
            .find(|row| row.state.input_id == second)
            .expect("the queued prompt's row");
        assert_eq!(
            row.seed.terminal_outcome, None,
            "the refused restart leaves the recovered prompt open: {:?}",
            row.state.history
        );
        assert_eq!(row.seed.phase, Phase::Queued, "the prompt stays queued");
        assert!(
            row.state.stop_requeue.is_none(),
            "the prompt is kept as the store holds it, with no requeue mark"
        );
    }

    let start = Start::open(&stores, &root, &client);
    let requests_before = client.requests.load(Ordering::SeqCst);
    client.push(Step::Text("second answer"));
    start.cold_attach(&id).await;
    start.materialize(Some(&id)).await;
    let rows = start
        .committed_rows_ending_with(&id, "assistant second answer")
        .await;
    start.wait_runtime_settled(&id).await;
    assert_eq!(
        client.requests.load(Ordering::SeqCst),
        requests_before + 1,
        "the kept prompt calls the model once"
    );
    assert_eq!(
        rows,
        [
            "user first",
            "assistant first answer",
            "user second",
            "assistant second answer",
        ],
        "the kept prompt runs once after the refused restart"
    );
}

#[tokio::test]
async fn a_refused_restart_keeps_a_recovered_prompt_whole_blob() {
    a_refused_restart_keeps_a_recovered_prompt(false).await;
}

#[tokio::test]
async fn a_refused_restart_keeps_a_recovered_prompt_head_canonical() {
    a_refused_restart_keeps_a_recovered_prompt(true).await;
}

/// The control: a materialization's rollback after an input was admitted to
/// its registration still stops. The admitted prompt ends as stopped, so a
/// turn its caller saw refused never runs on a later start.
async fn a_rollback_after_admission_still_stops_the_admitted_input(head_canonical: bool) {
    use meerkat_runtime::input_state::{InputAbandonReason, InputTerminalOutcome};

    let root = TempDir::new().expect("temp");
    let stores = Stores::new(head_canonical, &root);
    let client = Arc::new(ScriptedModel::new());
    let id = {
        let start = Start::open(&stores, &root, &client);
        let id = start.materialize(None).await;
        client.push(Step::Text("first answer"));
        let outcome = start.turn(&id, "first").await;
        assert!(
            matches!(outcome, Some(CompletionOutcome::Completed(_))),
            "the first turn commits: {outcome:?}"
        );
        start.service.try_shutdown().await.expect("shutdown");
        id
    };

    let admitted = {
        let start = Start::open(&stores, &root, &client);
        start.cold_attach(&id).await;
        let mut prepared = start
            .machine
            .prepare_session_materialization(id.clone())
            .await
            .expect("prepare the registration");
        let input = Input::Prompt(PromptInput::new("admitted", None));
        let admitted = input.id().clone();
        start
            .machine
            .accept_input_with_completion(&id, input)
            .await
            .expect("the registration admits the prompt");
        prepared
            .rollback_now()
            .await
            .expect("roll the registration back");
        start.service.try_shutdown().await.expect("shutdown");
        admitted
    };
    {
        let store = stores.operator_store();
        let row = input_rows(store.as_ref(), &id)
            .await
            .into_iter()
            .find(|row| row.state.input_id == admitted)
            .expect("the admitted prompt's row");
        assert_eq!(
            row.seed.terminal_outcome,
            Some(InputTerminalOutcome::Abandoned {
                reason: InputAbandonReason::Stopped
            }),
            "the rollback stops the prompt it admitted: {:?}",
            row.state.history
        );
    }

    let start = Start::open(&stores, &root, &client);
    let requests_before = client.requests.load(Ordering::SeqCst);
    start.cold_attach(&id).await;
    start.materialize(Some(&id)).await;
    start.wait_runtime_settled(&id).await;
    assert_eq!(
        client.requests.load(Ordering::SeqCst),
        requests_before,
        "the stopped prompt never runs"
    );
    assert_eq!(
        start.committed_rows(&id).await,
        ["user first", "assistant first answer"]
    );
}

#[tokio::test]
async fn a_rollback_after_admission_still_stops_the_admitted_input_whole_blob() {
    a_rollback_after_admission_still_stops_the_admitted_input(false).await;
}

#[tokio::test]
async fn a_rollback_after_admission_still_stops_the_admitted_input_head_canonical() {
    a_rollback_after_admission_still_stops_the_admitted_input(true).await;
}

/// The first lifetime of a keeping-rollback row: the session's first turn
/// commits, and the process stops.
async fn keeping_rollback_first_turn(
    stores: &Stores,
    root: &TempDir,
    client: &Arc<ScriptedModel>,
) -> SessionId {
    let start = Start::open(stores, root, client);
    let id = start.materialize(None).await;
    client.push(Step::Text("first answer"));
    let outcome = start.turn(&id, "first").await;
    assert!(
        matches!(outcome, Some(CompletionOutcome::Completed(_))),
        "the first turn commits: {outcome:?}"
    );
    start.service.try_shutdown().await.expect("shutdown");
    id
}

/// Leave `input` queued in the store behind the first turn, as a process that
/// stopped before running it would.
async fn keeping_rollback_queued_row(
    stores: &Stores,
    id: &SessionId,
    input: &Input,
) -> meerkat_core::InputId {
    let store = stores.operator_store();
    let first_row = input_rows(store.as_ref(), id)
        .await
        .into_iter()
        .next()
        .expect("the first turn's row");
    let row = stranded_row(&first_row, input, None);
    write_input_row(store.as_ref(), id, row.clone()).await;
    row.state.input_id
}

/// Each of `inputs` ended as stopped, with no requeue mark.
async fn assert_stopped_by_the_rollback(
    stores: &Stores,
    id: &SessionId,
    inputs: &[(&meerkat_core::InputId, &str)],
) {
    use meerkat_runtime::input_state::{InputAbandonReason, InputTerminalOutcome};

    let store = stores.operator_store();
    let rows = input_rows(store.as_ref(), id).await;
    for (input_id, what) in inputs {
        let row = rows
            .iter()
            .find(|row| &row.state.input_id == *input_id)
            .unwrap_or_else(|| panic!("the {what} prompt's row"));
        assert_eq!(
            row.seed.terminal_outcome,
            Some(InputTerminalOutcome::Abandoned {
                reason: InputAbandonReason::Stopped
            }),
            "the rollback stops the {what} prompt: {:?}",
            row.state.history
        );
        assert!(
            row.state.stop_requeue.is_none(),
            "the {what} prompt has no requeue mark"
        );
    }
}

/// The next start runs nothing: no model call, and the transcript stays the
/// first turn's.
async fn assert_next_start_runs_nothing(
    stores: &Stores,
    root: &TempDir,
    client: &Arc<ScriptedModel>,
    id: &SessionId,
) {
    let start = Start::open(stores, root, client);
    let requests_before = client.requests.load(Ordering::SeqCst);
    start.cold_attach(id).await;
    start.materialize(Some(id)).await;
    start.wait_runtime_settled(id).await;
    assert_eq!(
        client.requests.load(Ordering::SeqCst),
        requests_before,
        "no stopped prompt runs"
    );
    assert_eq!(
        start.committed_rows(id).await,
        ["user first", "assistant first answer"]
    );
}

/// A keeping rollback keeps only what its registration recovered, and only
/// while the registration served no live work. Once it admitted an input,
/// the machine records the unregister as Stop: the prompt an earlier process
/// left queued and the prompt admitted to the rolled-back registration both
/// end as stopped, and neither runs on the next start. RED before: the keep
/// left the recovered prompt queued, and the next start ran it.
async fn a_keeping_rollback_after_an_admission_keeps_nothing(head_canonical: bool) {
    let root = TempDir::new().expect("temp");
    let stores = Stores::new(head_canonical, &root);
    let client = Arc::new(ScriptedModel::new());
    let id = keeping_rollback_first_turn(&stores, &root, &client).await;
    let recovered = keeping_rollback_queued_row(
        &stores,
        &id,
        &Input::Prompt(PromptInput::new("second", None)),
    )
    .await;

    let admitted = {
        let start = Start::open(&stores, &root, &client);
        start.cold_attach(&id).await;
        let mut prepared = start
            .machine
            .prepare_session_materialization(id.clone())
            .await
            .expect("prepare the registration");
        let input = Input::Prompt(PromptInput::new("admitted", None));
        let admitted = input.id().clone();
        start
            .machine
            .accept_input_with_completion(&id, input)
            .await
            .expect("the registration admits the prompt");
        prepared
            .rollback_now()
            .await
            .expect("roll the registration back");
        start.service.try_shutdown().await.expect("shutdown");
        admitted
    };
    assert_stopped_by_the_rollback(
        &stores,
        &id,
        &[(&recovered, "recovered"), (&admitted, "admitted")],
    )
    .await;
    assert_next_start_runs_nothing(&stores, &root, &client, &id).await;
}

#[tokio::test]
async fn a_keeping_rollback_after_an_admission_keeps_nothing_whole_blob() {
    a_keeping_rollback_after_an_admission_keeps_nothing(false).await;
}

#[tokio::test]
async fn a_keeping_rollback_after_an_admission_keeps_nothing_head_canonical() {
    a_keeping_rollback_after_an_admission_keeps_nothing(true).await;
}

/// A host's recycle in the prepared window recovers every active input again,
/// the prompt admitted to the registration included, so the registration's
/// recovered set no longer holds only what it recovered from the store. The
/// keeping rollback after it keeps nothing: the admitted prompt ends as
/// stopped and never runs on a later start, so its caller, who saw the
/// materialization refused, never sees its turn run. RED before: the recycle
/// put the admitted prompt among the recovered ones, the keep left it queued,
/// and the next start ran it.
async fn a_keeping_rollback_after_a_recycle_stops_the_admitted_input(head_canonical: bool) {
    let root = TempDir::new().expect("temp");
    let stores = Stores::new(head_canonical, &root);
    let client = Arc::new(ScriptedModel::new());
    let id = keeping_rollback_first_turn(&stores, &root, &client).await;

    let admitted = {
        let start = Start::open(&stores, &root, &client);
        start.cold_attach(&id).await;
        let mut prepared = start
            .machine
            .prepare_session_materialization(id.clone())
            .await
            .expect("prepare the registration");
        let input = Input::Prompt(PromptInput::new("admitted", None));
        let admitted = input.id().clone();
        start
            .machine
            .accept_input_with_completion(&id, input)
            .await
            .expect("the registration admits the prompt");
        meerkat_runtime::RuntimeControlPlane::recycle(
            start.machine.as_ref(),
            &LogicalRuntimeId::for_session(&id),
        )
        .await
        .expect("a host recycles the runtime in the prepared window");
        prepared
            .rollback_now()
            .await
            .expect("roll the registration back");
        start.service.try_shutdown().await.expect("shutdown");
        admitted
    };
    assert_stopped_by_the_rollback(&stores, &id, &[(&admitted, "admitted")]).await;
    assert_next_start_runs_nothing(&stores, &root, &client, &id).await;
}

#[tokio::test]
async fn a_keeping_rollback_after_a_recycle_stops_the_admitted_input_whole_blob() {
    a_keeping_rollback_after_a_recycle_stops_the_admitted_input(false).await;
}

#[tokio::test]
async fn a_keeping_rollback_after_a_recycle_stops_the_admitted_input_head_canonical() {
    a_keeping_rollback_after_a_recycle_stops_the_admitted_input(true).await;
}

/// A resubmission deduplicated onto a prompt the registration recovered
/// waits on that prompt. The keeping rollback after it keeps nothing: the
/// recovered prompt ends as stopped and never runs on a later start, so the
/// resubmitting caller, who saw the materialization refused, never sees the
/// turn run. The persistent store answers the duplicate before the machine's
/// own admission would; its verdict still goes through the machine. RED
/// before: the keep left the recovered prompt queued, and the next start ran
/// it.
async fn a_keeping_rollback_after_a_deduplicated_resubmission_keeps_nothing(head_canonical: bool) {
    let root = TempDir::new().expect("temp");
    let stores = Stores::new(head_canonical, &root);
    let client = Arc::new(ScriptedModel::new());
    let id = keeping_rollback_first_turn(&stores, &root, &client).await;
    let keyed_prompt = || {
        let mut prompt = PromptInput::new("second", None);
        prompt.header.idempotency_key =
            Some(meerkat_runtime::IdempotencyKey::new("resubmitted-second"));
        Input::Prompt(prompt)
    };
    let recovered = keeping_rollback_queued_row(&stores, &id, &keyed_prompt()).await;

    {
        let start = Start::open(&stores, &root, &client);
        start.cold_attach(&id).await;
        let mut prepared = start
            .machine
            .prepare_session_materialization(id.clone())
            .await
            .expect("prepare the registration");
        let (outcome, _completion) = start
            .machine
            .accept_input_with_completion(&id, keyed_prompt())
            .await
            .expect("the resubmission is answered");
        assert!(
            matches!(
                &outcome,
                meerkat_runtime::AcceptOutcome::Deduplicated { existing_id, .. }
                    if existing_id == &recovered
            ),
            "the resubmission waits on the recovered prompt: {outcome:?}"
        );
        prepared
            .rollback_now()
            .await
            .expect("roll the registration back");
        start.service.try_shutdown().await.expect("shutdown");
    }
    assert_stopped_by_the_rollback(&stores, &id, &[(&recovered, "recovered")]).await;
    assert_next_start_runs_nothing(&stores, &root, &client, &id).await;
}

#[tokio::test]
async fn a_keeping_rollback_after_a_deduplicated_resubmission_keeps_nothing_whole_blob() {
    a_keeping_rollback_after_a_deduplicated_resubmission_keeps_nothing(false).await;
}

#[tokio::test]
async fn a_keeping_rollback_after_a_deduplicated_resubmission_keeps_nothing_head_canonical() {
    a_keeping_rollback_after_a_deduplicated_resubmission_keeps_nothing(true).await;
}

/// A prompt staged for a run that committed is not proven stranded: that
/// run's boundary receipt shows it committed, so the prompt's content may be
/// in the committed history. The tail stays held for its inputs and the
/// operator verb refuses, typed, with nothing written. A run the stranded
/// prompt names cannot hold a tail of its own: the store keeps one tail per
/// session, and run A's holds it.
async fn the_operator_refuses_a_prompt_staged_for_a_run_that_committed(head_canonical: bool) {
    use meerkat_runtime::recovery::{HeldTailResolution, resolve_held_durable_tail};

    let root = TempDir::new().expect("temp");
    let stores = Stores::new(head_canonical, &root);
    let client = Arc::new(ScriptedModel::new());
    let wedge = pre_release_wedge(&stores, &root, &client, Strand::CommittedRunPrompt).await;
    let id = &wedge.id;
    let store = stores.operator_store();

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
async fn the_operator_refuses_a_prompt_staged_for_a_run_that_committed_whole_blob() {
    the_operator_refuses_a_prompt_staged_for_a_run_that_committed(false).await;
}

#[tokio::test]
async fn the_operator_refuses_a_prompt_staged_for_a_run_that_committed_head_canonical() {
    the_operator_refuses_a_prompt_staged_for_a_run_that_committed(true).await;
}

/// A row the tail's run cursor binds is the tail's own content, even when it
/// was staged again for a run that never committed: the cursor binds exactly
/// the inputs whose content the tail holds. Beside a prompt stranded for
/// another run, the operator's repair consumes that row with the tail, once,
/// and requeues the stranded prompt. Run A is committed as interrupted, its
/// input's content appears once in history, the stranded prompt runs once
/// after it, and the next turn completes. Before the cursor attribution the
/// verb refused this shape, which left a HeadCanonical session held.
async fn an_operator_repair_consumes_a_cursor_bound_row_staged_for_another_run_once(
    head_canonical: bool,
) {
    use meerkat_runtime::input_state::{InputLifecycleState as Phase, InputTerminalOutcome};
    use meerkat_runtime::recovery::{
        HeldTailRepairVerdict, HeldTailResolution, resolve_held_durable_tail,
    };

    let root = TempDir::new().expect("temp");
    let stores = Stores::new(head_canonical, &root);
    let client = Arc::new(ScriptedModel::new());
    let wedge = pre_release_wedge(&stores, &root, &client, Strand::NewRunPrompt).await;
    let id = &wedge.id;
    let run_a_input = wedge.run_a_row.state.input_id.clone();
    let store = stores.operator_store();
    // Run A's own input, which its cursor binds, staged again for a run that
    // never committed.
    let mut row = wedge.run_a_row.clone();
    row.seed.last_run_id = Some(RunId::new());
    write_input_row(store.as_ref(), id, row).await;

    let HeldTailResolution::StrandedInputs(plan) =
        resolve_held_durable_tail(store.as_ref(), id, false)
            .await
            .expect("the dry run reads the held tail")
    else {
        panic!("the tail is held beside a stranded prompt");
    };
    assert_eq!(plan.verdict, HeldTailRepairVerdict::WouldRepair);
    assert_eq!(plan.run_id, wedge.run_a);
    assert_eq!(
        plan.terminalized_inputs,
        vec![run_a_input.clone()],
        "the repair consumes the row the cursor binds"
    );
    assert_eq!(plan.requeued_inputs, vec![wedge.second.clone()]);

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
    let rows = input_rows(store.as_ref(), id).await;
    let consumed = rows
        .iter()
        .find(|row| row.state.input_id == run_a_input)
        .expect("run A's own input row");
    assert_eq!(
        consumed.seed.phase,
        Phase::Consumed,
        "consumed with the tail"
    );
    assert_eq!(
        consumed.seed.terminal_outcome,
        Some(InputTerminalOutcome::Consumed)
    );
    assert_eq!(consumed.seed.last_run_id.as_ref(), Some(&wedge.run_a));
    let second = rows
        .iter()
        .find(|row| row.state.input_id == wedge.second)
        .expect("the stranded prompt's row");
    assert_eq!(second.seed.phase, Phase::Queued);
    assert_eq!(second.seed.last_run_id, None);
    drop(store);

    let start = Start::open(&stores, &root, &client);
    let requests_before = client.requests.load(Ordering::SeqCst);
    client.push(Step::Text("second answer"));
    start.materialize(Some(id)).await;
    let rows = start
        .committed_rows_ending_with(id, "assistant second answer")
        .await;
    assert_eq!(
        client.requests.load(Ordering::SeqCst),
        requests_before + 1,
        "only the requeued prompt calls the model: run A's input does not run again"
    );
    assert_eq!(
        rows,
        history_with_closed_run_a("second", "second answer"),
        "run A is closed once with its input's content, and the requeued prompt runs once"
    );
    client.push(Step::Text("third answer"));
    let outcome = start.turn(id, "third").await;
    assert!(
        matches!(outcome, Some(CompletionOutcome::Completed(ref run)) if run.text == "third answer"),
        "the next turn completes: {outcome:?}"
    );
}

#[tokio::test]
async fn an_operator_repair_consumes_a_cursor_bound_row_staged_for_another_run_once_whole_blob() {
    an_operator_repair_consumes_a_cursor_bound_row_staged_for_another_run_once(false).await;
}

#[tokio::test]
async fn an_operator_repair_consumes_a_cursor_bound_row_staged_for_another_run_once_head_canonical()
{
    an_operator_repair_consumes_a_cursor_bound_row_staged_for_another_run_once(true).await;
}

/// A prompt queued beside run A and bound to no run, as a pre-release build
/// of 0.9.5 could leave it, is not in A's tail: the tail's run cursor does
/// not list it, and it names no run. The fixture writes its row while A is
/// held. The prompt stays queued with its content, the operator verb finds
/// no tail to resolve, and the next start runs the prompt once.
/// - With `cold` false the cancelled run's settle closes A's tail as
///   interrupted beside the prompt. Before the cursor attribution the settle
///   held A's tail beside it, and the operator verb refused to repair that
///   shape.
/// - With `cold` true a row naming another run stands beside the tail too,
///   so the settle holds; once that row has ended, the next start's ordinary
///   recovery (a restart's) closes A's tail beside the prompt. RED before:
///   that recovery held the tail beside the prompt as well.
async fn an_unattributed_prompt_beside_an_interrupted_tail_runs_once_after_it(
    head_canonical: bool,
    cold: bool,
) {
    use meerkat_runtime::input_state::{
        InputAbandonReason, InputLifecycleState as Phase, InputStateHistoryEntry,
        InputTerminalOutcome,
    };
    use meerkat_runtime::recovery::{HeldTailResolution, resolve_held_durable_tail};

    let root = TempDir::new().expect("temp");
    let stores = Stores::new(head_canonical, &root);
    let client = Arc::new(ScriptedModel::new());
    let (id, second, other_runs_row) = {
        let start = Start::open(&stores, &root, &client);
        let id = start.materialize(None).await;
        let (turn_a, run_a) = run_a_held_after_a_round_commit(&start, &client, &id).await;
        let run_a_row = input_rows(start.runtime_store.as_ref(), &id)
            .await
            .into_iter()
            .find(|row| row.seed.last_run_id.as_ref() == Some(&run_a))
            .expect("run A's own input row");
        let second = Input::Prompt(PromptInput::new("second", None));
        write_input_row(
            start.runtime_store.as_ref(),
            &id,
            stranded_row(&run_a_row, &second, None),
        )
        .await;
        let other_runs_row = cold.then(|| {
            let mut row = stranded_row(
                &run_a_row,
                &Input::Prompt(PromptInput::new("another run's", None)),
                Some(RunId::new()),
            );
            row.seed.admission_sequence = row.seed.admission_sequence.map(|sequence| sequence + 1);
            row
        });
        if let Some(row) = &other_runs_row {
            write_input_row(start.runtime_store.as_ref(), &id, row.clone()).await;
        }
        interrupt_run_a(&start, &id, turn_a).await;
        if cold {
            assert_eq!(
                start.tail_run(&id).await,
                Some(run_a.clone()),
                "the settle holds run A's tail beside the other run's row"
            );
        } else {
            assert_eq!(
                start.tail_run(&id).await,
                None,
                "the settle closes run A's tail beside the unattributed prompt"
            );
        }
        start.service.try_shutdown().await.expect("shutdown");
        (id, second.id().clone(), other_runs_row)
    };

    let store = stores.operator_store();
    if let Some(mut ended) = other_runs_row {
        // That row ends (an operator's part: the fixture writes its terminal
        // row); the tail is then no longer held.
        ended.state.history.push(InputStateHistoryEntry {
            timestamp: chrono::Utc::now(),
            from: Phase::Staged,
            to: Phase::Abandoned,
            reason: Some("Abandon".to_string()),
        });
        ended.seed.phase = Phase::Abandoned;
        ended.seed.terminal_outcome = Some(InputTerminalOutcome::Abandoned {
            reason: InputAbandonReason::Cancelled,
        });
        ended.seed.recovery_lane = None;
        write_input_row(store.as_ref(), &id, ended).await;
        assert!(
            matches!(
                resolve_held_durable_tail(store.as_ref(), &id, false)
                    .await
                    .expect("the operator reads the store"),
                HeldTailResolution::NotHeld { .. }
            ),
            "ordinary recovery commits the tail beside the queued prompt"
        );
    }
    let rows = input_rows(store.as_ref(), &id).await;
    let row = rows
        .iter()
        .find(|row| row.state.input_id == second)
        .expect("the prompt's row");
    assert_eq!(row.seed.phase, Phase::Queued, "the prompt stays queued");
    assert_eq!(row.seed.last_run_id, None, "the prompt is bound to no run");
    assert!(
        row.state.persisted_input.is_some(),
        "the prompt keeps its content"
    );
    if !cold {
        assert_eq!(
            resolve_held_durable_tail(store.as_ref(), &id, false)
                .await
                .expect("the operator reads the store"),
            HeldTailResolution::NoTail,
            "the operator finds no tail"
        );
    }
    drop(store);

    let start = Start::open(&stores, &root, &client);
    let requests_before = client.requests.load(Ordering::SeqCst);
    client.push(Step::Text("second answer"));
    start.cold_attach(&id).await;
    start.materialize(Some(&id)).await;
    let rows = start
        .committed_rows_ending_with(&id, "assistant second answer")
        .await;
    assert_eq!(
        client.requests.load(Ordering::SeqCst),
        requests_before + 1,
        "the queued prompt calls the model once"
    );
    assert_eq!(
        rows,
        history_with_closed_run_a("second", "second answer"),
        "run A is closed once, and the queued prompt runs once after it"
    );
}

#[tokio::test]
async fn an_unattributed_prompt_beside_an_interrupted_tail_runs_once_after_it_whole_blob() {
    an_unattributed_prompt_beside_an_interrupted_tail_runs_once_after_it(false, false).await;
}

#[tokio::test]
async fn an_unattributed_prompt_beside_an_interrupted_tail_runs_once_after_it_head_canonical() {
    an_unattributed_prompt_beside_an_interrupted_tail_runs_once_after_it(true, false).await;
}

#[tokio::test]
async fn a_prompt_queued_beside_an_interrupted_tail_runs_once_after_a_cold_recovery_whole_blob() {
    an_unattributed_prompt_beside_an_interrupted_tail_runs_once_after_it(false, true).await;
}

#[tokio::test]
async fn a_prompt_queued_beside_an_interrupted_tail_runs_once_after_a_cold_recovery_head_canonical()
{
    an_unattributed_prompt_beside_an_interrupted_tail_runs_once_after_it(true, true).await;
}

/// A steer prompt: it joins a running run only at the run's model boundary.
fn steer(prompt: &str) -> Input {
    Input::Prompt(PromptInput::new(
        prompt,
        Some(
            meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
                ..Default::default()
            },
        ),
    ))
}

/// A peer message, queued like a prompt.
fn peer_message(body: &str) -> Input {
    use meerkat_runtime::input::{
        InputDurability, InputHeader, InputOrigin, InputVisibility, PeerConvention, PeerInput,
    };
    Input::Peer(PeerInput {
        directed_interaction_id: None,
        objective_id: None,
        system_prompts: Vec::new(),
        injected_context: Vec::new(),
        sender_taint: None,
        header: InputHeader {
            ingress_context: None,
            retained_resume: None,
            authority_association: None,
            id: meerkat_core::lifecycle::InputId::new(),
            timestamp: chrono::Utc::now(),
            source: InputOrigin::Peer {
                peer_id: "peer-1".into(),
                display_identity: None,
                runtime_id: None,
            },
            durability: InputDurability::Durable,
            visibility: InputVisibility::default(),
            idempotency_key: None,
            supersession_key: None,
            correlation_id: None,
        },
        convention: Some(PeerConvention::Message),
        content: body.into(),
        payload: None,
        handling_mode: None,
        in_reply_to: None,
    })
}

/// Interrupt run A; it ends cancelled and its provisional tail is closed.
async fn interrupt_run_a(
    start: &Start,
    id: &SessionId,
    turn_a: tokio::task::JoinHandle<Option<CompletionOutcome>>,
) {
    start
        .machine
        .hard_cancel_current_run(id, "user interrupt")
        .await
        .expect("the interrupt reaches run A");
    let outcome_a = turn_a.await.expect("turn A task");
    assert!(
        matches!(outcome_a, Some(CompletionOutcome::Cancelled)),
        "run A ends cancelled: {outcome_a:?}"
    );
}

/// An input queued behind run A when A is interrupted after a round commit
/// runs once, after A's tail is closed as interrupted. RED before: the
/// cancellation's tail recovery read the queued row as content the tail
/// might hold and held the tail, so the queued input's turn was refused
/// under it.
async fn an_input_queued_at_an_interrupt_runs_once_after_it(
    head_canonical: bool,
    queued: Input,
    expected_rows: &[&str],
) {
    let root = TempDir::new().expect("temp");
    let stores = Stores::new(head_canonical, &root);
    let client = Arc::new(ScriptedModel::new());
    let start = Start::open(&stores, &root, &client);
    let id = start.materialize(None).await;
    let (turn_a, _run_a) = run_a_held_after_a_round_commit(&start, &client, &id).await;
    client.push(Step::Text("queued answer"));
    let queued = start.admit_input(&id, queued).await;

    interrupt_run_a(&start, &id, turn_a).await;
    let outcome = queued.await.expect("queued turn task");
    assert!(
        matches!(outcome, Some(CompletionOutcome::Completed(ref run)) if run.text == "queued answer"),
        "the queued input runs after the interrupt: {outcome:?}"
    );
    assert_eq!(start.tail_run(&id).await, None, "no tail stands");
    assert_eq!(
        client.requests.load(Ordering::SeqCst),
        3,
        "run A's two model calls, then the queued input's one"
    );
    let mut expected = vec![
        "user first".to_string(),
        "assistant tool use".to_string(),
        "tool results".to_string(),
        "notice Generic".to_string(),
    ];
    expected.extend(expected_rows.iter().map(ToString::to_string));
    expected.push("assistant queued answer".to_string());
    assert_eq!(start.committed_rows(&id).await, expected);
}

#[tokio::test]
async fn a_prompt_queued_at_an_interrupt_runs_once_after_it_whole_blob() {
    an_input_queued_at_an_interrupt_runs_once_after_it(
        false,
        Input::Prompt(PromptInput::new("queued", None)),
        &["user queued"],
    )
    .await;
}

#[tokio::test]
async fn a_prompt_queued_at_an_interrupt_runs_once_after_it_head_canonical() {
    an_input_queued_at_an_interrupt_runs_once_after_it(
        true,
        Input::Prompt(PromptInput::new("queued", None)),
        &["user queued"],
    )
    .await;
}

#[tokio::test]
async fn a_peer_message_queued_at_an_interrupt_runs_once_after_it_whole_blob() {
    an_input_queued_at_an_interrupt_runs_once_after_it(
        false,
        peer_message("peer body"),
        &["notice Comms"],
    )
    .await;
}

#[tokio::test]
async fn a_peer_message_queued_at_an_interrupt_runs_once_after_it_head_canonical() {
    an_input_queued_at_an_interrupt_runs_once_after_it(
        true,
        peer_message("peer body"),
        &["notice Comms"],
    )
    .await;
}

/// A steer that arrives while run A executes a tool cannot join A (a steer
/// joins only at a model boundary), so it waits in its lane naming no run.
/// Interrupting A then closes A's tail, and the steer runs once after it.
/// RED before: the cancellation's tail recovery held on the queued steer.
async fn a_steer_that_never_joined_runs_once_after_an_interrupt(head_canonical: bool) {
    let root = TempDir::new().expect("temp");
    let stores = Stores::new(head_canonical, &root);
    let client = Arc::new(ScriptedModel::new());
    let start = Start::open(&stores, &root, &client);
    let id = start.materialize(None).await;
    client.push(Step::Tool);
    client.hold_next_tool_call();
    let turn_a = start.admit(&id, "first").await;
    tokio::time::timeout(DEADLINE, client.tool_entered.acquire())
        .await
        .expect("run A runs its tool")
        .expect("tool gate")
        .forget();
    assert!(
        start.tail_run(&id).await.is_some(),
        "run A's round commit stands as the provisional tail"
    );
    client.push(Step::Text("steer answer"));
    let steered = start.admit_input(&id, steer("steered")).await;

    interrupt_run_a(&start, &id, turn_a).await;
    let outcome = steered.await.expect("steer turn task");
    assert!(
        matches!(outcome, Some(CompletionOutcome::Completed(ref run)) if run.text == "steer answer"),
        "the steer runs after the interrupt: {outcome:?}"
    );
    assert_eq!(start.tail_run(&id).await, None, "no tail stands");
    assert_eq!(
        client.requests.load(Ordering::SeqCst),
        2,
        "run A's one model call, then the steer's one"
    );
    let rows = start.committed_rows(&id).await;
    assert_eq!(
        rows.iter()
            .filter(|row| row.as_str() == "user steered")
            .count(),
        1,
        "the steer is in the history once: {rows:?}"
    );
    assert_eq!(
        rows.last().map(String::as_str),
        Some("assistant steer answer")
    );
}

#[tokio::test]
async fn a_steer_that_never_joined_runs_once_after_an_interrupt_whole_blob() {
    a_steer_that_never_joined_runs_once_after_an_interrupt(false).await;
}

#[tokio::test]
async fn a_steer_that_never_joined_runs_once_after_an_interrupt_head_canonical() {
    a_steer_that_never_joined_runs_once_after_an_interrupt(true).await;
}

/// Run A runs its first round's tool, held; a steer admitted then waits for
/// A's next model boundary. Releasing the tool takes A to that boundary,
/// where the steer joins A, and A holds at its next model call. Returns A's
/// completion waiter, its run id, the steer's own completion waiter and id.
///
/// The steer's live-boundary delivery registers at A's next boundary from
/// the runtime's ingress task, after the admission returns. The tool is
/// released only once that delivery waits there: released earlier, A can
/// pass the boundary first, and the steer joins a later one or none.
async fn a_steer_joins_run_a_at_its_next_model_boundary(
    start: &Start,
    client: &ScriptedModel,
    id: &SessionId,
) -> (
    tokio::task::JoinHandle<Option<CompletionOutcome>>,
    RunId,
    tokio::task::JoinHandle<Option<Result<CompletionOutcome, CompletionWaitError>>>,
    meerkat_core::lifecycle::InputId,
) {
    client.hold_next_tool_call();
    let turn_a = start.admit(id, "first").await;
    tokio::time::timeout(DEADLINE, client.tool_entered.acquire())
        .await
        .expect("run A runs its tool")
        .expect("tool gate")
        .forget();
    let run_a = start
        .tail_run(id)
        .await
        .expect("run A's round commit stands as the provisional tail");
    let boundary = client
        .boundary
        .lock()
        .expect("boundary")
        .clone()
        .expect("the live agent's turn boundary");
    assert!(
        !boundary.has_waiting_delivery_for_test(),
        "no delivery waits at A's next boundary before the steer"
    );
    let steer_input = steer("steered");
    let steer_id = steer_input.id().clone();
    let steered = start.admit_input_waiter(id, steer_input).await;
    let deadline = tokio::time::Instant::now() + DEADLINE;
    while !boundary.has_waiting_delivery_for_test() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the steer's delivery never waited at A's next model boundary"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    client.tool_release.add_permits(1);
    tokio::time::timeout(DEADLINE, client.entered.acquire())
        .await
        .expect("run A holds at a model call after the steer's boundary")
        .expect("model gate")
        .forget();
    wait_for_input_run(start, id, &steer_id, &run_a).await;
    (turn_a, run_a, steered, steer_id)
}

/// A steer that joins run A at A's model boundary, with A interrupted during
/// that model call before a round commit lists it, is not in A's tail: it is
/// requeued and runs once after A. GREEN before on this live path; the
/// durable loss (a cold recovery consumed the join by its run association)
/// is pinned by the recovery_contract requeued-join rows.
async fn a_steer_joined_but_never_committed_runs_once_after_an_interrupt(head_canonical: bool) {
    let root = TempDir::new().expect("temp");
    let stores = Stores::new(head_canonical, &root);
    let client = Arc::new(ScriptedModel::new());
    let start = Start::open(&stores, &root, &client);
    let id = start.materialize(None).await;
    client.push(Step::Tool);
    client.push(Step::Held);
    let (turn_a, _run_a, steered, _steer_id) =
        a_steer_joins_run_a_at_its_next_model_boundary(&start, &client, &id).await;
    client.push(Step::Text("steer answer"));

    interrupt_run_a(&start, &id, turn_a).await;
    let outcome = steered.await.expect("steer turn task");
    assert!(
        matches!(outcome, Some(Ok(CompletionOutcome::Completed(ref run))) if run.text == "steer answer"),
        "the steer runs after the interrupt: {outcome:?}"
    );
    assert_eq!(start.tail_run(&id).await, None, "no tail stands");
    assert_eq!(
        client.requests.load(Ordering::SeqCst),
        3,
        "run A's two model calls, then the steer's one"
    );
    let rows = start.committed_rows(&id).await;
    assert_eq!(
        rows.iter()
            .filter(|row| row.as_str() == "user steered")
            .count(),
        1,
        "the steer is in the history once: {rows:?}"
    );
    assert_eq!(
        rows.last().map(String::as_str),
        Some("assistant steer answer")
    );
}

#[tokio::test]
async fn a_steer_joined_but_never_committed_runs_once_after_an_interrupt_whole_blob() {
    a_steer_joined_but_never_committed_runs_once_after_an_interrupt(false).await;
}

#[tokio::test]
async fn a_steer_joined_but_never_committed_runs_once_after_an_interrupt_head_canonical() {
    a_steer_joined_but_never_committed_runs_once_after_an_interrupt(true).await;
}

/// A steer that joins run A at a model boundary, and that A's next round
/// commit lists, is in A's tail. The cancel drops the live image and requeues
/// the join, as for every discarded join; the cancelled run's settle then
/// closes A's tail as interrupted and consumes the steer with it, because the
/// tail's cursor lists it. The runtime adopts that terminal, so nothing runs
/// the steer again, and its waiter fails with the typed
/// `ConsumedWithInterruptedRun` naming run A. RED before: the requeued join
/// ran a second time, so its content was committed twice.
async fn a_steer_a_round_commit_listed_is_consumed_once_with_the_run(head_canonical: bool) {
    let root = TempDir::new().expect("temp");
    let stores = Stores::new(head_canonical, &root);
    let client = Arc::new(ScriptedModel::new());
    let start = Start::open(&stores, &root, &client);
    let id = start.materialize(None).await;
    client.push(Step::Tool);
    // The model call after the steer's boundary answers with a tool call, so
    // that round's commits list the steer; A then holds at its third call.
    client.push(Step::Tool);
    client.push(Step::Held);
    let (turn_a, run_a, steered, steer_id) =
        a_steer_joins_run_a_at_its_next_model_boundary(&start, &client, &id).await;

    interrupt_run_a(&start, &id, turn_a).await;
    let steer_outcome = steered.await.expect("steer turn task");
    let Some(Err(error)) = &steer_outcome else {
        panic!("the steer's waiter fails, typed: {steer_outcome:?}");
    };
    assert!(error.is_consumed_with_interrupted_run(), "{error:?}");
    assert_eq!(
        error,
        &CompletionWaitError::ConsumedWithInterruptedRun {
            run_id: run_a.clone()
        },
        "the steer was consumed with run A"
    );
    assert_eq!(start.tail_run(&id).await, None, "no tail stands");
    assert_eq!(
        client.requests.load(Ordering::SeqCst),
        3,
        "only run A's three model calls: nothing runs the steer again"
    );
    let rows = start.committed_rows(&id).await;
    assert_eq!(
        rows.iter()
            .filter(|row| row.as_str() == "user steered")
            .count(),
        1,
        "the steer is in A's committed history once: {rows:?}"
    );
    let stored = meerkat_runtime::SessionServiceRuntimeExt::input_state(
        start.machine.as_ref(),
        &id,
        &steer_id,
    )
    .await
    .expect("read the steer's state");
    assert!(
        stored.as_ref().is_none_or(|stored| matches!(
            stored.seed.terminal_outcome,
            Some(meerkat_runtime::input_state::InputTerminalOutcome::Consumed)
        )),
        "the steer is consumed with run A: {stored:?}"
    );
}

#[tokio::test]
async fn a_steer_a_round_commit_listed_is_consumed_once_with_the_run_whole_blob() {
    a_steer_a_round_commit_listed_is_consumed_once_with_the_run(false).await;
}

#[tokio::test]
async fn a_steer_a_round_commit_listed_is_consumed_once_with_the_run_head_canonical() {
    a_steer_a_round_commit_listed_is_consumed_once_with_the_run(true).await;
}

/// No in-place settle a hosted loop runs leaves a recovered boundary's
/// terminal batch pending under that loop. The cancelled run's settle is
/// that settle here: it commits A's tail as interrupted and consumes the
/// steer A's round commit listed while A's own cancellation batch still
/// waits for its publication, so the recovered boundary stages no batch for
/// the steer beside it. Once the loop has published A's terminal and run a
/// later turn to its end, no input of the session holds a terminal whose
/// receipt is still pending: the next start never meets a later run's
/// pending batch beside one this settle left. A pin, written to show
/// whether such a batch is reachable under a live loop.
async fn an_in_place_settle_leaves_no_terminal_batch_pending_under_the_loop(head_canonical: bool) {
    use meerkat_runtime::terminal_status::{InputTerminalReceiptObservation, InteractionSelector};

    let root = TempDir::new().expect("temp");
    let stores = Stores::new(head_canonical, &root);
    let client = Arc::new(ScriptedModel::new());
    let start = Start::open(&stores, &root, &client);
    let id = start.materialize(None).await;
    client.push(Step::Tool);
    client.push(Step::Tool);
    client.push(Step::Held);
    let (turn_a, _run_a, steered, _steer_id) =
        a_steer_joins_run_a_at_its_next_model_boundary(&start, &client, &id).await;
    interrupt_run_a(&start, &id, turn_a).await;
    let _ = steered.await.expect("steer waiter task");
    assert_eq!(
        start.tail_run(&id).await,
        None,
        "the cancelled run's settle committed A's tail"
    );

    client.push(Step::Text("after the interrupt"));
    let outcome = start.turn(&id, "later").await;
    assert!(
        matches!(outcome, Some(CompletionOutcome::Completed(ref run)) if run.text == "after the interrupt"),
        "the later turn completes: {outcome:?}"
    );
    start.wait_runtime_settled(&id).await;
    for row in input_rows(start.runtime_store.as_ref(), &id).await {
        let input_id = row.state.input_id.clone();
        let observed = start
            .machine
            .input_terminal_receipt_observation(&id, InteractionSelector::InputId(input_id.clone()))
            .await;
        assert!(
            !matches!(
                &observed,
                Ok(Some(sourced)) if matches!(
                    sourced.report,
                    InputTerminalReceiptObservation::Pending {
                        terminal: Some(_),
                        ..
                    }
                )
            ),
            "input {input_id} still waits for its terminal's receipt: {observed:?}"
        );
    }
}

#[tokio::test]
async fn an_in_place_settle_leaves_no_terminal_batch_pending_under_the_loop_whole_blob() {
    an_in_place_settle_leaves_no_terminal_batch_pending_under_the_loop(false).await;
}

#[tokio::test]
async fn an_in_place_settle_leaves_no_terminal_batch_pending_under_the_loop_head_canonical() {
    an_in_place_settle_leaves_no_terminal_batch_pending_under_the_loop(true).await;
}

/// How the forced-hold row repairs the held session.
#[derive(Clone, Copy, Debug)]
enum HeldSettleRepair {
    /// The other run's row ends (an operator's part: the fixture writes its
    /// terminal row), and the next start's ordinary recovery closes A's tail.
    TheOtherRunsRowEnds,
    /// `resolve-held-tail --apply` requeues the other run's row and consumes
    /// the steer with A's tail.
    TheOperatorVerb,
}

/// The cancelled run's settle holds A's tail when a content row naming
/// another run stands beside it: nothing proves that row is not in the tail.
/// Only a fault leaves that row, so the fixture writes it, as
/// `stranded_row` does, while A is held. A steer that joined A, and that A's
/// round commit listed, is requeued by the cancel, as every discarded join
/// is, and a run B takes it. B is refused at its first commit while A's tail
/// stands, so the steer's content is not committed a second time, and the
/// session is repair-blocked. The steer's waiter fails with B's refusal:
/// on WholeBlob B called the model and its first round commit was refused,
/// `BoundaryRolledBack` naming B; on HeadCanonical B was refused before its
/// model call, `AuthorityUnavailable`. After the repair the next start's
/// recovery (or the operator's) closes A's tail as interrupted and consumes
/// the steer with it, because A's cursor lists it: its content appears
/// exactly once in history, and nothing runs it again, which the row reads
/// only once the restarted runtime runs nothing and holds no open input. On
/// WholeBlob the restart's registration recovers its inputs (the steer
/// rolled back to its queue) before the materialization's recovery consumes
/// the steer; the attach adopts that terminal, so the runtime never stages
/// the steer again from its own row.
async fn a_listed_steer_beside_a_held_settle_is_committed_once_after_the_repair(
    head_canonical: bool,
    repair: HeldSettleRepair,
) {
    use meerkat_runtime::input_state::{
        InputAbandonReason, InputLifecycleState as Phase, InputStateHistoryEntry,
        InputTerminalOutcome,
    };
    use meerkat_runtime::recovery::{
        HeldTailRepairVerdict, HeldTailResolution, resolve_held_durable_tail,
    };

    let root = TempDir::new().expect("temp");
    let stores = Stores::new(head_canonical, &root);
    let client = Arc::new(ScriptedModel::new());
    let (id, run_a, steer_id, other_runs_row) = {
        let start = Start::open(&stores, &root, &client);
        let id = start.materialize(None).await;
        client.push(Step::Tool);
        client.push(Step::Tool);
        client.push(Step::Held);
        let (turn_a, run_a, steered, steer_id) =
            a_steer_joins_run_a_at_its_next_model_boundary(&start, &client, &id).await;
        let rows = input_rows(start.runtime_store.as_ref(), &id).await;
        let run_a_row = rows
            .iter()
            .find(|row| {
                row.seed.last_run_id.as_ref() == Some(&run_a) && row.state.input_id != steer_id
            })
            .expect("run A's own input row")
            .clone();
        let mut other_runs_row = stranded_row(
            &run_a_row,
            &Input::Prompt(PromptInput::new("another run's", None)),
            Some(RunId::new()),
        );
        other_runs_row.seed.admission_sequence = rows
            .iter()
            .filter_map(|row| row.seed.admission_sequence)
            .max()
            .map(|sequence| sequence + 1);
        write_input_row(start.runtime_store.as_ref(), &id, other_runs_row.clone()).await;
        // Run B, the requeued steer's, answers with a tool call if it calls
        // the model.
        client.push(Step::Tool);

        interrupt_run_a(&start, &id, turn_a).await;
        let outcome = steered.await.expect("steer waiter task");
        assert_eq!(
            start.tail_run(&id).await,
            Some(run_a.clone()),
            "the settle holds A's tail"
        );
        let steer_row = input_rows(start.runtime_store.as_ref(), &id)
            .await
            .into_iter()
            .find(|row| row.state.input_id == steer_id)
            .expect("the steer's row");
        assert_eq!(steer_row.seed.phase, Phase::Staged, "run B took the steer");
        let run_b = steer_row
            .seed
            .last_run_id
            .clone()
            .expect("the steer names run B");
        assert_ne!(run_b, run_a);
        if head_canonical {
            assert!(
                matches!(
                    outcome,
                    Some(Err(CompletionWaitError::AuthorityUnavailable(_)))
                ),
                "run B was refused before its model call: {outcome:?}"
            );
            assert_eq!(
                client.requests.load(Ordering::SeqCst),
                3,
                "only run A's three model calls"
            );
        } else {
            assert!(
                matches!(
                    &outcome,
                    Some(Err(CompletionWaitError::BoundaryRolledBack { run_id, .. }))
                        if *run_id == run_b
                ),
                "run B's first round commit was refused: {outcome:?}"
            );
            assert_eq!(
                client.requests.load(Ordering::SeqCst),
                4,
                "run A's three model calls, then run B's one"
            );
        }
        let refused = start
            .machine
            .accept_input_with_completion(&id, Input::Prompt(PromptInput::new("refused", None)))
            .await;
        assert!(
            matches!(
                refused,
                Err(meerkat_runtime::RuntimeDriverError::RecoveryRepairBlocked { .. })
            ),
            "the session is repair-blocked: {:?}",
            refused.map(|(outcome, _)| outcome)
        );
        start.service.try_shutdown().await.expect("shutdown");
        // Run B's step stays unused where B never called the model.
        client.steps.lock().expect("steps").clear();
        (id, run_a, steer_id, other_runs_row)
    };

    let store = stores.operator_store();
    let other_runs_input = other_runs_row.state.input_id.clone();
    match repair {
        HeldSettleRepair::TheOtherRunsRowEnds => {
            let mut ended = other_runs_row;
            ended.state.history.push(InputStateHistoryEntry {
                timestamp: chrono::Utc::now(),
                from: Phase::Staged,
                to: Phase::Abandoned,
                reason: Some("Abandon".to_string()),
            });
            ended.seed.phase = Phase::Abandoned;
            ended.seed.terminal_outcome = Some(InputTerminalOutcome::Abandoned {
                reason: InputAbandonReason::Cancelled,
            });
            ended.seed.recovery_lane = None;
            write_input_row(store.as_ref(), &id, ended).await;
        }
        HeldSettleRepair::TheOperatorVerb => {
            let HeldTailResolution::StrandedInputs(plan) =
                resolve_held_durable_tail(store.as_ref(), &id, false)
                    .await
                    .expect("the dry run reads the held tail")
            else {
                panic!("the tail is held beside the other run's row");
            };
            assert_eq!(plan.verdict, HeldTailRepairVerdict::WouldRepair);
            assert_eq!(plan.run_id, run_a);
            assert_eq!(
                plan.terminalized_inputs,
                vec![steer_id.clone()],
                "the repair consumes the steer A's cursor lists"
            );
            assert_eq!(plan.requeued_inputs, vec![other_runs_input.clone()]);
            let HeldTailResolution::StrandedInputs(applied) =
                resolve_held_durable_tail(store.as_ref(), &id, true)
                    .await
                    .expect("the repair applies")
            else {
                panic!("the tail is still held beside the other run's row when applied");
            };
            assert!(
                matches!(applied.verdict, HeldTailRepairVerdict::Repaired { .. }),
                "{:?}",
                applied.verdict
            );
            assert_eq!(
                resolve_held_durable_tail(store.as_ref(), &id, true)
                    .await
                    .expect("a second repair reads the store"),
                HeldTailResolution::NoTail,
                "the repair closed A's tail"
            );
        }
    }
    drop(store);

    let start = Start::open(&stores, &root, &client);
    let requests_before = client.requests.load(Ordering::SeqCst);
    if let HeldSettleRepair::TheOperatorVerb = repair {
        client.push(Step::Text("another run's answer"));
    }
    start.cold_attach(&id).await;
    start.materialize(Some(&id)).await;
    let mut expected = vec![
        "user first".to_string(),
        "assistant tool use".to_string(),
        "tool results".to_string(),
        "user steered".to_string(),
        "assistant tool use".to_string(),
        "tool results".to_string(),
        "notice Generic".to_string(),
    ];
    if let HeldSettleRepair::TheOperatorVerb = repair {
        // The requeued row runs once after A.
        expected.push("user another run's".to_string());
        expected.push("assistant another run's answer".to_string());
    }
    start
        .committed_rows_ending_with(&id, expected.last().expect("a last row"))
        .await;
    assert_eq!(start.tail_run(&id).await, None, "no tail stands");
    // Whatever this start still held to run runs now: once nothing runs and
    // no input is left open, a second run of the steer is in the history.
    start.wait_runtime_settled(&id).await;
    let rows = start.committed_rows(&id).await;
    assert_eq!(
        rows.iter()
            .filter(|row| row.as_str() == "user steered")
            .count(),
        1,
        "the steer's content appears once: {rows:?}"
    );
    assert_eq!(
        rows, expected,
        "A is closed as interrupted with the steer in it"
    );
    assert_eq!(
        client.requests.load(Ordering::SeqCst),
        requests_before + usize::from(matches!(repair, HeldSettleRepair::TheOperatorVerb)),
        "nothing runs the steer again"
    );
    let steer_row = input_rows(start.runtime_store.as_ref(), &id)
        .await
        .into_iter()
        .find(|row| row.state.input_id == steer_id)
        .expect("the steer's row");
    assert_eq!(
        steer_row.seed.terminal_outcome,
        Some(InputTerminalOutcome::Consumed),
        "the steer is consumed with A"
    );
}

#[tokio::test]
async fn a_listed_steer_beside_a_held_settle_is_committed_once_after_its_row_ends_whole_blob() {
    a_listed_steer_beside_a_held_settle_is_committed_once_after_the_repair(
        false,
        HeldSettleRepair::TheOtherRunsRowEnds,
    )
    .await;
}

#[tokio::test]
async fn a_listed_steer_beside_a_held_settle_is_committed_once_after_its_row_ends_head_canonical() {
    a_listed_steer_beside_a_held_settle_is_committed_once_after_the_repair(
        true,
        HeldSettleRepair::TheOtherRunsRowEnds,
    )
    .await;
}

#[tokio::test]
async fn a_listed_steer_beside_a_held_settle_is_committed_once_by_the_operator_whole_blob() {
    a_listed_steer_beside_a_held_settle_is_committed_once_after_the_repair(
        false,
        HeldSettleRepair::TheOperatorVerb,
    )
    .await;
}

#[tokio::test]
async fn a_listed_steer_beside_a_held_settle_is_committed_once_by_the_operator_head_canonical() {
    a_listed_steer_beside_a_held_settle_is_committed_once_after_the_repair(
        true,
        HeldSettleRepair::TheOperatorVerb,
    )
    .await;
}

/// The other run's row the held-settle fixtures strand beside A's tail,
/// written into the store while run A is held.
async fn strand_another_runs_row(
    start: &Start,
    id: &SessionId,
    run_a: &RunId,
    steer_id: Option<&meerkat_core::lifecycle::InputId>,
) -> StoredInputState {
    let rows = input_rows(start.runtime_store.as_ref(), id).await;
    let run_a_row = rows
        .iter()
        .find(|row| {
            row.seed.last_run_id.as_ref() == Some(run_a) && Some(&row.state.input_id) != steer_id
        })
        .expect("run A's own input row")
        .clone();
    let mut other_runs_row = stranded_row(
        &run_a_row,
        &Input::Prompt(PromptInput::new("another run's", None)),
        Some(RunId::new()),
    );
    other_runs_row.seed.admission_sequence = rows
        .iter()
        .filter_map(|row| row.seed.admission_sequence)
        .max()
        .map(|sequence| sequence + 1);
    write_input_row(start.runtime_store.as_ref(), id, other_runs_row.clone()).await;
    other_runs_row
}

/// End the stranded row of another run (an operator's part: the fixture
/// writes its terminal row), so nothing holds A's tail any more.
async fn end_another_runs_row(start: &Start, id: &SessionId, mut row: StoredInputState) {
    use meerkat_runtime::input_state::{
        InputAbandonReason, InputLifecycleState as Phase, InputStateHistoryEntry,
        InputTerminalOutcome,
    };
    row.state.history.push(InputStateHistoryEntry {
        timestamp: chrono::Utc::now(),
        from: Phase::Staged,
        to: Phase::Abandoned,
        reason: Some("Abandon".to_string()),
    });
    row.seed.phase = Phase::Abandoned;
    row.seed.terminal_outcome = Some(InputTerminalOutcome::Abandoned {
        reason: InputAbandonReason::Cancelled,
    });
    row.seed.recovery_lane = None;
    write_input_row(start.runtime_store.as_ref(), id, row).await;
}

/// A read of the hosted session, through the observation path that settles
/// the tail of an ended turn.
async fn observe_hosted_session(start: &Start, id: &SessionId) {
    let live = tokio::time::timeout(
        DEADLINE,
        meerkat_core::service::SessionService::has_live_session(start.service.as_ref(), id),
    )
    .await
    .expect("the read answers at once")
    .expect("the read");
    assert!(live, "the session's live actor is still hosted");
}

/// An observation of a hosted session settles the tail a cancelled run left
/// only between the runtime loop's laps, never between a lap's read of its
/// batch's rows and the batch's staging commit.
///
/// Run A is interrupted after a round commit that listed a steer. A's own
/// settle holds the tail beside a row of another run, and the cancel requeues
/// the steer. The next lap reads the steer's row, still queued, and is held
/// right before its staging commit. There the other run's row ends, and a
/// read of the session observes it: the lap holds the turn-finalization
/// boundary, so the read leaves A's tail to a later settle instead of
/// consuming the steer under the lap. The lap stages the steer for run B,
/// which the standing tail refuses before its model call, and the next
/// start's recovery closes A's tail with the steer in it: the steer's content
/// appears once, and the model never sees it again. RED before: the read's
/// settle consumed the steer between the lap's read and its staging commit,
/// the staging commit wrote the steer staged over its consumed row, and run B
/// ran it again, so the model saw it twice and the history held it twice.
#[tokio::test]
async fn an_observation_never_settles_inside_a_laps_staging_window_head_canonical() {
    use meerkat_runtime::input_state::InputTerminalOutcome;

    let root = TempDir::new().expect("temp");
    let stores = Stores::new(true, &root);
    let client = Arc::new(ScriptedModel::new());
    let (id, steer_id) = {
        let start = Start::open(&stores, &root, &client);
        let id = start.materialize(None).await;
        client.push(Step::Tool);
        client.push(Step::Tool);
        client.push(Step::Held);
        let (turn_a, run_a, steered, steer_id) =
            a_steer_joins_run_a_at_its_next_model_boundary(&start, &client, &id).await;
        let other_runs_row = strand_another_runs_row(&start, &id, &run_a, Some(&steer_id)).await;
        // Run B answers the steer if it calls the model.
        client.push(Step::Text("steer answer"));
        let (staging, release) = start
            .machine
            .arm_runtime_loop_before_batch_staging_test_hook(id.clone());

        interrupt_run_a(&start, &id, turn_a).await;
        tokio::time::timeout(DEADLINE, staging)
            .await
            .expect("the next lap reaches the steer's staging commit")
            .expect("the staging hook");
        assert_eq!(
            start.tail_run(&id).await,
            Some(run_a.clone()),
            "the cancelled run's settle holds A's tail"
        );
        end_another_runs_row(&start, &id, other_runs_row).await;
        observe_hosted_session(&start, &id).await;
        let _ = release.send(());
        let _ = steered.await.expect("steer waiter task");
        start.service.try_shutdown().await.expect("shutdown");
        // Run B's step stays unused where B never called the model.
        client.steps.lock().expect("steps").clear();
        (id, steer_id)
    };

    let requests_before = client.requests.load(Ordering::SeqCst);
    let start = Start::open(&stores, &root, &client);
    start.cold_attach(&id).await;
    start.materialize(Some(&id)).await;
    start.wait_runtime_settled(&id).await;
    let rows = start.committed_rows(&id).await;
    assert_eq!(
        rows.iter()
            .filter(|row| row.as_str() == "user steered")
            .count(),
        1,
        "the steer's content appears once: {rows:?}"
    );
    assert_eq!(
        requests_before, 3,
        "only run A's three model calls: the model never sees the steer again"
    );
    assert_eq!(
        client.requests.load(Ordering::SeqCst),
        requests_before,
        "nothing runs the steer after the restart"
    );
    let steer_row = input_rows(start.runtime_store.as_ref(), &id)
        .await
        .into_iter()
        .find(|row| row.state.input_id == steer_id)
        .expect("the steer's row");
    assert_eq!(
        steer_row.seed.terminal_outcome,
        Some(InputTerminalOutcome::Consumed),
        "the steer is consumed with A"
    );
}

/// A runtime loop lap held right before its staging commit after the live
/// actor is gone, with A's tail standing: what a row needs to release the
/// lap and to find the steer after a restart.
struct LapHeldInItsStagingWindow {
    run_a: RunId,
    steer_id: meerkat_core::lifecycle::InputId,
    steered: tokio::task::JoinHandle<Option<Result<CompletionOutcome, CompletionWaitError>>>,
    release: tokio::sync::oneshot::Sender<()>,
}

/// Hold a runtime loop lap in its staging window after the live actor is
/// gone. Run A is interrupted after a round commit that listed a steer, and
/// A's own settle holds the tail beside a row of another run. The next lap
/// reads the steer's row and is held right before its staging commit, with
/// the session's turn-finalization boundary held. There the other run's row
/// ends, so nothing holds A's tail any more, and the live actor is
/// discarded, as a read of a retired runtime discards it while a lap runs:
/// A's tail is now the cold path's to settle.
async fn a_lap_held_in_its_staging_window_after_the_actor_is_gone(
    start: &Start,
    client: &ScriptedModel,
    id: &SessionId,
) -> LapHeldInItsStagingWindow {
    client.push(Step::Tool);
    client.push(Step::Tool);
    client.push(Step::Held);
    let (turn_a, run_a, steered, steer_id) =
        a_steer_joins_run_a_at_its_next_model_boundary(start, client, id).await;
    let other_runs_row = strand_another_runs_row(start, id, &run_a, Some(&steer_id)).await;
    // Run B answers the steer if it calls the model.
    client.push(Step::Text("steer answer"));
    let (staging, release) = start
        .machine
        .arm_runtime_loop_before_batch_staging_test_hook(id.clone());

    interrupt_run_a(start, id, turn_a).await;
    tokio::time::timeout(DEADLINE, staging)
        .await
        .expect("the next lap reaches the steer's staging commit")
        .expect("the staging hook");
    assert_eq!(
        start.tail_run(id).await,
        Some(run_a.clone()),
        "the cancelled run's settle holds A's tail"
    );
    end_another_runs_row(start, id, other_runs_row).await;
    start
        .service
        .discard_live_session(id)
        .await
        .expect("discard the live actor while the lap runs");
    LapHeldInItsStagingWindow {
        run_a,
        steer_id,
        steered,
        release,
    }
}

impl LapHeldInItsStagingWindow {
    /// Release the lap, wait for the steer's own waiter and stop the start.
    /// Returns the steer's id.
    async fn release_and_stop(
        self,
        start: &Start,
        client: &ScriptedModel,
    ) -> meerkat_core::lifecycle::InputId {
        let _ = self.release.send(());
        let _ = self.steered.await.expect("steer waiter task");
        start.service.try_shutdown().await.expect("shutdown");
        // Run B's step stays unused where B never called the model.
        client.steps.lock().expect("steps").clear();
        self.steer_id
    }
}

/// The next start's recovery closes A's tail with the steer in it: the
/// steer's content appears once, the steer is consumed with A, and the model
/// never sees it again.
async fn assert_a_restart_consumes_the_steer_once_with_a(
    stores: &Stores,
    root: &TempDir,
    client: &Arc<ScriptedModel>,
    id: &SessionId,
    steer_id: &meerkat_core::lifecycle::InputId,
) {
    use meerkat_runtime::input_state::InputTerminalOutcome;

    let requests_before = client.requests.load(Ordering::SeqCst);
    let start = Start::open(stores, root, client);
    start.cold_attach(id).await;
    start.materialize(Some(id)).await;
    start.wait_runtime_settled(id).await;
    let rows = start.committed_rows(id).await;
    assert_eq!(
        rows.iter()
            .filter(|row| row.as_str() == "user steered")
            .count(),
        1,
        "the steer's content appears once: {rows:?}"
    );
    assert_eq!(
        requests_before, 3,
        "only run A's three model calls: the model never sees the steer again"
    );
    assert_eq!(
        client.requests.load(Ordering::SeqCst),
        requests_before,
        "nothing runs the steer after the restart"
    );
    let steer_row = input_rows(start.runtime_store.as_ref(), id)
        .await
        .into_iter()
        .find(|row| &row.state.input_id == steer_id)
        .expect("the steer's row");
    assert_eq!(
        steer_row.seed.terminal_outcome,
        Some(InputTerminalOutcome::Consumed),
        "the steer is consumed with A: {:?}",
        steer_row.state.history
    );
}

/// A cold attach never settles a tail inside a runtime loop lap's staging
/// window either. A lap can outlive the live actor (a read of a retired
/// runtime discards the actor while a lap holds the boundary), and a start
/// or attach then settles the tail on the cold path. With a lap held in its
/// staging window after the actor is gone
/// (`a_lap_held_in_its_staging_window_after_the_actor_is_gone`), a cold
/// attach of the session runs: the lap holds the turn-finalization boundary,
/// so the attach leaves A's tail to a settle between laps instead of
/// consuming the steer under the lap. RED before: the cold attach's settle
/// took only the recovery gate and committed A's tail with the steer between
/// the lap's read and its staging commit; the staging commit then wrote the
/// steer staged over its consumed row, so the steer ended stopped
/// (`Abandoned(Stopped)`) although its content was in A's committed history.
#[tokio::test]
async fn a_cold_attach_never_settles_inside_a_laps_staging_window_head_canonical() {
    let root = TempDir::new().expect("temp");
    let stores = Stores::new(true, &root);
    let client = Arc::new(ScriptedModel::new());
    let (id, steer_id) = {
        let start = Start::open(&stores, &root, &client);
        let id = start.materialize(None).await;
        let lap =
            a_lap_held_in_its_staging_window_after_the_actor_is_gone(&start, &client, &id).await;
        tokio::time::timeout(DEADLINE, start.cold_attach(&id))
            .await
            .expect("the cold attach answers while the lap is held");
        (id, lap.release_and_stop(&start, &client).await)
    };
    assert_a_restart_consumes_the_steer_once_with_a(&stores, &root, &client, &id, &steer_id).await;
}

/// A committed-boundary resume preparation, which a mob's resume verdict
/// runs (`prepare_committed_boundary_resume`), never settles a tail inside a
/// runtime loop lap's staging window either. With a lap held in its staging
/// window after the actor is gone, the preparation waits for the lap's
/// turn-finalization boundary, bounded, and refuses busy once the bound
/// passes, so A's tail is left to a settle between laps. RED before: the
/// preparation's recovery took only the recovery gate and committed A's tail
/// with the steer under the lap.
#[tokio::test]
async fn a_resume_preparation_never_settles_inside_a_laps_staging_window_head_canonical() {
    let root = TempDir::new().expect("temp");
    let stores = Stores::new(true, &root);
    let client = Arc::new(ScriptedModel::new());
    let (id, steer_id) = {
        let start = Start::open(&stores, &root, &client);
        let id = start.materialize(None).await;
        let lap =
            a_lap_held_in_its_staging_window_after_the_actor_is_gone(&start, &client, &id).await;
        let prepared = tokio::time::timeout(
            DEADLINE,
            start.service.prepare_committed_boundary_resume(&id),
        )
        .await
        .expect("the preparation answers while the lap is held");
        assert_eq!(
            start.tail_run(&id).await,
            Some(lap.run_a.clone()),
            "nothing settles A's tail inside the lap"
        );
        assert!(
            matches!(prepared, Err(SessionError::Busy { .. })),
            "the preparation refuses busy while the lap holds the boundary: {prepared:?}"
        );
        (id, lap.release_and_stop(&start, &client).await)
    };
    assert_a_restart_consumes_the_steer_once_with_a(&stores, &root, &client, &id, &steer_id).await;
}

/// A create that materializes a durable session never settles a tail inside
/// a runtime loop lap's staging window. A realtime open's recovery reads the
/// committed session strictly and lowers it to a create request with the
/// session's local runtime bindings, and a run can start after that read,
/// be interrupted with its settle held, and leave a lap that outlives the
/// live actor. Here the session is read and its bindings are prepared before
/// run A, a lap is held in its staging window after the actor is gone, and
/// the create runs: its durable-tail recovery waits for the lap's
/// turn-finalization boundary, bounded, and the create refuses busy once the
/// bound passes, so A's tail is left to a settle between laps. RED before:
/// the create's recovery ran before the create took the boundary and
/// committed A's tail with the steer under the lap.
#[tokio::test]
async fn a_create_never_settles_inside_a_laps_staging_window_head_canonical() {
    let root = TempDir::new().expect("temp");
    let stores = Stores::new(true, &root);
    let client = Arc::new(ScriptedModel::new());
    let (id, steer_id) = {
        let start = Start::open(&stores, &root, &client);
        let id = start.materialize(None).await;
        let committed = start
            .service
            .load_authoritative_session(&id)
            .await
            .expect("the strict read")
            .expect("the session is committed");
        let bindings = start
            .machine
            .prepare_local_session_bindings(id.clone())
            .await
            .expect("the session's local runtime bindings");
        let lap =
            a_lap_held_in_its_staging_window_after_the_actor_is_gone(&start, &client, &id).await;
        let mut request = deferred_create_request();
        request.build = Some(meerkat_core::service::SessionBuildOptions {
            resume_session: Some(committed),
            runtime_build_mode: meerkat_core::RuntimeBuildMode::SessionOwned(bindings),
            ..Default::default()
        });
        let create = {
            let service = start.service.clone();
            tokio::spawn(async move {
                let admission = service.reserve_create_session_admission().await?;
                service
                    .create_session_with_reserved_admission(request, admission)
                    .await
                    .map(|_| ())
            })
        };
        // The create cannot finish while the lap holds the boundary unless
        // it refuses: wait for its answer, or for a settle of A's tail.
        let deadline = tokio::time::Instant::now() + DEADLINE;
        while !create.is_finished() && start.tail_run(&id).await.is_some() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the create never answered while the lap is held"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(
            start.tail_run(&id).await,
            Some(lap.run_a.clone()),
            "nothing settles A's tail inside the lap"
        );
        let created = create.await.expect("create task");
        assert!(
            matches!(created, Err(SessionError::Busy { .. })),
            "the create refuses busy while the lap holds the boundary: {created:?}"
        );
        (id, lap.release_and_stop(&start, &client).await)
    };
    assert_a_restart_consumes_the_steer_once_with_a(&stores, &root, &client, &id, &steer_id).await;
}

/// A held turn-finalization boundary defers an observation's settle of the
/// tail a cancelled run left, and never drops it: the first read after the
/// boundary is released settles the tail.
///
/// Run A is interrupted after a round commit, and its own settle holds the
/// tail beside a row of another run; that row then ends. While the boundary
/// is held, as a runtime loop lap or a turn holds it, a read of the session
/// answers at once and A's tail stands. Once it is released, the next read
/// closes A's tail as interrupted. RED before: the read settled the tail
/// under the held boundary.
#[tokio::test]
async fn an_observation_defers_its_settle_past_a_held_turn_boundary_head_canonical() {
    let root = TempDir::new().expect("temp");
    let stores = Stores::new(true, &root);
    let client = Arc::new(ScriptedModel::new());
    let start = Start::open(&stores, &root, &client);
    let id = start.materialize(None).await;
    client.push(Step::Tool);
    client.push(Step::Held);
    let turn_a = start.admit(&id, "first").await;
    tokio::time::timeout(DEADLINE, client.entered.acquire())
        .await
        .expect("run A holds at its model call after its tool round")
        .expect("model gate")
        .forget();
    let run_a = start
        .tail_run(&id)
        .await
        .expect("run A's round commit stands as the provisional tail");
    let other_runs_row = strand_another_runs_row(&start, &id, &run_a, None).await;
    interrupt_run_a(&start, &id, turn_a).await;
    assert_eq!(
        start.tail_run(&id).await,
        Some(run_a.clone()),
        "the cancelled run's settle holds A's tail"
    );
    end_another_runs_row(&start, &id, other_runs_row).await;

    let boundary = start
        .service
        .acquire_runtime_turn_finalization_guard(&id)
        .await;
    observe_hosted_session(&start, &id).await;
    assert_eq!(
        start.tail_run(&id).await,
        Some(run_a.clone()),
        "a read under the held boundary leaves A's tail to a later settle"
    );
    drop(boundary);

    observe_hosted_session(&start, &id).await;
    assert_eq!(
        start.tail_run(&id).await,
        None,
        "the first read after the boundary is released settles A's tail"
    );
    let rows = start.committed_rows(&id).await;
    assert_eq!(
        rows.iter()
            .filter(|row| row.as_str() == "user first")
            .count(),
        1,
        "A's prompt is committed once: {rows:?}"
    );
    assert_eq!(
        rows.last().map(String::as_str),
        Some("notice Generic"),
        "A is closed as interrupted: {rows:?}"
    );
}

/// Run A's tail, standing after an observation deferred its settle under
/// a held turn-finalization boundary: run A is interrupted after a round
/// commit, its own settle holds the tail beside a row of another run, that
/// row ends, and a read of the session under the boundary (held here as a
/// runtime loop lap holds it) leaves the tail to a later settle. The live
/// actor is still hosted. Returns the session, run A and the held boundary.
async fn a_tail_deferred_under_a_held_boundary(
    start: &Start,
    client: &ScriptedModel,
) -> (SessionId, RunId, tokio::sync::OwnedMutexGuard<()>) {
    let id = start.materialize(None).await;
    client.push(Step::Tool);
    client.push(Step::Held);
    let turn_a = start.admit(&id, "first").await;
    tokio::time::timeout(DEADLINE, client.entered.acquire())
        .await
        .expect("run A holds at its model call after its tool round")
        .expect("model gate")
        .forget();
    let run_a = start
        .tail_run(&id)
        .await
        .expect("run A's round commit stands as the provisional tail");
    let other_runs_row = strand_another_runs_row(start, &id, &run_a, None).await;
    interrupt_run_a(start, &id, turn_a).await;
    end_another_runs_row(start, &id, other_runs_row).await;
    let boundary = start
        .service
        .acquire_runtime_turn_finalization_guard(&id)
        .await;
    observe_hosted_session(start, &id).await;
    assert_eq!(
        start.tail_run(&id).await,
        Some(run_a.clone()),
        "a read under the held boundary leaves A's tail to a later settle"
    );
    (id, run_a, boundary)
}

/// Retire the session's runtime while the boundary is held, as a retire
/// lands while a lap holds it, and read the session the way a host's start
/// does next: the read of a retired runtime discards the live actor whose
/// settle it deferred, so no later observation can settle A's tail.
async fn retire_and_discard_the_actor_under_the_boundary(start: &Start, id: &SessionId) {
    tokio::time::timeout(
        DEADLINE,
        start
            .machine
            .retire_runtime_control_plane(&LogicalRuntimeId::for_session(id)),
    )
    .await
    .expect("the retire answers under the held boundary")
    .expect("the retire");
    let live = tokio::time::timeout(
        DEADLINE,
        meerkat_core::service::SessionService::has_live_session(start.service.as_ref(), id),
    )
    .await
    .expect("the read answers at once")
    .expect("the read");
    assert!(
        !live,
        "the read of the retired runtime discards the live actor"
    );
    assert!(
        start.service.live_session_actor_witness(id).await.is_none(),
        "no live actor is hosted any more"
    );
}

/// The strict read of a session, and A's tail closed as interrupted under
/// it: the prompt committed once and the interrupted-run notice last.
async fn assert_the_strict_read_closed_a(
    start: &Start,
    id: &SessionId,
    read: Result<Option<meerkat_core::Session>, meerkat_core::service::SessionError>,
) {
    let session = read
        .expect("the strict read succeeds: no standing tail refuses it")
        .expect("the session is committed");
    assert_eq!(start.tail_run(id).await, None, "A's tail is settled");
    let rows = transcript_rows(&session);
    assert_eq!(
        rows.iter()
            .filter(|row| row.as_str() == "user first")
            .count(),
        1,
        "A's prompt is committed once: {rows:?}"
    );
    assert_eq!(
        rows.last().map(String::as_str),
        Some("notice Generic"),
        "A is closed as interrupted: {rows:?}"
    );
}

/// A settle an observation deferred under a held turn boundary survives the
/// discard of the live actor. A retire lands while the lap holds the
/// boundary, and the read of the retired runtime discards the actor before
/// the lap ends, so no observation settles A's tail any more. Once the lap
/// ends, a strict authoritative read of the session settles the tail and
/// reads the committed session. RED before: the deferred settle was dropped
/// with the actor, and the strict read refused the boundary with the typed
/// transcript revision conflict until a later start settled the tail.
#[tokio::test]
async fn a_strict_read_settles_the_tail_a_discarded_actor_left_head_canonical() {
    let root = TempDir::new().expect("temp");
    let stores = Stores::new(true, &root);
    let client = Arc::new(ScriptedModel::new());
    let start = Start::open(&stores, &root, &client);
    let (id, _run_a, boundary) = a_tail_deferred_under_a_held_boundary(&start, &client).await;
    retire_and_discard_the_actor_under_the_boundary(&start, &id).await;
    drop(boundary);

    let read = start.service.load_authoritative_session(&id).await;
    assert_the_strict_read_closed_a(&start, &id, read).await;
}

/// A strict read while the lap still holds the boundary waits for the lap
/// (bounded) and then settles the tail the discarded actor left: it never
/// settles inside the lap, and it never refuses the boundary because the
/// settle has not run yet. RED before: the read refused at once.
#[tokio::test]
async fn a_strict_read_under_a_held_boundary_settles_the_tail_after_the_lap_head_canonical() {
    let root = TempDir::new().expect("temp");
    let stores = Stores::new(true, &root);
    let client = Arc::new(ScriptedModel::new());
    let start = Start::open(&stores, &root, &client);
    let (id, run_a, boundary) = a_tail_deferred_under_a_held_boundary(&start, &client).await;
    retire_and_discard_the_actor_under_the_boundary(&start, &id).await;

    let read = {
        let service = start.service.clone();
        let id = id.clone();
        tokio::spawn(async move { service.load_authoritative_session(&id).await })
    };
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        start.tail_run(&id).await,
        Some(run_a),
        "nothing settles the tail inside the lap"
    );
    drop(boundary);
    let read = tokio::time::timeout(DEADLINE, read)
        .await
        .expect("the strict read answers once the lap ends")
        .expect("strict read task");
    assert_the_strict_read_closed_a(&start, &id, read).await;
}

/// A strict read after the lap settles the tail an observation deferred
/// while the live actor is still hosted: it is the first observation after
/// the lap the deferral names, so no other read has to come first. RED
/// before: only an observation settled the tail, and the strict read
/// refused the boundary.
#[tokio::test]
async fn a_strict_read_after_the_lap_settles_the_tail_an_observation_deferred_head_canonical() {
    let root = TempDir::new().expect("temp");
    let stores = Stores::new(true, &root);
    let client = Arc::new(ScriptedModel::new());
    let start = Start::open(&stores, &root, &client);
    let (id, _run_a, boundary) = a_tail_deferred_under_a_held_boundary(&start, &client).await;
    drop(boundary);

    let read = start.service.load_authoritative_session(&id).await;
    assert_the_strict_read_closed_a(&start, &id, read).await;
}

/// A committed-boundary resume preparation waits for the session's
/// turn-finalization boundary only when its caller does not hold it. Run A's
/// tail stands after an observation deferred its settle under the held
/// boundary, and the live actor is discarded. While the boundary stays held,
/// the plain preparation refuses busy once its bound passes and leaves A's
/// tail, and the preparation of a caller that holds the boundary
/// (`prepare_committed_boundary_resume_under_runtime_turn_boundary`) answers
/// at once and settles A's tail under that boundary.
#[tokio::test]
async fn a_boundary_holders_resume_preparation_does_not_wait_head_canonical() {
    let root = TempDir::new().expect("temp");
    let stores = Stores::new(true, &root);
    let client = Arc::new(ScriptedModel::new());
    let start = Start::open(&stores, &root, &client);
    let (id, run_a, boundary) = a_tail_deferred_under_a_held_boundary(&start, &client).await;
    start
        .service
        .discard_live_session(&id)
        .await
        .expect("discard the live actor under the held boundary");

    let refused = tokio::time::timeout(
        DEADLINE,
        start.service.prepare_committed_boundary_resume(&id),
    )
    .await
    .expect("the plain preparation answers within its bound");
    assert!(
        matches!(refused, Err(SessionError::Busy { .. })),
        "the plain preparation refuses busy under the held boundary: {refused:?}"
    );
    assert_eq!(
        start.tail_run(&id).await,
        Some(run_a),
        "the refused preparation leaves A's tail"
    );

    // The boundary is held for the whole test: a preparation that waited for
    // it would refuse Busy at its own 1 s bound or never answer.
    let prepared = tokio::time::timeout(
        DEADLINE,
        start
            .service
            .prepare_committed_boundary_resume_under_runtime_turn_boundary(&id),
    )
    .await
    .expect("the boundary holder's preparation does not wait for the boundary");
    assert!(
        prepared.is_ok(),
        "the boundary holder's preparation succeeds: {prepared:?}"
    );
    assert_eq!(
        start.tail_run(&id).await,
        None,
        "A's tail is settled under the caller's boundary"
    );
    drop(boundary);
}

/// Wait until `input_id` is bound to `run_id`: the steer joined the run.
async fn wait_for_input_run(
    start: &Start,
    id: &SessionId,
    input_id: &meerkat_core::lifecycle::InputId,
    run_id: &RunId,
) {
    let deadline = tokio::time::Instant::now() + DEADLINE;
    loop {
        let stored = meerkat_runtime::SessionServiceRuntimeExt::input_state(
            start.machine.as_ref(),
            id,
            input_id,
        )
        .await
        .expect("read the input's state");
        if stored
            .as_ref()
            .is_some_and(|stored| stored.seed.last_run_id.as_ref() == Some(run_id))
        {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the steer never joined run {run_id}: {stored:?}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
