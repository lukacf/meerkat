//! A stopped run followed by a model-selected successor on the same member.
//!
//! Reported by a T3 consumer on 0.8.49: a persistent TurnDriven member takes
//! an input, a durable Steer joins its run, the host stops that exact run
//! (`MeerkatMachine::stop_run`), and the next ordinary member turn carrying
//! an LLM identity override fails with "failed to reconfigure member session
//! ... LLM identity ...: Runtime not ready: destroyed". The stopped turn
//! committed no boundary, so the live session trails durable authority; the
//! per-turn reconfiguration read that intermediate state before the turn's own
//! resync could run.
//!
//! The successor must complete in the SAME session, on the selected model,
//! with the stopped input and the joined steer not replayed.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt as _;
use meerkat::{AgentFactory, Config, FactoryAgentBuilder, PersistentSessionService};
use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
use meerkat_core::types::HandlingMode;
use meerkat_core::{AgentEvent, Message};
use meerkat_mob::{
    AgentIdentity, MemberTurnOptions, MobBuilder, MobDefinition, MobId, MobRuntimeMode, MobStorage,
    Profile, ProfileBinding, ProfileName, SpawnMemberSpec, ToolConfig,
};
use meerkat_runtime::SessionServiceRuntimeExt as _;

const WORKER: &str = "t3-worker";
const FIRST_PROMPT: &str = "first input that the host stops";
const STEER_PROMPT: &str = "steer that joins the stopped run";
const NEXT_PROMPT: &str = "successor on the selected model";
const PROFILE_MODEL: &str = "gpt-5.4";
const SELECTED_MODEL: &str = "gpt-5.5";
const STEP: Duration = Duration::from_secs(30);

/// One recorded provider request.
#[derive(Debug, Clone)]
struct RecordedRequest {
    model: String,
    user_texts: Vec<String>,
}

/// Scripted model:
/// - the first request waits for the test, then calls the shell tool;
/// - later requests of the original run keep calling the shell tool, so the
///   run keeps reaching model boundaries, until one carries the durable
///   Steer (it joined the ORIGINAL run); that request never answers, and the
///   host stops the run inside it;
/// - the successor's request answers at once.
struct ScriptedClient {
    requests: Mutex<Vec<RecordedRequest>>,
    first_entered: tokio::sync::Notify,
    release_first: tokio::sync::Notify,
    steer_ingested: tokio::sync::Notify,
    /// Control variant: hang the original run's second request with no steer.
    hang_second_without_steer: bool,
    /// Errored variant: the original run's later requests fail.
    fail_after_first: bool,
}

impl ScriptedClient {
    fn new() -> Self {
        Self {
            requests: Mutex::new(Vec::new()),
            first_entered: tokio::sync::Notify::new(),
            release_first: tokio::sync::Notify::new(),
            steer_ingested: tokio::sync::Notify::new(),
            hang_second_without_steer: false,
            fail_after_first: false,
        }
    }

    fn recorded(&self) -> Vec<RecordedRequest> {
        self.requests.lock().unwrap().clone()
    }
}

/// Every non-system message as rendered JSON, so a prompt is found whichever
/// message kind carries it (user turn, peer delivery, steer append).
fn user_texts(messages: &[Message]) -> Vec<String> {
    messages
        .iter()
        .filter(|message| !matches!(message, Message::System(_)))
        .map(|message| serde_json::to_string(message).expect("message renders"))
        .collect()
}

fn mentions(texts: &[String], needle: &str) -> bool {
    texts.iter().any(|text| text.contains(needle))
}

fn done_events(
    request: &LlmRequest,
    mut events: Vec<LlmEvent>,
    stop: meerkat_core::StopReason,
) -> Vec<LlmEvent> {
    events.push(LlmEvent::UsageUpdate {
        usage: meerkat_core::TurnUsage::host_declared(
            meerkat_core::Provider::OpenAI,
            &request.model,
            meerkat_core::Usage::default(),
        ),
    });
    events.push(LlmEvent::Done {
        outcome: LlmDoneOutcome::Success { stop_reason: stop },
    });
    events
}

fn shell_call(request: &LlmRequest, index: usize) -> Vec<LlmEvent> {
    done_events(
        request,
        vec![LlmEvent::ToolCallComplete {
            id: format!("call-t3-shell-{index}"),
            name: "shell".to_string(),
            args: serde_json::json!({ "command": "true", "timeout_secs": 30 }),
            meta: None,
        }],
        meerkat_core::StopReason::ToolUse,
    )
}

#[async_trait::async_trait]
impl LlmClient for ScriptedClient {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> std::pin::Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>>
    {
        let texts = user_texts(&request.messages);
        let index = {
            let mut requests = self.requests.lock().unwrap();
            requests.push(RecordedRequest {
                model: request.model.clone(),
                user_texts: texts.clone(),
            });
            requests.len()
        };
        if mentions(&texts, NEXT_PROMPT) {
            return Box::pin(futures::stream::iter(
                done_events(
                    request,
                    vec![LlmEvent::TextDelta {
                        delta: "done".to_string(),
                        meta: None,
                    }],
                    meerkat_core::StopReason::EndTurn,
                )
                .into_iter()
                .map(Ok),
            ));
        }
        if self.fail_after_first && index > 1 {
            return Box::pin(futures::stream::once(async {
                Err(LlmError::InvalidRequest {
                    message: "scripted provider failure".to_string(),
                })
            }));
        }
        if mentions(&texts, STEER_PROMPT) || (self.hang_second_without_steer && index == 2) {
            return Box::pin(futures::stream::once(async move {
                self.steer_ingested.notify_one();
                std::future::pending::<Result<LlmEvent, LlmError>>().await
            }));
        }
        if index == 1 {
            let events = shell_call(request, index);
            return Box::pin(
                futures::stream::once(async move {
                    self.first_entered.notify_one();
                    self.release_first.notified().await;
                })
                .flat_map(move |()| futures::stream::iter(events.clone().into_iter().map(Ok))),
            );
        }
        // Bounded: a steer that never joins ends the run instead of looping.
        if index > 16 {
            return Box::pin(futures::stream::iter(
                done_events(request, Vec::new(), meerkat_core::StopReason::EndTurn)
                    .into_iter()
                    .map(Ok),
            ));
        }
        Box::pin(futures::stream::iter(
            shell_call(request, index).into_iter().map(Ok),
        ))
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::OpenAI
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

/// A durable Steer: no prompt text, one typed user conversation append, the
/// shape the runtime joins into a running turn at its next model call.
fn durable_steer(text: &str) -> meerkat_runtime::Input {
    use meerkat_core::lifecycle::{ConversationAppend, ConversationAppendRole, CoreRenderable};
    let mut prompt = meerkat_runtime::PromptInput::new(
        "",
        Some(
            meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                handling_mode: Some(HandlingMode::Steer),
                ..Default::default()
            },
        ),
    );
    prompt.typed_turn_appends = vec![ConversationAppend {
        runtime_source: None,
        role: ConversationAppendRole::User,
        content: CoreRenderable::Text {
            text: text.to_string(),
        },
        identity: None,
    }];
    meerkat_runtime::Input::Prompt(prompt)
}

async fn build_service(
    root: &Path,
    client: Arc<ScriptedClient>,
) -> (
    Arc<PersistentSessionService<FactoryAgentBuilder>>,
    Arc<meerkat_runtime::MeerkatMachine>,
) {
    let (_manifest, persistence) = meerkat::open_realm_persistence_in(
        root,
        "t3-stop-realm",
        Some(meerkat_store::RealmBackend::Sqlite),
        Some(meerkat_store::RealmOrigin::Explicit),
    )
    .await
    .expect("open realm persistence");
    let project = root.join("project");
    std::fs::create_dir_all(&project).expect("project dir");
    let factory = AgentFactory::new(root.join("sessions"))
        .runtime_root(root.join("realm"))
        .project_root(&project)
        .builtins(true)
        .shell(true)
        .comms(true);
    let mut config = Config::default();
    config.shell.program = "sh".to_string();
    config.shell.security_mode = meerkat_core::types::SecurityMode::Unrestricted;
    let mut builder = FactoryAgentBuilder::new(factory, config);
    builder.default_llm_client = Some(client);
    meerkat::surface::build_runtime_backed_service_with_default_reconfigure_host(
        builder,
        8,
        persistence,
        root.join("config-state.json"),
    )
}

fn mob_definition() -> MobDefinition {
    let mut profiles = BTreeMap::new();
    profiles.insert(
        ProfileName::from("worker"),
        ProfileBinding::Inline(Box::new(Profile {
            model_fallback: None,
            model: PROFILE_MODEL.to_string(),
            provider: None,
            self_hosted_server_id: None,
            image_generation_provider: None,
            auto_compact_threshold: None,
            resume_overrides: Vec::new(),
            skills: vec![],
            tools: ToolConfig {
                shell: true,
                comms: true,
                ..Default::default()
            },
            peer_description: "T3 worker".to_string(),
            external_addressable: true,
            backend: None,
            runtime_mode: MobRuntimeMode::TurnDriven,
            max_inline_peer_notifications: None,
            output_schema: None,
            provider_params: None,
        })),
    );
    let mut definition = MobDefinition::explicit(MobId::from(format!(
        "t3-stop-{}",
        uuid::Uuid::new_v4().simple()
    )));
    definition.profiles = profiles;
    definition
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stopped_run_does_not_break_the_next_model_selected_turn() {
    let temp = tempfile::tempdir().expect("temp dir");
    let client = Arc::new(ScriptedClient::new());
    // The surface's machine, which carries the LLM reconfigure host, is also
    // the mob's runtime (as a host wires it).
    let (service, adapter) = build_service(temp.path(), Arc::clone(&client)).await;
    let storage = MobStorage::persistent(temp.path().join("mob.db")).expect("mob storage");
    let handle = MobBuilder::new(mob_definition(), storage)
        .with_session_service(service.clone())
        .with_runtime_adapter(adapter.clone())
        .with_default_llm_client(client.clone())
        .create()
        .await
        .expect("create persistent mob");
    let worker = AgentIdentity::from(WORKER);
    let reviewer = AgentIdentity::from("t3-reviewer");
    for identity in [&worker, &reviewer] {
        handle
            .spawn_spec(SpawnMemberSpec::new("worker", identity.clone()))
            .await
            .expect("spawn member");
    }
    handle
        .wire(reviewer.clone(), worker.clone())
        .await
        .expect("wire reviewer to worker");
    let session_id = handle
        .resolve_bridge_session_id(&worker)
        .await
        .expect("worker session");
    let member = handle.member(&worker).await.expect("worker handle");

    // The first input starts a run.
    let (events_tx, mut events_rx) = tokio::sync::mpsc::channel(256);
    let first = member
        .start_turn(
            FIRST_PROMPT,
            HandlingMode::Queue,
            MemberTurnOptions::default(),
            Some(events_tx),
        )
        .await
        .expect("start the first turn");
    tokio::time::timeout(STEP, client.first_entered.notified())
        .await
        .expect("the first provider request is in flight");
    let run_id = tokio::time::timeout(STEP, async {
        loop {
            let envelope = events_rx.recv().await.expect("first turn events");
            if let AgentEvent::RunStarted { identity, .. } = envelope.payload
                && let Some(run_id) = identity.run_id
            {
                return run_id;
            }
        }
    })
    .await
    .expect("the first run reports its id");

    // A durable Steer (a typed conversation append) joins the original run
    // at its next model boundary.
    adapter
        .accept_input(&session_id, durable_steer(STEER_PROMPT))
        .await
        .expect("admit the durable steer");
    client.release_first.notify_one();
    tokio::time::timeout(STEP, client.steer_ingested.notified())
        .await
        .expect("the steer is ingested into the ORIGINAL run at a model boundary");

    // The host stops that exact run.
    let receipt = tokio::time::timeout(
        STEP,
        adapter.stop_run(&session_id, &run_id, "host stopped the selection"),
    )
    .await
    .expect("stop_run returns")
    .expect("stop_run");
    assert!(
        matches!(receipt, meerkat_runtime::RunStopReceipt::Stopped { .. }),
        "the exact run is stopped: {receipt:?}"
    );
    let _ = first;

    // An ordinary member turn on a selected model, in the same session.
    let mut options = MemberTurnOptions::default();
    options.model = Some(meerkat_core::lifecycle::run_primitive::ModelId::new(
        SELECTED_MODEL,
    ));
    options.provider = Some(meerkat_core::Provider::OpenAI);
    let successor = member
        .start_turn(NEXT_PROMPT, HandlingMode::Queue, options, None)
        .await
        .expect("admit the successor");
    assert_eq!(successor.session_id(), Some(&session_id));
    tokio::time::timeout(STEP, successor.wait())
        .await
        .expect("the successor completes")
        .expect("the successor runs on the selected model");

    let requests = client.recorded();
    let successors = requests
        .iter()
        .filter(|request| mentions(&request.user_texts, NEXT_PROMPT))
        .collect::<Vec<_>>();
    assert_eq!(successors.len(), 1, "one successor request: {requests:?}");
    let successor_request = successors[0];
    assert_eq!(successor_request.model, SELECTED_MODEL);
    assert_eq!(
        successor_request
            .user_texts
            .iter()
            .filter(|text| text.contains(NEXT_PROMPT))
            .count(),
        1,
        "the successor prompt reaches the provider once: {successor_request:?}"
    );
    assert!(
        !successor_request
            .user_texts
            .iter()
            .any(|text| text.contains(FIRST_PROMPT) || text.contains(STEER_PROMPT)),
        "the stopped input and the joined steer are not replayed: {successor_request:?}"
    );
    assert_eq!(
        handle.resolve_bridge_session_id(&worker).await,
        Some(session_id),
        "the member keeps its session"
    );
}

/// The reported shape: the stopped run's input arrives as a PEER message
/// (another member's review delivery), not a direct member turn.
#[tokio::test(flavor = "multi_thread")]
async fn a_stopped_peer_driven_run_does_not_break_the_next_model_selected_turn() {
    let temp = tempfile::tempdir().expect("temp dir");
    let client = Arc::new(ScriptedClient::new());
    // The surface's machine, which carries the LLM reconfigure host, is also
    // the mob's runtime (as a host wires it).
    let (service, adapter) = build_service(temp.path(), Arc::clone(&client)).await;
    let storage = MobStorage::persistent(temp.path().join("mob.db")).expect("mob storage");
    let handle = MobBuilder::new(mob_definition(), storage)
        .with_session_service(service.clone())
        .with_runtime_adapter(adapter.clone())
        .with_default_llm_client(client.clone())
        .create()
        .await
        .expect("create persistent mob");
    let worker = AgentIdentity::from(WORKER);
    let reviewer = AgentIdentity::from("t3-reviewer");
    for identity in [&worker, &reviewer] {
        handle
            .spawn_spec(SpawnMemberSpec::new("worker", identity.clone()))
            .await
            .expect("spawn member");
    }
    handle
        .wire(reviewer.clone(), worker.clone())
        .await
        .expect("wire reviewer to worker");
    let session_id = handle
        .resolve_bridge_session_id(&worker)
        .await
        .expect("worker session");
    let mut session_events = meerkat_core::service::SessionService::subscribe_session_events(
        service.as_ref(),
        &session_id,
    )
    .await
    .expect("subscribe to the worker session");

    // The first input is the reviewer's peer delivery.
    handle
        .send_peer_message(
            reviewer.clone(),
            worker.clone(),
            FIRST_PROMPT,
            HandlingMode::Queue,
        )
        .await
        .expect("deliver the peer review");
    tokio::time::timeout(STEP, client.first_entered.notified())
        .await
        .expect("the peer-driven provider request is in flight");
    let run_id = tokio::time::timeout(STEP, async {
        loop {
            let envelope = session_events.next().await.expect("worker session events");
            if let AgentEvent::RunStarted { identity, .. } = envelope.payload
                && let Some(run_id) = identity.run_id
            {
                return run_id;
            }
        }
    })
    .await
    .expect("the peer-driven run reports its id");

    let member = handle.member(&worker).await.expect("worker handle");
    // A durable Steer (a typed conversation append) joins the original run
    // at its next model boundary.
    adapter
        .accept_input(&session_id, durable_steer(STEER_PROMPT))
        .await
        .expect("admit the durable steer");
    client.release_first.notify_one();
    tokio::time::timeout(STEP, client.steer_ingested.notified())
        .await
        .expect("the steer is ingested into the ORIGINAL run at a model boundary");

    let receipt = tokio::time::timeout(
        STEP,
        adapter.stop_run(&session_id, &run_id, "coordinator stopped the run"),
    )
    .await
    .expect("stop_run returns")
    .expect("stop_run");
    assert!(
        matches!(receipt, meerkat_runtime::RunStopReceipt::Stopped { .. }),
        "the exact run is stopped: {receipt:?}"
    );

    let mut options = MemberTurnOptions::default();
    options.model = Some(meerkat_core::lifecycle::run_primitive::ModelId::new(
        SELECTED_MODEL,
    ));
    options.provider = Some(meerkat_core::Provider::OpenAI);
    let successor = member
        .start_turn(NEXT_PROMPT, HandlingMode::Queue, options, None)
        .await
        .expect("admit the successor");
    tokio::time::timeout(STEP, successor.wait())
        .await
        .expect("the successor completes")
        .expect("the successor runs on the selected model");

    let requests = client.recorded();
    let successors = requests
        .iter()
        .filter(|request| mentions(&request.user_texts, NEXT_PROMPT))
        .collect::<Vec<_>>();
    assert_eq!(successors.len(), 1, "one successor request: {requests:?}");
    let successor_request = successors[0];
    assert_eq!(successor_request.model, SELECTED_MODEL);
    assert!(
        !successor_request
            .user_texts
            .iter()
            .any(|text| text.contains(FIRST_PROMPT) || text.contains(STEER_PROMPT)),
        "the stopped peer input and the joined steer are not replayed: {requests:?}"
    );
}

/// Control: the same stop at the same model boundary, with NO steer joined.
#[tokio::test(flavor = "multi_thread")]
async fn control_a_stopped_run_without_a_joined_steer() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut scripted = ScriptedClient::new();
    scripted.hang_second_without_steer = true;
    let client = Arc::new(scripted);
    let (service, adapter) = build_service(temp.path(), Arc::clone(&client)).await;
    let storage = MobStorage::persistent(temp.path().join("mob.db")).expect("mob storage");
    let handle = MobBuilder::new(mob_definition(), storage)
        .with_session_service(service.clone())
        .with_runtime_adapter(adapter.clone())
        .with_default_llm_client(client.clone())
        .create()
        .await
        .expect("create persistent mob");
    let worker = AgentIdentity::from(WORKER);
    handle
        .spawn_spec(SpawnMemberSpec::new("worker", worker.clone()))
        .await
        .expect("spawn worker");
    let session_id = handle
        .resolve_bridge_session_id(&worker)
        .await
        .expect("session");
    let member = handle.member(&worker).await.expect("worker handle");
    let (events_tx, mut events_rx) = tokio::sync::mpsc::channel(256);
    let _first = member
        .start_turn(
            FIRST_PROMPT,
            HandlingMode::Queue,
            MemberTurnOptions::default(),
            Some(events_tx),
        )
        .await
        .expect("start the first turn");
    tokio::time::timeout(STEP, client.first_entered.notified())
        .await
        .expect("first request");
    let run_id = tokio::time::timeout(STEP, async {
        loop {
            let envelope = events_rx.recv().await.expect("events");
            if let AgentEvent::RunStarted { identity, .. } = envelope.payload
                && let Some(run_id) = identity.run_id
            {
                return run_id;
            }
        }
    })
    .await
    .expect("run id");
    client.release_first.notify_one();
    tokio::time::timeout(STEP, client.steer_ingested.notified())
        .await
        .expect("second request");
    let receipt = adapter
        .stop_run(&session_id, &run_id, "host stop")
        .await
        .expect("stop_run");
    assert!(
        matches!(receipt, meerkat_runtime::RunStopReceipt::Stopped { .. }),
        "{receipt:?}"
    );
    let mut options = MemberTurnOptions::default();
    options.model = Some(meerkat_core::lifecycle::run_primitive::ModelId::new(
        SELECTED_MODEL,
    ));
    options.provider = Some(meerkat_core::Provider::OpenAI);
    let successor = member
        .start_turn(NEXT_PROMPT, HandlingMode::Queue, options, None)
        .await
        .expect("admit the successor");
    tokio::time::timeout(STEP, successor.wait())
        .await
        .expect("the successor completes")
        .expect("control: the successor runs on the selected model");
}

/// An ERRORED original run (the provider fails after a tool round) leaves the
/// same uncommitted live image as a stopped one; the model-selected successor
/// must still run in the same session.
#[tokio::test(flavor = "multi_thread")]
async fn an_errored_run_does_not_break_the_next_model_selected_turn() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut scripted = ScriptedClient::new();
    scripted.fail_after_first = true;
    let client = Arc::new(scripted);
    let (service, adapter) = build_service(temp.path(), Arc::clone(&client)).await;
    let storage = MobStorage::persistent(temp.path().join("mob.db")).expect("mob storage");
    let handle = MobBuilder::new(mob_definition(), storage)
        .with_session_service(service.clone())
        .with_runtime_adapter(adapter.clone())
        .with_default_llm_client(client.clone())
        .create()
        .await
        .expect("create persistent mob");
    let worker = AgentIdentity::from(WORKER);
    handle
        .spawn_spec(SpawnMemberSpec::new("worker", worker.clone()))
        .await
        .expect("spawn worker");
    let session_id = handle
        .resolve_bridge_session_id(&worker)
        .await
        .expect("session");
    let member = handle.member(&worker).await.expect("worker handle");
    let first = member
        .start_turn(
            FIRST_PROMPT,
            HandlingMode::Queue,
            MemberTurnOptions::default(),
            None,
        )
        .await
        .expect("start the first turn");
    tokio::time::timeout(STEP, client.first_entered.notified())
        .await
        .expect("first request");
    client.release_first.notify_one();
    let errored = tokio::time::timeout(STEP, first.wait())
        .await
        .expect("the errored turn terminates");
    assert!(errored.is_err(), "the original turn errors: {errored:?}");

    let mut options = MemberTurnOptions::default();
    options.model = Some(meerkat_core::lifecycle::run_primitive::ModelId::new(
        SELECTED_MODEL,
    ));
    options.provider = Some(meerkat_core::Provider::OpenAI);
    let successor = member
        .start_turn(NEXT_PROMPT, HandlingMode::Queue, options, None)
        .await
        .expect("admit the successor");
    assert_eq!(successor.session_id(), Some(&session_id));
    tokio::time::timeout(STEP, successor.wait())
        .await
        .expect("the successor completes")
        .expect("the successor runs on the selected model after an errored turn");
    let requests = client.recorded();
    let successor_request = requests
        .iter()
        .find(|request| mentions(&request.user_texts, NEXT_PROMPT))
        .expect("the successor reached the provider");
    assert_eq!(successor_request.model, SELECTED_MODEL);
}
