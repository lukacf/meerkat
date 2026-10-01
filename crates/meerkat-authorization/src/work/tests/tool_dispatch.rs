//! Actual core dispatch and Agent-loop tests with a test-only admitted-work owner.
//! These exercise native tool entry, not a production ingress admission claim.

use std::sync::Condvar;

use async_trait::async_trait;
use meerkat_core::agent::{
    AgentLlmRequestAttempt, RequestAttemptAuthority, ToolDispatchContext,
    dispatch_tool_execution_plan_fenced, resolve_tool_execution_plan_fenced,
};
use meerkat_core::authorization::{
    ModelAuthorizationFacts, ModelAuthorizationUse, ToolAuthorizationTarget,
    WorkAuthorizationContext,
};
use meerkat_core::lifecycle::run_primitive::ProviderParamsOverride;
use meerkat_core::tool_consequence_policy::{
    BoundToolConsequencePolicy, PolicyDigest, PolicyEvaluationProvenance,
    PolicyEvaluationSupervisorConfig, PolicyId, PolicyProviderGeneration, PolicyProviderId,
    PolicyRevision, ToolConsequenceFailure, ToolConsequenceNarrowingPolicy,
    ToolConsequencePolicyRegistry, ToolConsequencePolicySnapshot, ToolConsequenceRequest,
    ToolConsequenceVerdict,
};
use meerkat_core::tool_execution_policy::{ExecutionPolicyGatedDispatcher, ToolExecutionPolicy};
use meerkat_core::{
    AgentError, AgentEvent, AgentLlmClient, AgentSessionStore, AgentToolDispatcher, AssistantBlock,
    Config, LlmStreamResult, Message, MobMemberBinding, Provider, Session, StopReason,
    ToolCallView, ToolDef, ToolDispatchOutcome, ToolError, ToolExecutionResolutionContext,
    ToolResult,
};
use serde::Deserialize;
use serde_json::value::RawValue;
use tokio::sync::{Notify, mpsc};

use super::*;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RecordArguments {
    namespace: String,
}

struct ToolPolicyOwner {
    admitted: Arc<InputAuthorityAssociation>,
    native_scope: OperationExecutionScope,
    relation: ExactOperationRelation,
    revoked: AtomicBool,
    calls: AtomicUsize,
}

impl LocalWorkPolicy for ToolPolicyOwner {
    fn evaluate(
        &self,
        association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        _now_ms: u64,
    ) -> Result<LocalPolicyAllowance, meerkat_core::OperationAuthorizationError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        if association != self.admitted.as_ref()
            || binding.facts().execution_scope != self.native_scope
            || self.revoked.load(Ordering::Relaxed)
        {
            return Err(denied().into());
        }
        // This fixture owns exactly one bare in-process controller route. It
        // does not broaden the tool relation or admit arbitrary provider facts.
        if let AuthorizationOperation::Model(model) = &binding.facts().operation {
            let expected = fixture_controller_target();
            if model.identity.as_ref() != expected.identity.as_ref()
                || model.wire_model != expected.wire_model
                || model.backend_profile_id != expected.backend_profile_id
                || model.backend_kind != expected.backend_kind
                || model.endpoint != expected.endpoint
                || model.credential.is_some()
                || !model.hosted_capabilities.is_empty()
                || !matches!(model.usage, ModelAuthorizationUse::Inference)
                || model.live_channel.is_some()
            {
                return Err(denied().into());
            }
            return Ok(LocalPolicyAllowance {
                operation_values: vec![tuple("infer", "fixture-controller")],
                restrictions: ExecutionRestrictions::unrestricted(),
                expires_at_ms: 2_000,
            });
        }
        let AuthorizationOperation::Tool(tool) = &binding.facts().operation else {
            return Err(denied().into());
        };
        if !matches!(tool.target, ToolAuthorizationTarget::Dispatcher(_)) {
            return Err(denied().into());
        }
        let action = match tool.name.as_str() {
            "read_record" => "read",
            "write_record" => "write",
            _ => return Err(denied().into()),
        };
        let arguments: RecordArguments =
            serde_json::from_str(tool.arguments.get()).map_err(|_| malformed())?;
        let actual = tuple(action, &arguments.namespace);
        if !self.relation.contains(&actual) {
            return Err(denied().into());
        }
        Ok(LocalPolicyAllowance {
            operation_values: vec![actual],
            restrictions: ExecutionRestrictions::unrestricted(),
            expires_at_ms: 2_000,
        })
    }
}

struct ToolWork {
    context: WorkAuthorizationContext,
    owner: Arc<ToolPolicyOwner>,
    publication: LocalAuthorizationPublication,
}

impl ToolWork {
    fn for_session(session_id: &SessionId) -> Self {
        let admitted = Arc::new(association(ExecutionRestrictions::unrestricted()));
        let mut native_scope = scope();
        if let OperationExecutionScope::RuntimeInput {
            owner_session_id, ..
        } = &mut native_scope
        {
            *owner_session_id = session_id.clone();
        }
        // This explicit fixture owner retains the accepted full association and
        // the fixture's accepted input identity for the actual session. It never admits wire data
        // merely because it can be decoded by the compiler.
        let owner = Arc::new(ToolPolicyOwner {
            admitted: admitted.clone(),
            native_scope: native_scope.clone(),
            relation: ExactOperationRelation::new(vec![
                tuple("read", "public"),
                tuple("write", "private"),
            ]),
            revoked: AtomicBool::new(false),
            calls: AtomicUsize::new(0),
        });
        let publication = LocalAuthorizationPublication::new();
        let authorization = Arc::new(LocalWorkAuthorization::new(
            admitted,
            owner.clone(),
            publication.clone(),
            Arc::new(FixtureClock::new()),
        ));
        Self {
            context: WorkAuthorizationContext::new(authorization, native_scope),
            owner,
            publication,
        }
    }

    fn revoke(&self) {
        let publication = self
            .publication
            .begin_owner_change()
            .expect("owner publication");
        self.owner.revoked.store(true, Ordering::Relaxed);
        drop(publication);
    }
}

struct Call {
    id: String,
    name: String,
    args: Box<RawValue>,
}

impl Call {
    fn new(id: &str, name: &str, namespace: &str) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            args: RawValue::from_string(serde_json::json!({ "namespace": namespace }).to_string())
                .expect("arguments"),
        }
    }

    fn view(&self) -> ToolCallView<'_> {
        ToolCallView {
            id: &self.id,
            name: &self.name,
            args: &self.args,
        }
    }
}

#[derive(Default)]
struct RecordingDispatcher {
    bodies: Mutex<Vec<(String, String, String)>>,
    entered_contexts: Mutex<Vec<ToolDispatchContext>>,
}

#[async_trait]
impl AgentToolDispatcher for RecordingDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        ["read_record", "write_record"]
            .into_iter()
            .map(|name| {
                Arc::new(ToolDef::new(
                    name,
                    "fixture record operation",
                    serde_json::json!({
                        "type": "object",
                        "properties": { "namespace": { "type": "string" } },
                        "required": ["namespace"],
                        "additionalProperties": false,
                    }),
                ))
            })
            .collect()
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        self.bodies.lock().expect("recorded bodies").push((
            call.id.into(),
            call.name.into(),
            call.args.get().into(),
        ));
        Ok(ToolResult::new(call.id.into(), "executed".into(), false).into())
    }

    async fn dispatch_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        self.entered_contexts
            .lock()
            .expect("entered contexts")
            .push(context.clone());
        self.dispatch(call).await
    }
}

async fn dispatch(
    dispatcher: &Arc<ExecutionPolicyGatedDispatcher<RecordingDispatcher>>,
    call: &Call,
    context: &ToolDispatchContext,
) -> Result<ToolDispatchOutcome, ToolError> {
    let plan = resolve_tool_execution_plan_fenced(
        dispatcher,
        call.view(),
        context,
        &ToolExecutionResolutionContext::new(
            meerkat_core::ToolDeadlineChain::new(vec![
                meerkat_core::ToolDeadlineContributor::finite(
                    meerkat_core::ToolDeadlineOwner::DirectCaller,
                    Duration::from_secs(5),
                ),
            ])
            .expect("finite caller deadline"),
        ),
    )
    .expect("real plan resolution");
    dispatch_tool_execution_plan_fenced(dispatcher, call.view(), context, &plan).await
}

fn assert_tool_refused(result: Result<ToolDispatchOutcome, ToolError>, kind: OperationRefusalKind) {
    assert!(
        matches!(result, Err(ToolError::AuthorizationRefused { refusal }) if refusal.kind() == kind)
    );
}

#[tokio::test]
async fn real_dispatch_refuses_crossed_permission_and_runs_permitted_sibling() {
    let work = ToolWork::for_session(&SessionId::new());
    let body = Arc::new(RecordingDispatcher::default());
    let dispatcher = Arc::new(ExecutionPolicyGatedDispatcher::new(
        body.clone(),
        ToolExecutionPolicy::unrestricted(),
    ));
    let context =
        ToolDispatchContext::default().with_work_authorization(Some(work.context.clone()));
    let denied_call = Call::new("denied", "read_record", "private");
    assert_tool_refused(
        dispatch(&dispatcher, &denied_call, &context).await,
        OperationRefusalKind::Denied,
    );
    assert!(body.bodies.lock().expect("bodies").is_empty());
    let allowed = Call::new("allowed", "write_record", "private");
    dispatch(&dispatcher, &allowed, &context)
        .await
        .expect("permitted sibling");
    let bodies = body.bodies.lock().expect("bodies");
    assert_eq!(bodies.len(), 1);
    assert_eq!(bodies[0].0, "allowed");
    assert_eq!(bodies[0].1, "write_record");
    assert_eq!(work.owner.calls.load(Ordering::Relaxed), 2);
}

#[tokio::test]
async fn captured_legitimate_context_cannot_authorize_retargeted_or_reallocated_call() {
    let work = ToolWork::for_session(&SessionId::new());
    let body = Arc::new(RecordingDispatcher::default());
    let dispatcher = Arc::new(ExecutionPolicyGatedDispatcher::new(
        body.clone(),
        ToolExecutionPolicy::unrestricted(),
    ));
    let context =
        ToolDispatchContext::default().with_work_authorization(Some(work.context.clone()));
    let allowed = Call::new("allowed", "read_record", "public");
    dispatch(&dispatcher, &allowed, &context)
        .await
        .expect("legitimate entry");
    let captured = body.entered_contexts.lock().expect("contexts")[0].clone();
    // An extension can retain a legitimate context, but cannot attach it to
    // equal-looking allocations or another call and inherit its authorization.
    for attempted in [
        Call::new("allowed", "read_record", "public"),
        Call::new("retargeted", "read_record", "private"),
    ] {
        assert_tool_refused(
            dispatcher
                .dispatch_with_context(attempted.view(), &captured)
                .await,
            OperationRefusalKind::MalformedFacts,
        );
    }
    let replaced_work = WorkAuthorizationContext::new(
        work.context.authorization().clone(),
        work.context.execution_scope().clone(),
    );
    assert!(!replaced_work.same_context(&work.context));
    let replaced_context = captured
        .clone()
        .with_work_authorization(Some(replaced_work));
    assert_tool_refused(
        dispatcher
            .dispatch_with_context(allowed.view(), &replaced_context)
            .await,
        OperationRefusalKind::MalformedFacts,
    );
    assert_eq!(body.bodies.lock().expect("bodies").len(), 1);
    assert_eq!(work.owner.calls.load(Ordering::Relaxed), 1);
}

struct PausedConsequence {
    entered: Notify,
    // Condvar requires the predicate and sleep to share this mutex.
    #[allow(clippy::mutex_atomic)]
    release: Mutex<bool>,
    wake: Condvar,
}

impl PausedConsequence {
    fn resume(&self) {
        *self.release.lock().expect("consequence release") = true;
        self.wake.notify_all();
    }
}

impl ToolConsequencePolicySnapshot for PausedConsequence {
    fn provenance(&self) -> PolicyEvaluationProvenance {
        PolicyEvaluationProvenance {
            revision: PolicyRevision(1),
            digest: PolicyDigest::from_canonical_bytes(b"allow-after-explicit-test-release"),
        }
    }

    fn evaluate(&self, _request: &ToolConsequenceRequest) -> ToolConsequenceVerdict {
        self.entered.notify_one();
        let released = self.release.lock().expect("consequence release");
        let (released, _) = self
            .wake
            .wait_timeout_while(released, Duration::from_secs(3), |released| !*released)
            .expect("bounded consequence wait");
        assert!(*released, "controller must release the consequence worker");
        ToolConsequenceVerdict::Allow
    }
}

struct ConsequenceProvider {
    id: PolicyProviderId,
    snapshot: Arc<PausedConsequence>,
}

impl ToolConsequenceNarrowingPolicy for ConsequenceProvider {
    fn provider_id(&self) -> &PolicyProviderId {
        &self.id
    }
    fn generation(&self) -> PolicyProviderGeneration {
        PolicyProviderGeneration(1)
    }
    fn snapshot(
        &self,
        _policy_id: &PolicyId,
    ) -> Result<Arc<dyn ToolConsequencePolicySnapshot>, ToolConsequenceFailure> {
        Ok(self.snapshot.clone())
    }
}

fn consequence(snapshot: Arc<PausedConsequence>) -> BoundToolConsequencePolicy {
    let id = PolicyProviderId::new("fixture-consequence").expect("provider id");
    let provider = Arc::new(ConsequenceProvider {
        id: id.clone(),
        snapshot,
    });
    Arc::new(
        ToolConsequencePolicyRegistry::new(
            vec![provider],
            PolicyEvaluationSupervisorConfig {
                workers_per_provider: 1,
                queue_capacity_per_provider: 1,
                evaluation_deadline: Duration::from_secs(4),
            },
            None,
        )
        .expect("consequence registry"),
    )
    .bind(
        MobMemberBinding {
            mob_id: "fixture".into(),
            role: "worker".into(),
            member: "one".into(),
        },
        id,
        PolicyId::new("pause-then-allow").expect("policy id"),
    )
    .expect("consequence binding")
}

#[tokio::test]
async fn revocation_during_actual_awaited_consequence_check_prevents_body_entry() {
    let work = ToolWork::for_session(&SessionId::new());
    let paused = Arc::new(PausedConsequence {
        entered: Notify::new(),
        release: Mutex::new(false),
        wake: Condvar::new(),
    });
    let body = Arc::new(RecordingDispatcher::default());
    let dispatcher = Arc::new(
        ExecutionPolicyGatedDispatcher::new(body.clone(), ToolExecutionPolicy::unrestricted())
            .with_consequence_policy(consequence(paused.clone())),
    );
    let context =
        ToolDispatchContext::default().with_work_authorization(Some(work.context.clone()));
    let call = Call::new("paused", "read_record", "public");
    let controller = async {
        let entered = tokio::time::timeout(Duration::from_secs(2), paused.entered.notified()).await;
        if entered.is_ok() {
            assert!(body.bodies.lock().expect("bodies").is_empty());
            assert_eq!(work.owner.calls.load(Ordering::Relaxed), 1);
            work.revoke();
        }
        // Release even if the assertion below fails, so a failed test cannot
        // strand the supervisor's blocking worker.
        paused.resume();
        entered.expect("consequence evaluation really suspended dispatch");
    };
    let (result, ()) = tokio::join!(dispatch(&dispatcher, &call, &context), controller);
    assert_tool_refused(result, OperationRefusalKind::Denied);
    assert!(body.bodies.lock().expect("bodies").is_empty());
    assert_eq!(
        work.owner.calls.load(Ordering::Relaxed),
        2,
        "stale check re-prepares once"
    );
}

#[derive(Default)]
struct RecordingStore(Mutex<Vec<Session>>);

#[async_trait]
impl AgentSessionStore for RecordingStore {
    async fn save(&self, session: &Session) -> Result<(), AgentError> {
        self.0.lock().expect("saved sessions").push(session.clone());
        Ok(())
    }
    async fn load(&self, id: &str) -> Result<Option<Session>, AgentError> {
        let Ok(id) = SessionId::parse(id) else {
            return Ok(None);
        };
        Ok(self
            .0
            .lock()
            .expect("saved sessions")
            .iter()
            .rev()
            .find(|session| session.id() == &id)
            .cloned())
    }
}

fn fixture_controller_target() -> ModelAuthorizationFacts {
    ModelAuthorizationFacts {
        identity: Arc::new(meerkat_core::SessionLlmIdentity {
            model: "claude-sonnet-4-5".into(),
            provider: Provider::Anthropic,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        }),
        wire_model: Arc::from("claude-sonnet-4-5"),
        hosted_capabilities: Arc::from([]),
        backend_profile_id: None,
        backend_kind: Arc::from("in_process_fixture"),
        endpoint: Arc::from("in-process://sibling-client"),
        credential: None,
        usage: ModelAuthorizationUse::Inference,
        live_channel: None,
    }
}

struct SiblingAttempt {
    client: Arc<SiblingClient>,
    messages: Arc<Vec<Message>>,
    tools: Arc<[Arc<ToolDef>]>,
    max_tokens: u32,
    temperature: Option<f32>,
    provider_params: Option<ProviderParamsOverride>,
    authorization: Option<meerkat_core::LlmRequestAuthorization>,
}

#[async_trait]
impl AgentLlmRequestAttempt for SiblingAttempt {
    fn request_pressure(
        &self,
    ) -> Result<Option<meerkat_core::ProviderRequestPressure>, AgentError> {
        Ok(None)
    }

    async fn stream_response(
        &self,
        _assistant_message_id: meerkat_core::AssistantMessageId,
    ) -> Result<LlmStreamResult, AgentError> {
        self.client
            .stream_response_authorized(
                &self.messages,
                &self.tools,
                self.max_tokens,
                self.temperature,
                self.provider_params.as_ref(),
                self.authorization.clone(),
            )
            .await
    }
}

#[derive(Default)]
struct SiblingClient(Mutex<Vec<Vec<Message>>>);

#[async_trait]
impl AgentLlmClient for SiblingClient {
    fn prepare_request_attempt_authorized(
        self: Arc<Self>,
        messages: Arc<Vec<Message>>,
        tools: Arc<[Arc<ToolDef>]>,
        max_tokens: u32,
        temperature: Option<f32>,
        provider_params: Option<ProviderParamsOverride>,
        authorization: Option<meerkat_core::LlmRequestAuthorization>,
    ) -> Result<Arc<dyn AgentLlmRequestAttempt>, AgentError> {
        Ok(Arc::new(SiblingAttempt {
            client: self,
            messages,
            tools,
            max_tokens,
            temperature,
            provider_params,
            authorization,
        }))
    }

    fn request_attempt_authority(&self) -> RequestAttemptAuthority {
        RequestAttemptAuthority::Unified
    }

    async fn stream_response_authorized(
        &self,
        messages: &[Message],
        tools: &[Arc<ToolDef>],
        max_tokens: u32,
        temperature: Option<f32>,
        provider_params: Option<&ProviderParamsOverride>,
        authorization: Option<meerkat_core::LlmRequestAuthorization>,
    ) -> Result<LlmStreamResult, AgentError> {
        let prepared = authorization
            .as_ref()
            .map(|authorization| authorization.prepare(fixture_controller_target()))
            .transpose()
            .map_err(AgentError::from)?;
        let _current = prepared
            .as_ref()
            .map(|prepared| prepared.current())
            .transpose()
            .map_err(AgentError::from)?;
        self.stream_response(messages, tools, max_tokens, temperature, provider_params)
            .await
    }

    async fn stream_response(
        &self,
        messages: &[Message],
        _tools: &[Arc<ToolDef>],
        _max_tokens: u32,
        _temperature: Option<f32>,
        _provider_params: Option<&ProviderParamsOverride>,
    ) -> Result<LlmStreamResult, AgentError> {
        let mut requests = self.0.lock().expect("model requests");
        let first = requests.is_empty();
        requests.push(messages.to_vec());
        let blocks = if first {
            [
                ("denied", "read_record", "private"),
                ("allowed", "write_record", "private"),
            ]
            .into_iter()
            .map(|(id, name, namespace)| {
                let call = Call::new(id, name, namespace);
                AssistantBlock::ToolUse {
                    id: call.id,
                    name: call.name,
                    args: call.args,
                    meta: None,
                }
            })
            .collect()
        } else {
            vec![AssistantBlock::Text {
                text: "continued after the tool refusal".into(),
                meta: None,
            }]
        };
        Ok(LlmStreamResult::new(
            blocks,
            if first {
                StopReason::ToolUse
            } else {
                StopReason::EndTurn
            },
            meerkat_core::TurnUsage::host_declared(
                self.provider(),
                self.model(),
                Default::default(),
            )
            .into_inner(),
        ))
    }
    fn provider(&self) -> Provider {
        Provider::Anthropic
    }
    fn model(&self) -> &str {
        "claude-sonnet-4-5"
    }
}

#[tokio::test]
async fn factory_agent_preserves_run_and_model_continuation_after_local_tool_refusal() {
    let client = Arc::new(SiblingClient::default());
    let body = Arc::new(RecordingDispatcher::default());
    let store = Arc::new(RecordingStore::default());
    let mut agent = meerkat::AgentFactory::minimal()
        .build_agent(
            meerkat::AgentBuildConfig {
                agent_llm_client_override: Some(client.clone()),
                tool_dispatcher_override: Some(body.clone()),
                session_store_override: Some(store.clone()),
                ..meerkat::AgentBuildConfig::new("claude-sonnet-4-5")
            },
            &Config::default(),
        )
        .await
        .expect("real factory with explicit host resources");
    let work = ToolWork::for_session(agent.session().id());
    let (events, mut received) = mpsc::channel(256);
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        agent.run_with_events_and_work_authorization(
            "perform both fixture operations".to_string().into(),
            vec![],
            vec![],
            None,
            events,
            Some(work.context.clone()),
        ),
    )
    .await
    .expect("bounded actual agent run")
    .expect("operation refusal is not a run failure");
    assert_eq!(result.turns, 2);
    let requests = client.0.lock().expect("model requests");
    assert_eq!(
        requests.len(),
        2,
        "agent must send a follow-up model request"
    );
    let results: Vec<_> = requests[1]
        .iter()
        .flat_map(|message| match message {
            Message::ToolResults { results, .. } => results.as_slice(),
            _ => &[],
        })
        .collect();
    assert_eq!(results.len(), 2);
    let denied = results
        .iter()
        .find(|result| result.tool_use_id == "denied")
        .expect("denied result");
    assert!(denied.is_error);
    let denied_text = meerkat_core::types::text_content(&denied.content);
    assert!(denied_text.contains("operation_refused"));
    for private_fact in ["fixture-domain", "resource-owner", "private"] {
        assert!(!denied_text.contains(private_fact));
    }
    let allowed = results
        .iter()
        .find(|result| result.tool_use_id == "allowed")
        .expect("allowed result");
    assert!(!allowed.is_error);
    let bodies = body.bodies.lock().expect("bodies");
    assert_eq!(bodies.len(), 1);
    assert_eq!(bodies[0].0, "allowed");
    assert!(!store.0.lock().expect("saved sessions").is_empty());
    let mut completed = 0;
    while let Ok(event) = received.try_recv() {
        assert!(!matches!(event, AgentEvent::RunFailed { .. }));
        if matches!(event, AgentEvent::RunCompleted { .. }) {
            completed += 1;
        }
    }
    assert_eq!(completed, 1);
}

#[tokio::test]
async fn fixture_controller_checks_current_work_before_recording_model_input() {
    let work = ToolWork::for_session(&SessionId::new());
    let client = SiblingClient::default();
    let authorization = meerkat_core::LlmRequestAuthorization::new(
        work.context.clone(),
        meerkat_core::OperationId::new(),
        ModelAuthorizationUse::Inference,
    );
    let mut wrong_target = fixture_controller_target();
    wrong_target.endpoint = Arc::from("in-process://other-client");
    assert!(authorization.prepare(wrong_target).is_err());
    let mut hosted = fixture_controller_target();
    hosted.hosted_capabilities = Arc::from([meerkat_core::ServerToolKind::WebSearch]);
    assert!(authorization.prepare(hosted).is_err());
    work.revoke();
    assert!(matches!(
        client
            .stream_response_authorized(
                &[Message::User(meerkat_core::UserMessage::text(
                    "must not be recorded"
                ))],
                &[],
                100,
                None,
                None,
                Some(authorization),
            )
            .await,
        Err(AgentError::OperationRefused { .. })
    ));
    assert!(client.0.lock().expect("model requests").is_empty());
}
