//! A deferred session's staged create-time prompt (`DeferredPromptPolicy::Stage`)
//! on its first runtime-authored turn, through the real agent and the
//! runtime: the run-start hooks and `RunStarted` see exactly the rows the turn
//! commits, a run-start veto commits nothing and leaves the prompt pending,
//! and a session persisted with a staged prompt by Meerkat 0.9.4 delivers it
//! once after the upgrade.

use super::*;
use async_trait::async_trait;
use futures::{StreamExt, stream};
use meerkat_client::{LlmDoneOutcome, LlmEvent, LlmRequest};
use meerkat_core::service::{DeferredPromptPolicy, InitialTurnPolicy};
use meerkat_core::types::ContentInput;
use meerkat_core::{
    AgentError, HookDecision, HookEngine, HookEngineError, HookExecutionReport, HookId,
    HookInvocation, HookPoint, HookReasonCode, HookRunOverrides, Provider, SessionId,
};
use meerkat_runtime::completion::CompletionOutcome;
use meerkat_runtime::{InMemoryRuntimeStore, Input, MeerkatMachine, PromptInput};
use meerkat_session::PersistentSessionService;
use meerkat_store::{MemoryBlobStore, MemoryStore};
use std::pin::Pin;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tempfile::TempDir;

const STAGED: &str = "Remember the codeword ORBIT-7 for later.";
const DEADLINE: Duration = Duration::from_secs(20);
/// A session document as Meerkat 0.9.4 persisted it for a deferred session
/// that `live/open` materialized with a staged create-time prompt.
const V0_9_4_STAGED_PROMPT_SESSION: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/v0_9_4_staged_prompt_session/session.json"
);

#[derive(Default)]
struct RecordingModel {
    requests: StdMutex<Vec<Vec<meerkat_core::Message>>>,
}

impl RecordingModel {
    fn requests(&self) -> Vec<String> {
        self.requests
            .lock()
            .expect("recorded requests")
            .iter()
            .map(|messages| format!("{messages:?}"))
            .collect()
    }
}

#[async_trait]
impl LlmClient for RecordingModel {
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
        self.requests
            .lock()
            .expect("recorded requests")
            .push(request.messages.clone());
        let usage = meerkat_core::TurnUsage::host_declared(
            Provider::Anthropic,
            &request.model,
            Default::default(),
        );
        Box::pin(stream::iter(vec![
            Ok(LlmEvent::TextDelta {
                delta: "noted".into(),
                meta: None,
            }),
            Ok(LlmEvent::UsageUpdate { usage }),
            Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::success(meerkat_core::StopReason::EndTurn),
            }),
        ]))
    }

    fn provider(&self) -> Provider {
        Provider::Anthropic
    }

    async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
        Ok(())
    }
}

/// A run-start hook that records the run input it is offered and, when
/// armed, vetoes a run whose input carries the staged text.
#[derive(Default)]
struct RunStartHooks {
    veto_staged: AtomicBool,
    run_started_inputs: StdMutex<Vec<String>>,
}

impl RunStartHooks {
    fn run_started_inputs(&self) -> Vec<String> {
        self.run_started_inputs
            .lock()
            .expect("run-start inputs")
            .clone()
    }
}

#[async_trait]
impl HookEngine for RunStartHooks {
    fn mandatory_topology(
        &self,
        _overrides: Option<&HookRunOverrides>,
    ) -> Result<meerkat_core::hooks::HookMandatoryTopology, HookEngineError> {
        Ok(meerkat_core::hooks::HookMandatoryTopology::none())
    }

    async fn execute(
        &self,
        invocation: HookInvocation,
        _overrides: Option<&HookRunOverrides>,
    ) -> Result<HookExecutionReport, HookEngineError> {
        if invocation.point != HookPoint::RunStarted {
            return Ok(HookExecutionReport::empty());
        }
        let text = invocation
            .prompt_input
            .as_ref()
            .and_then(|input| input.content())
            .map(ContentInput::text_content)
            .unwrap_or_default();
        self.run_started_inputs
            .lock()
            .expect("run-start inputs")
            .push(text.clone());
        let mut report = HookExecutionReport::empty();
        if self.veto_staged.load(Ordering::SeqCst) && text.contains("ORBIT-7") {
            report.decision = Some(HookDecision::Deny {
                hook_id: HookId::new("staged-text-veto"),
                reason_code: HookReasonCode::PolicyViolation,
                message: "the staged text is vetoed".into(),
                payload: None,
            });
        }
        Ok(report)
    }
}

/// The ordinary factory build with only the model client and the hook
/// engine overridden.
struct StagedPromptBuilder {
    factory: AgentFactory,
    client: Arc<RecordingModel>,
    hooks: Arc<RunStartHooks>,
}

#[async_trait]
impl SessionAgentBuilder for StagedPromptBuilder {
    type Agent = FactoryAgent;

    async fn build_agent(
        &self,
        request: &CreateSessionRequest,
        event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<FactoryAgent, SessionError> {
        let mut config = AgentBuildConfig::from_create_session_request(request, event_tx);
        config.llm_client_override = Some(self.client.clone());
        config.hook_engine_override = Some(self.hooks.clone());
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
        Ok(FactoryAgent {
            agent,
            session_context,
            pending_head_canonical_boundary: None,
            acknowledged_head_canonical_boundary: None,
        })
    }
}

struct Fixture {
    _temp: TempDir,
    service: Arc<PersistentSessionService<StagedPromptBuilder>>,
    machine: Arc<MeerkatMachine>,
    client: Arc<RecordingModel>,
    hooks: Arc<RunStartHooks>,
}

impl Fixture {
    fn new() -> Self {
        let temp = TempDir::new().expect("temp");
        let store = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(InMemoryRuntimeStore::new());
        let blob_store = Arc::new(MemoryBlobStore::new());
        let machine = Arc::new(
            MeerkatMachine::persistent(runtime_store.clone(), blob_store.clone())
                .expect("canonical machine"),
        );
        let client = Arc::new(RecordingModel::default());
        let hooks = Arc::new(RunStartHooks::default());
        let builder = StagedPromptBuilder {
            factory: AgentFactory::new(temp.path().join("sessions")).session_store(store.clone()),
            client: client.clone(),
            hooks: hooks.clone(),
        };
        let service = Arc::new(
            PersistentSessionService::new(builder, 4, store, runtime_store, blob_store)
                .with_canonical_runtime_adapter(machine.clone()),
        );
        Self {
            _temp: temp,
            service,
            machine,
            client,
            hooks,
        }
    }

    /// Materialize a deferred session the way the realtime-open promotion
    /// does: `InitialTurnPolicy::Defer` with the given prompt policy.
    async fn materialize(
        &self,
        session: meerkat_core::Session,
        prompt: &str,
        deferred_prompt_policy: DeferredPromptPolicy,
    ) -> SessionId {
        let request = CreateSessionRequest {
            model: "claude-sonnet-4-5".into(),
            prompt: prompt.into(),
            injected_context: Vec::new(),
            system_prompt: meerkat_core::SystemPromptOverride::Inherit,
            max_tokens: None,
            event_tx: None,
            initial_turn: InitialTurnPolicy::Defer,
            deferred_prompt_policy,
            build: None,
            labels: None,
        };
        let created = Box::pin(crate::surface::materialize_session(
            &self.service,
            &self.machine,
            session,
            request,
            {
                let service = self.service.clone();
                let machine = self.machine.clone();
                move |id| crate::surface::default_persistent_executor(service, machine, id)
            },
        ))
        .await
        .expect("materialize the deferred session and its executor");
        created.session_id
    }

    /// One runtime-authored turn: an ordinary prompt input admitted by the
    /// machine.
    async fn turn(&self, id: &SessionId, prompt: &str) -> CompletionOutcome {
        let (_, completion) = self
            .machine
            .accept_input_with_completion(id, Input::Prompt(PromptInput::new(prompt, None)))
            .await
            .expect("the runtime accepts the prompt");
        tokio::time::timeout(DEADLINE, completion.expect("a completion").wait())
            .await
            .expect("the turn settles")
            .expect("the completion resolves")
    }

    /// The committed transcript's user rows (injected context excluded).
    async fn user_rows(&self, id: &SessionId) -> Vec<String> {
        let session = self
            .service
            .load_authoritative_session(id)
            .await
            .expect("load the committed session")
            .expect("the session is committed");
        session
            .messages()
            .iter()
            .filter_map(|message| match message {
                meerkat_core::Message::User(user)
                    if !user.transcript_role.is_injected_context() =>
                {
                    Some(user.text_content())
                }
                _ => None,
            })
            .collect()
    }
}

/// Drain the `RunStarted` inputs the session published so far.
async fn run_started_inputs(events: &mut meerkat_core::EventStream) -> Vec<String> {
    let mut inputs = Vec::new();
    while let Ok(Some(envelope)) =
        tokio::time::timeout(Duration::from_millis(500), events.next()).await
    {
        if let AgentEvent::RunStarted { input, .. } = envelope.payload {
            inputs.push(
                input
                    .content()
                    .map(ContentInput::text_content)
                    .unwrap_or_default(),
            );
        }
    }
    inputs
}

/// The staged prompt is folded into the turn's committed user row, and
/// the run-start hooks and `RunStarted` see exactly that row.
#[tokio::test]
async fn staged_prompt_is_committed_and_run_started_equals_the_committed_row() {
    let fixture = Fixture::new();
    let id = fixture
        .materialize(
            meerkat_core::Session::new(),
            STAGED,
            DeferredPromptPolicy::Stage,
        )
        .await;
    let mut events = fixture
        .service
        .subscribe_session_events(&id)
        .await
        .expect("session event stream");
    let outcome = fixture.turn(&id, "What is the codeword?").await;
    let run_started = run_started_inputs(&mut events).await;
    let rows = fixture.user_rows(&id).await;
    let hook_inputs = fixture.hooks.run_started_inputs();
    let requests = fixture.client.requests();
    let expected = format!("{STAGED}\n\nWhat is the codeword?");
    assert!(
        matches!(outcome, CompletionOutcome::Completed(_))
            && rows == [expected.clone()]
            && run_started == [expected.clone()]
            && hook_inputs == [expected.clone()]
            && requests.len() == 1
            && requests[0].matches("ORBIT-7").count() == 1,
        "the committed row, RunStarted and the run-start hook input must all equal \
         {expected:?}: outcome={outcome:?} rows={rows:?} run_started={run_started:?} \
         hook_inputs={hook_inputs:?} requests={requests:?}"
    );
    fixture.service.try_shutdown().await.expect("shutdown");
}

/// A run-start hook that vetoes the staged text vetoes the run and
/// nothing is committed; the prompt stays pending and the retry delivers it
/// exactly once.
#[tokio::test]
async fn a_run_start_veto_on_the_staged_text_commits_nothing_and_the_retry_delivers_it_once() {
    let fixture = Fixture::new();
    let id = fixture
        .materialize(
            meerkat_core::Session::new(),
            STAGED,
            DeferredPromptPolicy::Stage,
        )
        .await;
    fixture.hooks.veto_staged.store(true, Ordering::SeqCst);
    let vetoed = fixture.turn(&id, "first try").await;
    let rows_after_veto = fixture.user_rows(&id).await;
    let requests_after_veto = fixture.client.requests().len();

    fixture.hooks.veto_staged.store(false, Ordering::SeqCst);
    let retried = fixture.turn(&id, "second try").await;
    let rows = fixture.user_rows(&id).await;
    let staged_rows = rows.iter().filter(|row| row.contains("ORBIT-7")).count();
    let requests = fixture.client.requests();
    let hook_inputs = fixture.hooks.run_started_inputs();
    // The runtime re-applies the vetoed input ahead of the next one, so the
    // retry can take two runs; the staged prompt must still land once.
    assert!(
        !matches!(vetoed, CompletionOutcome::Completed(_))
            && rows_after_veto.is_empty()
            && requests_after_veto == 0
            && matches!(retried, CompletionOutcome::Completed(_))
            && staged_rows == 1
            && requests
                .iter()
                .all(|request| request.matches("ORBIT-7").count() <= 1)
            && requests
                .last()
                .is_some_and(|request| request.matches("ORBIT-7").count() == 1),
        "a vetoed run must commit nothing and keep the staged prompt for the retry, which \
         delivers it once: vetoed={vetoed:?} rows_after_veto={rows_after_veto:?} \
         requests_after_veto={requests_after_veto} retried={retried:?} rows={rows:?} \
         hook_inputs={hook_inputs:?} requests={requests:?}"
    );
    fixture.service.try_shutdown().await.expect("shutdown");
}

/// A session that Meerkat 0.9.4 persisted with a staged prompt (the
/// document `live/open` left behind before any turn) delivers it once on its
/// first runtime-authored turn after the upgrade, and never again.
#[tokio::test]
async fn a_session_persisted_with_a_staged_prompt_by_0_9_4_delivers_it_once_after_upgrade() {
    let document =
        std::fs::read_to_string(V0_9_4_STAGED_PROMPT_SESSION).expect("the 0.9.4 fixture");
    let session: meerkat_core::Session =
        serde_json::from_str(&document).expect("a 0.9.4 session document");
    let staged = session
        .try_deferred_turn_state()
        .expect("decode the persisted deferred-turn state")
        .and_then(|state| {
            state
                .pending_initial_prompt()
                .map(|staged| staged.prompt.text_content())
        });
    let conversational_rows = session
        .messages()
        .iter()
        .filter(|message| !matches!(message, meerkat_core::Message::System(_)))
        .count();
    assert!(
        conversational_rows == 0 && staged.as_deref() == Some(STAGED),
        "the fixture is a 0.9.4 deferred session with a staged prompt and no turn yet: \
         conversational_rows={conversational_rows} staged={staged:?}"
    );

    let fixture = Fixture::new();
    let id = fixture
        .materialize(session, "", DeferredPromptPolicy::Discard)
        .await;
    let first = fixture.turn(&id, "What is the codeword?").await;
    let second = fixture.turn(&id, "Anything else?").await;
    let rows = fixture.user_rows(&id).await;
    let staged_rows = rows.iter().filter(|row| row.contains("ORBIT-7")).count();
    let requests = fixture.client.requests();
    assert!(
        matches!(first, CompletionOutcome::Completed(_))
            && matches!(second, CompletionOutcome::Completed(_))
            && staged_rows == 1
            && requests
                .first()
                .is_some_and(|request| request.contains("ORBIT-7")),
        "the persisted staged prompt must land once after the upgrade: first={first:?} \
         second={second:?} rows={rows:?} requests={requests:?}"
    );
    fixture.service.try_shutdown().await.expect("shutdown");
}

/// A voice-first session: the live open delivers the staged prompt as the
/// session's first user row, through the run-start hooks a turn carrying it
/// would run, exactly once. A later first text turn neither folds it in again
/// nor sends it twice, and a second open finds nothing staged.
#[tokio::test]
async fn a_voice_first_open_delivers_the_staged_prompt_once() {
    let fixture = Fixture::new();
    let id = fixture
        .materialize(
            meerkat_core::Session::new(),
            STAGED,
            DeferredPromptPolicy::Stage,
        )
        .await;

    let delivered = fixture
        .service
        .deliver_staged_prompt_for_live_open(&id)
        .await
        .expect("the open delivers the staged prompt");
    let rows_after_open = fixture.user_rows(&id).await;
    let hook_inputs_after_open = fixture.hooks.run_started_inputs();
    let again = fixture
        .service
        .deliver_staged_prompt_for_live_open(&id)
        .await
        .expect("a second open answers");

    let outcome = fixture.turn(&id, "What is the codeword?").await;
    let rows = fixture.user_rows(&id).await;
    let staged_rows = rows.iter().filter(|row| row.contains("ORBIT-7")).count();
    let requests = fixture.client.requests();
    assert!(
        matches!(
            delivered,
            meerkat_session::StagedPromptLiveOpenDelivery::Delivered
        ) && rows_after_open == [STAGED.to_string()]
            && hook_inputs_after_open == [STAGED.to_string()]
            && matches!(
                again,
                meerkat_session::StagedPromptLiveOpenDelivery::NothingStaged
            )
            && matches!(outcome, CompletionOutcome::Completed(_))
            && rows == [STAGED.to_string(), "What is the codeword?".to_string()]
            && staged_rows == 1
            && requests.len() == 1
            && requests[0].matches("ORBIT-7").count() == 1,
        "the open must deliver the staged prompt once and the first text turn must not \
         deliver it again: delivered={delivered:?} rows_after_open={rows_after_open:?} \
         hook_inputs_after_open={hook_inputs_after_open:?} again={again:?} \
         outcome={outcome:?} rows={rows:?} requests={requests:?}"
    );
    fixture.service.try_shutdown().await.expect("shutdown");
}

/// A text-first session: the first text turn consumes the staged prompt, and
/// a later live open finds nothing to deliver and commits no second row.
#[tokio::test]
async fn a_live_open_after_a_text_first_turn_delivers_nothing_again() {
    let fixture = Fixture::new();
    let id = fixture
        .materialize(
            meerkat_core::Session::new(),
            STAGED,
            DeferredPromptPolicy::Stage,
        )
        .await;
    let outcome = fixture.turn(&id, "What is the codeword?").await;
    let rows_after_turn = fixture.user_rows(&id).await;

    let delivery = fixture
        .service
        .deliver_staged_prompt_for_live_open(&id)
        .await
        .expect("the open answers");
    let rows = fixture.user_rows(&id).await;
    let expected = format!("{STAGED}\n\nWhat is the codeword?");
    assert!(
        matches!(outcome, CompletionOutcome::Completed(_))
            && rows_after_turn == [expected.clone()]
            && matches!(
                delivery,
                meerkat_session::StagedPromptLiveOpenDelivery::NothingStaged
            )
            && rows == [expected.clone()],
        "a prompt the first text turn consumed is never delivered again by a live open: \
         outcome={outcome:?} rows_after_turn={rows_after_turn:?} delivery={delivery:?} \
         rows={rows:?}"
    );
    fixture.service.try_shutdown().await.expect("shutdown");
}

/// A run-start hook that vetoes the staged text refuses its delivery at the
/// live open: nothing is committed, the prompt stays staged, and the first
/// text turn delivers it exactly once.
#[tokio::test]
async fn a_run_start_veto_keeps_the_staged_prompt_from_the_live_open_for_the_first_turn() {
    let fixture = Fixture::new();
    let id = fixture
        .materialize(
            meerkat_core::Session::new(),
            STAGED,
            DeferredPromptPolicy::Stage,
        )
        .await;
    fixture.hooks.veto_staged.store(true, Ordering::SeqCst);
    let refused = fixture
        .service
        .deliver_staged_prompt_for_live_open(&id)
        .await
        .expect("the open answers");
    let rows_after_veto = fixture.user_rows(&id).await;

    fixture.hooks.veto_staged.store(false, Ordering::SeqCst);
    let outcome = fixture.turn(&id, "What is the codeword?").await;
    let rows = fixture.user_rows(&id).await;
    let staged_rows = rows.iter().filter(|row| row.contains("ORBIT-7")).count();
    assert!(
        matches!(
            refused,
            meerkat_session::StagedPromptLiveOpenDelivery::Refused(_)
        ) && rows_after_veto.is_empty()
            && matches!(outcome, CompletionOutcome::Completed(_))
            && staged_rows == 1,
        "a vetoed delivery commits nothing and the first text turn delivers the staged \
         prompt once: refused={refused:?} rows_after_veto={rows_after_veto:?} \
         outcome={outcome:?} rows={rows:?}"
    );
    fixture.service.try_shutdown().await.expect("shutdown");
}

/// An open that never waits behind a running turn (the mirrored channel and
/// the concurrent summary open) only tries the session's turn boundary: while
/// a turn holds it, the delivery answers at once, asks nothing of the actor
/// and commits nothing, so the prompt stays staged; once the boundary is free
/// the same delivery commits it once.
#[tokio::test]
async fn a_non_waiting_open_delivers_nothing_while_a_turn_holds_the_boundary() {
    let fixture = Fixture::new();
    let id = fixture
        .materialize(
            meerkat_core::Session::new(),
            STAGED,
            DeferredPromptPolicy::Stage,
        )
        .await;

    let boundary = fixture
        .service
        .acquire_runtime_turn_finalization_guard(&id)
        .await;
    let held = tokio::time::timeout(
        DEADLINE,
        fixture.service.try_deliver_staged_prompt_for_live_open(&id),
    )
    .await
    .expect("the delivery never waits for the held turn boundary")
    .expect("the delivery answers");
    drop(boundary);
    let rows_while_held = fixture.user_rows(&id).await;
    let delivered = fixture
        .service
        .try_deliver_staged_prompt_for_live_open(&id)
        .await
        .expect("the delivery answers");
    let rows = fixture.user_rows(&id).await;
    assert!(
        matches!(
            held,
            meerkat_session::StagedPromptLiveOpenDelivery::TurnBoundaryHeld
        ) && rows_while_held.is_empty()
            && matches!(
                delivered,
                meerkat_session::StagedPromptLiveOpenDelivery::Delivered
            )
            && rows == [STAGED.to_string()],
        "a held boundary defers the delivery without waiting and without committing; a \
         free one delivers once: held={held:?} rows_while_held={rows_while_held:?} \
         delivered={delivered:?} rows={rows:?}"
    );
    fixture.service.try_shutdown().await.expect("shutdown");
}

/// The live orchestrator over a fixture's service and machine, for the open
/// projection rows.
#[cfg(feature = "live")]
struct LiveOpenParts {
    staged_sessions: Arc<crate::StagedSessionRegistry>,
    staged_capacity_admissions: crate::session_runtime::admission::StagedCapacityAdmissions,
}

#[cfg(feature = "live")]
impl LiveOpenParts {
    fn new() -> Self {
        Self {
            staged_sessions: Arc::new(crate::StagedSessionRegistry::new()),
            staged_capacity_admissions: Arc::new(StdMutex::new(std::collections::HashMap::new())),
        }
    }

    fn orchestrator<'a>(
        &'a self,
        fixture: &'a Fixture,
    ) -> crate::session_runtime::live_orchestration::LiveOrchestrator<'a, StagedPromptBuilder> {
        crate::session_runtime::live_orchestration::LiveOrchestrator {
            service: &fixture.service,
            staged_sessions: &self.staged_sessions,
            staged_capacity_admissions: &self.staged_capacity_admissions,
            runtime_adapter: &fixture.machine,
            host: None,
            config_runtime: None,
            default_llm_client: None,
            agent_llm_client_decorator: None,
            external_tools: None,
            archive_runtime_cleanup: crate::session_runtime::runtime_state::ArchiveRuntimeCleanup {
                runtime_adapter: Arc::clone(&fixture.machine),
                pending_session_event_streams: None,
                mcp_state: None,
                mob_state: None,
            },
            realm_id: None,
            instance_id: None,
            backend: None,
            ingress_reconciler: None,
            actor_witness_capture: Arc::new(
                |_session_id: SessionId,
                 _actor_witness_slot: crate::LiveSessionActorWitnessSlot| {},
            ),
            session_addressing:
                crate::session_runtime::live_orchestration::LiveSessionAddressing::SessionScoped,
        }
    }
}

/// How many of a realtime seed's user rows carry the staged prompt.
#[cfg(feature = "live")]
fn seeded_staged_rows(
    config: &meerkat_client::realtime_session::RealtimeSessionOpenConfig,
) -> usize {
    config
        .seed_messages()
        .iter()
        .filter(|message| {
            matches!(
                message,
                meerkat_core::Message::User(user) if user.text_content().contains("ORBIT-7")
            )
        })
        .count()
}

/// One open projection, dropped before this returns: whether it is marked
/// with a refused staged prompt, and how many seed rows carry the prompt.
#[cfg(feature = "live")]
async fn open_projection_marks(
    orchestrator: &crate::session_runtime::live_orchestration::LiveOrchestrator<
        '_,
        StagedPromptBuilder,
    >,
    id: &SessionId,
) -> (bool, usize) {
    let projection = orchestrator
        .live_open_projection_for_session(
            id,
            meerkat_contracts::RealtimeTurningMode::ProviderManaged,
            None,
        )
        .await
        .expect("the open projection");
    (
        projection.staged_prompt_refused(),
        seeded_staged_rows(&projection.open_config),
    )
}

/// Through the live open projection itself: the realtime seed carries the
/// staged prompt as a committed user row, once, and a second open projection
/// commits no second row. Each projection is dropped before the next: an open
/// projection holds the service's one open admission until then.
#[cfg(feature = "live")]
#[tokio::test]
async fn a_voice_first_open_projection_seeds_the_realtime_model_with_the_staged_prompt() {
    let fixture = Fixture::new();
    let id = fixture
        .materialize(
            meerkat_core::Session::new(),
            STAGED,
            DeferredPromptPolicy::Stage,
        )
        .await;
    let parts = LiveOpenParts::new();
    let orchestrator = parts.orchestrator(&fixture);

    let first = seeded_staged_rows(
        &orchestrator
            .realtime_session_open_config(
                &id,
                meerkat_contracts::RealtimeTurningMode::ProviderManaged,
            )
            .await
            .expect("the first open projection"),
    );
    let second = seeded_staged_rows(
        &orchestrator
            .realtime_session_open_config(
                &id,
                meerkat_contracts::RealtimeTurningMode::ProviderManaged,
            )
            .await
            .expect("a second open projection"),
    );
    let rows = fixture.user_rows(&id).await;
    assert!(
        first == 1 && second == 1 && rows == [STAGED.to_string()],
        "the realtime seed carries the staged prompt once and no open commits it twice: \
         first={first} second={second} rows={rows:?}"
    );
    fixture.service.try_shutdown().await.expect("shutdown");
}

/// A run-start hook that vetoes the staged prompt at the open marks the open
/// projection, which the open result reports as the `staged_prompt_refused`
/// advisory: the seed goes without the prompt and nothing is committed. The
/// next open, with the hook admitting it, delivers it once and is not marked.
#[cfg(feature = "live")]
#[tokio::test]
async fn a_vetoed_staged_prompt_marks_the_open_for_the_refused_advisory() {
    let fixture = Fixture::new();
    let id = fixture
        .materialize(
            meerkat_core::Session::new(),
            STAGED,
            DeferredPromptPolicy::Stage,
        )
        .await;
    let parts = LiveOpenParts::new();
    let orchestrator = parts.orchestrator(&fixture);

    fixture.hooks.veto_staged.store(true, Ordering::SeqCst);
    let (vetoed_marked, vetoed_seeded) = open_projection_marks(&orchestrator, &id).await;
    let rows_after_veto = fixture.user_rows(&id).await;
    fixture.hooks.veto_staged.store(false, Ordering::SeqCst);
    let (admitted_marked, admitted_seeded) = open_projection_marks(&orchestrator, &id).await;
    let rows = fixture.user_rows(&id).await;
    assert!(
        vetoed_marked
            && vetoed_seeded == 0
            && rows_after_veto.is_empty()
            && !admitted_marked
            && admitted_seeded == 1
            && rows == [STAGED.to_string()],
        "a vetoed delivery marks its open and commits nothing; the next open delivers once: \
         vetoed_marked={vetoed_marked} vetoed_seeded={vetoed_seeded} \
         rows_after_veto={rows_after_veto:?} admitted_marked={admitted_marked} \
         admitted_seeded={admitted_seeded} rows={rows:?}"
    );
    fixture.service.try_shutdown().await.expect("shutdown");
}
