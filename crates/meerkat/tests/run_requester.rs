//! A run's original requester reaches tool dispatch and the transcript.
//!
//! The host admits a prompt with a requester (an opaque principal it
//! authenticated) on a runtime-backed session whose scripted model calls one
//! tool. The tool sees the run's requester in its dispatch context, as a
//! direct requester, and the transcript rows of the turn name it.

#![cfg(all(feature = "anthropic", not(target_arch = "wasm32")))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use meerkat::{AgentFactory, FactoryAgentBuilder};
use meerkat_core::connection::AuthCredentialIdentity;
use meerkat_core::service::{
    CreateSessionRequest, DeferredPromptPolicy, InitialTurnPolicy, SessionHistoryQuery,
    SessionServiceHistoryExt,
};
use meerkat_core::{
    AgentError, AgentSessionStore, AgentToolDispatcher, Config, ControllerModelSelection, Message,
    PrincipalKind, PrincipalRef, Provider, Session, SessionId, SessionLlmIdentity, StopReason,
    ToolCallView, ToolDef, ToolDispatchContext, ToolDispatchOutcome, ToolError, ToolResult,
    TrustDomainId, TurnRequester,
};
use meerkat_llm_core::{
    LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest, LlmStream, PreparedLlmRequest,
};
use meerkat_runtime::completion::CompletionOutcome;
use meerkat_runtime::input::{Input, PromptInput};
use meerkat_runtime::meerkat_machine::MeerkatMachine;
use meerkat_session::EphemeralSessionService;

const MODEL: &str = "claude-sonnet-4-5";
const ENDPOINT: &str = "https://run-requester.invalid/messages";
const TOOL: &str = "record_requester";
const PROMPT: &str = "Call the tool once.";
const ANSWER: &str = "the tool ran";
const CREDENTIAL_PUBLICATION_WAIT: Duration = Duration::from_secs(30);
const TURN_DEADLINE: Duration = Duration::from_secs(30);

fn person(subject: &str) -> PrincipalRef {
    PrincipalRef::in_domain(
        PrincipalKind::Human,
        subject,
        TrustDomainId::new("oidc-issuer:https://issuer.example").expect("trust domain"),
    )
    .expect("qualified requester")
}

fn identity() -> SessionLlmIdentity {
    SessionLlmIdentity {
        model: MODEL.into(),
        provider: Provider::Anthropic,
        self_hosted_server_id: None,
        provider_params: None,
        auth_binding: None,
    }
}

fn credential() -> AuthCredentialIdentity {
    serde_json::from_value(
        serde_json::json!({"realm":"run-requester", "account":"scripted-controller"}),
    )
    .expect("nonsecret identity of the scripted transport")
}

fn selection() -> ControllerModelSelection {
    ControllerModelSelection::new(
        identity(),
        credential(),
        "scripted-profile".into(),
        "scripted-transport".into(),
    )
}

fn done(reason: StopReason) -> Result<LlmEvent, LlmError> {
    Ok(LlmEvent::Done {
        outcome: LlmDoneOutcome::success(reason),
    })
}

/// First request: one tool call. Second request: the answer.
struct ScriptedModel {
    requests: AtomicUsize,
}

impl ScriptedModel {
    fn script(&self) -> LlmStream<'_> {
        let index = self.requests.fetch_add(1, Ordering::Relaxed);
        if index == 0 {
            Box::pin(futures::stream::iter([
                Ok(LlmEvent::ToolCallComplete {
                    id: "record-0".into(),
                    name: TOOL.into(),
                    args: serde_json::json!({}),
                    meta: None,
                }),
                done(StopReason::ToolUse),
            ]))
        } else {
            Box::pin(futures::stream::iter([
                Ok(LlmEvent::TextDelta {
                    delta: ANSWER.into(),
                    meta: None,
                }),
                done(StopReason::EndTurn),
            ]))
        }
    }
}

#[async_trait]
impl LlmClient for ScriptedModel {
    fn plain_model_route(
        &self,
        logical_model: &str,
    ) -> Result<meerkat_llm_core::PlainModelRoute, meerkat_core::ControllerFactsUnavailable> {
        if logical_model != MODEL {
            return Err(meerkat_core::ControllerFactsUnavailable);
        }
        meerkat_llm_core::PlainModelRoute::new(ENDPOINT, MODEL)
    }

    fn controller_model_selection(&self) -> Option<ControllerModelSelection> {
        Some(selection())
    }

    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(&'a self, _: &'a LlmRequest) -> LlmStream<'a> {
        self.script()
    }

    fn stream_prepared<'a>(&'a self, _: &'a PreparedLlmRequest) -> LlmStream<'a> {
        self.script()
    }

    fn provider(&self) -> Provider {
        Provider::Anthropic
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

/// Records the requester each call's dispatch context carries.
#[derive(Default)]
struct RecordingTool {
    seen: Mutex<Vec<(Option<TurnRequester>, Option<PrincipalRef>)>>,
}

#[async_trait]
impl AgentToolDispatcher for RecordingTool {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::from([Arc::new(ToolDef::new(
            TOOL,
            "Records the run's requester",
            serde_json::json!({"type": "object"}),
        ))])
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        self.dispatch_with_context(call, &ToolDispatchContext::default())
            .await
    }

    async fn dispatch_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        if call.name != TOOL {
            return Err(ToolError::not_found(call.name));
        }
        self.seen.lock().expect("seen").push((
            context.requester().cloned(),
            context.direct_requester().cloned(),
        ));
        Ok(ToolResult::new(call.id.to_owned(), "recorded".to_owned(), false).into())
    }
}

#[derive(Default)]
struct SessionStore(Mutex<HashMap<SessionId, Session>>);

#[async_trait]
impl AgentSessionStore for SessionStore {
    async fn save(&self, session: &Session) -> Result<(), AgentError> {
        self.0
            .lock()
            .expect("session store")
            .insert(session.id().clone(), session.clone());
        Ok(())
    }

    async fn load(&self, id: &str) -> Result<Option<Session>, AgentError> {
        Ok(SessionId::parse(id)
            .ok()
            .and_then(|id| self.0.lock().expect("session store").get(&id).cloned()))
    }
}

struct Fixture {
    machine: Arc<MeerkatMachine>,
    service: Arc<EphemeralSessionService<FactoryAgentBuilder>>,
    session: SessionId,
    tool: Arc<RecordingTool>,
}

impl Fixture {
    async fn new() -> Self {
        let machine = Arc::new(MeerkatMachine::ephemeral());
        let model = Arc::new(ScriptedModel {
            requests: AtomicUsize::new(0),
        });
        let tool = Arc::new(RecordingTool::default());
        let mut config = Config::default();
        config.agent.max_turns = Some(4);
        let mut builder = FactoryAgentBuilder::new(AgentFactory::minimal(), config);
        builder.default_llm_client = Some(model);
        builder.default_tool_dispatcher = Some(tool.clone());
        builder.default_session_store = Some(Arc::new(SessionStore::default()));
        let service = Arc::new(EphemeralSessionService::new(builder, 2));
        let created = meerkat::surface::materialize_ephemeral_runtime_session(
            &service,
            &machine,
            CreateSessionRequest {
                model: MODEL.into(),
                prompt: "".into(),
                injected_context: Vec::new(),
                system_prompt: meerkat_core::config::SystemPromptOverride::Inherit,
                max_tokens: None,
                event_tx: None,
                initial_turn: InitialTurnPolicy::Defer,
                deferred_prompt_policy: DeferredPromptPolicy::Discard,
                build: None,
                labels: None,
            },
            false,
        )
        .await
        .unwrap();
        let session = created.session_id;
        let actor = service.live_session_actor_witness(&session).await.unwrap();
        let pin = service
            .pin_controller_client_for_actor(&actor)
            .await
            .unwrap();
        let owner = machine.generated_auth_lease_handle();
        meerkat_core::publish_token_lifecycle_acquired_for_identity_within(
            &owner,
            pin.selection().credential(),
            &meerkat_core::auth::PersistedTokens::api_key("synthetic-run-requester-credential"),
            tokio::time::Instant::now() + CREDENTIAL_PUBLICATION_WAIT,
        )
        .await
        .unwrap();
        Self {
            machine,
            service,
            session,
            tool,
        }
    }

    async fn close(self) {
        self.machine.retire_runtime(&self.session).await.unwrap();
        self.service.try_shutdown().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_requester_reaches_tool_dispatch() {
    tokio::time::timeout(TURN_DEADLINE, async {
        let fixture = Fixture::new().await;
        let mut prompt = PromptInput::new(PROMPT, None);
        prompt.header.requester = Some(person("a"));
        let (accepted, completion) = fixture
            .machine
            .accept_input_with_completion(&fixture.session, Input::Prompt(prompt))
            .await
            .unwrap();
        assert!(accepted.is_accepted());
        let outcome = completion
            .expect("completion handle")
            .wait()
            .await
            .expect("turn completion");
        let CompletionOutcome::Completed(result) = outcome else {
            panic!("the turn must complete normally")
        };
        assert_eq!(result.text, ANSWER);

        assert_eq!(
            *fixture.tool.seen.lock().expect("seen"),
            vec![(Some(TurnRequester::direct(person("a"))), Some(person("a")))],
            "the tool's dispatch context carries the run's direct requester"
        );

        let history = fixture
            .service
            .read_history(
                &fixture.session,
                SessionHistoryQuery {
                    offset: 0,
                    limit: None,
                },
            )
            .await
            .expect("transcript read");
        let user = history
            .messages
            .iter()
            .find_map(|message| match message {
                Message::User(user) if user.text_content() == PROMPT => Some(user),
                _ => None,
            })
            .expect("the turn's user row");
        assert_eq!(
            user.identity.requester,
            Some(TurnRequester::direct(person("a"))),
            "the transcript names the turn's requester"
        );
        fixture.close().await;
    })
    .await
    .expect("the turn completes in time");
}
