//! A durable run resumed after its process was killed at a committed round,
//! on the REST surface over a HeadCanonical SQLite realm.
//!
//! From a durable run's first tool round until its boundary commit, each
//! round commit applies the run's provisional tail to the physical head while
//! the RuntimeStore boundary stays at the committed snapshot. A child
//! incarnation (this test binary) runs a session whose second turn takes one
//! tool round and reaches a kill point at its second model call; the parent
//! kills it with SIGKILL, opens a new REST state over the same realm and
//! sends the session its next input. The killed run resumes, completes and
//! commits its boundary over the committed history.

use super::*;
use meerkat_client::{LlmDoneOutcome, LlmEvent, LlmRequest};
use meerkat_runtime::RuntimeStore as _;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;

/// Bound on one step of the row: it only turns a hang into a failure.
const DEADLINE: std::time::Duration = std::time::Duration::from_secs(60);
/// The realm both incarnations open.
const REALM: &str = "rest-resumed-run-after-kill";
/// The scripted tool call: a name no catalog has, so the call is answered
/// with its typed refusal and the round still commits.
const PROBE_TOOL: &str = "resumed_run_probe";
/// Names the child incarnation's root; without it the child test is a no-op.
const ROOT_ENV: &str = "MEERKAT_REST_RESUMED_RUN_ROOT";
/// The child incarnation's libtest path.
const CHILD_TEST: &str = "resumed_run_tests::rest_resumed_run_child_incarnation";

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
    steps: std::sync::Mutex<VecDeque<Step>>,
    requests: AtomicUsize,
    kill_marker: Option<PathBuf>,
}

impl ScriptedModel {
    fn new(steps: impl IntoIterator<Item = Step>, kill_marker: Option<PathBuf>) -> Arc<Self> {
        Arc::new(Self {
            steps: std::sync::Mutex::new(steps.into_iter().collect()),
            requests: AtomicUsize::new(0),
            kill_marker,
        })
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
            Provider::Anthropic,
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

    fn provider(&self) -> Provider {
        Provider::Anthropic
    }

    async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
        Ok(())
    }
}

/// A REST state over the HeadCanonical SQLite realm in `root`, built on the
/// test thread (the crate's test admission holds one state per thread).
async fn sqlite_state(root: &std::path::Path, model: Arc<ScriptedModel>) -> AppState {
    let mut bootstrap = RuntimeBootstrap::default();
    bootstrap.realm.selection = RealmSelection::Explicit {
        realm_id: REALM.to_string(),
    };
    bootstrap.realm.backend_hint = Some("sqlite".to_string());
    bootstrap.realm.state_root = Some(root.join("realms"));
    bootstrap.context.context_root = Some(root.to_path_buf());
    bootstrap.context.user_config_root = Some(root.join("user"));
    let mut state = AppState::load_from_with_bootstrap(
        root.to_path_buf(),
        bootstrap,
        false,
        test_credential_store(root),
    )
    .await
    .expect("open the REST state over the SQLite realm");
    state.llm_client_override = Some(model);
    state
}

fn create_request(prompt: &str) -> CreateSessionRequest {
    CreateSessionRequest {
        prompt: prompt.to_string().into(),
        injected_context: None,
        transient_turn_context: None,
        system_prompt: meerkat::SystemPromptOverride::Inherit,
        model: Some("claude-sonnet-4-5".to_string()),
        provider: None,
        max_tokens: None,
        auth_binding: None,
        output_schema: None,
        structured_output_retries: None,
        verbose: false,
        keep_alive: None,
        comms_name: None,
        peer_meta: None,
        hooks_override: None,
        enable_builtins: Some(false),
        enable_shell: Some(false),
        enable_memory: Some(false),
        enable_mob: Some(false),
        enable_schedule: Some(false),
        enable_web_search: Some(false),
        enable_workgraph: Some(false),
        budget_limits: None,
        provider_params: None,
        preload_skills: None,
        skill_refs: None,
        labels: None,
        additional_instructions: None,
        app_context: None,
        shell_env: None,
    }
}

fn continue_request(session_id: &str, prompt: &str) -> ContinueSessionRequest {
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

fn terminal<T>(outcome: RequestTerminal<T>) -> T {
    match outcome {
        RequestTerminal::Publish(payload) | RequestTerminal::RespondWithoutPublish(payload) => {
            payload
        }
    }
}

/// The run of the store's provisional tail, if one stands.
async fn tail_run(state: &AppState, id: &SessionId) -> Option<meerkat_core::RunId> {
    state
        .session_service
        .runtime_store()
        .load_head_canonical_provisional_tail(&meerkat_runtime::LogicalRuntimeId::for_session(id))
        .await
        .expect("read the provisional tail")
        .map(|tail| tail.run_id().clone())
}

/// Whether the committed transcript of `id` holds an assistant `text`.
async fn committed_text(state: &AppState, id: &SessionId, text: &str) -> bool {
    let Ok(Some(session)) = state.session_service.load_authoritative_session(id).await else {
        return false;
    };
    session.messages().iter().any(|message| match message {
        meerkat_core::Message::BlockAssistant(assistant) => {
            assistant.text_blocks().collect::<String>().contains(text)
        }
        _ => false,
    })
}

/// Whether the committed transcript of `id` holds the killed run's tool
/// call: a resumed run keeps the round it committed before the kill, while
/// a discarded tail would lose it.
async fn committed_probe_call(state: &AppState, id: &SessionId) -> bool {
    let Ok(Some(session)) = state.session_service.load_authoritative_session(id).await else {
        return false;
    };
    session.messages().iter().any(|message| match message {
        meerkat_core::Message::BlockAssistant(assistant) => {
            assistant.tool_calls().any(|call| call.name == PROBE_TOOL)
        }
        _ => false,
    })
}

/// The child incarnation: a session whose second turn takes one tool round
/// and then reaches the kill point at its second model call. It records the
/// session id and the kill point, and waits to be killed.
#[tokio::test(flavor = "multi_thread")]
async fn rest_resumed_run_child_incarnation() {
    let Some(root) = std::env::var_os(ROOT_ENV).map(PathBuf::from) else {
        return;
    };
    let state = sqlite_state(
        &root,
        ScriptedModel::new(
            [Step::Text("first answer"), Step::Tool, Step::KillPoint],
            Some(root.join("killed")),
        ),
    )
    .await;
    let created = terminal(
        Box::pin(create_session_inner(
            &state,
            create_request("first"),
            None,
            None,
        ))
        .await,
    )
    .expect("create the session and run its first turn");
    let session_id = created.0.session_id.to_string();
    std::fs::write(root.join("session-id"), &session_id).expect("record the session");
    let _held = Box::pin(continue_session_inner(
        &state,
        &session_id,
        continue_request(&session_id, "second"),
        None,
    ))
    .await;
    std::future::pending::<()>().await;
}

/// Kill the child incarnation at its kill point and return the session it
/// recorded.
#[allow(clippy::disallowed_methods)]
async fn kill_child_at_its_kill_point(root: &std::path::Path) -> SessionId {
    let log = std::fs::File::create(root.join("child.log")).expect("child log");
    let mut child = std::process::Command::new(std::env::current_exe().expect("current_exe"))
        .args(["--exact", CHILD_TEST, "--nocapture", "--test-threads=1"])
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
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
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

/// REST: the session's next input on a new state over the realm resumes
/// the killed run, which completes and commits its boundary over the
/// committed history.
#[tokio::test(flavor = "multi_thread")]
async fn rest_run_resumed_after_a_kill_at_a_committed_round_completes() {
    let root = tempfile::tempdir().expect("tempdir");
    let session_id = kill_child_at_its_kill_point(root.path()).await;

    let state = sqlite_state(
        root.path(),
        ScriptedModel::new(
            [Step::Text("resumed answer"), Step::Text("third answer")],
            None,
        ),
    )
    .await;
    let open_run = tail_run(&state, &session_id)
        .await
        .expect("the killed run's round commit stands as the provisional tail");

    let continued = terminal(
        Box::pin(continue_session_inner(
            &state,
            &session_id.to_string(),
            continue_request(&session_id.to_string(), "third"),
            None,
        ))
        .await,
    );
    assert!(
        continued.is_ok(),
        "the restarted REST state admits the session's next input: {:?}",
        continued.err()
    );
    let deadline = tokio::time::Instant::now() + DEADLINE;
    loop {
        if committed_text(&state, &session_id, "resumed answer").await
            && tail_run(&state, &session_id).await.is_none()
        {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the resumed run {open_run} completes and commits its boundary"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(
        committed_probe_call(&state, &session_id).await,
        "the resumed run keeps the round the killed run committed"
    );
    assert!(
        committed_text(&state, &session_id, "first answer").await,
        "the resumed run continues the committed history"
    );
}
