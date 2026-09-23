use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::Duration;

use async_trait::async_trait;
use futures::{stream, Stream};
use meerkat::surface::{noop_request_action, PublishOutcome, SurfaceRequestExecutor};
use meerkat::{FactoryAgentBuilder, SessionService};
use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
use meerkat_core::service::{
    CreateSessionRequest, DeferredPromptPolicy, InitialTurnPolicy, SessionBuildOptions,
    SessionQuery, StartTurnRequest,
};
use meerkat_core::{Config, Message, Provider, Session, SystemPromptOverride};
use meerkat_session::{EphemeralSessionService, SessionAgentBuilder};
use serde_json::json;
use tokio::sync::Notify;

use crate::state::{workspace_factory, ForceState};
use crate::tools::{cancellation, consult, deliberate};

#[derive(Default)]
pub(crate) struct CaptureClient {
    pub requests: Mutex<Vec<LlmRequest>>,
    pub started: Notify,
    pub block: AtomicBool,
}

#[async_trait]
impl LlmClient for CaptureClient {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }
    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        self.requests.lock().unwrap().push(request.clone());
        self.started.notify_one();
        if self.block.load(Ordering::SeqCst) {
            return Box::pin(stream::pending());
        }
        Box::pin(stream::iter(vec![
            Ok(LlmEvent::TextDelta {
                delta: "synthetic {{output}} {\"nested\":{\"ok\":true}}".into(),
                meta: None,
            }),
            Ok(LlmEvent::UsageUpdate {
                usage: meerkat_core::TurnUsage::host_declared(
                    Provider::Gemini,
                    &request.model,
                    meerkat_core::Usage {
                        input_tokens: 1,
                        output_tokens: 1,
                        ..Default::default()
                    },
                ),
            }),
            Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::Success {
                    stop_reason: meerkat_core::StopReason::EndTurn,
                },
            }),
        ]))
    }
    fn provider(&self) -> Provider {
        Provider::Other
    }
    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

fn fixture() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix(".audit-fixture-")
        .tempdir_in(std::env::var_os("CARGO_MANIFEST_DIR").unwrap())
        .unwrap()
}

fn request() -> CreateSessionRequest {
    CreateSessionRequest {
        model: "gemini-3.5-flash".into(),
        prompt: "".into(),
        injected_context: vec![],
        system_prompt: SystemPromptOverride::Set(
            "Return a synthetic reply. Do not use tools.".into(),
        ),
        max_tokens: Some(64),
        event_tx: None,
        initial_turn: InitialTurnPolicy::Defer,
        deferred_prompt_policy: DeferredPromptPolicy::Discard,
        build: Some(SessionBuildOptions::default()),
        labels: None,
    }
}

fn turn() -> StartTurnRequest {
    StartTurnRequest {
        prompt: "synthetic question".into(),
        injected_context: vec![],
        system_prompt: None,
        event_tx: None,
        runtime: Default::default(),
    }
}

fn executor() -> SurfaceRequestExecutor {
    SurfaceRequestExecutor::new(Duration::from_secs(1))
}

#[tokio::test]
async fn continuation_provider_params_are_rejected_before_turn_or_lookup() {
    let dir = fixture();
    let client = Arc::new(CaptureClient::default());
    let state = ForceState::with_test_client(dir.path(), client.clone());
    for params in [
        json!({"temperature": 0.2}),
        json!({"temperature": "invalid"}),
    ] {
        let error = consult::handle(&state, &json!({"question":"synthetic", "session_id":Session::new().id(), "provider_params":params}), None).await.unwrap_err();
        assert_eq!(error.code, -32602);
        assert!(error.message.contains("provider_params"));
    }
    assert!(client.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn new_consult_applies_provider_params_before_first_request() {
    let dir = fixture();
    let client = Arc::new(CaptureClient::default());
    let state = ForceState::with_test_client(dir.path(), client.clone());
    let result = consult::handle(
        &state,
        &json!({"question":"synthetic", "model":"gemini-3.5-flash", "provider_params":{"temperature":0.2}}),
        None,
    ).await.unwrap();
    assert_eq!(client.requests.lock().unwrap()[0].temperature, Some(0.2));
    let sid = result["content"][1]["text"]
        .as_str()
        .unwrap()
        .split("session_id: ")
        .nth(1)
        .unwrap();
    state
        .session_service
        .archive(&meerkat_core::SessionId::parse(sid).unwrap())
        .await
        .unwrap();
}

#[tokio::test]
async fn consult_preserves_creation_params_and_publishes_continuable_session() {
    let dir = fixture();
    let client = Arc::new(CaptureClient::default());
    let state = ForceState::with_test_client(dir.path(), client.clone());
    let executor = executor();
    let context = executor.begin_request("create", noop_request_action());
    let result = consult::handle(&state, &json!({"question":"synthetic", "model":"gemini-3.5-flash", "provider_params":{"temperature":0.2}}), Some(context)).await.unwrap();
    assert_eq!(
        executor.publish_or_cancelled("create").await.unwrap(),
        PublishOutcome::Published
    );
    let sid = result["content"][1]["text"]
        .as_str()
        .unwrap()
        .split("session_id: ")
        .nth(1)
        .unwrap();
    consult::handle(
        &state,
        &json!({"question":"another synthetic question", "session_id":sid}),
        None,
    )
    .await
    .unwrap();
    {
        let requests = client.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].temperature, Some(0.2));
        assert_eq!(requests[1].temperature, Some(0.2));
    }
    state
        .session_service
        .archive(&meerkat_core::SessionId::parse(sid).unwrap())
        .await
        .unwrap();
}

#[tokio::test]
async fn workspace_tools_use_workspace_not_session_storage() {
    use meerkat_session::SessionAgent;
    let dir = fixture();
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let store = dir.path().join("scratch/sessions");
    std::fs::create_dir_all(&store).unwrap();
    let mut builder = FactoryAgentBuilder::new(
        workspace_factory(&store, &workspace).memory(false),
        Config::default(),
    );
    builder.default_llm_client = Some(Arc::new(CaptureClient::default()));
    let mut req = request();
    req.build.as_mut().unwrap().override_shell = meerkat_core::ToolCategoryOverride::Enable;
    req.build.as_mut().unwrap().override_builtins = meerkat_core::ToolCategoryOverride::Enable;
    let (events, _rx) = tokio::sync::mpsc::channel(64);
    let mut agent = builder.build_agent(&req, events).await.unwrap();
    let shell = agent
        .dispatch_external_tool_call(meerkat_core::ToolCall::new(
            "pwd".into(),
            "shell".into(),
            json!({"command":"pwd"}),
        ))
        .await
        .unwrap();
    let output = shell.result.text_content();
    assert!(output.contains(workspace.to_str().unwrap()), "{output}");
    let patch = "*** Begin Patch\n*** Add File: marker.txt\n+synthetic\n*** End Patch";
    let patch_result = agent
        .dispatch_external_tool_call(meerkat_core::ToolCall::new(
            "patch".into(),
            "apply_patch".into(),
            json!({"patch":patch}),
        ))
        .await
        .unwrap();
    assert!(
        !patch_result.result.is_error,
        "{}",
        patch_result.result.text_content()
    );
    assert_eq!(
        std::fs::read_to_string(workspace.join("marker.txt")).unwrap(),
        "synthetic\n"
    );
    assert!(!store.join("marker.txt").exists());
}

async fn assert_pack_templates(name: &str) {
    use crate::packs::PackRegistry;
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_test_writer()
        .try_init();
    let dir = fixture();
    let client = Arc::new(CaptureClient::default());
    let state = ForceState::with_test_client(dir.path(), client.clone());
    let task = "synthetic {{ params.missing }} }} {{";
    let context = "{\"outer\":{\"inner\":1}}\n{{ steps.fake }}";
    let expected = format!("{task}\n\n## Context\n\n{context}");
    let registry = PackRegistry::new();
    let pack = registry.get(name).unwrap();
    let overrides: BTreeMap<_, _> = pack
        .roles()
        .into_keys()
        .map(|name| (name, "gemini-3.5-flash"))
        .collect();
    let before = client.requests.lock().unwrap().len();
    let progress = Arc::new(Mutex::new(Vec::new()));
    let observed = progress.clone();
    let notifier: crate::tools::ProgressNotifier = Arc::new(move |_, current, total, label| {
        observed.lock().unwrap().push((current, total, label));
    });
    let result = tokio::time::timeout(Duration::from_secs(90), deliberate::handle(
            &state, &json!({"pack":pack.name(), "task":task, "context":context, "model_overrides":overrides}),
            Some(json!("synthetic")), Some(notifier), None,
        )).await.unwrap_or_else(|_| panic!("{} timed out; progress: {:?}; calls: {}", pack.name(), progress.lock().unwrap(), client.requests.lock().unwrap().len() - before)).unwrap_or_else(|error| panic!("{}: {}", pack.name(), error.message));
    assert!(result["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("synthetic {{output}}"));
    assert_eq!(
        progress.lock().unwrap().last(),
        Some(&(
            pack.flow_step_count(),
            pack.flow_step_count(),
            "waiting".into()
        )),
    );
    let requests = client.requests.lock().unwrap();
    let user_text: Vec<_> = requests[before..]
        .iter()
        .flat_map(|request| request.messages.iter())
        .filter_map(|message| {
            if let Message::User(user) = message {
                Some(user.text_content())
            } else {
                None
            }
        })
        .collect();
    assert!(
        user_text.iter().any(|text| text.contains(&expected)),
        "{}: {user_text:?}",
        pack.name()
    );
    if pack.flow_step_count() > 1 {
        assert!(
            user_text
                .iter()
                .any(|text| text.contains("synthetic {{output}}")),
            "{} did not forward prior output: {user_text:?}",
            pack.name(),
        );
    }
}

macro_rules! pack_template_test {
    ($test:ident, $name:literal) => {
        #[tokio::test]
        async fn $test() {
            assert_pack_templates($name).await;
        }
    };
}

pack_template_test!(templates_advisor, "advisor");
pack_template_test!(templates_architect, "architect");
pack_template_test!(templates_brainstorm, "brainstorm");
pack_template_test!(templates_implement, "implement");
pack_template_test!(templates_panel, "panel");
pack_template_test!(templates_rct, "rct");
pack_template_test!(templates_red_team, "red-team");
pack_template_test!(templates_review, "review");

#[tokio::test]
async fn custom_modes_admit_members_and_preserve_task_aliases_as_opaque_data() {
    let dir = fixture();
    let client = Arc::new(CaptureClient::default());
    let state = ForceState::with_test_client(dir.path(), client.clone());
    let mobs_dir = dir.path().join(".codemob-mcp/mobs");
    std::fs::create_dir_all(&mobs_dir).unwrap();
    let task = "{{not a template}} }} {{";
    let context = "{\"outer\":{\"inner\":true}}";
    for mode in ["comms", "flow"] {
        let mut config = json!({
            "name":mode, "description":"synthetic",
            "agents": {"lead":{"model":"gemini-3.5-flash", "skill":"Return the requested artifact."}},
        });
        if mode == "flow" {
            config["mode"] = json!("flow");
            config["flows"] = json!({"main":[
                {"id":"first","role":"lead","message":"{{task}}"},
                {"id":"second","role":"lead","message":"{{ task }}\n{{ steps.first }}","depends_on":["first"]}
            ]});
        }
        std::fs::write(mobs_dir.join(format!("{mode}.json")), config.to_string()).unwrap();
        state.reload_user_packs();
        let before = client.requests.lock().unwrap().len();
        tokio::time::timeout(
            Duration::from_secs(15),
            deliberate::handle(
                &state,
                &json!({"pack":mode,"task":task,"context":context}),
                None,
                None,
                None,
            ),
        )
        .await
        .unwrap()
        .unwrap();
        let requests = client.requests.lock().unwrap();
        let expected = format!("{task}\n\n## Context\n\n{context}");
        assert!(requests[before..].iter().flat_map(|request| request.messages.iter())
            .any(|message| matches!(message, Message::User(user) if user.text_content().contains(&expected))));
    }
}

#[tokio::test]
async fn already_cancelled_consult_and_deliberate_admit_nothing() {
    let dir = fixture();
    let client = Arc::new(CaptureClient::default());
    let state = ForceState::with_test_client(dir.path(), client.clone());
    let executor = executor();
    for (key, is_mob) in [("consult", false), ("deliberate", true)] {
        let context = executor.begin_request(key, noop_request_action());
        executor.cancel_request(key).await;
        let error = if is_mob {
            deliberate::handle(
                &state,
                &json!({"pack":"advisor", "task":"synthetic"}),
                None,
                None,
                Some(context),
            )
            .await
            .unwrap_err()
        } else {
            consult::handle(
                &state,
                &json!({"question":"synthetic", "model":"gemini-3.5-flash"}),
                Some(context),
            )
            .await
            .unwrap_err()
        };
        assert_eq!(error.code, -32005);
        executor.finish_unpublished(key).await;
    }
    assert!(client.requests.lock().unwrap().is_empty());
    assert!(state
        .session_service
        .list(SessionQuery::default())
        .await
        .unwrap()
        .is_empty());
    assert!(state.mob_state.mob_list().await.unwrap().is_empty());
}

struct BlockingBuilder {
    inner: FactoryAgentBuilder,
    started: Arc<Notify>,
    release: Arc<Notify>,
}

#[async_trait]
impl SessionAgentBuilder for BlockingBuilder {
    type Agent = <FactoryAgentBuilder as SessionAgentBuilder>::Agent;
    async fn build_agent(
        &self,
        req: &CreateSessionRequest,
        events: tokio::sync::mpsc::Sender<meerkat_core::AgentEvent>,
    ) -> Result<Self::Agent, meerkat_core::service::SessionError> {
        self.started.notify_one();
        self.release.notified().await;
        self.inner.build_agent(req, events).await
    }
}

#[tokio::test]
async fn cancellation_during_construction_never_registers_or_runs_a_session() {
    let dir = fixture();
    let client = Arc::new(CaptureClient::default());
    let started = Arc::new(Notify::new());
    let mut builder = FactoryAgentBuilder::new(
        workspace_factory(&dir.path().join("store"), dir.path()).memory(false),
        Config::default(),
    );
    builder.default_llm_client = Some(client.clone());
    let service = Arc::new(EphemeralSessionService::new(
        BlockingBuilder {
            inner: builder,
            started: started.clone(),
            release: Arc::new(Notify::new()),
        },
        8,
    ));
    let executor = executor();
    let context = executor.begin_request("blocked", noop_request_action());
    let work = cancellation::create_and_run(&service, request(), turn(), Some(&context));
    let cancel = async {
        started.notified().await;
        executor.cancel_request("blocked").await;
    };
    let (result, _) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(work, cancel)
    })
    .await
    .unwrap();
    assert_eq!(result.unwrap_err().code, -32005);
    executor.finish_unpublished("blocked").await;
    assert!(service
        .list(SessionQuery::default())
        .await
        .unwrap()
        .is_empty());
    assert!(client.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn idle_continuation_cancelled_before_admission_does_not_run_or_destroy_session() {
    let dir = fixture();
    let client = Arc::new(CaptureClient::default());
    let state = ForceState::with_test_client(dir.path(), client.clone());
    let created = state
        .session_service
        .create_session(request())
        .await
        .unwrap();
    let executor = executor();
    let context = executor.begin_request("idle", noop_request_action());
    let signal = cancellation::cancellation_signal(Some(&context))
        .await
        .unwrap();
    executor.cancel_request("idle").await;
    let result =
        cancellation::run_turn(&state.session_service, &created.session_id, turn(), &signal).await;
    assert_eq!(result.unwrap_err().code, -32005);
    assert!(client.requests.lock().unwrap().is_empty());
    assert!(state
        .session_service
        .read(&created.session_id)
        .await
        .is_ok());
    state
        .session_service
        .archive(&created.session_id)
        .await
        .unwrap();
}

#[tokio::test]
async fn cancellation_while_waiting_for_admission_releases_claim_for_next_turn() {
    let dir = fixture();
    let client = Arc::new(CaptureClient::default());
    let state = ForceState::with_test_client(dir.path(), client.clone());
    let created = state
        .session_service
        .create_session(request())
        .await
        .unwrap();
    let guard = state
        .session_service
        .acquire_runtime_turn_finalization_guard(&created.session_id)
        .await;
    let executor = executor();
    let context = executor.begin_request("admission", noop_request_action());
    let signal = cancellation::cancellation_signal(Some(&context))
        .await
        .unwrap();
    let work = cancellation::run_turn(&state.session_service, &created.session_id, turn(), &signal);
    let (result, _) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(biased; work, executor.cancel_request("admission"))
    })
    .await
    .unwrap();
    assert_eq!(result.unwrap_err().code, -32005);
    assert!(client.requests.lock().unwrap().is_empty());
    drop(guard);
    state
        .session_service
        .start_turn(&created.session_id, turn())
        .await
        .unwrap();
    assert_eq!(client.requests.lock().unwrap().len(), 1);
    state
        .session_service
        .archive(&created.session_id)
        .await
        .unwrap();
}

#[tokio::test]
async fn admitted_consult_cancel_joins_turn_and_discards_unpublished_actor() {
    let dir = fixture();
    let client = Arc::new(CaptureClient::default());
    client.block.store(true, Ordering::SeqCst);
    let state = ForceState::with_test_client(dir.path(), client.clone());
    let executor = executor();
    let context = executor.begin_request("running", noop_request_action());
    let arguments = json!({"question":"synthetic", "model":"gemini-3.5-flash"});
    let work = consult::handle(&state, &arguments, Some(context));
    let cancel = async {
        client.started.notified().await;
        executor.cancel_request("running").await;
    };
    let (result, _) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(work, cancel)
    })
    .await
    .unwrap();
    assert_eq!(result.unwrap_err().code, -32005);
    executor.finish_unpublished("running").await;
    assert!(state
        .session_service
        .list(SessionQuery::default())
        .await
        .unwrap()
        .is_empty());
    assert_eq!(client.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn cancel_between_consult_completion_and_publication_discards_actor() {
    let dir = fixture();
    let client = Arc::new(CaptureClient::default());
    let state = ForceState::with_test_client(dir.path(), client);
    let executor = executor();
    let context = executor.begin_request("publish", noop_request_action());
    consult::handle(
        &state,
        &json!({"question":"synthetic", "model":"gemini-3.5-flash"}),
        Some(context),
    )
    .await
    .unwrap();
    executor.cancel_request("publish").await;
    assert_eq!(
        executor.publish_or_cancelled("publish").await.unwrap(),
        PublishOutcome::CancelledBeforePublish
    );
    assert!(state
        .session_service
        .list(SessionQuery::default())
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn reload_restores_builtin_after_override_and_removes_ordinary_custom_pack() {
    let dir = fixture();
    let state = ForceState::in_workspace(dir.path().to_path_buf()).unwrap();
    let mobs_dir = dir.path().join(".codemob-mcp/mobs");
    std::fs::create_dir_all(&mobs_dir).unwrap();
    for name in ["review", "ordinary"] {
        let path = mobs_dir.join(format!("{name}.json"));
        std::fs::write(&path, json!({"name":name, "description":"override", "agents":{"custom_role":{"model":"gemini-3.5-flash", "skill":"Synthetic role"}}}).to_string()).unwrap();
        state.reload_user_packs();
        assert_eq!(
            state.pack_registry().get(name).unwrap().description(),
            "override"
        );
        assert_eq!(
            state.pack_registry().get(name).unwrap().roles(),
            BTreeMap::from([("custom_role".into(), "gemini-3.5-flash".into())])
        );
        std::fs::remove_file(path).unwrap();
        state.reload_user_packs();
        if name == "review" {
            assert_ne!(
                state.pack_registry().get(name).unwrap().description(),
                "override"
            );
        } else {
            assert!(state.pack_registry().get(name).is_none());
        }
    }
}
