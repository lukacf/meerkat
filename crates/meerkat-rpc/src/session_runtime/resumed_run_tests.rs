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

/// The libtest path of the child incarnation that is killed while two
/// steers still wait for its run's next boundary.
const WAITING_STEERS_CHILD_TEST: &str =
    "session_runtime::resumed_run_tests::rpc_resumed_run_with_waiting_steers_child_incarnation";
/// The payloads of the two steers that child admits, in admission order.
const WAITING_STEER_TOKENS: [&str; 2] = ["WAITING-STEER-FIRST-ROW", "WAITING-STEER-SECOND-ROW"];

/// The payload of the steer the late-admission row admits in the new
/// process, after the kill.
const LATE_STEER_TOKEN: &str = "LATE-STEER-AFTER-RESTART-ROW";

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
    /// The messages each request carried, in order.
    request_messages: StdMutex<Vec<Vec<meerkat_core::Message>>>,
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
            request_messages: StdMutex::new(Vec::new()),
            kill_marker,
            hold_reached: AtomicBool::new(false),
            hold_released: AtomicBool::new(false),
        })
    }

    fn request_messages(&self) -> Vec<Vec<meerkat_core::Message>> {
        self.request_messages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
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
        self.request_messages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(request.messages.clone());
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
/// order, at the runtime adapter (`accept_input_with_completion`, the method
/// the RPC handlers call), not through a JSON-RPC request. Released, the request returns a tool call: the round commits, and
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

/// The positions of the messages of `messages` that carry `token`.
fn rows_carrying(messages: &[meerkat_core::Message], token: &str) -> Vec<usize> {
    messages
        .iter()
        .enumerate()
        .filter(|(_, message)| format!("{message:?}").contains(token))
        .map(|(index, _)| index)
        .collect()
}

/// The JSON-RPC session runtime, the actual round commit of the real agent
/// loop and a real kill. The three steers are admitted at the runtime adapter
/// (`accept_input_with_completion`, the method the RPC handlers call), not
/// through a JSON-RPC request. Actual process kill and restart; scripted
/// model. The store is the HeadCanonical SQLite realm.
///
/// A run joins three steers at one boundary, commits one more round and is
/// killed. The round commit's cursor binds the input that started the run
/// and then the three steers, in admission order, and each steer's stored row
/// is bound to the run. A new runtime over the realm resumes that run; it
/// completes and commits. No joined steer is delivered twice and none is
/// lost: the one request the resumed runtime makes carries each steer's row
/// exactly once, in admission order (no follow-up turn starts with a steer);
/// the committed transcript holds each steer's row exactly once, in
/// admission order; and each steer's row ends consumed by the resumed run.
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
    // What the resumed run's one request carried: each steer's row once, in
    // admission order.
    let requests = model.request_messages();
    assert_eq!(requests.len(), 1);
    let mut in_request = Vec::new();
    for token in JOINED_STEER_TOKENS {
        let rows = rows_carrying(&requests[0], token);
        assert_eq!(
            rows.len(),
            1,
            "the resumed run's request carries {token} exactly once: {rows:?}"
        );
        in_request.push(rows[0]);
    }
    assert!(
        in_request.windows(2).all(|pair| pair[0] < pair[1]),
        "the request carries the steers' rows in admission order: {in_request:?}"
    );
    let session = runtime
        .service
        .load_authoritative_session(&session_id)
        .await
        .expect("read the committed session")
        .expect("the session is committed");
    let mut positions = Vec::new();
    for token in JOINED_STEER_TOKENS {
        let rows = rows_carrying(session.messages(), token);
        assert_eq!(
            rows.len(),
            1,
            "{token} is in the committed transcript exactly once: {rows:?}"
        );
        positions.push(rows[0]);
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

/// The child incarnation of the waiting-steers row. Its second turn takes
/// one tool round, which commits, so the run is resumable. Its next model
/// request is held in flight and never released. While it is held, two text
/// steers are admitted, in order, at the runtime adapter
/// (`accept_input_with_completion`, the method the RPC handlers call), not
/// through a JSON-RPC request, and both admissions are acknowledged. Neither
/// steer has joined a request when the child records its kill point.
#[tokio::test(flavor = "multi_thread")]
async fn rpc_resumed_run_with_waiting_steers_child_incarnation() {
    let Some(root) = std::env::var_os(ROOT_ENV).map(PathBuf::from) else {
        return;
    };
    let runtime = sqlite_runtime(&root).await;
    let model = ScriptedModel::new(
        [Step::Text("first answer"), Step::Tool, Step::HeldTool],
        None,
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
            "the second turn's request after its first round is in flight"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    // Two steers admitted and acknowledged while that request is in flight.
    // The request is never released: neither steer joins a request here.
    let adapter = runtime.runtime_adapter();
    let mut steer_ids = Vec::new();
    for token in WAITING_STEER_TOKENS {
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
    // Let the oldest steer register for the run's next boundary, then tell
    // the parent to kill this process.
    tokio::time::sleep(Duration::from_millis(300)).await;
    std::fs::write(root.join("killed"), "steers_waiting").expect("record the kill point");
    std::future::pending::<()>().await;
}

/// The JSON-RPC session runtime and a real kill, with steers that are still
/// WAITING at the kill. Actual process kill and restart; scripted model. The
/// store is the HeadCanonical SQLite realm. The two steers are admitted at
/// the runtime adapter (`accept_input_with_completion`), not through a
/// JSON-RPC request.
///
/// A run commits one round and holds its next model request in flight. Two
/// text steers are admitted and acknowledged while it is held; the request is
/// not released, so neither has joined. The process is killed with SIGKILL. A
/// new runtime over the same realm resumes the run.
///
/// Before the restart the store holds the run's round commit, whose cursor
/// binds only the input that started the run, and the two steers' rows,
/// queued. After it: the resumed run's next request carries both steers'
/// rows once each, in admission order; the resumed runtime asks the model
/// exactly once, so no follow-up turn starts with a steer; each steer's row
/// ends consumed by the resumed run; and the committed transcript holds each
/// steer's row exactly once, in admission order.
#[tokio::test(flavor = "multi_thread")]
async fn rpc_run_killed_while_two_steers_wait_resumes_and_its_next_request_carries_both() {
    let root = tempfile::tempdir().expect("tempdir");
    let session_id = kill_child_at_its_kill_point(root.path(), WAITING_STEERS_CHILD_TEST).await;
    let steer_ids = std::fs::read_to_string(root.path().join("steer-ids"))
        .expect("the child recorded its steers")
        .lines()
        .map(str::to_string)
        .collect::<Vec<_>>();
    assert_eq!(steer_ids.len(), 2, "the child admitted two steers");

    let runtime = sqlite_runtime(root.path()).await;
    let model = ScriptedModel::new([Step::Text("resumed answer")], None);
    runtime.set_default_llm_client(Some(model.clone()));
    let open_run = tail_run(&runtime, &session_id)
        .await
        .expect("the killed run's round commit stands as the provisional tail");

    // What the kill left: the round commit binds only the starting input,
    // and the two steers are stored, queued, never bound to a run.
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
        1,
        "only the starting input is bound: {bound:?}"
    );
    assert!(!steer_ids.contains(&bound[0]));
    let stored = store
        .load_input_states_strict(&runtime_id)
        .await
        .expect("the input rows decode");
    let mut steers = Vec::new();
    for steer_id in &steer_ids {
        let row = stored
            .iter()
            .find(|row| &row.state.input_id.to_string() == steer_id)
            .unwrap_or_else(|| panic!("the acknowledged steer {steer_id} is stored"));
        assert_eq!(
            format!("{:?}", row.seed.phase),
            "Queued",
            "the steer {steer_id} was still waiting at the kill"
        );
        steers.push(row.state.input_id.clone());
    }

    // The restart: the new runtime resumes the run.
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

    // The resumed run's next request: both steers' rows, once each, in
    // admission order.
    let requests = model.request_messages();
    let resumed_request = requests.first().expect("the resumed run asks the model");
    let mut in_request = Vec::new();
    for token in WAITING_STEER_TOKENS {
        let rows = rows_carrying(resumed_request, token);
        assert_eq!(
            rows.len(),
            1,
            "a steer acknowledged and still waiting when the process was killed is in the \
             resumed run's next request exactly once: {token} at {rows:?}"
        );
        in_request.push(rows[0]);
    }
    assert!(
        in_request.windows(2).all(|pair| pair[0] < pair[1]),
        "the resumed run's next request carries the steers in admission order: {in_request:?}"
    );

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
            "each waiting steer ends consumed by the resumed run: {consumed:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // A steer left for a follow-up turn would ask the model again.
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
    for token in WAITING_STEER_TOKENS {
        let rows = rows_carrying(session.messages(), token);
        assert_eq!(
            rows.len(),
            1,
            "{token} is in the committed transcript exactly once: {rows:?}"
        );
        positions.push(rows[0]);
    }
    assert!(
        positions.windows(2).all(|pair| pair[0] < pair[1]),
        "the steers' rows are in admission order: {positions:?}"
    );
}

/// A host's resumed-run admission that holds the resume until the row
/// releases it, then lets the run resume. The runtime asks it after the
/// session's executor is attached and before it begins the resumed run.
#[derive(Default)]
struct HeldResumeAdmission {
    asked: AtomicBool,
    released: AtomicBool,
}

#[async_trait::async_trait]
impl meerkat_runtime::ResumedRunAdmission for HeldResumeAdmission {
    async fn admit_resumed_run(
        &self,
        _request: &meerkat_runtime::ResumedRunAdmissionRequest,
    ) -> meerkat_runtime::ResumedRunAdmissionDecision {
        self.asked.store(true, std::sync::atomic::Ordering::SeqCst);
        while !self.released.load(std::sync::atomic::Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        meerkat_runtime::ResumedRunAdmissionDecision::Continue
    }
}

/// The JSON-RPC session runtime and a real kill, with two steers that are
/// still waiting at the kill and a THIRD steer admitted in the new process
/// before the resumed run begins. Actual process kill and restart; scripted
/// model. The store is the HeadCanonical SQLite realm. Every steer is
/// admitted at the runtime adapter (`accept_input_with_completion`), not
/// through a JSON-RPC request.
///
/// The kill is the waiting-steers row's: a run commits one round and holds
/// its next model request; two text steers are admitted and acknowledged;
/// SIGKILL. In the new process the row registers the session, binds a host
/// resumed-run admission that holds the resume, and attaches the executor.
/// The runtime asks that admission before it begins the resumed run. While
/// it is held (no model request has been made), the third steer is admitted.
/// Then the admission answers that the run may resume.
///
/// Asserted: each of the three steers is delivered exactly once (one row in
/// the committed transcript, in no request twice, its stored row consumed),
/// and the two that waited come ahead of the late one, in admission order,
/// in the transcript and in the first request that carries each.
///
/// Asserted too, the path that serves the late steer: the new process makes
/// exactly one model request, the resumed run's first; that request carries
/// the first waiting steer, the second waiting steer and the late steer, in
/// that order; all three rows are consumed by the resumed run; and no second
/// run starts with any of them. The row also prints what it read, as the
/// line `LATE_ADMISSION_PATH`.
///
/// Bound: the window this row places the late steer in is "executor
/// attached, resumed run not begun". A steer admitted once the waiting
/// steers' work is already running is another scene.
#[tokio::test(flavor = "multi_thread")]
async fn rpc_run_killed_while_two_steers_wait_then_a_third_admitted_before_the_resume_is_delivered_after_them()
 {
    let root = tempfile::tempdir().expect("tempdir");
    let session_id = kill_child_at_its_kill_point(root.path(), WAITING_STEERS_CHILD_TEST).await;
    let waiting_ids = std::fs::read_to_string(root.path().join("steer-ids"))
        .expect("the child recorded its steers")
        .lines()
        .map(str::to_string)
        .collect::<Vec<_>>();
    assert_eq!(waiting_ids.len(), 2, "the child admitted two steers");

    let runtime = sqlite_runtime(root.path()).await;
    let model = ScriptedModel::new([Step::Text("resumed answer")], None);
    runtime.set_default_llm_client(Some(model.clone()));
    let open_run = tail_run(&runtime, &session_id)
        .await
        .expect("the killed run's round commit stands as the provisional tail");
    let store = runtime.service.runtime_store();
    let runtime_id = meerkat_runtime::LogicalRuntimeId::for_session(&session_id);

    // Hold the resume at the host's resumed-run admission: register the
    // session, bind the admission, then attach the executor.
    let adapter = runtime.runtime_adapter();
    adapter
        .register_session(session_id.clone())
        .await
        .expect("the session's runtime registers");
    let registration = adapter
        .current_session_registration_witness(&session_id)
        .await
        .expect("the registration");
    let admission = Arc::new(HeldResumeAdmission::default());
    adapter
        .bind_resumed_run_admission(&registration, Arc::clone(&admission) as _)
        .await
        .expect("bind the admission");
    let attached = runtime.ensure_runtime_executor(&session_id).await;
    assert!(
        attached.is_ok(),
        "the restarted runtime attaches the session whose run it resumes: {attached:?}"
    );
    let asked_by = tokio::time::Instant::now() + Duration::from_secs(30);
    while !admission.asked.load(std::sync::atomic::Ordering::SeqCst) {
        assert!(
            tokio::time::Instant::now() < asked_by,
            "the runtime never asked the resumed-run admission: this rig cannot hold the \
             resume before the run begins"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(
        model.requests.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "the resume is held: the resumed run has made no model request"
    );

    // The third steer: admitted in the new process, before the resumed run
    // begins.
    let late = meerkat_runtime::Input::Prompt(meerkat_runtime::PromptInput::new(
        LATE_STEER_TOKEN,
        Some(
            meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
                ..Default::default()
            },
        ),
    ));
    let late_id = late.id().to_string();
    let (outcome, _completion) = adapter
        .accept_input_with_completion(&session_id, late)
        .await
        .expect("the late steer is admitted while the resume is held");
    assert!(outcome.is_accepted());
    assert_eq!(
        model.requests.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "the late steer was admitted before the resumed run made a request"
    );
    admission
        .released
        .store(true, std::sync::atomic::Ordering::SeqCst);

    // Wait until the resumed run committed and every steer's row is consumed.
    let all_ids = waiting_ids
        .iter()
        .cloned()
        .chain(std::iter::once(late_id.clone()))
        .collect::<Vec<_>>();
    let deadline = tokio::time::Instant::now() + DEADLINE;
    let rows = loop {
        let stored = store
            .load_input_states_strict(&runtime_id)
            .await
            .expect("the input rows decode");
        let rows = all_ids
            .iter()
            .map(|id| {
                let row = stored
                    .iter()
                    .find(|row| &row.state.input_id.to_string() == id)
                    .unwrap_or_else(|| panic!("the acknowledged steer {id} is stored"));
                (
                    format!("{:?}", row.seed.phase),
                    row.seed.last_run_id.clone(),
                )
            })
            .collect::<Vec<_>>();
        if rows.iter().all(|(phase, _)| phase == "Consumed")
            && committed_text(&runtime, &session_id, "resumed answer").await
            && tail_run(&runtime, &session_id).await.is_none()
        {
            break rows;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the resumed run {open_run} commits and each steer's row ends consumed: {rows:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    // Let a follow-up turn, if one runs, make its request.
    tokio::time::sleep(Duration::from_secs(2)).await;

    let tokens = [
        WAITING_STEER_TOKENS[0],
        WAITING_STEER_TOKENS[1],
        LATE_STEER_TOKEN,
    ];
    let requests = model.request_messages();
    // The first request that carries each steer, and its place in it.
    let mut first_carried = Vec::new();
    for token in tokens {
        let mut first = None;
        for (index, request) in requests.iter().enumerate() {
            let at = rows_carrying(request, token);
            assert!(
                at.len() <= 1,
                "no request carries {token} twice: request {index} at {at:?}"
            );
            if first.is_none()
                && let Some(position) = at.first()
            {
                first = Some((index, *position));
            }
        }
        first_carried.push(first.unwrap_or_else(|| panic!("{token} reaches a model request")));
    }
    let session = runtime
        .service
        .load_authoritative_session(&session_id)
        .await
        .expect("read the committed session")
        .expect("the session is committed");
    let mut transcript = Vec::new();
    for token in tokens {
        let at = rows_carrying(session.messages(), token);
        assert_eq!(
            at.len(),
            1,
            "{token} is in the committed transcript exactly once: {at:?}"
        );
        transcript.push(at[0]);
    }

    // The path that served the late steer, printed before it is asserted so
    // that a failing run still says what it read.
    let (late_request, _) = first_carried[2];
    let late_run = rows[2].1.clone();
    let path = if late_run.as_ref() == Some(&open_run) && late_request == 0 {
        "the resumed run's first request"
    } else if late_run.as_ref() == Some(&open_run) {
        "a later request of the resumed run"
    } else {
        "a follow-up run"
    };
    eprintln!(
        "LATE_ADMISSION_PATH path=\"{path}\" requests={} first_carried(request,position)={first_carried:?} transcript_rows={transcript:?} resumed_run={open_run} runs={:?}",
        requests.len(),
        rows.iter().map(|(_, run)| run.clone()).collect::<Vec<_>>()
    );

    // The path: one model request in the new process, the resumed run's
    // first, carrying the two that waited and then the late one; every row
    // consumed by the resumed run; no second run starts with any of them.
    assert_eq!(
        requests.len(),
        1,
        "the new process makes exactly one model request, the resumed run's first: a steer \
         left for a follow-up run would ask the model again"
    );
    assert!(
        first_carried.iter().all(|(request, _)| *request == 0),
        "the resumed run's first request carries all three steers: {first_carried:?}"
    );
    assert!(
        first_carried.windows(2).all(|pair| pair[0].1 < pair[1].1),
        "that request carries waiting one, waiting two, then the late steer: {first_carried:?}"
    );
    for (token, (phase, run)) in tokens.iter().zip(&rows) {
        assert_eq!(
            (phase.as_str(), run.as_ref()),
            ("Consumed", Some(&open_run)),
            "{token} is consumed by the resumed run, not by a second run"
        );
    }

    // The two that waited come ahead of the late one, in admission order.
    assert!(
        transcript.windows(2).all(|pair| pair[0] < pair[1]),
        "the transcript holds the two waiting steers, then the late one: {transcript:?}"
    );
    assert!(
        first_carried.windows(2).all(|pair| pair[0] < pair[1]),
        "each steer first reaches the model no earlier than the one admitted before it, and \
         after it within one request: {first_carried:?}"
    );
}
