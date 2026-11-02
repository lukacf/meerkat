//! Per-round commits and resumed runs, through the real agent loop.
//!
//! A run whose agent has a session checkpointer commits each tool round
//! twice: the first commit stages the round's assistant message in the run
//! cursor, the second writes the message and its results into the transcript.
//! A run stopped between two commits (its future dropped, as by a crash)
//! continues from its cursor with `Agent::resume_run`. These tests drive the
//! loop with a scripted model, recording tools and a checkpointer that keeps
//! every checkpoint it writes, as a store keeps a run's provisional tail.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use crate as meerkat_core;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use meerkat_core::types::{AssistantMessageId, UserMessage};
use meerkat_core::{
    AgentBuilder, AgentError, AgentEvent, AgentLlmClient, AgentSessionStore, AgentToolDispatcher,
    AssistantBlock, BlockAssistantMessage, LlmStreamResult, Message, RunCheckpointReceipt, RunId,
    RunResumePoint, Session, SessionCheckpointer, SessionRunCursor, StopReason, ToolCallDedupeKey,
    ToolCallView, ToolDef, ToolDispatchContext, ToolResult, TurnUsage, Usage,
};
use serde_json::value::RawValue;
use tokio::sync::{Notify, mpsc};

fn mock_usage() -> Usage {
    TurnUsage::host_declared(
        meerkat_core::Provider::Other,
        "mock-model",
        Usage::default(),
    )
    .into_inner()
}

fn tool_use(id: &str, name: &str, args: &str) -> AssistantBlock {
    AssistantBlock::ToolUse {
        id: id.to_string(),
        name: name.into(),
        args: RawValue::from_string(args.to_string()).unwrap(),
        meta: None,
    }
}

/// One scripted model response.
enum Step {
    /// Tool calls: `(call id, tool name, JSON arguments)`.
    Tools(Vec<(&'static str, &'static str, &'static str)>),
    /// A final answer.
    Answer(&'static str),
    /// No answer ever: the run is stopped here, as by a crash.
    Hang,
}

struct ScriptedClient {
    steps: Mutex<VecDeque<Step>>,
    calls: AtomicUsize,
    requests: Mutex<Vec<Vec<Message>>>,
    hung: Notify,
}

impl ScriptedClient {
    fn new(steps: Vec<Step>) -> Arc<Self> {
        Arc::new(Self {
            steps: Mutex::new(steps.into()),
            calls: AtomicUsize::new(0),
            requests: Mutex::new(Vec::new()),
            hung: Notify::new(),
        })
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    fn requests(&self) -> Vec<Vec<Message>> {
        self.requests.lock().unwrap().clone()
    }
}

#[async_trait]
impl AgentLlmClient for ScriptedClient {
    async fn stream_response(
        &self,
        messages: &[Message],
        _tools: &[Arc<ToolDef>],
        _max_tokens: u32,
        _temperature: Option<f32>,
        _provider_params: Option<&meerkat_core::lifecycle::run_primitive::ProviderParamsOverride>,
    ) -> Result<LlmStreamResult, AgentError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.requests.lock().unwrap().push(messages.to_vec());
        let step = self
            .steps
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Step::Answer("done"));
        match step {
            Step::Tools(calls) => Ok(LlmStreamResult::new(
                calls
                    .into_iter()
                    .map(|(id, name, args)| tool_use(id, name, args))
                    .collect(),
                StopReason::ToolUse,
                mock_usage(),
            )),
            Step::Answer(text) => Ok(LlmStreamResult::new(
                vec![AssistantBlock::Text {
                    text: text.to_string(),
                    meta: None,
                }],
                StopReason::EndTurn,
                mock_usage(),
            )),
            Step::Hang => {
                self.hung.notify_one();
                std::future::pending().await
            }
        }
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::Other
    }

    fn model(&self) -> &'static str {
        "mock-model"
    }
}

/// Records every dispatch with the dedupe key its context gives the call. A
/// call to the tool named by `hang_on` signals `entered` and never returns,
/// as a crash leaves it.
struct RecordingTools {
    tools: Arc<[Arc<ToolDef>]>,
    dispatched: Mutex<Vec<(String, Option<ToolCallDedupeKey>)>>,
    hang_on: Option<&'static str>,
    entered: Notify,
    /// Ask the running run to cancel at its next boundary from the first
    /// dispatch.
    boundary_cancel: Mutex<Option<meerkat_core::CancelAfterBoundarySender>>,
    /// Close this checkpointer's self-fence window once the first call
    /// starts.
    fence_on_first_dispatch: Mutex<Option<Arc<RecordingCheckpointer>>>,
}

impl RecordingTools {
    fn new(tools: Vec<ToolDef>, hang_on: Option<&'static str>) -> Arc<Self> {
        Arc::new(Self {
            tools: tools.into_iter().map(Arc::new).collect(),
            dispatched: Mutex::new(Vec::new()),
            hang_on,
            entered: Notify::new(),
            boundary_cancel: Mutex::new(None),
            fence_on_first_dispatch: Mutex::new(None),
        })
    }

    fn cancel_at_next_boundary(&self, sender: meerkat_core::CancelAfterBoundarySender) {
        *self.boundary_cancel.lock().unwrap() = Some(sender);
    }

    fn close_fence_when_a_call_starts(&self, checkpointer: Arc<RecordingCheckpointer>) {
        *self.fence_on_first_dispatch.lock().unwrap() = Some(checkpointer);
    }

    fn probes() -> Vec<ToolDef> {
        ["probe", "probe2"]
            .into_iter()
            .map(|name| ToolDef::new(name, "returns", serde_json::json!({ "type": "object" })))
            .collect()
    }

    fn dispatched(&self) -> Vec<(String, Option<ToolCallDedupeKey>)> {
        self.dispatched.lock().unwrap().clone()
    }
}

#[async_trait]
impl AgentToolDispatcher for RecordingTools {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::clone(&self.tools)
    }

    async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
        Ok(ToolResult::new(call.id.to_string(), format!("{} ok", call.name), false).into())
    }

    async fn dispatch_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
    ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
        self.dispatched
            .lock()
            .unwrap()
            .push((call.id.to_string(), context.tool_call_dedupe_key(call)));
        let fence = self.fence_on_first_dispatch.lock().unwrap().take();
        if let Some(checkpointer) = fence {
            checkpointer.fenced.store(true, Ordering::SeqCst);
        }
        let boundary_cancel = self.boundary_cancel.lock().unwrap().take();
        if let (Some(sender), Some(run_id)) = (boundary_cancel, context.run_id()) {
            sender
                .send(meerkat_core::CancelAfterBoundaryCommand::for_run(
                    run_id.clone(),
                ))
                .expect("the agent holds the receiver");
        }
        if self.hang_on == Some(call.name) {
            self.entered.notify_one();
            std::future::pending::<()>().await;
        }
        self.dispatch(call).await
    }
}

/// Issues the run's next contiguous provisional receipt for every checkpoint
/// and keeps the checkpointed session with it. Without receipts it answers
/// as a store with no committed boundary to chain from.
struct RecordingCheckpointer {
    receipts: bool,
    commits: Mutex<Vec<(Session, RunCheckpointReceipt)>>,
    calls: AtomicUsize,
    /// The writer's self-fence window has closed: dispatch is refused.
    fenced: std::sync::atomic::AtomicBool,
    /// Close the window as soon as a round's first commit lands.
    fence_after_first_commit: bool,
}

impl RecordingCheckpointer {
    fn with(receipts: bool, fenced: bool, fence_after_first_commit: bool) -> Arc<Self> {
        Arc::new(Self {
            receipts,
            commits: Mutex::new(Vec::new()),
            calls: AtomicUsize::new(0),
            fenced: std::sync::atomic::AtomicBool::new(fenced),
            fence_after_first_commit,
        })
    }

    fn new() -> Arc<Self> {
        Self::with(true, false, false)
    }

    fn without_receipts() -> Arc<Self> {
        Self::with(false, false, false)
    }

    fn commits(&self) -> Vec<(Session, RunCheckpointReceipt)> {
        self.commits.lock().unwrap().clone()
    }

    /// The latest checkpoint: the provisional tail a restart adopts.
    fn tail(&self) -> (Session, RunCheckpointReceipt) {
        self.commits().pop().expect("the run checkpointed")
    }
}

fn receipt(session: &Session, run_id: &RunId, sequence: u64) -> RunCheckpointReceipt {
    let authority = meerkat_core::HeadCanonicalProvisionalTailAuthority::issued(
        session.id().clone(),
        1,
        "committed-head".to_string(),
        1 + sequence,
        format!("provisional-head-{sequence}"),
        run_id.clone(),
        sequence,
    )
    .expect("a contiguous provisional authority");
    RunCheckpointReceipt::issued(
        meerkat_core::RunCheckpointAuthority::HeadCanonical(authority),
        format!("digest-{sequence}"),
        session.messages().len() as u64,
    )
    .expect("a valid receipt")
}

#[async_trait]
impl SessionCheckpointer for RecordingCheckpointer {
    async fn checkpoint_run(
        &self,
        session: &mut Session,
        run_id: &RunId,
        previous: Option<&RunCheckpointReceipt>,
    ) -> Result<Option<RunCheckpointReceipt>, AgentError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if !self.receipts {
            return Ok(None);
        }
        let sequence = previous.map_or(1, |previous| previous.candidate_sequence() + 1);
        let next = receipt(session, run_id, sequence);
        if self.fence_after_first_commit
            && cursor_of(session).is_some_and(|cursor| {
                matches!(cursor.resume_point, RunResumePoint::ToolsPending { .. })
            })
        {
            self.fenced.store(true, Ordering::SeqCst);
        }
        self.commits
            .lock()
            .unwrap()
            .push((session.clone(), next.clone()));
        Ok(Some(next))
    }

    fn dispatch_fence_check(&self) -> Result<(), AgentError> {
        if self.fenced.load(Ordering::SeqCst) {
            return Err(AgentError::session_durable_projection_authority_unknown(
                "the writer's self-fence window closed",
            ));
        }
        Ok(())
    }
}

struct NoopStore;

#[async_trait]
impl AgentSessionStore for NoopStore {
    async fn save(&self, _session: &Session) -> Result<(), AgentError> {
        Ok(())
    }

    async fn load(&self, _id: &str) -> Result<Option<Session>, AgentError> {
        Ok(None)
    }
}

type TestAgent = meerkat_core::Agent<ScriptedClient, RecordingTools, NoopStore>;

/// An agent over `session`. `content_turn` stamps the runtime execution kind
/// of an ordinary content turn, as the runtime does before `run`; a resume
/// is not a content turn.
async fn build_agent(
    session: Session,
    client: &Arc<ScriptedClient>,
    tools: &Arc<RecordingTools>,
    checkpointer: Option<Arc<RecordingCheckpointer>>,
    content_turn: bool,
    max_turns: Option<u32>,
) -> TestAgent {
    let mut builder = AgentBuilder::new()
        .resume_session(session)
        .with_turn_state_handle(Arc::new(
            crate::agent::test_turn_state_handle::TestTurnStateHandle::new(),
        ));
    if content_turn {
        builder = builder.with_runtime_execution_kind_for_test(
            meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn,
        );
    }
    if let Some(turns) = max_turns {
        builder = builder.max_turns(turns);
    }
    if let Some(checkpointer) = checkpointer {
        builder = builder.with_checkpointer(checkpointer);
    }
    builder
        .build_standalone(Arc::clone(client), Arc::clone(tools), Arc::new(NoopStore))
        .await
}

fn fresh_session() -> Session {
    let mut session = Session::new();
    session
        .set_build_state(meerkat_core::SessionBuildState::default())
        .expect("test session build state should serialize");
    session
}

/// Drive `run` until `stop` is signalled, then drop it: the run ends as a
/// crash would end it, with no exit path of its own.
async fn run_until_stopped<F: std::future::Future>(run: F, stop: &Notify) {
    tokio::select! {
        () = stop.notified() => {}
        _ = run => panic!("the run ended before it reached the point it is stopped at"),
    }
}

fn cursor_of(session: &Session) -> Option<SessionRunCursor> {
    session.try_run_cursor().expect("the cursor decodes")
}

fn assistant_rows(session: &Session) -> Vec<&BlockAssistantMessage> {
    session
        .messages()
        .iter()
        .filter_map(|message| match message {
            Message::BlockAssistant(assistant) => Some(assistant),
            _ => None,
        })
        .collect()
}

fn tool_results(session: &Session) -> Vec<ToolResult> {
    session
        .messages()
        .iter()
        .filter_map(|message| match message {
            Message::ToolResults { results, .. } => Some(results.clone()),
            _ => None,
        })
        .flatten()
        .collect()
}

fn drain(rx: &mut mpsc::Receiver<AgentEvent>) -> Vec<AgentEvent> {
    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }
    events
}

/// A run stopped inside its first tool round, after the round's first
/// commit, and the tail that commit left.
struct StoppedRun {
    tail: Session,
    receipt: RunCheckpointReceipt,
    cursor: SessionRunCursor,
    first_keys: Vec<(String, Option<ToolCallDedupeKey>)>,
}

/// Run until the tool `hang_on` is entered in the first round, then drop the
/// run. The round's first commit is the run's latest checkpoint.
async fn stop_inside_first_round(
    calls: Vec<(&'static str, &'static str, &'static str)>,
    tools: Vec<ToolDef>,
    hang_on: &'static str,
) -> StoppedRun {
    let client = ScriptedClient::new(vec![Step::Tools(calls)]);
    let tools = RecordingTools::new(tools, Some(hang_on));
    let checkpointer = RecordingCheckpointer::new();
    let mut agent = build_agent(
        fresh_session(),
        &client,
        &tools,
        Some(Arc::clone(&checkpointer)),
        true,
        None,
    )
    .await;
    run_until_stopped(agent.run("start".into()), &tools.entered).await;
    let (tail, receipt) = checkpointer.tail();
    let cursor = cursor_of(&tail).expect("the round's first commit wrote a cursor");
    StoppedRun {
        tail,
        receipt,
        cursor,
        first_keys: tools.dispatched(),
    }
}

/// The first commit of a round stages its assistant message in the cursor
/// and writes no transcript row; the second writes the message and its
/// results together. The run's end saves without a cursor. The first round
/// takes a first commit too.
#[tokio::test]
async fn each_tool_round_commits_twice_and_the_end_clears_the_cursor() {
    let client = ScriptedClient::new(vec![
        Step::Tools(vec![("call-1", "probe", "{}")]),
        Step::Answer("done"),
    ]);
    let tools = RecordingTools::new(RecordingTools::probes(), None);
    let checkpointer = RecordingCheckpointer::new();
    let mut agent = build_agent(
        fresh_session(),
        &client,
        &tools,
        Some(Arc::clone(&checkpointer)),
        true,
        None,
    )
    .await;

    let result = agent.run("start".into()).await.expect("the run completes");
    assert_eq!(result.text, "done");

    let commits = checkpointer.commits();
    assert_eq!(
        commits.len(),
        3,
        "first commit, second commit, end-of-run save"
    );

    let (first, _) = &commits[0];
    let cursor = cursor_of(first).expect("the first commit carries a cursor");
    assert_eq!(cursor.round, 0);
    let RunResumePoint::ToolsPending { assistant_message } = &cursor.resume_point else {
        panic!(
            "the first commit stages the round: {:?}",
            cursor.resume_point
        );
    };
    assert_eq!(
        assistant_message
            .tool_calls()
            .map(|call| call.id.to_string())
            .collect::<Vec<_>>(),
        vec!["call-1".to_string()]
    );
    assert_eq!(
        assistant_message.identity.run_id.as_ref(),
        Some(&cursor.run_id),
        "the staged message is stamped with the run"
    );
    assert!(
        assistant_rows(first).is_empty(),
        "the first commit writes no assistant row: the transcript never ends in a tool call \
         without its result"
    );

    let (second, _) = &commits[1];
    let cursor = cursor_of(second).expect("the second commit carries a cursor");
    assert_eq!(cursor.round, 0);
    assert!(matches!(cursor.resume_point, RunResumePoint::AwaitingModel));
    assert_eq!(cursor.loop_state.tool_call_count, 1);
    assert_eq!(cursor.loop_state.turn_count, 1);
    assert!(matches!(
        second.messages().last(),
        Some(Message::ToolResults { .. })
    ));

    let (end, _) = &commits[2];
    assert!(
        cursor_of(end).is_none(),
        "the end-of-run save removes the cursor"
    );
    assert!(cursor_of(agent.session()).is_none());
}

/// A run stopped after its round's first commit resumes there: the staged
/// message is the round's model response, so the model is not asked for the
/// round again, and its call is dispatched again under the same call id and
/// the same dedupe key. Nothing is published again for the staged message;
/// `RunResumed` announces the run in place of `RunStarted`.
#[tokio::test]
async fn a_run_resumed_at_its_first_commit_dispatches_the_staged_calls_again() {
    let stopped = stop_inside_first_round(
        vec![("call-1", "probe", r#"{"q":1}"#)],
        RecordingTools::probes(),
        "probe",
    )
    .await;
    assert!(matches!(
        stopped.cursor.resume_point,
        RunResumePoint::ToolsPending { .. }
    ));
    let RunResumePoint::ToolsPending { assistant_message } = stopped.cursor.resume_point.clone()
    else {
        unreachable!()
    };

    let client = ScriptedClient::new(vec![Step::Answer("done")]);
    let tools = RecordingTools::new(RecordingTools::probes(), None);
    let checkpointer = RecordingCheckpointer::new();
    let mut agent = build_agent(
        stopped.tail.clone(),
        &client,
        &tools,
        Some(Arc::clone(&checkpointer)),
        false,
        None,
    )
    .await;
    let (tx, mut rx) = mpsc::channel(1024);
    let result = agent
        .resume_run(
            stopped.cursor.clone(),
            stopped.receipt.clone(),
            Some(tx),
            None,
            Vec::new(),
        )
        .await
        .expect("the resumed run completes");
    assert_eq!(result.text, "done");
    assert_eq!(result.tool_calls, 1);

    assert_eq!(
        client.calls(),
        1,
        "only the call after the resumed round asks the model"
    );
    let dispatched = tools.dispatched();
    assert_eq!(dispatched.len(), 1);
    assert_eq!(dispatched[0].0, "call-1");
    let key = dispatched[0]
        .1
        .clone()
        .expect("the resumed call carries a key");
    assert_eq!(key.run_id, stopped.cursor.run_id);
    assert_eq!(key.round, 0);
    assert_eq!(key.call_id, "call-1");
    assert_eq!(
        Some(key),
        stopped.first_keys[0].1.clone(),
        "the call has the same dedupe key fresh and after the resume"
    );

    let events = drain(&mut rx);
    assert!(
        matches!(
            events.first(),
            Some(AgentEvent::RunResumed {
                run_id,
                round: 0,
                resume_point: meerkat_core::RunResumePointKind::ToolsPending,
                ..
            }) if *run_id == stopped.cursor.run_id
        ),
        "the resumed run opens with RunResumed: {events:?}"
    );
    assert!(
        !events.iter().any(|event| matches!(
            event,
            AgentEvent::RunStarted { .. } | AgentEvent::ToolCallRequested { .. }
        )),
        "a resumed run publishes no RunStarted and no request for the staged calls: {events:?}"
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, AgentEvent::TurnCompleted { .. }))
            .count(),
        1,
        "only the model call after the resumed round completes a turn: {events:?}"
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, AgentEvent::RunCompleted { .. }))
    );

    // The staged message enters the transcript with its own identity, once,
    // followed by its results and the final answer.
    let session = agent.session();
    let assistants = assistant_rows(session);
    assert_eq!(assistants.len(), 2);
    assert_eq!(
        assistants[0].assistant_message_id, assistant_message.assistant_message_id,
        "the staged message keeps its assistant message id"
    );
    let results = tool_results(session);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].tool_use_id, "call-1");
    assert!(!results[0].is_error);
    assert!(cursor_of(session).is_none());

    // The resumed run's checkpoints chain from the adopted tail's receipt.
    let commits = checkpointer.commits();
    let (second, second_receipt) = &commits[0];
    assert_eq!(
        second_receipt.candidate_sequence(),
        stopped.receipt.candidate_sequence() + 1
    );
    let cursor = cursor_of(second).expect("the resumed round's second commit");
    assert_eq!(cursor.round, 0);
    assert!(matches!(cursor.resume_point, RunResumePoint::AwaitingModel));
}

/// A run stopped after its round's second commit resumes with its next model
/// call, from the transcript that commit wrote: no tool runs again.
#[tokio::test]
async fn a_run_resumed_after_its_second_commit_asks_the_model_next() {
    let client = ScriptedClient::new(vec![
        Step::Tools(vec![("call-1", "probe", "{}")]),
        Step::Hang,
    ]);
    let tools = RecordingTools::new(RecordingTools::probes(), None);
    let checkpointer = RecordingCheckpointer::new();
    let mut agent = build_agent(
        fresh_session(),
        &client,
        &tools,
        Some(Arc::clone(&checkpointer)),
        true,
        None,
    )
    .await;
    run_until_stopped(agent.run("start".into()), &client.hung).await;
    let (tail, receipt) = checkpointer.tail();
    let cursor = cursor_of(&tail).expect("the second commit wrote a cursor");
    assert!(matches!(cursor.resume_point, RunResumePoint::AwaitingModel));
    assert_eq!(cursor.round, 0);

    let client = ScriptedClient::new(vec![Step::Answer("done")]);
    let tools = RecordingTools::new(RecordingTools::probes(), None);
    let mut agent = build_agent(
        tail,
        &client,
        &tools,
        Some(RecordingCheckpointer::new()),
        false,
        None,
    )
    .await;
    let (tx, mut rx) = mpsc::channel(1024);
    let result = agent
        .resume_run(cursor.clone(), receipt, Some(tx), None, Vec::new())
        .await
        .expect("the resumed run completes");
    assert_eq!(result.text, "done");
    assert!(tools.dispatched().is_empty(), "no tool runs again");
    assert_eq!(client.calls(), 1);
    let request = &client.requests()[0];
    assert!(
        request.iter().any(|message| matches!(
            message,
            Message::ToolResults { results, .. }
                if results.iter().any(|result| result.tool_use_id == "call-1")
        )),
        "the model sees the committed round's results"
    );
    let events = drain(&mut rx);
    assert!(
        matches!(
            events.first(),
            Some(AgentEvent::RunResumed {
                round: 0,
                resume_point: meerkat_core::RunResumePointKind::AwaitingModel,
                ..
            })
        ),
        "{events:?}"
    );
}

/// A staged call whose process custody shows that its process may have run
/// before the restart is not dispatched again: it gets an outcome-uncertain
/// result. A call whose process never started runs again.
#[tokio::test]
async fn a_resumed_round_answers_a_call_whose_process_may_have_run_as_interrupted() {
    let stopped = stop_inside_first_round(
        vec![("call-1", "probe", "{}"), ("call-2", "probe2", "{}")],
        RecordingTools::probes(),
        "probe",
    )
    .await;
    let evidence = |call: &str, cessation| crate::tool_process::InterruptedToolCall {
        entry_id: uuid::Uuid::new_v4(),
        run_id: stopped.cursor.run_id.clone(),
        tool_call_id: Some(call.to_string()),
        spawner: crate::tool_process::ToolProcessSpawner::ShellCall,
        cessation,
        settlement: crate::tool_process::InterruptedToolSettlement::Pending,
    };
    let interrupted = vec![
        evidence(
            "call-1",
            crate::tool_process::ToolProcessCessation::KilledByRecovery { members: 1 },
        ),
        evidence(
            "call-2",
            crate::tool_process::ToolProcessCessation::NeverStarted,
        ),
    ];

    let client = ScriptedClient::new(vec![Step::Answer("done")]);
    let tools = RecordingTools::new(RecordingTools::probes(), None);
    let mut agent = build_agent(
        stopped.tail.clone(),
        &client,
        &tools,
        Some(RecordingCheckpointer::new()),
        false,
        None,
    )
    .await;
    agent
        .resume_run(
            stopped.cursor.clone(),
            stopped.receipt.clone(),
            None,
            None,
            interrupted,
        )
        .await
        .expect("the resumed run completes");

    let dispatched = tools.dispatched();
    assert_eq!(
        dispatched
            .iter()
            .map(|(id, _)| id.as_str())
            .collect::<Vec<_>>(),
        vec!["call-2"],
        "only the call whose process never started runs again"
    );
    let results = tool_results(agent.session());
    let interrupted = results
        .iter()
        .find(|result| result.tool_use_id == "call-1")
        .expect("the interrupted call has a result");
    assert!(interrupted.is_error);
    assert!(
        interrupted.text_content().contains("outcome_uncertain"),
        "{}",
        interrupted.text_content()
    );
    assert!(
        results
            .iter()
            .any(|result| result.tool_use_id == "call-2" && !result.is_error)
    );
}

/// A staged call to one of Core's comms tools that sends to another agent is
/// not sent again after a restart: whether it was delivered is unknown.
#[tokio::test]
async fn a_resumed_round_does_not_send_a_comms_message_again() {
    let comms = ToolDef::new(
        "send",
        "sends to a peer",
        serde_json::json!({ "type": "object" }),
    )
    .with_provenance(crate::ToolProvenance::new(
        crate::ToolSourceKind::Comms,
        "comms",
    ));
    let mut tool_defs = RecordingTools::probes();
    tool_defs.push(comms);
    let stopped = stop_inside_first_round(
        vec![("call-1", "send", r#"{"to":"peer"}"#)],
        tool_defs.clone(),
        "send",
    )
    .await;

    let client = ScriptedClient::new(vec![Step::Answer("done")]);
    let tools = RecordingTools::new(tool_defs, None);
    let mut agent = build_agent(
        stopped.tail.clone(),
        &client,
        &tools,
        Some(RecordingCheckpointer::new()),
        false,
        None,
    )
    .await;
    agent
        .resume_run(
            stopped.cursor.clone(),
            stopped.receipt.clone(),
            None,
            None,
            Vec::new(),
        )
        .await
        .expect("the resumed run completes");

    assert!(
        tools.dispatched().is_empty(),
        "the message is not sent again"
    );
    let results = tool_results(agent.session());
    assert_eq!(results.len(), 1);
    assert!(results[0].is_error);
    assert!(results[0].text_content().contains("outcome_uncertain"));
}

/// A resumed call that admission refuses now (here its tool is gone after
/// the restart) may have taken effect before it: its result says the
/// outcome is unknown and carries the refusal, instead of reading as a call
/// that never ran.
#[tokio::test]
async fn a_resumed_call_refused_at_admission_answers_outcome_unknown() {
    let stopped = stop_inside_first_round(
        vec![("call-1", "probe", "{}")],
        RecordingTools::probes(),
        "probe",
    )
    .await;

    let client = ScriptedClient::new(vec![Step::Answer("done")]);
    let without_probe = vec![ToolDef::new(
        "probe2",
        "returns",
        serde_json::json!({ "type": "object" }),
    )];
    let tools = RecordingTools::new(without_probe, None);
    let mut agent = build_agent(
        stopped.tail.clone(),
        &client,
        &tools,
        Some(RecordingCheckpointer::new()),
        false,
        None,
    )
    .await;
    agent
        .resume_run(
            stopped.cursor.clone(),
            stopped.receipt.clone(),
            None,
            None,
            Vec::new(),
        )
        .await
        .expect("the resumed run completes");

    assert!(tools.dispatched().is_empty());
    let results = tool_results(agent.session());
    assert_eq!(results.len(), 1);
    assert!(results[0].is_error);
    let text = results[0].text_content();
    assert!(text.contains("outcome_uncertain"), "{text}");
    assert!(text.contains("refused"), "{text}");
}

/// A first commit the checkpointer does not make durable (no receipt) fails
/// the round before any of its tools runs.
#[tokio::test]
async fn a_first_commit_without_a_receipt_runs_no_tool() {
    let client = ScriptedClient::new(vec![Step::Tools(vec![("call-1", "probe", "{}")])]);
    let tools = RecordingTools::new(RecordingTools::probes(), None);
    let checkpointer = RecordingCheckpointer::without_receipts();
    let mut agent = build_agent(
        fresh_session(),
        &client,
        &tools,
        Some(Arc::clone(&checkpointer)),
        true,
        None,
    )
    .await;

    let error = agent
        .run("start".into())
        .await
        .expect_err("the round fails at its first commit");
    assert!(
        error.to_string().contains("made nothing durable"),
        "{error}"
    );
    assert!(tools.dispatched().is_empty(), "no tool of the round runs");
    assert_eq!(checkpointer.calls.load(Ordering::SeqCst), 1);
}

/// A writer past its self-fence window dispatches nothing: no model call
/// when the window closed before the run, and no tool when it closed right
/// after a round's first commit. The run stops with the teardown-required
/// failure and writes nothing more.
#[tokio::test]
async fn a_writer_past_its_fence_window_dispatches_nothing() {
    let client = ScriptedClient::new(vec![Step::Answer("done")]);
    let tools = RecordingTools::new(RecordingTools::probes(), None);
    let checkpointer = RecordingCheckpointer::with(true, true, false);
    let mut agent = build_agent(
        fresh_session(),
        &client,
        &tools,
        Some(Arc::clone(&checkpointer)),
        true,
        None,
    )
    .await;
    let error = agent
        .run("start".into())
        .await
        .expect_err("a fenced writer calls no model");
    assert!(error.requires_session_teardown(), "{error}");
    assert_eq!(client.calls(), 0);

    let client = ScriptedClient::new(vec![Step::Tools(vec![("call-1", "probe", "{}")])]);
    let tools = RecordingTools::new(RecordingTools::probes(), None);
    let checkpointer = RecordingCheckpointer::with(true, false, true);
    let mut agent = build_agent(
        fresh_session(),
        &client,
        &tools,
        Some(Arc::clone(&checkpointer)),
        true,
        None,
    )
    .await;
    let error = agent
        .run("start".into())
        .await
        .expect_err("a writer fenced after the first commit runs no tool");
    assert!(error.requires_session_teardown(), "{error}");
    assert!(tools.dispatched().is_empty(), "no tool of the round runs");
    assert_eq!(
        checkpointer.commits().len(),
        1,
        "only the round's first commit was written; the fenced run writes nothing more"
    );
}

/// A writer whose self-fence window closes while a round's calls run starts
/// no further call. With one call at a time, the round's second call asks
/// the fence after the first started, is refused, and never runs. The round
/// fails before its second commit with the teardown-required failure, the
/// run writes nothing more, and its first commit, cursor included, stays the
/// resume point.
#[tokio::test]
async fn a_call_after_the_fence_closes_never_starts() {
    let client = ScriptedClient::new(vec![Step::Tools(vec![
        ("call-1", "probe", "{}"),
        ("call-2", "probe2", "{}"),
    ])]);
    let tools = RecordingTools::new(RecordingTools::probes(), None);
    let checkpointer = RecordingCheckpointer::new();
    tools.close_fence_when_a_call_starts(Arc::clone(&checkpointer));
    let mut agent = AgentBuilder::new()
        .resume_session(fresh_session())
        .with_turn_state_handle(Arc::new(
            crate::agent::test_turn_state_handle::TestTurnStateHandle::new(),
        ))
        .with_runtime_execution_kind_for_test(
            meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn,
        )
        // The builder takes the trait object; the row keeps the concrete
        // handle for its own assertions.
        .with_checkpointer(Arc::clone(&checkpointer) as Arc<dyn SessionCheckpointer>)
        .with_tools_config(meerkat_core::ToolsConfig {
            max_concurrent: 1,
            ..meerkat_core::ToolsConfig::default()
        })
        .build_standalone(Arc::clone(&client), Arc::clone(&tools), Arc::new(NoopStore))
        .await;

    let error = agent
        .run("start".into())
        .await
        .expect_err("the fence refusal fails the round");
    assert!(error.requires_session_teardown(), "{error}");
    let dispatched = tools.dispatched();
    assert_eq!(
        dispatched
            .iter()
            .map(|(id, _)| id.as_str())
            .collect::<Vec<_>>(),
        vec!["call-1"],
        "the first call started before the window closed; the second never ran"
    );
    let commits = checkpointer.commits();
    assert_eq!(
        commits.len(),
        1,
        "only the round's first commit: no second commit and no closing write"
    );
    let cursor = cursor_of(&commits[0].0).expect("the first commit keeps its cursor");
    assert!(matches!(
        cursor.resume_point,
        RunResumePoint::ToolsPending { .. }
    ));
    assert!(
        tool_results(agent.session()).is_empty(),
        "no result of the round reached the transcript"
    );
    assert!(cursor_of(agent.session()).is_none(), "no cursor stays live");
    assert_eq!(
        agent.tool_dispatch_context.round(),
        None,
        "the failed round no longer names its round for the next dispatch"
    );
}

/// A checkpointer whose writer's fence reads closed at once and is restored
/// by the wait, as a host store writer that a timed-out renew stopped and
/// whose re-acquire then succeeds: its check without a wait refuses, and its
/// waiting check yields once and allows. It counts the waits.
struct RestoredFenceCheckpointer {
    inner: Arc<RecordingCheckpointer>,
    waits: AtomicUsize,
}

#[async_trait]
impl SessionCheckpointer for RestoredFenceCheckpointer {
    async fn checkpoint_run(
        &self,
        session: &mut Session,
        run_id: &RunId,
        previous: Option<&RunCheckpointReceipt>,
    ) -> Result<Option<RunCheckpointReceipt>, AgentError> {
        self.inner.checkpoint_run(session, run_id, previous).await
    }

    fn dispatch_fence_check(&self) -> Result<(), AgentError> {
        Err(AgentError::session_durable_projection_authority_unknown(
            "the writer stopped; only its re-acquire restores the fence",
        ))
    }

    async fn dispatch_fence_ready(
        &self,
        _suspendable_run: Option<&RunId>,
    ) -> Result<(), AgentError> {
        self.waits.fetch_add(1, Ordering::SeqCst);
        tokio::task::yield_now().await;
        Ok(())
    }
}

/// Every dispatch of a run awaits the checkpointer's waiting fence, never
/// its check without a wait: a fence that reads closed at once but is
/// restored by the wait lets the run make both model calls and start both
/// calls of its round, one at a time, and the run completes. The run asks
/// the fence before each model call (2), once after the round's first
/// commit (1), and before each call starts (2).
#[tokio::test]
async fn every_dispatch_awaits_the_waiting_fence() {
    let client = ScriptedClient::new(vec![
        Step::Tools(vec![("call-1", "probe", "{}"), ("call-2", "probe2", "{}")]),
        Step::Answer("done"),
    ]);
    let tools = RecordingTools::new(RecordingTools::probes(), None);
    let checkpointer = Arc::new(RestoredFenceCheckpointer {
        inner: RecordingCheckpointer::new(),
        waits: AtomicUsize::new(0),
    });
    let mut agent = AgentBuilder::new()
        .resume_session(fresh_session())
        .with_turn_state_handle(Arc::new(
            crate::agent::test_turn_state_handle::TestTurnStateHandle::new(),
        ))
        .with_runtime_execution_kind_for_test(
            meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn,
        )
        .with_checkpointer(Arc::clone(&checkpointer) as Arc<dyn SessionCheckpointer>)
        .with_tools_config(meerkat_core::ToolsConfig {
            max_concurrent: 1,
            ..meerkat_core::ToolsConfig::default()
        })
        .build_standalone(Arc::clone(&client), Arc::clone(&tools), Arc::new(NoopStore))
        .await;

    agent
        .run("start".into())
        .await
        .expect("the restored fence admits every dispatch");
    assert_eq!(client.calls(), 2, "both model calls were made");
    assert_eq!(
        tools
            .dispatched()
            .iter()
            .map(|(id, _)| id.as_str())
            .collect::<Vec<_>>(),
        vec!["call-1", "call-2"],
        "both calls of the round started"
    );
    assert_eq!(
        checkpointer.waits.load(Ordering::SeqCst),
        5,
        "the fence was awaited before each model call, after the round's first commit and before each call"
    );
}

/// A checkpointer whose fence lets the run's model call and its round's
/// post-commit check through, then answers the round's two per-call checks
/// out of order: the first refuses after a short wait, the second allows
/// after a longer one, so both are waiting when the refusal comes in. It
/// counts the waits.
struct SplitFenceCheckpointer {
    inner: Arc<RecordingCheckpointer>,
    waits: AtomicUsize,
}

#[async_trait]
impl SessionCheckpointer for SplitFenceCheckpointer {
    async fn checkpoint_run(
        &self,
        session: &mut Session,
        run_id: &RunId,
        previous: Option<&RunCheckpointReceipt>,
    ) -> Result<Option<RunCheckpointReceipt>, AgentError> {
        self.inner.checkpoint_run(session, run_id, previous).await
    }

    async fn dispatch_fence_ready(
        &self,
        _suspendable_run: Option<&RunId>,
    ) -> Result<(), AgentError> {
        match self.waits.fetch_add(1, Ordering::SeqCst) {
            // The model call and the round's post-commit check.
            0 | 1 => Ok(()),
            // The first call's check refuses after a short wait.
            2 => {
                for _ in 0..2 {
                    tokio::task::yield_now().await;
                }
                Err(AgentError::session_durable_projection_authority_unknown(
                    "the writer's fence window closed",
                ))
            }
            // The other call's check allows, but only after that refusal.
            _ => {
                for _ in 0..20 {
                    tokio::task::yield_now().await;
                }
                Ok(())
            }
        }
    }
}

/// A refusal that comes in while another call of the round waits at the
/// fence stops that call too, although its own wait then allows: each call
/// checks the round's refusal again after its wait. With two calls at once,
/// the first call's check refuses while the second's still waits; the second
/// call never starts, and the round fails with the refusal.
#[tokio::test]
async fn a_refusal_while_another_call_waits_stops_that_call_too() {
    let client = ScriptedClient::new(vec![Step::Tools(vec![
        ("call-1", "probe", "{}"),
        ("call-2", "probe2", "{}"),
    ])]);
    let tools = RecordingTools::new(RecordingTools::probes(), None);
    let checkpointer = Arc::new(SplitFenceCheckpointer {
        inner: RecordingCheckpointer::new(),
        waits: AtomicUsize::new(0),
    });
    let mut agent = AgentBuilder::new()
        .resume_session(fresh_session())
        .with_turn_state_handle(Arc::new(
            crate::agent::test_turn_state_handle::TestTurnStateHandle::new(),
        ))
        .with_runtime_execution_kind_for_test(
            meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn,
        )
        .with_checkpointer(Arc::clone(&checkpointer) as Arc<dyn SessionCheckpointer>)
        .with_tools_config(meerkat_core::ToolsConfig {
            max_concurrent: 2,
            ..meerkat_core::ToolsConfig::default()
        })
        .build_standalone(Arc::clone(&client), Arc::clone(&tools), Arc::new(NoopStore))
        .await;

    let error = agent
        .run("start".into())
        .await
        .expect_err("the refusal fails the round");
    assert!(error.requires_session_teardown(), "{error}");
    assert!(
        tools.dispatched().is_empty(),
        "neither call started: the refused one, nor the one whose wait allowed after the refusal"
    );
    assert_eq!(
        checkpointer.waits.load(Ordering::SeqCst),
        4,
        "both calls waited at the fence: the model call, the post-commit check and two calls"
    );
}

/// No round outlives its run. A run whose round fails (here its first
/// commit returns no receipt) leaves no round bound, and a call the host
/// dispatches next carries no dedupe key, even under the failed round's call
/// id and arguments: tool call ids repeat across turns, and a host that
/// deduplicates on the key must never answer a new call with an old result.
/// Dropping a run's future closes its dispatch context, round included, and
/// a host call carries no key even while a round is bound.
#[tokio::test]
async fn a_host_call_after_a_failed_round_carries_no_key() {
    let client = ScriptedClient::new(vec![Step::Tools(vec![("call-1", "probe", "{}")])]);
    let tools = RecordingTools::new(RecordingTools::probes(), None);
    let mut agent = build_agent(
        fresh_session(),
        &client,
        &tools,
        Some(RecordingCheckpointer::without_receipts()),
        true,
        None,
    )
    .await;
    agent
        .run("start".into())
        .await
        .expect_err("the round fails at its first commit");
    assert!(tools.dispatched().is_empty(), "no tool of the round ran");
    assert_eq!(
        agent.tool_dispatch_context.round(),
        None,
        "the failed round is not bound after the run"
    );
    agent
        .dispatch_external_tool_call(meerkat_core::ToolCall::new(
            "call-1".into(),
            "probe".into(),
            serde_json::json!({}),
        ))
        .await
        .expect("the host call dispatches");
    assert_eq!(
        tools.dispatched(),
        vec![("call-1".to_string(), None)],
        "the host call carries no key"
    );

    let client = ScriptedClient::new(vec![Step::Tools(vec![("call-1", "probe", "{}")])]);
    let tools = RecordingTools::new(RecordingTools::probes(), Some("probe"));
    let mut agent = build_agent(
        fresh_session(),
        &client,
        &tools,
        Some(RecordingCheckpointer::new()),
        true,
        None,
    )
    .await;
    run_until_stopped(agent.run("start".into()), &tools.entered).await;
    let round_key = tools.dispatched()[0]
        .1
        .clone()
        .expect("the round's own call carries its key");
    assert_eq!(round_key.round, 0);
    assert_eq!(
        agent.tool_dispatch_context.round(),
        None,
        "dropping the run future closed its dispatch context"
    );
    // The host call path drops the round itself: bind the dropped run's
    // identity and round as its loop had them, so the call could carry a key.
    agent
        .tool_dispatch_context
        .bind_run_id(round_key.run_id.clone());
    agent.tool_dispatch_context.bind_round(0);
    agent
        .dispatch_external_tool_call(meerkat_core::ToolCall::new(
            "call-1".into(),
            "probe2".into(),
            serde_json::json!({}),
        ))
        .await
        .expect("the host call dispatches");
    assert_eq!(
        tools.dispatched()[1],
        ("call-1".to_string(), None),
        "a host call carries no key while a round is bound"
    );
    let _ = agent.cancel_dropped_run();
    assert_eq!(agent.tool_dispatch_context.round(), None);
}

/// A run that ends without an end-of-run save (here at its turn cap, right
/// after a round's second commit) closes its cursor with one more
/// checkpoint, so its boundary never commits a cursor.
#[tokio::test]
async fn a_run_capped_after_a_round_closes_its_cursor() {
    let client = ScriptedClient::new(vec![Step::Tools(vec![("call-1", "probe", "{}")])]);
    let tools = RecordingTools::new(RecordingTools::probes(), None);
    let checkpointer = RecordingCheckpointer::new();
    let mut agent = build_agent(
        fresh_session(),
        &client,
        &tools,
        Some(Arc::clone(&checkpointer)),
        true,
        Some(1),
    )
    .await;

    let _ = agent.run("start".into()).await;
    let commits = checkpointer.commits();
    assert_eq!(
        commits.len(),
        3,
        "two round commits, then the closing write"
    );
    let (closing, _) = commits.last().unwrap();
    assert!(
        cursor_of(closing).is_none(),
        "the closing write has no cursor"
    );
    assert!(matches!(
        closing.messages().last(),
        Some(Message::ToolResults { .. })
    ));
}

/// A run interrupted between the first and second commits of its first
/// round keeps the first commit as its latest checkpoint, cursor included,
/// for recovery; its live session keeps no cursor, so no later save of it
/// commits one, and the dispatch context no longer names the round.
#[tokio::test]
async fn an_interrupted_run_keeps_its_commit_and_leaves_no_live_cursor() {
    let client = ScriptedClient::new(vec![Step::Tools(vec![("call-1", "probe", "{}")])]);
    let tools = RecordingTools::new(RecordingTools::probes(), Some("probe"));
    let checkpointer = RecordingCheckpointer::new();
    let mut agent = build_agent(
        fresh_session(),
        &client,
        &tools,
        Some(Arc::clone(&checkpointer)),
        true,
        None,
    )
    .await;
    run_until_stopped(agent.run("start".into()), &tools.entered).await;
    assert!(
        cursor_of(agent.session()).is_some(),
        "the dropped run left its cursor live"
    );

    let _ = agent.cancel_dropped_run();

    let commits = checkpointer.commits();
    assert_eq!(commits.len(), 1, "only the round's first commit");
    let cursor = cursor_of(&commits[0].0).expect("the durable commit keeps its cursor");
    assert!(matches!(
        cursor.resume_point,
        RunResumePoint::ToolsPending { .. }
    ));
    assert!(cursor_of(agent.session()).is_none(), "no cursor stays live");
}

/// A run cancelled at a boundary ends for good: its exit writes the closing
/// checkpoint without the cursor, so no committed boundary claims an open
/// round.
#[tokio::test]
async fn a_run_cancelled_at_a_boundary_closes_its_cursor() {
    let client = ScriptedClient::new(vec![
        Step::Tools(vec![("call-1", "probe", "{}")]),
        Step::Answer("never asked"),
    ]);
    let tools = RecordingTools::new(RecordingTools::probes(), None);
    let checkpointer = RecordingCheckpointer::new();
    let mut agent = build_agent(
        fresh_session(),
        &client,
        &tools,
        Some(Arc::clone(&checkpointer)),
        true,
        None,
    )
    .await;
    tools.cancel_at_next_boundary(agent.cancel_after_boundary_handle());

    let error = agent
        .run("start".into())
        .await
        .expect_err("the run stops at its next boundary");
    assert!(
        matches!(error.primary_error(), AgentError::Cancelled),
        "{error}"
    );
    assert_eq!(client.calls(), 1, "no model call after the cancel");
    let commits = checkpointer.commits();
    assert_eq!(
        commits.len(),
        3,
        "both round commits, then the closing write"
    );
    let (last, _) = commits.last().unwrap();
    assert!(
        cursor_of(last).is_none(),
        "a cancelled run claims no open round"
    );
    assert!(cursor_of(agent.session()).is_none());
}

/// A run the host suspends (it drains and requeues its work) is dropped at
/// its next await point and realizes nothing: no terminal, no write. Its
/// last round commit, cursor included, stays its resume point; the live
/// session keeps no cursor; and the suspended run comes back as resumable,
/// with its id, for the session layer's typed outcome.
#[tokio::test]
async fn a_suspended_run_keeps_its_last_round_commit_and_realizes_nothing() {
    let client = ScriptedClient::new(vec![Step::Tools(vec![("call-1", "probe", "{}")])]);
    let tools = RecordingTools::new(RecordingTools::probes(), Some("probe"));
    let checkpointer = RecordingCheckpointer::new();
    let mut agent = build_agent(
        fresh_session(),
        &client,
        &tools,
        Some(Arc::clone(&checkpointer)),
        true,
        None,
    )
    .await;
    run_until_stopped(agent.run("start".into()), &tools.entered).await;

    let suspension = agent
        .suspend_dropped_run()
        .expect("the suspended run had started");
    let meerkat_core::DroppedRunSuspension::Resumable { run_id: suspended } = suspension else {
        panic!("a run stopped inside a round resumes there: {suspension:?}");
    };

    let commits = checkpointer.commits();
    assert_eq!(
        commits.len(),
        1,
        "nothing written after the round's first commit"
    );
    let cursor = cursor_of(&commits[0].0).expect("the resume point keeps its cursor");
    assert_eq!(cursor.run_id, suspended);
    assert!(matches!(
        cursor.resume_point,
        RunResumePoint::ToolsPending { .. }
    ));
    assert!(cursor_of(agent.session()).is_none(), "no cursor stays live");
    assert!(
        agent.suspend_dropped_run().is_none(),
        "the agent holds no run after the suspension"
    );
}

/// A run whose future the host drops after its end-of-run save landed has
/// nothing left to resume: its latest checkpoint holds its answer and no
/// cursor. The agent reports it past its final save, so the stop runs none
/// of its inputs again, and leaves it to the caller's cancellation, which
/// gives the session's stream the run's terminal it was still owed.
#[tokio::test]
async fn a_run_suspended_after_its_final_save_reports_it() {
    let client = ScriptedClient::new(vec![
        Step::Tools(vec![("call-1", "probe", "{}")]),
        Step::Answer("done"),
    ]);
    let tools = RecordingTools::new(RecordingTools::probes(), None);
    let checkpointer = RecordingCheckpointer::new();
    let mut agent = build_agent(
        fresh_session(),
        &client,
        &tools,
        Some(Arc::clone(&checkpointer)),
        true,
        None,
    )
    .await;
    let final_save_landed = || {
        checkpointer
            .commits()
            .last()
            .is_some_and(|(session, _)| cursor_of(session).is_none())
    };
    // A one-event channel, drained one event at a time: each poll of the
    // run publishes one event and then waits at its next, so the run is
    // dropped at the first event it publishes after its final save (its
    // `RunCompleted`).
    let (tx, mut rx) = mpsc::channel(1);
    {
        let run = agent.run_with_events("start".into(), tx);
        tokio::pin!(run);
        loop {
            tokio::select! {
                biased;
                _ = &mut run => panic!("the run ended before it was dropped"),
                event = rx.recv() => {
                    assert!(event.is_some(), "the run holds the sender");
                    if final_save_landed() {
                        break;
                    }
                }
            }
        }
    }

    let suspension = agent
        .suspend_dropped_run()
        .expect("the suspended run had started");
    let meerkat_core::DroppedRunSuspension::PastFinalSave { run_id } = suspension else {
        panic!("the run's final save had landed: {suspension:?}");
    };
    let commits = checkpointer.commits();
    assert_eq!(
        commits.len(),
        3,
        "two round commits, then the end-of-run save"
    );
    let (tail, _) = commits.last().unwrap();
    assert!(cursor_of(tail).is_none(), "the final save has no cursor");
    assert!(
        assistant_rows(tail).iter().any(|assistant| assistant
            .blocks
            .iter()
            .any(|block| matches!(block, AssistantBlock::Text { text, .. } if text == "done"))),
        "the final save holds the run's answer"
    );
    let (first, _) = &commits[0];
    assert_eq!(
        cursor_of(first)
            .expect("a round commit carries the cursor")
            .run_id,
        run_id
    );
    assert!(cursor_of(agent.session()).is_none());

    let owed = agent.cancel_dropped_run();
    assert!(
        matches!(owed, Some(AgentEvent::RunFailed { .. })),
        "the stream is still owed the run's terminal: {owed:?}"
    );
    assert!(
        agent.suspend_dropped_run().is_none(),
        "the cancellation ended the run"
    );
}

/// A run that takes no round commits has no resume point, so a stop that
/// keeps the session's work cannot suspend it: the agent reports it not
/// resumable and leaves its state for the caller's cancellation. That holds
/// for an agent without a session checkpointer and for a live bridge run,
/// which commits nothing although its session is durable.
#[tokio::test]
async fn a_run_without_round_commits_is_not_resumable() {
    for live_bridge in [false, true] {
        let client = ScriptedClient::new(vec![Step::Tools(vec![("call-1", "probe", "{}")])]);
        let tools = RecordingTools::new(RecordingTools::probes(), Some("probe"));
        let checkpointer = RecordingCheckpointer::new();
        let mut agent = build_agent(
            fresh_session(),
            &client,
            &tools,
            live_bridge.then(|| Arc::clone(&checkpointer)),
            true,
            None,
        )
        .await;
        if live_bridge {
            // As the live bridge operation installs it: a run that commits
            // nothing, over the bridge's frozen tool surface.
            agent.noncommitting_live_bridge_run = true;
            agent.live_bridge_tool_defs = Some(tools.tools());
        }
        run_until_stopped(agent.run("start".into()), &tools.entered).await;

        let suspension = agent
            .suspend_dropped_run()
            .expect("the dropped run had started");
        let meerkat_core::DroppedRunSuspension::NotResumable { run_id } = suspension else {
            panic!("a run without round commits has no resume point: {suspension:?}");
        };
        let key = tools.dispatched()[0]
            .1
            .clone()
            .expect("the round's call carries its key");
        assert_eq!(key.run_id, run_id, "live bridge: {live_bridge}");
        assert!(
            checkpointer.commits().is_empty(),
            "nothing durable names the run's progress"
        );
        assert_eq!(
            agent.tool_dispatch_context.round(),
            None,
            "dropping the run future closed its dispatch context"
        );
        let again = agent.suspend_dropped_run();
        assert!(
            matches!(
                &again,
                Some(meerkat_core::DroppedRunSuspension::NotResumable { run_id: kept })
                    if *kept == run_id
            ),
            "the run's state is left for the caller's cancellation: {again:?}"
        );

        let _ = agent.cancel_dropped_run();
        assert_eq!(agent.tool_dispatch_context.round(), None);
        assert!(
            agent.suspend_dropped_run().is_none(),
            "the cancellation ended the run"
        );
    }
}

/// A run that re-based before its restart still says so after it: the
/// resumed run restores the flag from its cursor, and every later commit's
/// cursor carries it, although the run does not re-base again. A restart
/// therefore never forgets that the run's inputs are in the committed
/// history.
#[tokio::test]
async fn a_resumed_run_keeps_the_rebased_flag_its_cursor_carried() {
    let mut stopped = stop_inside_first_round(
        vec![("call-1", "probe", "{}")],
        RecordingTools::probes(),
        "probe",
    )
    .await;
    assert!(!stopped.cursor.rebased, "the run had not re-based");
    stopped.cursor.rebased = true;
    stopped
        .tail
        .set_run_cursor(&stopped.cursor)
        .expect("the cursor records");

    let client = ScriptedClient::new(vec![Step::Answer("done")]);
    let tools = RecordingTools::new(RecordingTools::probes(), None);
    let checkpointer = RecordingCheckpointer::new();
    let mut agent = build_agent(
        stopped.tail.clone(),
        &client,
        &tools,
        Some(Arc::clone(&checkpointer)),
        false,
        None,
    )
    .await;
    agent
        .resume_run(
            stopped.cursor.clone(),
            stopped.receipt.clone(),
            None,
            None,
            Vec::new(),
        )
        .await
        .expect("the resumed run completes");

    let commits = checkpointer.commits();
    assert_eq!(
        commits.len(),
        2,
        "the resumed round's second commit, then the end-of-run save"
    );
    let cursor = cursor_of(&commits[0].0).expect("a round commit carries the cursor");
    assert!(matches!(cursor.resume_point, RunResumePoint::AwaitingModel));
    assert!(cursor.rebased, "the restored flag rides every later cursor");
    assert!(
        cursor_of(&commits[1].0).is_none(),
        "the end-of-run save has no cursor"
    );
}

/// The cursor lists the inputs the session layer staged for the run; a run
/// nobody staged inputs for (a direct run) lists none.
#[tokio::test]
async fn the_cursor_binds_the_runs_start_inputs() {
    for staged in [
        vec![
            meerkat_core::lifecycle::InputId::new(),
            meerkat_core::lifecycle::InputId::new(),
        ],
        Vec::new(),
    ] {
        let client = ScriptedClient::new(vec![
            Step::Tools(vec![("call-1", "probe", "{}")]),
            Step::Answer("done"),
        ]);
        let tools = RecordingTools::new(RecordingTools::probes(), None);
        let checkpointer = RecordingCheckpointer::new();
        let mut agent = build_agent(
            fresh_session(),
            &client,
            &tools,
            Some(Arc::clone(&checkpointer)),
            true,
            None,
        )
        .await;
        agent.set_run_start_input_ids(staged.clone());
        agent.run("start".into()).await.expect("the run completes");
        let (first, _) = &checkpointer.commits()[0];
        assert_eq!(
            cursor_of(first).expect("first commit").bound_input_ids,
            staged
        );
    }
}

/// A staged round built by hand: the run's input and a ToolsPending cursor
/// whose run-stamped message calls `probe`.
fn staged_tail(
    run_id: &RunId,
    bound: Vec<meerkat_core::lifecycle::InputId>,
) -> (Session, SessionRunCursor) {
    let mut session = fresh_session();
    session.push(Message::User(UserMessage::text("start")));
    let mut assistant =
        BlockAssistantMessage::new(vec![tool_use("call-1", "probe", "{}")], StopReason::ToolUse);
    assistant.identity.run_id = Some(run_id.clone());
    assistant.assistant_message_id = Some(AssistantMessageId::mint());
    let cursor = SessionRunCursor::new(
        run_id.clone(),
        bound,
        0,
        RunResumePoint::ToolsPending {
            assistant_message: assistant,
        },
    );
    session.set_run_cursor(&cursor).expect("the cursor encodes");
    (session, cursor)
}

/// A resumed run's dispatched inputs must be the inputs its cursor binds,
/// and the receipt it continues from must be its own run's.
#[tokio::test]
async fn resume_refuses_a_foreign_receipt_or_other_inputs() {
    let run_id = RunId::new();
    let bound = meerkat_core::lifecycle::InputId::new();

    let (tail, cursor) = staged_tail(&run_id, vec![bound.clone()]);
    let client = ScriptedClient::new(vec![Step::Answer("done")]);
    let tools = RecordingTools::new(RecordingTools::probes(), None);
    let mut agent = build_agent(
        tail.clone(),
        &client,
        &tools,
        Some(RecordingCheckpointer::new()),
        false,
        None,
    )
    .await;
    agent.set_run_start_input_ids(vec![meerkat_core::lifecycle::InputId::new()]);
    let error = agent
        .resume_run(
            cursor.clone(),
            receipt(&tail, &run_id, 1),
            None,
            None,
            Vec::new(),
        )
        .await
        .expect_err("other dispatched inputs are refused");
    assert!(error.to_string().contains("dispatched inputs"), "{error}");

    let mut agent = build_agent(
        tail.clone(),
        &client,
        &tools,
        Some(RecordingCheckpointer::new()),
        false,
        None,
    )
    .await;
    let error = agent
        .resume_run(
            cursor,
            receipt(&tail, &RunId::new(), 1),
            None,
            None,
            Vec::new(),
        )
        .await
        .expect_err("another run's receipt is refused");
    assert!(error.to_string().contains("checkpoint receipt"), "{error}");

    assert_eq!(client.calls(), 0);
    assert!(tools.dispatched().is_empty());
}

/// Asynchronous operations a run was waiting for when its second commit
/// landed ended with the restart: the resumed run does not wait for them,
/// and the model is told their results will not arrive.
#[tokio::test]
async fn a_run_resumed_before_its_barrier_settled_tells_the_model_the_work_is_lost() {
    let run_id = RunId::new();
    let mut tail = fresh_session();
    tail.push(Message::User(UserMessage::text("start")));
    let mut assistant =
        BlockAssistantMessage::new(vec![tool_use("call-1", "probe", "{}")], StopReason::ToolUse);
    assistant.identity.run_id = Some(run_id.clone());
    tail.push(Message::BlockAssistant(assistant));
    tail.push(Message::tool_results(vec![ToolResult::new(
        "call-1".to_string(),
        "started".to_string(),
        false,
    )]));
    let mut cursor =
        SessionRunCursor::new(run_id.clone(), Vec::new(), 0, RunResumePoint::AwaitingModel);
    cursor.loop_state.turn_count = 1;
    cursor.loop_state.tool_call_count = 1;
    cursor.loop_state.pending_ops = vec![crate::session::RoundPendingOperation {
        call_id: "call-1".to_string(),
        operation: crate::ops::AsyncOpRef {
            operation_id: crate::ops::OperationId::new(),
            wait_policy: crate::ops::WaitPolicy::Barrier,
        },
    }];
    tail.set_run_cursor(&cursor).expect("the cursor encodes");

    let client = ScriptedClient::new(vec![Step::Answer("done")]);
    let tools = RecordingTools::new(RecordingTools::probes(), None);
    let mut agent = build_agent(
        tail.clone(),
        &client,
        &tools,
        Some(RecordingCheckpointer::new()),
        false,
        None,
    )
    .await;
    let result = agent
        .resume_run(cursor, receipt(&tail, &run_id, 2), None, None, Vec::new())
        .await
        .expect("the resumed run completes");
    assert_eq!(result.text, "done");
    assert_eq!(result.tool_calls, 1, "the counters carry over");
    let request = &client.requests()[0];
    assert!(
        request.iter().any(|message| matches!(
            message,
            Message::SystemNotice(notice)
                if notice.body.as_deref().is_some_and(|body| body.contains("call-1"))
        )),
        "the model is told the operation's results will not arrive"
    );
}

/// An agent without a checkpointer (an ephemeral session) commits nothing
/// and never carries a cursor, but its calls still carry a dedupe key.
#[tokio::test]
async fn an_ephemeral_run_commits_nothing_and_still_keys_its_calls() {
    let client = ScriptedClient::new(vec![
        Step::Tools(vec![("call-1", "probe", "{}")]),
        Step::Tools(vec![("call-2", "probe", "{}")]),
        Step::Answer("done"),
    ]);
    let tools = RecordingTools::new(RecordingTools::probes(), None);
    let mut agent = build_agent(fresh_session(), &client, &tools, None, true, None).await;
    agent.run("start".into()).await.expect("the run completes");

    assert!(cursor_of(agent.session()).is_none());
    let rounds = tools
        .dispatched()
        .into_iter()
        .map(|(_, key)| key.expect("every call is keyed").round)
        .collect::<Vec<_>>();
    assert_eq!(rounds, vec![0, 1], "each tool round has its own index");
}

/// The dedupe key binds the call's name and arguments: a provider that
/// reuses a call id for another call gets another key, and a call outside an
/// agent-loop round has none.
#[test]
fn the_dedupe_key_tells_a_reused_call_id_apart() {
    fn view(args: &RawValue) -> ToolCallView<'_> {
        ToolCallView {
            id: "fc_0",
            name: "search",
            args,
        }
    }
    let args = |json: &str| RawValue::from_string(json.to_string()).unwrap();
    let first_args = args(r#"{"b":1,"a":"x"}"#);
    let same_args = args(r#"{"a":"x","b":1}"#);
    let other_args = args(r#"{"a":"y"}"#);

    let mut context = ToolDispatchContext::default();
    assert!(context.tool_call_dedupe_key(view(&first_args)).is_none());
    context.bind_run_id(RunId::new());
    assert!(
        context.tool_call_dedupe_key(view(&first_args)).is_none(),
        "no key before the loop binds a round"
    );
    context.bind_round(3);
    let first = context.tool_call_dedupe_key(view(&first_args)).unwrap();
    assert_eq!(first.round, 3);
    assert_eq!(
        Some(first.clone()),
        context.tool_call_dedupe_key(view(&same_args)),
        "the same call in another key order has the same key"
    );
    assert_ne!(
        Some(first),
        context.tool_call_dedupe_key(view(&other_args)),
        "a reused call id with other arguments has another key"
    );
    context.clear_round();
    assert!(
        context.tool_call_dedupe_key(view(&first_args)).is_none(),
        "a call dispatched between rounds carries no key"
    );
}
