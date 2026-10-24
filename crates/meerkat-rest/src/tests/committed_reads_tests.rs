//! Committed reads while a durable run holds its provisional tail, on the REST
//! surface.
//!
//! From a durable run's first tool round until its boundary commit, each
//! round commit applies the run's provisional tail to the physical head while
//! the RuntimeStore boundary stays at the committed snapshot. On a
//! HeadCanonical store (the SQLite realm) a body read of that boundary then
//! refuses with `TranscriptRevisionConflict`, which REST answered as a 500.
//!
//! Each row holds a real run after one round commit: a scripted model answers
//! one tool call, then holds its second model call until the test releases
//! it. The row drives one REST operation against the running session and
//! then lets the run complete. An operation that waits for the run's turn
//! boundary is run across the held run: it starts while the run holds and
//! answers once the run is released. A read the session serves mid-run must
//! answer while the run holds.
//!
//! The WholeBlob twin of a row (a JSONL realm) is the oracle: there the tail
//! is a separate row and the committed document never moves, so nothing
//! conflicts and the operation answers as it did before rounds were
//! committed. The HeadCanonical row pins the same exact answer.

use super::*;
use futures::StreamExt as _;
use meerkat_core::lifecycle::RunId;
use meerkat_runtime::RuntimeStore as _;
use std::collections::VecDeque;
use std::time::Duration as StdDuration;
use tokio::sync::Semaphore;

/// Bound on one step of a row: it only turns a hang into a failure.
pub(crate) const DEADLINE: StdDuration = StdDuration::from_secs(60);
/// How long an operation run across the held run is given to answer before
/// the run is released: one that waits for the run's turn boundary is still
/// waiting then.
const GRACE: StdDuration = StdDuration::from_millis(500);
/// Bound on a read that must answer while the run holds.
const MID_RUN_BOUND: StdDuration = StdDuration::from_secs(15);
/// The scripted tool call: a name no catalog has, so the call is answered
/// with its typed refusal and the round closes without any tool.
const PROBE_TOOL: &str = "committed_reads_probe";

/// What the scripted model answers to one request.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Step {
    /// One call of [`PROBE_TOOL`].
    Tool,
    /// Text, once the test releases the model.
    Held,
    /// Text at once.
    Text(&'static str),
}

pub(crate) struct ScriptedModel {
    steps: StdMutex<VecDeque<Step>>,
    requests: AtomicUsize,
    entered: Semaphore,
    release: Semaphore,
}

impl ScriptedModel {
    pub(crate) fn new(steps: impl IntoIterator<Item = Step>) -> Arc<Self> {
        Arc::new(Self {
            steps: StdMutex::new(steps.into_iter().collect()),
            requests: AtomicUsize::new(0),
            entered: Semaphore::new(0),
            release: Semaphore::new(0),
        })
    }

    pub(crate) fn push(&self, step: Step) {
        self.steps
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push_back(step);
    }

    async fn wait_entered(&self) {
        tokio::time::timeout(DEADLINE, self.entered.acquire())
            .await
            .expect("the run reaches its held model call")
            .expect("model gate")
            .forget();
    }

    pub(crate) fn release(&self) {
        self.release.add_permits(1);
    }
}

#[async_trait]
impl LlmClient for ScriptedModel {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        let n = self.requests.fetch_add(1, AtomicOrdering::SeqCst);
        let step = self
            .steps
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop_front()
            .unwrap_or(Step::Text("unscripted"));
        let usage = || LlmEvent::UsageUpdate {
            usage: meerkat_core::TurnUsage::host_declared(
                provider_for_successful_rest_test_model(&request.model),
                &request.model,
                meerkat_core::Usage::default(),
            ),
        };
        let text = |text: &str| {
            vec![
                Ok(LlmEvent::TextDelta {
                    delta: text.into(),
                    meta: None,
                }),
                Ok(usage()),
                Ok(LlmEvent::Done {
                    outcome: LlmDoneOutcome::success(meerkat_core::StopReason::EndTurn),
                }),
            ]
        };
        match step {
            Step::Tool => Box::pin(stream::iter(vec![
                Ok(LlmEvent::ToolCallComplete {
                    id: format!("probe-call-{n}"),
                    name: PROBE_TOOL.into(),
                    args: json!({}),
                    meta: None,
                }),
                Ok(usage()),
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

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::Other
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

/// A REST state over a JSONL realm: the WholeBlob profile.
async fn load_whole_blob_rest_state(temp: &TempDir) -> AppState {
    let mut bootstrap = RuntimeBootstrap::default();
    bootstrap.realm.state_root = Some(temp.path().join("realms"));
    bootstrap.realm.selection = meerkat_core::RealmSelection::Explicit {
        realm_id: "rest-committed-reads-whole-blob".to_string(),
    };
    bootstrap.realm.backend_hint = Some("jsonl".to_string());
    bootstrap.context.context_root = Some(temp.path().to_path_buf());
    bootstrap.context.user_config_root = Some(temp.path().join("user"));
    AppState::load_from_with_bootstrap(
        temp.path().to_path_buf(),
        bootstrap,
        false,
        test_credential_store(temp.path()),
    )
    .await
    .expect("load the JSONL REST state")
}

pub(crate) fn continue_request(session_id: &SessionId, prompt: &str) -> ContinueSessionRequest {
    ContinueSessionRequest {
        injected_context: None,
        transient_turn_context: None,
        session_id: session_id.to_string(),
        prompt: ContentInput::Text(prompt.to_string()),
        system_prompt: None,
        output_schema: None,
        structured_output_retries: None,
        keep_alive: None,
        comms_name: None,
        peer_meta: None,
        verbose: false,
        model: None,
        provider: None,
        auth_binding: None,
        max_tokens: None,
        hooks_override: None,
        enable_web_search: None,
        skill_refs: None,
        turn_tool_overlay: None,
        additional_instructions: None,
    }
}

/// One continue turn on `session_id`, as `POST /sessions/{id}/messages`
/// runs it.
pub(crate) async fn continue_turn(
    state: &AppState,
    session_id: &SessionId,
    prompt: &str,
) -> Result<SessionResponse, ApiError> {
    match Box::pin(continue_session_inner(
        state,
        &session_id.to_string(),
        continue_request(session_id, prompt),
        None,
    ))
    .await
    {
        RequestTerminal::Publish(result) | RequestTerminal::RespondWithoutPublish(result) => {
            result.map(|Json(response)| response)
        }
    }
}

/// The run of the store's provisional tail, if one stands.
pub(crate) async fn tail_run(state: &AppState, id: &SessionId) -> Option<RunId> {
    let runtime_store = state.session_service.runtime_store();
    let runtime_id = meerkat_runtime::LogicalRuntimeId::for_session(id);
    match runtime_store.session_persistence_profile() {
        meerkat_runtime::RuntimeSessionPersistenceProfile::HeadCanonicalV1 => runtime_store
            .load_head_canonical_provisional_tail(&runtime_id)
            .await
            .expect("read the provisional tail")
            .map(|tail| tail.run_id().clone()),
        _ => runtime_store
            .load_whole_blob_provisional_tail(&runtime_id)
            .await
            .expect("read the provisional tail")
            .map(|tail| tail.authority().run_id().clone()),
    }
}

/// How many assistant messages of the committed transcript of `id` say
/// exactly `text`.
pub(crate) async fn committed_text_count(state: &AppState, id: &SessionId, text: &str) -> usize {
    let Ok(Some(session)) = state.session_service.load_authoritative_session(id).await else {
        return 0;
    };
    session
        .messages()
        .iter()
        .filter(|message| match message {
            meerkat_core::Message::BlockAssistant(assistant) => {
                assistant.text_blocks().collect::<String>() == text
            }
            _ => false,
        })
        .count()
}

/// A read that must answer while the run holds, without waiting for it.
pub(crate) async fn mid_run<T>(read: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(MID_RUN_BOUND, read)
        .await
        .expect("the read answers while the run holds, without waiting for the run")
}

/// A REST session whose second turn holds after one round commit.
pub(crate) struct RestHeldRun {
    _temp: TempDir,
    pub(crate) state: AppState,
    pub(crate) model: Arc<ScriptedModel>,
    pub(crate) session_id: SessionId,
    pub(crate) run_id: RunId,
    /// The committed message count before the held run started (T0).
    pub(crate) committed_before: usize,
    turn: tokio::task::JoinHandle<Result<SessionResponse, ApiError>>,
}

impl RestHeldRun {
    /// A session over the given profile: one completed turn, then a second
    /// turn whose first tool round committed and whose second model call
    /// holds.
    pub(crate) async fn start(head_canonical: bool) -> Self {
        let temp = TempDir::new().expect("tempdir");
        let mut state = if head_canonical {
            AppState::load_from(temp.path().to_path_buf())
                .await
                .expect("load the SQLite REST state")
        } else {
            load_whole_blob_rest_state(&temp).await
        };
        let model = ScriptedModel::new([Step::Text("first answer"), Step::Tool, Step::Held]);
        state.llm_client_override = Some(model.clone());
        let session_id = create_deferred_rest_runtime_session(&state).await;
        let first = continue_turn(&state, &session_id, "first")
            .await
            .expect("the first turn completes");
        assert_eq!(first.text, "first answer");
        let committed_before = state
            .session_service
            .load_authoritative_session(&session_id)
            .await
            .expect("read the committed session")
            .expect("the session is committed")
            .messages()
            .len();

        let turn = tokio::spawn({
            let state = state.clone();
            let session_id = session_id.clone();
            async move { continue_turn(&state, &session_id, "second").await }
        });
        model.wait_entered().await;
        let run_id = tail_run(&state, &session_id)
            .await
            .expect("the held run's round commit stands as the provisional tail");
        Self {
            _temp: temp,
            state,
            model,
            session_id,
            run_id,
            committed_before,
            turn,
        }
    }

    /// Run `op` across the held run: start it while the run holds, release
    /// the run [`GRACE`] later, and return its outcome and whether it had
    /// already answered when the run was released.
    pub(crate) async fn across<T: Send + 'static>(
        &self,
        op: impl std::future::Future<Output = T> + Send + 'static,
    ) -> (T, bool) {
        let task = tokio::spawn(op);
        tokio::time::sleep(GRACE).await;
        let answered_mid_run = task.is_finished();
        self.model.release();
        let outcome = tokio::time::timeout(DEADLINE, task)
            .await
            .expect("the operation answers once the held run is released")
            .expect("operation task");
        (outcome, answered_mid_run)
    }

    /// The held turn's result, once its run has ended.
    pub(crate) async fn finish(mut self) -> (Result<SessionResponse, ApiError>, Self) {
        let result = tokio::time::timeout(DEADLINE, &mut self.turn)
            .await
            .expect("the held turn ends")
            .expect("turn task");
        (result, self)
    }

    /// The held run completes with its answer (release it first).
    pub(crate) async fn completes(self) -> Self {
        let (result, held) = self.finish().await;
        assert_eq!(result.expect("the held run completes").text, "held answer");
        held
    }

    /// Release the held run and require it to complete.
    pub(crate) async fn release_and_complete(self) -> Self {
        self.model.release();
        self.completes().await
    }
}

// ---------------------------------------------------------------------------
// POST /sessions/{id}/runs/{run_id}/stop
// ---------------------------------------------------------------------------

async fn rest_stop_run_mid_run(head_canonical: bool) {
    let held = RestHeldRun::start(head_canonical).await;
    let stopped = mid_run(stop_session_run(
        State(held.state.clone()),
        Path((held.session_id.to_string(), held.run_id.to_string())),
        Json(meerkat_contracts::StopRunRequest {
            reason: "committed reads row".to_string(),
        }),
    ))
    .await;
    match stopped {
        Ok(Json(result)) => assert!(
            matches!(
                result.receipt,
                meerkat_contracts::WireRunStopReceipt::Stopped { ref run_id, .. }
                    if run_id == &held.run_id.to_string()
            ),
            "the exact running run is stopped: {:?}",
            result.receipt
        ),
        Err(error) => panic!("the run-fenced stop of a running run answers: {error:?}"),
    }
    let (result, _held) = held.finish().await;
    assert!(result.is_err(), "the stopped turn does not complete");
}

#[tokio::test]
async fn rest_stop_run_mid_run_head_canonical() {
    rest_stop_run_mid_run(true).await;
}

#[tokio::test]
async fn rest_stop_run_mid_run_whole_blob() {
    rest_stop_run_mid_run(false).await;
}

// ---------------------------------------------------------------------------
// POST /sessions/{id}/messages while a run is going
// ---------------------------------------------------------------------------

/// A continue of a running session queues behind the run and runs once after
/// it, answering then.
async fn rest_continue_mid_run_queues_behind_the_running_run(head_canonical: bool) {
    let held = RestHeldRun::start(head_canonical).await;
    held.model.push(Step::Text("queued answer"));
    let state = held.state.clone();
    let session_id = held.session_id.clone();
    let (queued, answered_mid_run) = held
        .across(async move { continue_turn(&state, &session_id, "queued").await })
        .await;
    assert!(
        !answered_mid_run,
        "the continue queues behind the running run: {queued:?}"
    );
    assert_eq!(
        queued
            .expect("the queued continue runs after the held run")
            .text,
        "queued answer"
    );
    let held = held.completes().await;
    assert_eq!(
        committed_text_count(&held.state, &held.session_id, "queued answer").await,
        1,
        "the queued continue runs exactly once"
    );
}

#[tokio::test]
async fn rest_continue_mid_run_queues_behind_the_running_run_head_canonical() {
    rest_continue_mid_run_queues_behind_the_running_run(true).await;
}

#[tokio::test]
async fn rest_continue_mid_run_queues_behind_the_running_run_whole_blob() {
    rest_continue_mid_run_queues_behind_the_running_run(false).await;
}

// ---------------------------------------------------------------------------
// Webhooks: /external-events (without wake) and /peer-response-terminal
// ---------------------------------------------------------------------------

/// Both webhook modes admit into a running session at once (202).
async fn rest_webhook_mid_run_admits_in_both_modes(head_canonical: bool) {
    let held = RestHeldRun::start(head_canonical).await;
    held.model.push(Step::Text("after the webhook"));
    let generic = make_runtime_external_event_input(
        "committed_reads_row",
        json!({"while": "running"}),
        None,
        None,
    )
    .expect("external event input");
    let without_wake = mid_run(admit_runtime_input_via_webhook(
        &held.state,
        &held.session_id,
        generic,
        WebhookAdmissionMode::WithoutWake,
    ))
    .await;
    assert!(
        matches!(without_wake, Ok((StatusCode::ACCEPTED, _))),
        "an external event into a running session is admitted: {:?}",
        without_wake
            .as_ref()
            .map_err(axum::response::Response::status)
    );
    let peer = meerkat_runtime::peer_response_terminal_input(
        meerkat_core::comms::PeerId::new(),
        None,
        meerkat_core::PeerCorrelationId::from_uuid(uuid::Uuid::new_v4()),
        meerkat_contracts::PeerResponseTerminalStatusWire::Completed,
        json!({"while": "running"}),
    );
    let wakeful = mid_run(admit_runtime_input_via_webhook(
        &held.state,
        &held.session_id,
        peer,
        WebhookAdmissionMode::Wakeful,
    ))
    .await;
    assert!(
        matches!(wakeful, Ok((StatusCode::ACCEPTED, _))),
        "a terminal peer response into a running session is admitted: {:?}",
        wakeful.as_ref().map_err(axum::response::Response::status)
    );
    held.release_and_complete().await;
}

#[tokio::test]
async fn rest_webhook_mid_run_admits_in_both_modes_head_canonical() {
    rest_webhook_mid_run_admits_in_both_modes(true).await;
}

#[tokio::test]
async fn rest_webhook_mid_run_admits_in_both_modes_whole_blob() {
    rest_webhook_mid_run_admits_in_both_modes(false).await;
}

// ---------------------------------------------------------------------------
// GET /sessions/{id}/events
// ---------------------------------------------------------------------------

async fn rest_events_opened_mid_run_streams(head_canonical: bool) {
    use http_body_util::BodyExt;

    let held = RestHeldRun::start(head_canonical).await;
    let sse = mid_run(session_events(
        State(held.state.clone()),
        Path(held.session_id.to_string()),
    ))
    .await;
    let response = match sse {
        Ok(sse) => sse.into_response(),
        Err(error) => panic!("the event stream opens on a running session: {error:?}"),
    };
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body();
    let frame = mid_run(body.frame())
        .await
        .expect("a first frame")
        .expect("a readable frame");
    let data =
        String::from_utf8(frame.into_data().expect("a data frame").to_vec()).expect("UTF-8 event");
    assert!(
        data.contains("event: session_loaded"),
        "the stream starts with session_loaded: {data}"
    );
    assert!(
        data.contains(&format!("\"message_count\":{}", held.committed_before)),
        "session_loaded carries the committed snapshot's message count {}: {data}",
        held.committed_before
    );
    drop(body);
    held.release_and_complete().await;
}

#[tokio::test]
async fn rest_events_opened_mid_run_streams_head_canonical() {
    rest_events_opened_mid_run_streams(true).await;
}

#[tokio::test]
async fn rest_events_opened_mid_run_streams_whole_blob() {
    rest_events_opened_mid_run_streams(false).await;
}

// ---------------------------------------------------------------------------
// POST /sessions/{id}/mcp/{add,remove,reload}
// ---------------------------------------------------------------------------

/// The live MCP precheck finds a running session, and a reload and a remove
/// of an unknown server are staged on it at once.
#[cfg(feature = "mcp")]
async fn rest_mcp_add_remove_reload_mid_run(head_canonical: bool) {
    let held = RestHeldRun::start(head_canonical).await;
    let adapter = Arc::new(McpRouterAdapter::new(McpRouter::new_with_stdio_profile(
        meerkat_mcp::McpStdioLaunchProfile::trusted_host(),
    )));
    let (lifecycle_tx, lifecycle_rx) = mpsc::unbounded_channel();
    held.state.mcp_sessions.write().await.insert(
        held.session_id.clone(),
        SessionMcpState {
            adapter,
            turn_counter: 0,
            lifecycle_tx,
            lifecycle_rx,
            drain_task_running: Arc::new(AtomicBool::new(false)),
        },
    );
    let resolved = mid_run(resolve_mcp_adapter(&held.state, &held.session_id)).await;
    assert!(
        resolved.is_ok(),
        "the live MCP precheck finds the running session: {:?}",
        resolved.err()
    );
    let reloaded = mid_run(mcp_reload(
        State(held.state.clone()),
        Path(held.session_id.to_string()),
        Json(meerkat_contracts::McpReloadParams {
            session_id: held.session_id.to_string(),
            server_name: None,
            persisted: false,
        }),
    ))
    .await;
    assert!(
        reloaded.is_ok(),
        "a live MCP reload of a running session is staged: {:?}",
        reloaded.err()
    );
    let removed = mid_run(mcp_remove(
        State(held.state.clone()),
        Path(held.session_id.to_string()),
        Json(meerkat_contracts::McpRemoveParams {
            session_id: held.session_id.to_string(),
            server_name: "absent-server".to_string(),
            persisted: false,
        }),
    ))
    .await;
    assert!(
        !matches!(removed, Err(ApiError::Internal(ref message)) if message.starts_with("Failed to load session")),
        "a live MCP remove on a running session passes the presence precheck: {:?}",
        removed.err()
    );
    held.release_and_complete().await;
}

#[cfg(feature = "mcp")]
#[tokio::test]
async fn rest_mcp_add_remove_reload_mid_run_head_canonical() {
    rest_mcp_add_remove_reload_mid_run(true).await;
}

#[cfg(feature = "mcp")]
#[tokio::test]
async fn rest_mcp_add_remove_reload_mid_run_whole_blob() {
    rest_mcp_add_remove_reload_mid_run(false).await;
}

// ---------------------------------------------------------------------------
// GET /sessions/{id}/history
// ---------------------------------------------------------------------------

async fn rest_history_mid_run(head_canonical: bool) {
    let held = RestHeldRun::start(head_canonical).await;
    let history = mid_run(get_session_history(
        State(held.state.clone()),
        Path(held.session_id.to_string()),
        Query(SessionHistoryQuery {
            offset: None,
            limit: None,
        }),
    ))
    .await;
    match history {
        Ok(Json(history)) => assert_eq!(
            history.message_count, held.committed_before,
            "history mid-run serves the committed snapshot, nothing of the running run"
        ),
        Err(error) => panic!("history of a running session answers: {error:?}"),
    }
    held.release_and_complete().await;
}

#[tokio::test]
async fn rest_history_mid_run_head_canonical() {
    rest_history_mid_run(true).await;
}

#[tokio::test]
async fn rest_history_mid_run_whole_blob() {
    rest_history_mid_run(false).await;
}
