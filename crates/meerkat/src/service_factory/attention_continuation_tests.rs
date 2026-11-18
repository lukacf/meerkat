//! A stored WorkGraph attention continuation on the runtime-backed executor,
//! through the real agent build and the runtime: a continuation whose
//! projection the store no longer holds as current is refused when it is
//! applied, calls the model zero times and ends abandoned.

use super::*;
use async_trait::async_trait;
use futures::stream;
use meerkat_client::{LlmDoneOutcome, LlmEvent, LlmRequest};
use meerkat_core::service::{DeferredPromptPolicy, InitialTurnPolicy};
use meerkat_core::{AgentError, Provider, SessionId};
use meerkat_runtime::completion::CompletionOutcome;
use meerkat_runtime::{InMemoryRuntimeStore, Input, MeerkatMachine};
use meerkat_session::PersistentSessionService;
use meerkat_store::{MemoryBlobStore, MemoryStore};
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tempfile::TempDir;

const DEADLINE: Duration = Duration::from_secs(20);

/// A model that counts its calls and answers each with one word.
#[derive(Default)]
struct CountingModel {
    calls: AtomicUsize,
}

#[async_trait]
impl LlmClient for CountingModel {
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
        self.calls.fetch_add(1, Ordering::SeqCst);
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

/// The ordinary factory build with only the model client overridden.
struct CountingModelBuilder {
    factory: AgentFactory,
    client: Arc<CountingModel>,
}

#[async_trait]
impl SessionAgentBuilder for CountingModelBuilder {
    type Agent = FactoryAgent;

    fn session_comms_identity_root(&self) -> meerkat_core::SessionCommsIdentityRoot {
        self.factory.session_comms_identity_root()
    }

    async fn build_agent(
        &self,
        request: &CreateSessionRequest,
        event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<FactoryAgent, SessionError> {
        let mut config = AgentBuildConfig::from_create_session_request(request, event_tx);
        config.llm_client_override = Some(self.client.clone());
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

/// A persistent session service on a machine, a WorkGraph, and one session
/// served by the runtime-backed executor that is given that WorkGraph.
struct Fixture {
    _temp: TempDir,
    service: Arc<PersistentSessionService<CountingModelBuilder>>,
    machine: Arc<MeerkatMachine>,
    client: Arc<CountingModel>,
    workgraph_service: crate::WorkGraphService,
    session_id: SessionId,
}

impl Fixture {
    async fn new() -> Self {
        let temp = TempDir::new().expect("temp");
        let store = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(InMemoryRuntimeStore::new());
        let blob_store = Arc::new(MemoryBlobStore::new());
        let machine = Arc::new(
            MeerkatMachine::persistent(runtime_store.clone(), blob_store.clone())
                .expect("canonical machine"),
        );
        let client = Arc::new(CountingModel::default());
        let builder = CountingModelBuilder {
            factory: AgentFactory::new(temp.path().join("sessions")).session_store(store.clone()),
            client: client.clone(),
        };
        let service = Arc::new(
            PersistentSessionService::new(builder, 4, store, runtime_store, blob_store)
                .with_canonical_runtime_adapter(machine.clone()),
        );
        let workgraph_service =
            crate::WorkGraphService::new(Arc::new(crate::MemoryWorkGraphStore::new()));
        let request = CreateSessionRequest {
            model: "claude-sonnet-4-5".into(),
            prompt: "a session a goal is bound to".into(),
            injected_context: Vec::new(),
            system_prompt: meerkat_core::SystemPromptOverride::Inherit,
            max_tokens: None,
            event_tx: None,
            initial_turn: InitialTurnPolicy::Defer,
            deferred_prompt_policy: DeferredPromptPolicy::default(),
            build: None,
            labels: None,
        };
        let created = Box::pin(crate::surface::materialize_session(
            &service,
            &machine,
            meerkat_core::Session::new(),
            request,
            {
                let service = service.clone();
                let machine = machine.clone();
                let workgraph_service = workgraph_service.clone();
                move |id| {
                    crate::surface::default_persistent_executor_with_workgraph_service(
                        service,
                        machine,
                        id,
                        workgraph_service,
                    )
                }
            },
        ))
        .await
        .expect("materialize the session and its executor");
        Self {
            _temp: temp,
            service,
            machine,
            client,
            workgraph_service,
            session_id: created.session_id,
        }
    }

    /// A goal whose attention is bound to the fixture's session.
    async fn bound_goal(&self) -> crate::GoalCreateResult {
        self.workgraph_service
            .create_goal(crate::GoalCreateRequest {
                failed_child_join_policy: Default::default(),
                cancelled_child_join_policy: Default::default(),
                priority: Default::default(),
                labels: Default::default(),
                due_at: None,
                not_before: None,
                snoozed_until: None,
                external_refs: Vec::new(),
                evidence_refs: Vec::new(),
                status: None,
                realm_id: None,
                namespace: None,
                title: "A goal the session is bound to".to_string(),
                description: None,
                target: crate::GoalAttentionTarget::Session {
                    session_id: self.session_id.clone(),
                },
                mode: crate::WorkAttentionMode::Pursue,
                completion_policy: crate::WorkCompletionPolicy::SelfAttest,
                delegated_authority: Default::default(),
                projection_policy: Default::default(),
            })
            .await
            .expect("create the session-bound goal")
    }

    /// Hand the session `input` and wait for its end.
    async fn handed(&self, input: Input) -> CompletionOutcome {
        let (outcome, completion) = self
            .machine
            .accept_input_with_completion(&self.session_id, input)
            .await
            .expect("the runtime answers the hand-over");
        assert!(
            outcome.is_accepted(),
            "the continuation is admitted: {outcome:?}"
        );
        tokio::time::timeout(DEADLINE, completion.expect("a completion").wait())
            .await
            .expect("the input settles")
            .expect("the completion resolves")
    }
}

/// A stored attention continuation whose binding the host stopped after
/// its projection was made calls the model zero times on the runtime-backed
/// executor: it is refused when it is applied and ends abandoned, with the
/// binding named to the one who waits.
#[tokio::test]
async fn a_stored_attention_continuation_of_a_stopped_binding_calls_no_model() {
    let fixture = Fixture::new().await;
    let goal = fixture.bound_goal().await;
    let projection = fixture
        .workgraph_service
        .attention_projection(crate::AttentionProjectionRequest {
            binding_id: goal.attention.binding_id.clone(),
            realm_id: None,
            namespace: None,
        })
        .await
        .expect("the binding's projection")
        .projection;
    let continuation = crate::surface::workgraph_attention_continuation(&projection)
        .expect("build the continuation");

    fixture
        .workgraph_service
        .close(crate::CloseWorkItemRequest {
            id: goal.item.id.clone(),
            realm_id: None,
            namespace: None,
            expected_revision: goal.item.revision,
            status: crate::WorkStatus::Cancelled,
        })
        .await
        .expect("the host closes the bound item");

    let ended = fixture.handed(Input::Continuation(continuation)).await;
    let calls = fixture.client.calls.load(Ordering::SeqCst);
    let told = format!("{ended:?}");
    let binding = goal.attention.binding_id.to_string();
    assert!(
        calls == 0
            && matches!(
                ended,
                CompletionOutcome::Abandoned { .. } | CompletionOutcome::AbandonedWithError { .. }
            )
            && told.contains(&binding),
        "a continuation of a stopped binding calls no model and ends abandoned, naming the \
         binding {binding}: model calls {calls}, ended {told}"
    );
    fixture.service.try_shutdown().await.expect("shutdown");
}
