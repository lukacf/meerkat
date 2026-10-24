//! Committed reads while a durable run holds its provisional tail, from the
//! rkat CLI.
//!
//! A session another service (another process: an `rkat-rpc` server or a
//! long `rkat run`) is running holds a provisional tail from its run's first
//! round commit until its boundary commit. On a HeadCanonical store (the
//! SQLite realm) a body read of the committed boundary then refuses with
//! `TranscriptRevisionConflict`, so `rkat session show` and the CLI's
//! schedule host failed on a running session.
//!
//! Each row runs a holder service over the realm: its session's second turn
//! makes one tool round (a scripted model answers one tool call) and then
//! holds its second model call until the test releases it. The CLI reads the
//! same realm through its own service meanwhile.

use super::*;
use futures::StreamExt as _;
use std::collections::VecDeque;
use std::sync::atomic::AtomicUsize;
use tokio::sync::Semaphore;

/// Bound on one step of a row: it only turns a hang into a failure.
const DEADLINE: std::time::Duration = std::time::Duration::from_secs(60);
/// The realm every row opens.
const REALM: &str = "cli-committed-reads";
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
    steps: Mutex<VecDeque<Step>>,
    requests: AtomicUsize,
    entered: Semaphore,
    release: Semaphore,
}

impl ScriptedModel {
    fn new(steps: impl IntoIterator<Item = Step>) -> Arc<Self> {
        Arc::new(Self {
            steps: Mutex::new(steps.into_iter().collect()),
            requests: AtomicUsize::new(0),
            entered: Semaphore::new(0),
            release: Semaphore::new(0),
        })
    }

    async fn wait_entered(&self) {
        tokio::time::timeout(DEADLINE, self.entered.acquire())
            .await
            .expect("the run reaches its held model call")
            .expect("model gate")
            .forget();
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
        let n = self
            .requests
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let step = self
            .steps
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop_front()
            .unwrap_or(Step::Text("unscripted"));
        let usage = || LlmEvent::UsageUpdate {
            usage: meerkat_core::TurnUsage::host_declared(
                meerkat_core::Provider::Anthropic,
                &request.model,
                Usage::default(),
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
                    args: serde_json::json!({}),
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
        meerkat_core::Provider::Anthropic
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

/// A holder service over the realm in `state_root`, as another process
/// opens it: its session's second turn holds after one round commit.
struct HeldElsewhere {
    service: Arc<meerkat::PersistentSessionService<FactoryAgentBuilder>>,
    machine: Arc<meerkat_runtime::MeerkatMachine>,
    model: Arc<ScriptedModel>,
    session_id: SessionId,
    turn: tokio::task::JoinHandle<
        Option<Result<meerkat_runtime::completion::CompletionOutcome, String>>,
    >,
}

impl HeldElsewhere {
    async fn start(state_root: &std::path::Path) -> Self {
        let (_manifest, bundle) =
            meerkat::open_realm_persistence_in(state_root, REALM, Some(RealmBackend::Sqlite), None)
                .await
                .expect("open the SQLite realm");
        let (service, machine) = meerkat::surface::build_runtime_backed_service(
            FactoryAgentBuilder::new(
                AgentFactory::new(state_root.join("holder-sessions")),
                Config::default(),
            ),
            10,
            bundle,
        )
        .expect("compose the holder service");
        let service = Arc::new(service);
        let model = ScriptedModel::new([Step::Text("first answer"), Step::Tool, Step::Held]);
        let request = CreateSessionRequest {
            model: MODEL.into(),
            prompt: "unused".into(),
            injected_context: Vec::new(),
            system_prompt: meerkat_core::SystemPromptOverride::Inherit,
            max_tokens: None,
            event_tx: None,
            initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
            build: Some(SessionBuildOptions {
                llm_client_override: Some(meerkat::encode_llm_client_override_for_service(
                    model.clone(),
                )),
                ..SessionBuildOptions::default()
            }),
            labels: None,
        };
        let created = Box::pin(meerkat::surface::materialize_session(
            &service,
            &machine,
            meerkat_core::Session::new(),
            request,
            {
                let service = Arc::clone(&service);
                let machine = Arc::clone(&machine);
                move |id| meerkat::surface::default_persistent_executor(service, machine, id)
            },
        ))
        .await
        .expect("materialize the holder's session");
        let session_id = created.session_id;
        let first = Self::admit(&machine, &session_id, "first")
            .await
            .await
            .expect("first turn task");
        assert!(
            matches!(
                first,
                Some(Ok(meerkat_runtime::completion::CompletionOutcome::Completed(ref run)))
                    if run.text == "first answer"
            ),
            "the first turn completes: {first:?}"
        );
        let turn = Self::admit(&machine, &session_id, "second").await;
        model.wait_entered().await;
        let runtime_id = meerkat_runtime::LogicalRuntimeId::for_session(&session_id);
        assert!(
            service
                .runtime_store()
                .load_head_canonical_provisional_tail(&runtime_id)
                .await
                .expect("read the provisional tail")
                .is_some(),
            "the held run's round commit stands as the provisional tail"
        );
        Self {
            service,
            machine,
            model,
            session_id,
            turn,
        }
    }

    async fn admit(
        machine: &Arc<meerkat_runtime::MeerkatMachine>,
        id: &SessionId,
        prompt: &str,
    ) -> tokio::task::JoinHandle<
        Option<Result<meerkat_runtime::completion::CompletionOutcome, String>>,
    > {
        let (_, completion) = machine
            .accept_input_with_completion(
                id,
                meerkat_runtime::Input::Prompt(meerkat_runtime::PromptInput::new(prompt, None)),
            )
            .await
            .expect("the holder's runtime accepts the prompt");
        let completion = completion.expect("a completion");
        tokio::spawn(async move {
            tokio::time::timeout(DEADLINE, completion.wait())
                .await
                .ok()
                .map(|outcome| outcome.map_err(|error| error.to_string()))
        })
    }

    /// Release the held run and require it to complete.
    async fn complete(self) {
        self.model.release.add_permits(1);
        let outcome = self.turn.await.expect("held turn task");
        assert!(
            matches!(
                outcome,
                Some(Ok(meerkat_runtime::completion::CompletionOutcome::Completed(ref run)))
                    if run.text == "held answer"
            ),
            "the held run completes: {outcome:?}"
        );
        drop(self.service);
        drop(self.machine);
    }
}

/// `rkat session show` of a session another service is running reads the
/// committed snapshot.
#[cfg(feature = "session-store")]
#[tokio::test(flavor = "multi_thread")]
async fn rkat_session_show_while_another_service_runs_the_session() {
    let temp = tempfile::tempdir().expect("tempdir");
    let held = HeldElsewhere::start(temp.path()).await;
    let scope = test_scope(temp.path().to_path_buf(), REALM);
    let shown = tokio::time::timeout(DEADLINE, show_session(&held.session_id.to_string(), &scope))
        .await
        .expect("rkat session show answers while the run holds");
    assert!(
        shown.is_ok(),
        "rkat session show prints the committed snapshot of a running session: {shown:?}"
    );
    held.complete().await;
}

/// The CLI schedule host's probe and owner-identity read of a session
/// another service is running read its committed document.
#[cfg(feature = "session-store")]
#[tokio::test(flavor = "multi_thread")]
async fn cli_schedule_probe_and_identity_while_the_session_runs() {
    let temp = tempfile::tempdir().expect("tempdir");
    let held = HeldElsewhere::start(temp.path()).await;
    let scope = test_scope(temp.path().to_path_buf(), REALM);
    let (config, _) = load_config(&scope).await.expect("load the realm config");
    let (service, runtime_adapter) = build_cli_persistent_service(&scope, config)
        .await
        .expect("the CLI service over the realm");
    let host = CliScheduleSessionHost {
        service,
        runtime_adapter,
        workgraph_service: meerkat::WorkGraphService::with_scope(
            Arc::new(meerkat::MemoryWorkGraphStore::new()),
            REALM.to_owned(),
            meerkat::WorkNamespace::default(),
        ),
    };
    let binding = meerkat::SessionTargetBinding::ResumableSession {
        session_id: held.session_id.clone(),
        action: meerkat::ScheduledSessionAction::Prompt {
            prompt: meerkat_core::ContentInput::Text("scheduled".to_string()),
            system_prompt: None,
            render_metadata: None,
            skill_refs: Vec::new(),
            additional_instructions: Vec::new(),
        },
    };
    let probe = tokio::time::timeout(DEADLINE, host.probe_session_target(&binding))
        .await
        .expect("the probe answers while the run holds");
    assert!(
        matches!(probe, Ok(meerkat::TargetProbeOutcome::Ready)),
        "the CLI probes the running session present: {probe:?}"
    );
    let identity = tokio::time::timeout(DEADLINE, host.recover_session_target_identity(&binding))
        .await
        .expect("the identity read answers while the run holds");
    assert!(
        matches!(identity, Ok(None)),
        "an ordinary session's owner identity reads from its committed metadata: {identity:?}"
    );
    held.complete().await;
}
