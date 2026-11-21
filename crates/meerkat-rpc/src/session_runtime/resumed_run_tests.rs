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
//!
//! One row runs a second kind of child: three steers join one boundary of
//! the run, the run commits a further round, and the child is killed. The
//! round commit the real agent loop wrote must bind the three steers to the
//! run, and the resumed run must deliver none of them a second time.

use super::*;
use meerkat_client::{LlmDoneOutcome, LlmEvent, LlmRequest};
use meerkat_runtime::RuntimeStore as _;
use std::collections::VecDeque;
use std::sync::atomic::AtomicUsize;

/// Bound on one step of the row: it only turns a hang into a failure.
const DEADLINE: Duration = Duration::from_mins(1);
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
/// The libtest path of the child incarnation whose run joins three steers at
/// one boundary before its last round commit.
const JOINED_STEERS_CHILD_TEST: &str =
    "session_runtime::resumed_run_tests::rpc_resumed_run_with_joined_steers_child_incarnation";
/// The payloads of the three steers that child admits, in admission order.
const JOINED_STEER_TOKENS: [&str; 3] = [
    "JOINED-STEER-ONE-1R",
    "JOINED-STEER-TWO-2S",
    "JOINED-STEER-THREE-3T",
];

/// What the scripted model answers to one request.
#[derive(Clone, Copy, Debug)]
enum Step {
    /// One call of [`PROBE_TOOL`].
    Tool,
    /// Hold the request in flight until the test releases it, then one call
    /// of [`PROBE_TOOL`].
    HeldTool,
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
    /// Set once a [`Step::HeldTool`] request is in flight.
    hold_reached: AtomicBool,
    /// Set by the test to let the held request answer.
    hold_released: AtomicBool,
}

impl ScriptedModel {
    fn new(steps: impl IntoIterator<Item = Step>, kill_marker: Option<PathBuf>) -> Arc<Self> {
        Arc::new(Self {
            steps: StdMutex::new(steps.into_iter().collect()),
            requests: AtomicUsize::new(0),
            models: StdMutex::new(Vec::new()),
            kill_marker,
            hold_reached: AtomicBool::new(false),
            hold_released: AtomicBool::new(false),
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
            Step::HeldTool => {
                let events = vec![
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
                ];
                Box::pin(
                    futures::stream::once(async move {
                        self.hold_reached
                            .store(true, std::sync::atomic::Ordering::SeqCst);
                        while !self.hold_released.load(std::sync::atomic::Ordering::SeqCst) {
                            tokio::time::sleep(Duration::from_millis(5)).await;
                        }
                        events
                    })
                    .flat_map(futures::stream::iter),
                )
            }
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

/// The child incarnation of the joined-steers row. Its second turn's first
/// model request is held in flight while three text steers are admitted, in
/// order. Released, the request returns a tool call: the round commits, and
/// the boundary before the next request joins the three steers. The next
/// request returns a tool call too, so the run commits one more round, the
/// first whose document holds the steers' rows. The request after it is the
/// kill point.
#[tokio::test(flavor = "multi_thread")]
async fn rpc_resumed_run_with_joined_steers_child_incarnation() {
    let Some(root) = std::env::var_os(ROOT_ENV).map(PathBuf::from) else {
        return;
    };
    let runtime = sqlite_runtime(&root).await;
    let model = ScriptedModel::new(
        [
            Step::Text("first answer"),
            Step::HeldTool,
            Step::Tool,
            Step::KillPoint,
        ],
        Some(root.join("killed")),
    );
    let build_config = AgentBuildConfig {
        llm_client_override: Some(model.clone()),
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

    let turn_runtime = Arc::clone(&runtime);
    let turn_session = session_id.clone();
    let _held = tokio::spawn(async move { run_turn(&turn_runtime, &turn_session, "second").await });
    let started = std::time::Instant::now();
    while !model.hold_reached.load(std::sync::atomic::Ordering::SeqCst) {
        assert!(
            started.elapsed() < DEADLINE,
            "the second turn's first request is in flight"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    // Three steers admitted while that request is in flight.
    let adapter = runtime.runtime_adapter();
    let mut steer_ids = Vec::new();
    for token in JOINED_STEER_TOKENS {
        let steer = meerkat_runtime::Input::Prompt(meerkat_runtime::PromptInput::new(
            token,
            Some(
                meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                    handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
                    ..Default::default()
                },
            ),
        ));
        steer_ids.push(steer.id().to_string());
        let (outcome, _completion) = adapter
            .accept_input_with_completion(&session_id, steer)
            .await
            .expect("the steer is admitted while the turn runs");
        assert!(outcome.is_accepted());
    }
    std::fs::write(root.join("steer-ids"), steer_ids.join("\n")).expect("record the steers");
    // Let the oldest steer register for the run's next boundary.
    tokio::time::sleep(Duration::from_millis(300)).await;
    model
        .hold_released
        .store(true, std::sync::atomic::Ordering::SeqCst);
    std::future::pending::<()>().await;
}

/// How many transcript messages of `session` carry `token`.
fn rows_carrying(session: &meerkat_core::Session, token: &str) -> usize {
    session
        .messages()
        .iter()
        .filter(|message| format!("{message:?}").contains(token))
        .count()
}

/// JSON-RPC, the actual round commit of the real agent loop and a real
/// kill: a run joins three steers at one boundary, commits one more round and
/// is killed. The round commit's cursor binds the input that started the run
/// and then the three steers, in admission order, and each steer's stored row
/// is bound to the run. A new runtime over the realm resumes that run; it
/// completes and commits. No joined steer is delivered twice and none is
/// lost: the committed transcript holds each steer's row exactly once, in
/// admission order; the resumed runtime asks the model exactly once (no
/// follow-up turn starts with a steer); and each steer's row ends consumed
/// by the resumed run.
#[tokio::test(flavor = "multi_thread")]
async fn rpc_run_killed_after_a_round_commit_holds_three_joined_steers_resumes_and_delivers_none_twice()
 {
    let root = tempfile::tempdir().expect("tempdir");
    let session_id = kill_child_at_its_kill_point(root.path(), JOINED_STEERS_CHILD_TEST).await;
    let steer_ids = std::fs::read_to_string(root.path().join("steer-ids"))
        .expect("the child recorded its steers")
        .lines()
        .map(str::to_string)
        .collect::<Vec<_>>();
    assert_eq!(steer_ids.len(), 3, "the child admitted three steers");

    let runtime = sqlite_runtime(root.path()).await;
    let model = ScriptedModel::new([Step::Text("resumed answer")], None);
    runtime.set_default_llm_client(Some(model.clone()));
    let open_run = tail_run(&runtime, &session_id)
        .await
        .expect("the killed run's round commit stands as the provisional tail");

    // The round commit the killed run's agent loop wrote, read as recovery
    // reads it: the run its cursor names and the inputs the cursor binds.
    let store = runtime.service.runtime_store();
    let runtime_id = meerkat_runtime::LogicalRuntimeId::for_session(&session_id);
    let observed = meerkat_runtime::recovery::observe_resumable_run(store.as_ref(), &session_id)
        .await
        .expect("read the round commit")
        .expect("the round commit leaves a continuable run");
    assert_eq!(observed.run_id, open_run);
    let bound = observed
        .bound_input_ids
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    assert_eq!(
        bound.len(),
        4,
        "the cursor binds the input that started the run and the three joined steers: {bound:?}"
    );
    assert!(
        !steer_ids.contains(&bound[0]),
        "the input that started the run is bound first: {bound:?}"
    );
    assert_eq!(
        bound[1..],
        steer_ids[..],
        "then the three joined steers, in admission order"
    );
    let steers = observed.bound_input_ids[1..].to_vec();
    for input_id in &steers {
        let row = store
            .load_input_state(&runtime_id, input_id)
            .await
            .expect("load the joined steer's row")
            .expect("the joined steer's row is stored");
        assert_eq!(
            (
                format!("{:?}", row.seed.phase),
                row.seed.last_run_id.clone()
            ),
            ("Staged".to_string(), Some(open_run.clone())),
            "the joined steer {input_id} is bound to the open run"
        );
    }

    // The restart: the new runtime resumes the run from that round commit.
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
    // Each steer's row ends consumed by the resumed run.
    loop {
        let mut consumed = Vec::new();
        for input_id in &steers {
            let row = store
                .load_input_state(&runtime_id, input_id)
                .await
                .expect("load the steer's row")
                .expect("the steer's row is stored");
            consumed.push((
                format!("{:?}", row.seed.phase),
                row.seed.last_run_id.clone(),
            ));
        }
        if consumed
            .iter()
            .all(|row| row == &("Consumed".to_string(), Some(open_run.clone())))
        {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "each joined steer ends consumed by the resumed run: {consumed:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // A steer delivered a second time would start a follow-up turn, which
    // asks the model again.
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(
        model.requests.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the resumed runtime asks the model once: no follow-up turn starts with a steer"
    );
    let session = runtime
        .service
        .load_authoritative_session(&session_id)
        .await
        .expect("read the committed session")
        .expect("the session is committed");
    let mut positions = Vec::new();
    for token in JOINED_STEER_TOKENS {
        assert_eq!(
            rows_carrying(&session, token),
            1,
            "{token} is in the committed transcript exactly once"
        );
        positions.push(
            session
                .messages()
                .iter()
                .position(|message| format!("{message:?}").contains(token))
                .expect("the steer's row"),
        );
    }
    assert!(
        positions.windows(2).all(|pair| pair[0] < pair[1]),
        "the steers' rows are in admission order: {positions:?}"
    );
    assert!(
        committed_probe_call(&runtime, &session_id).await,
        "the resumed run keeps the rounds the killed run committed"
    );
}
