//! A durable run resumed after its process was killed at a committed round,
//! on the JSON-RPC session runtime over a HeadCanonical SQLite realm.
//!
//! From a durable run's first tool round until its boundary commit, each
//! round commit applies the run's provisional tail to the physical head while
//! the RuntimeStore boundary stays at the committed snapshot. A child
//! incarnation (this test binary) runs a session whose second turn takes one
//! tool round and reaches a kill point at its second model call; the parent
//! kills it with SIGKILL, opens a new runtime over the same realm, and lets
//! that runtime resume the open run. The resumed run must complete and commit
//! its boundary over the committed history.

use super::*;
use meerkat_client::{LlmDoneOutcome, LlmEvent, LlmRequest};
use meerkat_runtime::RuntimeStore as _;
use std::collections::VecDeque;
use std::sync::atomic::AtomicUsize;

/// Bound on one step of the row: it only turns a hang into a failure.
const DEADLINE: Duration = Duration::from_secs(60);
/// The realm both incarnations open.
const REALM: &str = "resumed-run-after-kill";
/// The model id the session runs.
const MODEL: &str = "claude-sonnet-4-5";
/// The scripted tool call: a name no catalog has, so the call is answered
/// with its typed refusal and the round still commits.
const PROBE_TOOL: &str = "resumed_run_probe";
/// Names the child incarnation's root; without it the child test is a no-op.
const ROOT_ENV: &str = "MEERKAT_RPC_RESUMED_RUN_ROOT";
/// The child incarnation's libtest path.
const CHILD_TEST: &str = "session_runtime::resumed_run_tests::rpc_resumed_run_child_incarnation";
/// The libtest path of the child incarnation whose second turn carries a
/// per-turn model override.
const OVERRIDE_CHILD_TEST: &str =
    "session_runtime::resumed_run_tests::rpc_resumed_run_with_a_model_override_child_incarnation";
/// The model the override child's second turn selects: one the session's
/// provider owns, other than [`MODEL`].
const OVERRIDE_MODEL: &str = "claude-opus-4-8";

/// What the scripted model answers to one request.
#[derive(Clone, Copy, Debug)]
enum Step {
    /// One call of [`PROBE_TOOL`].
    Tool,
    /// Text at once.
    Text(&'static str),
    /// Record the kill point and never answer: the incarnation is killed
    /// while it waits.
    KillPoint,
}

struct ScriptedModel {
    steps: StdMutex<VecDeque<Step>>,
    requests: AtomicUsize,
    /// The model each request named, in order.
    models: StdMutex<Vec<String>>,
    kill_marker: Option<PathBuf>,
}

impl ScriptedModel {
    fn new(steps: impl IntoIterator<Item = Step>, kill_marker: Option<PathBuf>) -> Arc<Self> {
        Arc::new(Self {
            steps: StdMutex::new(steps.into_iter().collect()),
            requests: AtomicUsize::new(0),
            models: StdMutex::new(Vec::new()),
            kill_marker,
        })
    }

    fn models(&self) -> Vec<String> {
        self.models
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
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
        self.models
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(request.model.clone());
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
        match step {
            Step::Tool => Box::pin(futures::stream::iter(vec![
                Ok(LlmEvent::ToolCallComplete {
                    id: format!("probe-call-{n}"),
                    name: PROBE_TOOL.into(),
                    args: serde_json::json!({}),
                    meta: None,
                }),
                Ok(LlmEvent::UsageUpdate { usage }),
                Ok(LlmEvent::Done {
                    outcome: LlmDoneOutcome::success(meerkat_core::StopReason::ToolUse),
                }),
            ])),
            Step::Text(answer) => Box::pin(futures::stream::iter(vec![
                Ok(LlmEvent::TextDelta {
                    delta: answer.into(),
                    meta: None,
                }),
                Ok(LlmEvent::UsageUpdate { usage }),
                Ok(LlmEvent::Done {
                    outcome: LlmDoneOutcome::success(meerkat_core::StopReason::EndTurn),
                }),
            ])),
            Step::KillPoint => {
                let marker = self.kill_marker.clone();
                Box::pin(futures::stream::once(async move {
                    if let Some(marker) = marker {
                        std::fs::write(&marker, "model_call").expect("record the kill point");
                    }
                    std::future::pending::<Result<LlmEvent, meerkat_client::LlmError>>().await
                }))
            }
        }
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::Anthropic
    }

    async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
        Ok(())
    }
}

/// An RPC runtime over the HeadCanonical SQLite realm in `root`.
async fn sqlite_runtime(root: &std::path::Path) -> Arc<SessionRuntime> {
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

/// The run of the store's provisional tail, if one stands.
async fn tail_run(runtime: &SessionRuntime, id: &SessionId) -> Option<RunId> {
    runtime
        .service
        .runtime_store()
        .load_head_canonical_provisional_tail(&meerkat_runtime::LogicalRuntimeId::for_session(id))
        .await
        .expect("read the provisional tail")
        .map(|tail| tail.run_id().clone())
}

/// One runtime-routed turn on `id`, as `turn/start` admits it.
async fn run_turn(
    runtime: &Arc<SessionRuntime>,
    id: &SessionId,
    prompt: &str,
) -> Result<RunResult, RpcError> {
    run_turn_with_overrides(runtime, id, prompt, None).await
}

/// [`run_turn`], with the request's per-turn `overrides`.
async fn run_turn_with_overrides(
    runtime: &Arc<SessionRuntime>,
    id: &SessionId,
    prompt: &str,
    overrides: Option<crate::handlers::turn::TurnOverrides>,
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
            overrides,
        )
        .await;
    let _ = drain.await;
    result
}

/// Whether the committed transcript of `id` holds an assistant `text`.
async fn committed_text(runtime: &SessionRuntime, id: &SessionId, text: &str) -> bool {
    let Ok(Some(session)) = runtime.service.load_authoritative_session(id).await else {
        return false;
    };
    session.messages().iter().any(|message| match message {
        Message::BlockAssistant(assistant) => {
            assistant.text_blocks().collect::<String>().contains(text)
        }
        _ => false,
    })
}

/// Whether the committed transcript of `id` holds the killed run's tool
/// call: a resumed run keeps the round it committed before the kill, while
/// a discarded tail would lose it.
async fn committed_probe_call(runtime: &SessionRuntime, id: &SessionId) -> bool {
    let Ok(Some(session)) = runtime.service.load_authoritative_session(id).await else {
        return false;
    };
    session.messages().iter().any(|message| match message {
        Message::BlockAssistant(assistant) => {
            assistant.tool_calls().any(|call| call.name == PROBE_TOOL)
        }
        _ => false,
    })
}

/// The child incarnation: a session whose second turn takes one tool round
/// and then reaches the kill point at its second model call. It records the
/// session id and the kill point, and waits to be killed.
#[tokio::test(flavor = "multi_thread")]
async fn rpc_resumed_run_child_incarnation() {
    let Some(root) = std::env::var_os(ROOT_ENV).map(PathBuf::from) else {
        return;
    };
    let runtime = sqlite_runtime(&root).await;
    let model = ScriptedModel::new(
        [Step::Text("first answer"), Step::Tool, Step::KillPoint],
        Some(root.join("killed")),
    );
    let build_config = AgentBuildConfig {
        llm_client_override: Some(model),
        ..AgentBuildConfig::new(MODEL)
    };
    let session_id = runtime
        .create_session(build_config, None, None, Vec::new())
        .await
        .expect("create the session");
    std::fs::write(root.join("session-id"), session_id.to_string()).expect("record the session");
    run_turn(&runtime, &session_id, "first")
        .await
        .expect("the first turn completes");
    let _held = run_turn(&runtime, &session_id, "second").await;
    std::future::pending::<()>().await;
}

/// The child incarnation of the override row: as
/// [`rpc_resumed_run_child_incarnation`], but its second turn carries a
/// per-turn model override. The turn swaps the session's LLM client to the
/// overriding model, and saves the session with it, before its first round.
#[tokio::test(flavor = "multi_thread")]
async fn rpc_resumed_run_with_a_model_override_child_incarnation() {
    let Some(root) = std::env::var_os(ROOT_ENV).map(PathBuf::from) else {
        return;
    };
    let runtime = sqlite_runtime(&root).await;
    let model = ScriptedModel::new(
        [Step::Text("first answer"), Step::Tool, Step::KillPoint],
        Some(root.join("killed")),
    );
    // The swap builds the session's new client from the runtime's default
    // one: the same scripted model.
    runtime.set_default_llm_client(Some(model.clone()));
    let build_config = AgentBuildConfig {
        llm_client_override: Some(model),
        ..AgentBuildConfig::new(MODEL)
    };
    let session_id = runtime
        .create_session(build_config, None, None, Vec::new())
        .await
        .expect("create the session");
    std::fs::write(root.join("session-id"), session_id.to_string()).expect("record the session");
    run_turn(&runtime, &session_id, "first")
        .await
        .expect("the first turn completes");
    let _held = run_turn_with_overrides(
        &runtime,
        &session_id,
        "second",
        Some(crate::handlers::turn::TurnOverrides {
            model: Some(OVERRIDE_MODEL.to_string()),
            ..Default::default()
        }),
    )
    .await;
    std::future::pending::<()>().await;
}

/// Kill the child incarnation `child_test` at its kill point and return the
/// session it recorded.
#[allow(clippy::disallowed_methods)]
async fn kill_child_at_its_kill_point(root: &std::path::Path, child_test: &str) -> SessionId {
    let log = std::fs::File::create(root.join("child.log")).expect("child log");
    let mut child = std::process::Command::new(std::env::current_exe().expect("current_exe"))
        .args(["--exact", child_test, "--nocapture", "--test-threads=1"])
        .env(ROOT_ENV, root)
        .stdout(log.try_clone().expect("child log"))
        .stderr(log)
        .spawn()
        .expect("spawn the child incarnation");
    let started = std::time::Instant::now();
    let reached = loop {
        if root.join("killed").exists() {
            break Ok(());
        }
        match child.try_wait() {
            Ok(Some(status)) => break Err(format!("the child exited ({status}) first")),
            Ok(None) => {}
            Err(error) => break Err(format!("poll the child: {error}")),
        }
        if started.elapsed() > DEADLINE * 2 {
            break Err("the child never reached its kill point".to_string());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    // SIGKILL: the incarnation ends without any shutdown or cancellation.
    let _ = child.kill();
    let _ = child.wait();
    if let Err(error) = reached {
        panic!(
            "{error}: {}",
            std::fs::read_to_string(root.join("child.log")).unwrap_or_default()
        );
    }
    SessionId::parse(
        std::fs::read_to_string(root.join("session-id"))
            .expect("the child recorded its session")
            .trim(),
    )
    .expect("session id")
}

/// JSON-RPC: a new runtime over the realm resumes the killed run, which
/// completes and commits its boundary over the committed history.
#[tokio::test(flavor = "multi_thread")]
async fn rpc_run_resumed_after_a_kill_at_a_committed_round_completes() {
    let root = tempfile::tempdir().expect("tempdir");
    let session_id = kill_child_at_its_kill_point(root.path(), CHILD_TEST).await;

    let runtime = sqlite_runtime(root.path()).await;
    runtime.set_default_llm_client(Some(ScriptedModel::new(
        [Step::Text("resumed answer")],
        None,
    )));
    let open_run = tail_run(&runtime, &session_id)
        .await
        .expect("the killed run's round commit stands as the provisional tail");

    let attached = runtime.ensure_runtime_executor(&session_id).await;
    assert!(
        attached.is_ok(),
        "the restarted runtime attaches the session whose run it resumes: {attached:?}"
    );
    let deadline = tokio::time::Instant::now() + DEADLINE;
    loop {
        if committed_text(&runtime, &session_id, "resumed answer").await
            && tail_run(&runtime, &session_id).await.is_none()
        {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the resumed run {open_run} completes and commits its boundary"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        committed_probe_call(&runtime, &session_id).await,
        "the resumed run keeps the round the killed run committed"
    );
    assert!(
        committed_text(&runtime, &session_id, "first answer").await,
        "the resumed run continues the committed history"
    );
}

/// JSON-RPC: a run whose input carries a per-turn model override, killed at
/// a committed round, resumes at the next start and completes once, on the
/// overriding model, keeping the round the killed run committed.
///
/// The override's swap ran, and saved the session with its model, before
/// the run's first round commit, so the resumed run continues under it. The
/// session runtime must not swap the LLM client again ahead of the resumed
/// dispatch: the resumed actor continues the run's provisional tail, which
/// the swap's live reads refuse until the run commits, so the dispatch would
/// be refused before its turn on every attempt.
#[tokio::test(flavor = "multi_thread")]
async fn rpc_run_with_a_model_override_resumed_after_a_kill_completes_on_that_model() {
    let root = tempfile::tempdir().expect("tempdir");
    let session_id = kill_child_at_its_kill_point(root.path(), OVERRIDE_CHILD_TEST).await;

    let runtime = sqlite_runtime(root.path()).await;
    let model = ScriptedModel::new([Step::Text("resumed answer")], None);
    runtime.set_default_llm_client(Some(model.clone()));
    let open_run = tail_run(&runtime, &session_id)
        .await
        .expect("the killed run's round commit stands as the provisional tail");

    let attached = runtime.ensure_runtime_executor(&session_id).await;
    assert!(
        attached.is_ok(),
        "the restarted runtime attaches the session whose run it resumes: {attached:?}"
    );
    let deadline = tokio::time::Instant::now() + DEADLINE;
    let completed = loop {
        if committed_text(&runtime, &session_id, "resumed answer").await
            && tail_run(&runtime, &session_id).await.is_none()
        {
            break true;
        }
        if tokio::time::Instant::now() >= deadline {
            break false;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    let observed = (
        completed,
        committed_probe_call(&runtime, &session_id).await,
        model.models().first().cloned(),
    );
    assert_eq!(
        observed,
        (true, true, Some(OVERRIDE_MODEL.to_string())),
        "the resumed run {open_run} completes and commits its boundary, keeps the round the \
         killed run committed, and asks the overriding model"
    );
}
