//! Committed reads while a durable run holds its provisional tail, on the MCP
//! server surface.
//!
//! From a durable run's first tool round until its boundary commit, each
//! round commit applies the run's provisional tail to the physical head while
//! the RuntimeStore boundary stays at the committed snapshot. On a
//! HeadCanonical store (the SQLite realm) a body read of that boundary then
//! refuses with `TranscriptRevisionConflict`, which the MCP tools answered as
//! an internal tool error.
//!
//! Each row holds a real run after one round commit: a scripted model answers
//! one tool call, then holds its second model call until the test releases
//! it. The row drives one MCP operation against the running session and then
//! lets the run complete. An operation that waits for the run's turn boundary
//! (the live actor's turn-boundary lease) is run across the held run: it
//! starts while the run holds and answers once the run is released. A read
//! the session serves mid-run must answer while the run holds.
//!
//! The WholeBlob twin of a row (a JSONL realm) is the oracle: there the tail
//! is a separate row and the committed document never moves, so nothing
//! conflicts and the operation answers as it did before rounds were
//! committed. The HeadCanonical row pins the same exact answer.

use super::*;
use meerkat_core::lifecycle::RunId;
use meerkat_runtime::RuntimeStore as _;
use std::collections::VecDeque;
use std::sync::atomic::AtomicUsize;
use tokio::sync::Semaphore;

/// Bound on one step of a row: it only turns a hang into a failure.
pub(crate) const DEADLINE: Duration = Duration::from_secs(60);
/// How long an operation run across the held run is given to answer before
/// the run is released: one that waits for the run's turn boundary is still
/// waiting then.
const GRACE: Duration = Duration::from_millis(500);
/// Bound on a read that must answer while the run holds.
const MID_RUN_BOUND: Duration = Duration::from_secs(15);
/// The scripted tool call: a name no catalog has, so the call is answered
/// with its typed refusal and the round closes without any tool.
const PROBE_TOOL: &str = "committed_reads_probe";
/// The JSON-RPC code of an internal tool error.
const INTERNAL_ERROR: i32 = -32603;

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
    steps: std::sync::Mutex<VecDeque<Step>>,
    requests: AtomicUsize,
    entered: Semaphore,
    release: Semaphore,
}

impl ScriptedModel {
    pub(crate) fn new(steps: impl IntoIterator<Item = Step>) -> Arc<Self> {
        Arc::new(Self {
            steps: std::sync::Mutex::new(steps.into_iter().collect()),
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
        timeout(DEADLINE, self.entered.acquire())
            .await
            .expect("the run reaches its held model call")
            .expect("model gate")
            .forget();
    }

    pub(crate) fn release(&self) {
        self.release.add_permits(1);
    }
}

#[async_trait::async_trait]
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
        use futures::StreamExt as _;

        let n = self.requests.fetch_add(1, Ordering::SeqCst);
        let step = self
            .steps
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop_front()
            .unwrap_or(Step::Text("unscripted"));
        let usage = || LlmEvent::UsageUpdate {
            usage: meerkat_core::TurnUsage::host_declared(
                provider_for_successful_mcp_test_model(&request.model),
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

/// An MCP server state over a SQLite (HeadCanonical) or JSONL (WholeBlob)
/// realm under `root`, whose default model is `model`.
async fn realm_state(
    root: &std::path::Path,
    head_canonical: bool,
    model: Arc<ScriptedModel>,
) -> MeerkatMcpState {
    MeerkatMcpState::new_with_bootstrap_options_and_llm(
        RuntimeBootstrap {
            realm: meerkat_core::RealmConfig {
                selection: RealmSelection::Explicit {
                    realm_id: "mcp-committed-reads".to_string(),
                },
                instance_id: None,
                backend_hint: Some(if head_canonical { "sqlite" } else { "jsonl" }.to_string()),
                state_root: Some(root.to_path_buf()),
            },
            context: meerkat_core::ContextConfig {
                user_config_root: Some(root.join("user")),
                ..Default::default()
            },
        },
        false,
        Some(model),
        None,
    )
    .await
    .expect("MCP realm state")
}

/// The run of the store's provisional tail, if one stands.
pub(crate) async fn tail_run(state: &MeerkatMcpState, id: &meerkat::SessionId) -> Option<RunId> {
    let runtime_store = state.service.runtime_store();
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

/// One `meerkat_resume` turn on `id`.
pub(crate) async fn resume_turn(
    state: &MeerkatMcpState,
    id: &meerkat::SessionId,
    prompt: &str,
) -> Result<Value, ToolCallError> {
    handle_tools_call(
        state,
        "meerkat_resume",
        &json!({
            "session_id": id.to_string(),
            "prompt": prompt,
        }),
    )
    .await
}

fn payload_text(payload: &Value) -> String {
    payload.to_string()
}

/// The JSON a tool returned in its text content.
fn tool_payload(result: &Value) -> Value {
    let text = result["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("a text tool payload: {result}"));
    serde_json::from_str(text)
        .unwrap_or_else(|error| panic!("a JSON tool payload: {error}: {text}"))
}

/// An MCP session whose second turn holds after one round commit.
pub(crate) struct McpHeldRun {
    _root: tempfile::TempDir,
    pub(crate) state: Arc<MeerkatMcpState>,
    pub(crate) model: Arc<ScriptedModel>,
    pub(crate) session_id: meerkat::SessionId,
    pub(crate) run_id: RunId,
    /// The committed message count before the held run started (T0).
    pub(crate) committed_before: usize,
    turn: tokio::task::JoinHandle<Result<Value, ToolCallError>>,
}

impl McpHeldRun {
    /// A live session (with its MCP router) over the given profile: one
    /// completed turn, then a second turn whose first tool round committed
    /// and whose second model call holds.
    pub(crate) async fn start(head_canonical: bool) -> Self {
        Self::start_with_id(head_canonical, meerkat::SessionId::new()).await
    }

    pub(crate) async fn start_with_id(
        head_canonical: bool,
        session_id: meerkat::SessionId,
    ) -> Self {
        let root = tempfile::tempdir().expect("tempdir");
        let model = ScriptedModel::new([Step::Text("first answer"), Step::Tool, Step::Held]);
        let state = Arc::new(realm_state(root.path(), head_canonical, Arc::clone(&model)).await);
        let session = Session::with_id(session_id.clone());
        let session_model: Arc<dyn LlmClient> = model.clone();
        materialize_test_mcp_fixture(
            &state,
            &session_id,
            McpActorMaterializationMode::Fresh,
            move |bindings| {
                let router = Arc::new(meerkat_mcp::McpRouterAdapter::new(
                    McpRouter::new_with_surface_handle_and_stdio_profile(
                        Arc::clone(bindings.external_tool_surface()),
                        meerkat_mcp::McpStdioLaunchProfile::trusted_host(),
                    ),
                ));
                let router_tools: Arc<dyn AgentToolDispatcher> = router.clone();
                let mut request = mock_deferred_materialization_request(session, bindings, None);
                let build = request.build.get_or_insert_with(Default::default);
                build.external_tools = Some(router_tools);
                build.llm_client_override = Some(meerkat::encode_llm_client_override_for_service(
                    session_model,
                ));
                (request, router)
            },
        )
        .await;
        let first = resume_turn(&state, &session_id, "first")
            .await
            .expect("the first turn completes");
        assert!(payload_text(&first).contains("first answer"), "{first}");
        let committed_before = state
            .service
            .load_authoritative_session(&session_id)
            .await
            .expect("read the committed session")
            .expect("the session is committed")
            .messages()
            .len();

        let turn = tokio::spawn({
            let state = Arc::clone(&state);
            let session_id = session_id.clone();
            async move { resume_turn(&state, &session_id, "second").await }
        });
        model.wait_entered().await;
        let run_id = tail_run(&state, &session_id)
            .await
            .expect("the held run's round commit stands as the provisional tail");
        Self {
            _root: root,
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
        let outcome = timeout(DEADLINE, task)
            .await
            .expect("the operation answers once the held run is released")
            .expect("operation task");
        (outcome, answered_mid_run)
    }

    async fn finish(mut self) -> (Result<Value, ToolCallError>, Self) {
        let result = timeout(DEADLINE, &mut self.turn)
            .await
            .expect("the held turn ends")
            .expect("turn task");
        (result, self)
    }

    /// The held run completes with its answer (release it first).
    pub(crate) async fn completes(self) -> Self {
        let (result, held) = self.finish().await;
        let result = result.expect("the held run completes");
        assert!(payload_text(&result).contains("held answer"), "{result}");
        held
    }

    /// Release the held run and require it to complete.
    pub(crate) async fn release_and_complete(self) -> Self {
        self.model.release();
        self.completes().await
    }
}

/// A read that must answer while the run holds, without waiting for it.
pub(crate) async fn mid_run<T>(read: impl std::future::Future<Output = T>) -> T {
    timeout(MID_RUN_BOUND, read)
        .await
        .expect("the read answers while the run holds, without waiting for the run")
}

/// How many assistant messages of the committed transcript of `id` say
/// exactly `text`.
async fn committed_text_count(
    state: &MeerkatMcpState,
    id: &meerkat::SessionId,
    text: &str,
) -> usize {
    let Ok(Some(session)) = state.service.load_authoritative_session(id).await else {
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

// ---------------------------------------------------------------------------
// meerkat_resume on a running session
// ---------------------------------------------------------------------------

/// A resume of a running session waits for the run's turn boundary, then
/// runs once after it.
async fn mcp_meerkat_resume_on_a_running_session(head_canonical: bool) {
    let held = McpHeldRun::start(head_canonical).await;
    held.model.push(Step::Text("queued answer"));
    let state = Arc::clone(&held.state);
    let session_id = held.session_id.clone();
    let (queued, answered_mid_run) = held
        .across(async move { resume_turn(&state, &session_id, "queued").await })
        .await;
    assert!(
        !answered_mid_run,
        "the resume waits for the running run: {queued:?}"
    );
    let queued = queued.expect("the queued resume runs after the held run");
    assert!(payload_text(&queued).contains("queued answer"), "{queued}");
    let held = held.completes().await;
    assert_eq!(
        committed_text_count(&held.state, &held.session_id, "queued answer").await,
        1,
        "the queued resume runs exactly once"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mcp_meerkat_resume_on_a_running_session_head_canonical() {
    mcp_meerkat_resume_on_a_running_session(true).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn mcp_meerkat_resume_on_a_running_session_whole_blob() {
    mcp_meerkat_resume_on_a_running_session(false).await;
}

// ---------------------------------------------------------------------------
// Runtime input admission into a running session
// ---------------------------------------------------------------------------

/// Input admission into a running session waits for the run's turn
/// boundary, is accepted, and its input runs exactly once after the run.
async fn mcp_accept_input_mid_run(head_canonical: bool) {
    let held = McpHeldRun::start(head_canonical).await;
    held.model.push(Step::Text("accepted answer"));
    let state = Arc::clone(&held.state);
    let session_id = held.session_id.clone();
    let (accepted, answered_mid_run) = held
        .across(async move {
            state
                .runtime_ingress_context()
                .accept_input_with_completion(
                    &session_id,
                    meerkat_runtime::Input::Prompt(meerkat_runtime::PromptInput::new(
                        "accepted", None,
                    )),
                    None,
                )
                .await
        })
        .await;
    assert!(
        !answered_mid_run,
        "the admission waits for the running run's turn boundary"
    );
    let completion = match accepted {
        Ok((meerkat_runtime::AcceptOutcome::Accepted { .. }, Some(completion))) => completion,
        other => panic!(
            "an input into a running session is admitted with its completion: {:?}",
            other.map(|(outcome, completion)| (outcome, completion.is_some()))
        ),
    };
    let held = held.completes().await;
    let outcome = timeout(DEADLINE, completion.wait())
        .await
        .expect("the admitted input's run ends");
    assert!(
        matches!(
            outcome,
            Ok(meerkat_runtime::completion::CompletionOutcome::Completed(ref run))
                if run.text == "accepted answer"
        ),
        "the admitted input runs after the held run: {outcome:?}"
    );
    assert_eq!(
        committed_text_count(&held.state, &held.session_id, "accepted answer").await,
        1,
        "the admitted input runs exactly once"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mcp_accept_input_mid_run_head_canonical() {
    mcp_accept_input_mid_run(true).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn mcp_accept_input_mid_run_whole_blob() {
    mcp_accept_input_mid_run(false).await;
}

// ---------------------------------------------------------------------------
// Live MCP changes (meerkat_mcp_add / remove / reload) on a running session
// ---------------------------------------------------------------------------

/// A live MCP change of a running session waits for the run's turn boundary
/// (the live router lease), then is staged.
async fn mcp_live_mcp_add_mid_run(head_canonical: bool) {
    let held = McpHeldRun::start(head_canonical).await;
    let state = Arc::clone(&held.state);
    let session_id = held.session_id.clone();
    let (reloaded, answered_mid_run) = held
        .across(async move {
            handle_tools_call(
                &state,
                "meerkat_mcp_reload",
                &json!({ "session_id": session_id.to_string() }),
            )
            .await
        })
        .await;
    assert!(
        !answered_mid_run,
        "the live MCP change waits for the running run: {reloaded:?}"
    );
    assert!(
        reloaded.is_ok(),
        "a live MCP reload of a running session is staged after the run: {reloaded:?}"
    );
    held.completes().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn mcp_live_mcp_add_mid_run_head_canonical() {
    mcp_live_mcp_add_mid_run(true).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn mcp_live_mcp_add_mid_run_whole_blob() {
    mcp_live_mcp_add_mid_run(false).await;
}

// ---------------------------------------------------------------------------
// meerkat_history on a running session
// ---------------------------------------------------------------------------

async fn mcp_history_tool_mid_run(head_canonical: bool) {
    let held = McpHeldRun::start(head_canonical).await;
    let history = mid_run(handle_tools_call(
        &held.state,
        "meerkat_history",
        &json!({ "session_id": held.session_id.to_string() }),
    ))
    .await;
    match history {
        Ok(payload) => assert_eq!(
            tool_payload(&payload)["message_count"],
            json!(held.committed_before),
            "meerkat_history mid-run serves the committed snapshot, nothing of the running run"
        ),
        Err(error) => panic!("meerkat_history of a running session answers: {error:?}"),
    }
    held.release_and_complete().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn mcp_history_tool_mid_run_head_canonical() {
    mcp_history_tool_mid_run(true).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn mcp_history_tool_mid_run_whole_blob() {
    mcp_history_tool_mid_run(false).await;
}
