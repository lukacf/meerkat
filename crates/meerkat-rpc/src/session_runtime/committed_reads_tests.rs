//! Committed reads while a durable run holds its provisional tail, on the
//! JSON-RPC session runtime.
//!
//! From a durable run's first tool round until its boundary commit, each
//! round commit applies the run's provisional tail to the physical head while
//! the RuntimeStore boundary stays at the committed snapshot. On a
//! HeadCanonical store (the SQLite realm) a body read of that boundary then
//! refuses with `TranscriptRevisionConflict`, and so does every operation
//! that read the body only to learn presence or archive state.
//!
//! Each row holds a real run after one round commit: a scripted model answers
//! one tool call, then holds its second model call until the test releases
//! it. The row drives one RPC operation against the running session and then
//! lets the run complete. An operation that waits for the run's turn boundary
//! (as it did before rounds were committed) is run across the held run: it
//! starts while the run holds and answers once the run is released. A read
//! the session serves mid-run must answer while the run holds.
//!
//! The WholeBlob twin of a row (in-memory stores) is the oracle: there the
//! tail is a separate row and the committed document never moves, so nothing
//! conflicts and the operation answers as it did before rounds were
//! committed. The HeadCanonical row pins the same exact answer.

use super::*;
use meerkat_client::{LlmDoneOutcome, LlmEvent, LlmRequest};
use meerkat_runtime::RuntimeStore as _;
use std::collections::VecDeque;
use std::sync::atomic::AtomicUsize;
use tokio::sync::Semaphore;

/// Bound on one step of a row: it only turns a hang into a failure.
const DEADLINE: Duration = Duration::from_secs(60);
/// How long an operation run across the held run is given to answer before
/// the run is released: one that waits for the run's turn boundary is still
/// waiting then.
const GRACE: Duration = Duration::from_millis(500);
/// Bound on a read that must answer while the run holds.
const MID_RUN_BOUND: Duration = Duration::from_secs(15);
/// The realm every SQLite row opens.
const REALM: &str = "committed-reads-under-tail";
/// The model id every row's session runs.
const MODEL: &str = "claude-sonnet-4-5";
/// The scripted tool call: a name no catalog has, so the call is answered
/// with its typed refusal and the round closes without any tool.
const PROBE_TOOL: &str = "committed_reads_probe";

/// What the scripted model answers to one request.
#[derive(Clone, Copy, Debug)]
enum Step {
    /// One call of [`PROBE_TOOL`].
    Tool,
    /// Text, once the test releases the model.
    Held,
    /// Text at once.
    Text(&'static str),
}

struct ScriptedModel {
    steps: StdMutex<VecDeque<Step>>,
    requests: AtomicUsize,
    entered: Semaphore,
    release: Semaphore,
}

impl ScriptedModel {
    fn new(steps: impl IntoIterator<Item = Step>) -> Arc<Self> {
        Arc::new(Self {
            steps: StdMutex::new(steps.into_iter().collect()),
            requests: AtomicUsize::new(0),
            entered: Semaphore::new(0),
            release: Semaphore::new(0),
        })
    }

    fn push(&self, step: Step) {
        self.steps
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push_back(step);
    }

    /// Wait until the held model call is in flight.
    async fn wait_entered(&self) {
        tokio::time::timeout(DEADLINE, self.entered.acquire())
            .await
            .expect("the run reaches its held model call")
            .expect("model gate")
            .forget();
    }

    fn release(&self) {
        self.release.add_permits(1);
    }
}

#[async_trait::async_trait]
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
    ) -> std::pin::Pin<
        Box<dyn futures::Stream<Item = Result<LlmEvent, meerkat_client::LlmError>> + Send + 'a>,
    > {
        let n = self
            .requests
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let step = self
            .steps
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop_front()
            .unwrap_or(Step::Text("unscripted"));
        let usage = meerkat_core::TurnUsage::host_declared(
            meerkat_core::Provider::Anthropic,
            &request.model,
            Default::default(),
        );
        let text = move |text: &str| {
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
            Step::Tool => Box::pin(futures::stream::iter(vec![
                Ok(LlmEvent::ToolCallComplete {
                    id: format!("probe-call-{n}"),
                    name: PROBE_TOOL.into(),
                    args: serde_json::json!({}),
                    meta: None,
                }),
                Ok(LlmEvent::UsageUpdate {
                    usage: meerkat_core::TurnUsage::host_declared(
                        meerkat_core::Provider::Anthropic,
                        &request.model,
                        Default::default(),
                    ),
                }),
                Ok(LlmEvent::Done {
                    outcome: LlmDoneOutcome::success(meerkat_core::StopReason::ToolUse),
                }),
            ])),
            Step::Held => {
                let events = text("held answer");
                Box::pin(
                    futures::stream::once(async move {
                        self.entered.add_permits(1);
                        self.release
                            .acquire()
                            .await
                            .expect("release the model")
                            .forget();
                        events
                    })
                    .flat_map(futures::stream::iter),
                )
            }
            Step::Text(answer) => Box::pin(futures::stream::iter(text(answer))),
        }
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::Anthropic
    }

    async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
        Ok(())
    }
}

/// An RPC runtime over a HeadCanonical SQLite realm in `root`.
pub(super) async fn sqlite_runtime(root: &std::path::Path) -> Arc<SessionRuntime> {
    let (_manifest, bundle) = meerkat::open_realm_persistence_in(
        root,
        REALM,
        Some(meerkat_store::RealmBackend::Sqlite),
        None,
    )
    .await
    .expect("open the SQLite realm");
    let runtime = Arc::new(
        SessionRuntime::new(
            AgentFactory::new(root.join("agent-sessions")),
            Config::default(),
            10,
            bundle,
            crate::router::NotificationSink::noop(),
        )
        .expect("compose the RPC session runtime"),
    );
    runtime.set_realm_context(
        Some(meerkat_core::connection::RealmId::parse(REALM).expect("realm id")),
        None,
        None,
    );
    runtime
}

/// An RPC runtime over WholeBlob in-memory stores.
fn whole_blob_runtime(root: &std::path::Path) -> Arc<SessionRuntime> {
    let store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
    let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
        Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
    let blob_store: Arc<dyn meerkat_core::BlobStore> =
        Arc::new(meerkat_store::MemoryBlobStore::new());
    Arc::new(
        SessionRuntime::new(
            AgentFactory::new(root.join("agent-sessions")),
            Config::default(),
            10,
            meerkat::PersistenceBundle::new(store, runtime_store, blob_store)
                .expect("construct runtime authority"),
            crate::router::NotificationSink::noop(),
        )
        .expect("compose the RPC session runtime"),
    )
}

/// The run of the store's provisional tail, if one stands.
async fn tail_run(runtime: &SessionRuntime, id: &SessionId) -> Option<RunId> {
    let runtime_store = runtime.service.runtime_store();
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

/// One runtime-routed turn on `id`, as `turn/start` admits it.
async fn run_turn(
    runtime: &Arc<SessionRuntime>,
    id: &SessionId,
    prompt: &str,
) -> Result<RunResult, RpcError> {
    let (event_tx, mut event_rx) = mpsc::channel(256);
    let drain = tokio::spawn(async move { while event_rx.recv().await.is_some() {} });
    let result = runtime
        .start_turn_via_runtime(
            id,
            prompt.into(),
            Vec::new(),
            event_tx,
            None,
            None,
            None,
            None,
        )
        .await;
    let _ = drain.await;
    result
}

/// The committed message count of `id`, read once its run has ended.
async fn committed_message_count(runtime: &SessionRuntime, id: &SessionId) -> usize {
    runtime
        .service
        .load_authoritative_session(id)
        .await
        .expect("read the committed session")
        .expect("the session is committed")
        .messages()
        .len()
}

/// How many assistant messages of the committed transcript of `id` say
/// `text`.
async fn committed_text_count(runtime: &SessionRuntime, id: &SessionId, text: &str) -> usize {
    let Ok(Some(session)) = runtime.service.load_authoritative_session(id).await else {
        return 0;
    };
    session
        .messages()
        .iter()
        .filter(|message| match message {
            Message::BlockAssistant(assistant) => {
                assistant.text_blocks().collect::<String>() == text
            }
            _ => false,
        })
        .count()
}

/// Whether the committed transcript of `id` holds an assistant `text`.
async fn committed_text(runtime: &SessionRuntime, id: &SessionId, text: &str) -> bool {
    committed_text_count(runtime, id, text).await > 0
}

/// Wait until the committed transcript of `id` holds the assistant `text`,
/// then require it exactly once.
async fn committed_exactly_once(runtime: &SessionRuntime, id: &SessionId, text: &str) {
    let deadline = tokio::time::Instant::now() + DEADLINE;
    while committed_text_count(runtime, id, text).await == 0 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the admitted input runs after the held run: {text:?} never committed"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    tokio::time::sleep(GRACE).await;
    assert_eq!(
        committed_text_count(runtime, id, text).await,
        1,
        "the admitted input runs exactly once"
    );
}

/// A read that must answer while the run holds, without waiting for it.
pub(super) async fn mid_run<T>(read: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(MID_RUN_BOUND, read)
        .await
        .expect("the read answers while the run holds, without waiting for the run")
}

/// A session whose second turn holds after one round commit.
pub(super) struct HeldRun {
    /// The realm root: a second runtime over the same realm opens it.
    pub(super) root: tempfile::TempDir,
    pub(super) runtime: Arc<SessionRuntime>,
    model: Arc<ScriptedModel>,
    pub(super) session_id: SessionId,
    run_id: RunId,
    /// The committed message count before the held run started (T0).
    committed_before: usize,
    turn: tokio::task::JoinHandle<Result<RunResult, RpcError>>,
}

impl HeldRun {
    /// A session over the given profile: one completed turn, then a second
    /// turn whose first tool round committed and whose second model call
    /// holds.
    pub(super) async fn start(head_canonical: bool) -> Self {
        let root = tempfile::tempdir().expect("tempdir");
        let runtime = if head_canonical {
            sqlite_runtime(root.path()).await
        } else {
            whole_blob_runtime(root.path())
        };
        let model = ScriptedModel::new([Step::Text("first answer"), Step::Tool, Step::Held]);
        let build_config = AgentBuildConfig {
            llm_client_override: Some(model.clone()),
            ..AgentBuildConfig::new(MODEL)
        };
        let session_id = runtime
            .create_session(build_config, None, None, Vec::new())
            .await
            .expect("create the session");
        let first = run_turn(&runtime, &session_id, "first")
            .await
            .expect("the first turn completes");
        assert_eq!(first.text, "first answer");
        let committed_before = committed_message_count(&runtime, &session_id).await;

        let turn = tokio::spawn({
            let runtime = Arc::clone(&runtime);
            let session_id = session_id.clone();
            async move { run_turn(&runtime, &session_id, "second").await }
        });
        model.wait_entered().await;
        let run_id = tail_run(&runtime, &session_id)
            .await
            .expect("the held run's round commit stands as the provisional tail");
        Self {
            root,
            runtime,
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
    pub(super) async fn across<T: Send + 'static>(
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

    /// Release the held model call and return the held turn's result.
    pub(super) async fn release_and_finish(self) -> (Result<RunResult, RpcError>, Self) {
        self.model.release();
        self.finish().await
    }

    /// The held turn's result, once its run has ended.
    pub(super) async fn finish(mut self) -> (Result<RunResult, RpcError>, Self) {
        let result = tokio::time::timeout(DEADLINE, &mut self.turn)
            .await
            .expect("the held turn ends")
            .expect("turn task");
        (result, self)
    }

    /// The held run completes with its answer.
    async fn completes(self) -> Self {
        let (result, held) = self.finish().await;
        assert_eq!(result.expect("the held run completes").text, "held answer");
        held
    }
}

fn service_turn_request(prompt: &str) -> StartTurnRequest {
    StartTurnRequest {
        injected_context: Vec::new(),
        prompt: ContentInput::Text(prompt.to_string()),
        system_prompt: None,
        event_tx: None,
        runtime: StartTurnRuntimeSemantics::runtime_metadata(
            SessionRuntime::runtime_stamped_prompt_turn_metadata_from_overrides(
                None, None, None, None, None,
            ),
        ),
    }
}

// ---------------------------------------------------------------------------
// turn/interrupt
// ---------------------------------------------------------------------------

async fn turn_interrupt_cancels_a_run_held_after_a_round_commit(head_canonical: bool) {
    let held = HeldRun::start(head_canonical).await;
    let outcome = mid_run(held.runtime.interrupt(&held.session_id)).await;
    assert!(
        matches!(
            outcome,
            Ok(meerkat_contracts::wire::WireInterruptOutcome::Interrupted)
        ),
        "turn/interrupt cancels the running run: {outcome:?}"
    );
    let (result, held) = held.finish().await;
    assert!(
        result.is_err(),
        "the interrupted turn does not complete: {result:?}"
    );
    assert!(
        !committed_text(&held.runtime, &held.session_id, "held answer").await,
        "the held model call never answered into the transcript"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn turn_interrupt_cancels_a_run_held_after_a_round_commit_head_canonical() {
    turn_interrupt_cancels_a_run_held_after_a_round_commit(true).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn turn_interrupt_cancels_a_run_held_after_a_round_commit_whole_blob() {
    turn_interrupt_cancels_a_run_held_after_a_round_commit(false).await;
}

// ---------------------------------------------------------------------------
// turn/stop_run
// ---------------------------------------------------------------------------

async fn turn_stop_run_stops_a_run_held_after_a_round_commit(head_canonical: bool) {
    let held = HeldRun::start(head_canonical).await;
    let receipt = mid_run(held.runtime.stop_run(
        &held.session_id,
        &held.run_id,
        "committed reads row".to_string(),
    ))
    .await;
    assert!(
        matches!(
            receipt,
            Ok(meerkat_runtime::RunStopReceipt::Stopped { ref run_id, .. })
                if run_id == &held.run_id
        ),
        "turn/stop_run stops the running run: {receipt:?}"
    );
    let (result, _held) = held.finish().await;
    assert!(
        result.is_err(),
        "the stopped turn does not complete: {result:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn turn_stop_run_stops_a_run_held_after_a_round_commit_head_canonical() {
    turn_stop_run_stops_a_run_held_after_a_round_commit(true).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn turn_stop_run_stops_a_run_held_after_a_round_commit_whole_blob() {
    turn_stop_run_stops_a_run_held_after_a_round_commit(false).await;
}

// ---------------------------------------------------------------------------
// A turn start through the RPC-hosted mob session service
// ---------------------------------------------------------------------------

/// A turn started on a running session joins the run's admission and runs
/// once after it, answering then; before rounds were committed the start
/// never met the run's tail.
#[cfg(feature = "mob")]
async fn turn_start_on_a_run_held_after_a_round_commit_runs_after_it(head_canonical: bool) {
    let held = HeldRun::start(head_canonical).await;
    held.model.push(Step::Text("next answer"));
    let service = held.runtime.session_service();
    let session_id = held.session_id.clone();
    let (started, answered_mid_run) = held
        .across(async move {
            service
                .start_turn(&session_id, service_turn_request("while running"))
                .await
        })
        .await;
    assert!(
        !answered_mid_run,
        "the turn start waits for the running run: {started:?}"
    );
    assert_eq!(
        started
            .expect("the turn started on a running session runs after it")
            .text,
        "next answer"
    );
    let held = held.completes().await;
    assert_eq!(
        committed_text_count(&held.runtime, &held.session_id, "next answer").await,
        1,
        "the started turn runs exactly once"
    );
}

#[cfg(feature = "mob")]
#[tokio::test(flavor = "multi_thread")]
async fn turn_start_on_a_run_held_after_a_round_commit_runs_after_it_head_canonical() {
    turn_start_on_a_run_held_after_a_round_commit_runs_after_it(true).await;
}

#[cfg(feature = "mob")]
#[tokio::test(flavor = "multi_thread")]
async fn turn_start_on_a_run_held_after_a_round_commit_runs_after_it_whole_blob() {
    turn_start_on_a_run_held_after_a_round_commit_runs_after_it(false).await;
}

// ---------------------------------------------------------------------------
// Runtime input ingress
// ---------------------------------------------------------------------------

/// Runtime input ingress into a running session waits for the run's turn
/// boundary, is accepted, and its input runs exactly once after the run.
async fn runtime_input_ingress_admits_into_a_run_held_after_a_round_commit(head_canonical: bool) {
    let held = HeldRun::start(head_canonical).await;
    held.model.push(Step::Text("queued answer"));
    let runtime = Arc::clone(&held.runtime);
    let session_id = held.session_id.clone();
    let (accepted, answered_mid_run) = held
        .across(async move {
            let adapter = runtime.runtime_adapter();
            runtime
                .accept_runtime_input_with_active_admission(
                    &adapter,
                    &session_id,
                    meerkat_runtime::Input::Prompt(meerkat_runtime::PromptInput::new(
                        "queued", None,
                    )),
                )
                .await
        })
        .await;
    assert!(
        !answered_mid_run,
        "the ingress waits for the running run's turn boundary: {accepted:?}"
    );
    assert!(
        matches!(
            accepted,
            Ok(meerkat_runtime::AcceptOutcome::Accepted { .. })
        ),
        "runtime input ingress admits into the running session: {accepted:?}"
    );
    let held = held.completes().await;
    committed_exactly_once(&held.runtime, &held.session_id, "queued answer").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn runtime_input_ingress_admits_into_a_run_held_after_a_round_commit_head_canonical() {
    runtime_input_ingress_admits_into_a_run_held_after_a_round_commit(true).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn runtime_input_ingress_admits_into_a_run_held_after_a_round_commit_whole_blob() {
    runtime_input_ingress_admits_into_a_run_held_after_a_round_commit(false).await;
}

// ---------------------------------------------------------------------------
// session/history
// ---------------------------------------------------------------------------

async fn session_history_mid_run_serves_the_committed_snapshot(head_canonical: bool) {
    let held = HeldRun::start(head_canonical).await;
    let history = mid_run(held.runtime.read_session_history_rich(
        &held.session_id,
        SessionHistoryQuery {
            offset: 0,
            limit: None,
        },
    ))
    .await;
    match history {
        Ok(Some(history)) => assert_eq!(
            history.message_count, held.committed_before,
            "session/history mid-run serves the committed snapshot, nothing of the running run"
        ),
        other => panic!("session/history mid-run reads the committed snapshot: {other:?}"),
    }
    held.model.release();
    held.completes().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn session_history_mid_run_serves_the_committed_snapshot_head_canonical() {
    session_history_mid_run_serves_the_committed_snapshot(true).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn session_history_mid_run_serves_the_committed_snapshot_whole_blob() {
    session_history_mid_run_serves_the_committed_snapshot(false).await;
}

// ---------------------------------------------------------------------------
// Durable MCP attachment intents (restore, login reinstatement, hand-off)
// ---------------------------------------------------------------------------

#[cfg(feature = "mcp")]
async fn mcp_durable_intents_mid_run_read_the_committed_snapshot(head_canonical: bool) {
    let held = HeldRun::start(head_canonical).await;
    let intents = mid_run(
        held.runtime
            .durable_mcp_attachment_intents(&held.session_id),
    )
    .await;
    assert_eq!(
        intents,
        Some(Vec::new()),
        "the restore and login reinstatement read the session's durable MCP intents mid-run \
         (None reports every restore unavailable and skips the reinstatement)"
    );
    held.model.release();
    held.completes().await;
}

#[cfg(feature = "mcp")]
#[tokio::test(flavor = "multi_thread")]
async fn mcp_durable_intents_mid_run_read_the_committed_snapshot_head_canonical() {
    mcp_durable_intents_mid_run_read_the_committed_snapshot(true).await;
}

#[cfg(feature = "mcp")]
#[tokio::test(flavor = "multi_thread")]
async fn mcp_durable_intents_mid_run_read_the_committed_snapshot_whole_blob() {
    mcp_durable_intents_mid_run_read_the_committed_snapshot(false).await;
}
