//! Adapter boundary controls using the actual factory-selected OpenAI client
//! and local HTTP transport. The source and policy below are explicit test
//! owners, not proof of native original-input authentication or durable audit.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use axum::{Json, Router, extract::State, http::Uri, routing::post};
use futures::FutureExt;
use meerkat_core::approval::review::{
    BoundOperationReview, OperationReviewObserver, OperationReviewTier, ReviewAttemptStatus,
    ReviewEntrySupport, ReviewObservation, ReviewRetirementReason,
};
use meerkat_core::authorization::{
    AuthorizationOperation, ModelAuthorizationUse, OperationObservation, OperationObservationError,
    OperationRefusalKind, OperationRefused, PreparedAuthorizationBinding,
    PreparedOperationAuthorization, WorkAuthorization, WorkAuthorizationContext,
};
use meerkat_core::{
    AgentToolDispatcher, ApprovalService, AuthBindingRef, AuthCredentialIdentity, BindingId,
    BindingOrigin, Config, OperationExecutionScope, RealmConfigSection, RealmId, ToolCallView,
    ToolDeadlineChain, ToolDeadlineContributor, ToolDeadlineOwner, ToolDef, ToolDispatchContext,
    ToolDispatchOutcome, ToolError, ToolExecutionResolutionContext, ToolResult,
    dispatch_tool_execution_plan_fenced, resolve_tool_execution_plan_fenced,
};
use serde_json::{Value, json};

use super::*;

const MODEL: &str = "gpt-5.4";
const REVIEWED: &str = "reviewed_write";
const SIBLING: &str = "permitted_read";
const BOUND: Duration = Duration::from_secs(5);

struct Server {
    requests: Arc<Mutex<Vec<(String, Value)>>>,
    base_url: String,
    task: tokio::task::JoinHandle<()>,
}

impl Server {
    async fn start() -> Self {
        async fn reply(
            State(requests): State<Arc<Mutex<Vec<(String, Value)>>>>,
            uri: Uri,
            Json(body): Json<Value>,
        ) -> ([(&'static str, &'static str); 1], String) {
            requests.lock().unwrap().push((uri.path().to_owned(), body));
            let terminal = json!({
                "type": "response.completed",
                "response": {
                    "id": "review-response", "status": "completed",
                    "output": [{"type":"message", "role":"assistant", "content":[{
                        "type":"output_text", "text":"{\"verdict\":\"allow\"}"
                    }]}],
                    "usage": {"input_tokens": 1, "output_tokens": 1}
                }
            });
            (
                [("content-type", "text/event-stream")],
                format!("data: {terminal}\n\n"),
            )
        }
        let requests = Arc::new(Mutex::new(Vec::new()));
        let router = Router::new()
            .fallback(post(reply))
            .with_state(requests.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        // The OpenAI client appends `/v1/responses` to the backend base URL.
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        Self {
            requests,
            base_url,
            task,
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[derive(Clone, Copy)]
enum SourceAccess {
    Allowed,
    Unavailable,
    Denied,
    OversizeEncodedContext,
    NativeUnavailable,
}

struct TestSource {
    access: SourceAccess,
    calls: AtomicUsize,
    work: WorkAuthorizationContext,
    candidate: Arc<Mutex<Option<PreparedAuthorizationBinding>>>,
}

#[async_trait]
impl OperationReviewContextSource for TestSource {
    async fn read_context(
        &self,
        candidate: &ReviewCandidate<'_>,
    ) -> Result<ReviewContextMaterial, ReviewerFailure> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        assert!(candidate.work_authorization().same_context(&self.work));
        assert_eq!(candidate.tool_name(), REVIEWED);
        *self.candidate.lock().unwrap() = Some(candidate.binding().clone());
        match self.access {
            SourceAccess::Allowed => ReviewContextMaterial::from_text(
                "Test owner context: the original request permits this fixture write.".to_owned(),
            ),
            SourceAccess::OversizeEncodedContext => {
                ReviewContextMaterial::from_text("\0".repeat(MAX_REVIEW_CONTEXT_BYTES))
            }
            SourceAccess::Unavailable | SourceAccess::Denied | SourceAccess::NativeUnavailable => {
                Err(ReviewerFailure::Unavailable)
            }
        }
    }
}

#[derive(Clone, Copy)]
enum ModelPermission {
    Allowed,
    ModelDenied,
    ProcessorDenied,
}

struct TestPolicy {
    permission: ModelPermission,
    binding: AuthBindingRef,
    endpoint: String,
    candidate: Arc<Mutex<Option<PreparedAuthorizationBinding>>>,
    model_prepares: AtomicUsize,
    model_entries: Arc<AtomicUsize>,
    model_outcomes: Arc<AtomicUsize>,
    fail_model_outcome: bool,
}

struct Check {
    binding: PreparedAuthorizationBinding,
    tier: OperationReviewTier,
    model_entries: Option<Arc<AtomicUsize>>,
    model_outcomes: Arc<AtomicUsize>,
    fail_model_outcome: bool,
}

impl PreparedOperationAuthorization for Check {
    fn review_tier(&self) -> OperationReviewTier {
        self.tier
    }

    fn check_current(
        &self,
        binding: &PreparedAuthorizationBinding,
    ) -> Result<(), meerkat_core::OperationAuthorizationError> {
        if !self.binding.same_operation(binding) {
            return Err(OperationRefused::new(OperationRefusalKind::MalformedFacts).into());
        }
        Ok(())
    }

    fn observe(
        &self,
        binding: &PreparedAuthorizationBinding,
        observation: OperationObservation,
    ) -> Result<(), OperationObservationError> {
        if !self.binding.same_operation(binding) {
            return Err(OperationObservationError);
        }
        if let Some(entries) = &self.model_entries {
            match observation {
                OperationObservation::Entry => {
                    entries.fetch_add(1, Ordering::SeqCst);
                }
                OperationObservation::Outcome(outcome) => {
                    self.model_outcomes.fetch_add(1, Ordering::SeqCst);
                    if self.fail_model_outcome {
                        assert!(matches!(outcome,
                            meerkat_core::authorization::OperationObservedOutcome::HttpResponse { status: 200 }),
                            "inject only after the actual successful HTTP response headers");
                        return Err(OperationObservationError);
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }
}

impl WorkAuthorization for TestPolicy {
    fn prepare(
        &self,
        binding: &PreparedAuthorizationBinding,
    ) -> Result<Arc<dyn PreparedOperationAuthorization>, meerkat_core::OperationAuthorizationError>
    {
        let (tier, model_entries) = match &binding.facts().operation {
            AuthorizationOperation::Tool(tool) => (
                if tool.name.as_str() == REVIEWED {
                    OperationReviewTier::R2
                } else {
                    OperationReviewTier::R1
                },
                None,
            ),
            AuthorizationOperation::Model(model) => {
                self.model_prepares.fetch_add(1, Ordering::SeqCst);
                assert_eq!(model.identity.model, MODEL);
                assert_eq!(model.identity.auth_binding.as_ref(), Some(&self.binding));
                assert_eq!(
                    model.credential.as_ref(),
                    Some(&AuthCredentialIdentity::Binding(self.binding.clone()))
                );
                assert_eq!(model.endpoint.as_ref(), self.endpoint);
                assert!(model.hosted_capabilities.is_empty());
                assert!(model.usage == ModelAuthorizationUse::Inference);
                let candidate = self.candidate.lock().unwrap();
                let candidate = candidate
                    .as_ref()
                    .expect("context is read before model entry");
                assert_ne!(candidate.facts().operation_id, binding.facts().operation_id);
                assert_eq!(
                    candidate.facts().execution_scope,
                    binding.facts().execution_scope
                );
                assert_eq!(candidate.facts().run_id, binding.facts().run_id);
                assert_eq!(
                    candidate.facts().context_revision,
                    binding.facts().context_revision
                );
                let permitted_model = match self.permission {
                    ModelPermission::ModelDenied => "another-model",
                    _ => MODEL,
                };
                if model.identity.model != permitted_model {
                    return Err(OperationRefused::new(OperationRefusalKind::Denied).into());
                }
                let permitted_processor = match self.permission {
                    ModelPermission::ProcessorDenied => {
                        "https://another-processor.invalid/responses"
                    }
                    _ => self.endpoint.as_str(),
                };
                if model.endpoint.as_ref() != permitted_processor {
                    return Err(OperationRefused::new(OperationRefusalKind::Denied).into());
                }
                (OperationReviewTier::R1, Some(self.model_entries.clone()))
            }
            _ => return Err(OperationRefused::new(OperationRefusalKind::MalformedFacts).into()),
        };
        Ok(Arc::new(Check {
            binding: binding.clone(),
            tier,
            model_entries,
            model_outcomes: self.model_outcomes.clone(),
            fail_model_outcome: self.fail_model_outcome,
        }))
    }
}

#[derive(Default)]
struct Leaf(Mutex<Vec<String>>);

#[async_trait]
impl AgentToolDispatcher for Leaf {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        [REVIEWED, SIBLING]
            .into_iter()
            .map(|name| {
                Arc::new(ToolDef {
                    name: name.into(),
                    description: "test leaf".to_owned(),
                    input_schema: json!({"type":"object"}),
                    provenance: None,
                })
            })
            .collect::<Vec<_>>()
            .into()
    }
    fn review_entry_support(&self, _: &str) -> ReviewEntrySupport {
        ReviewEntrySupport::ConsumesAtEntry
    }
    async fn dispatch(&self, _: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        panic!("governed fixture must carry its context")
    }
    async fn dispatch_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        let _entering_context = context.enter_reviewed_effect(call, None)?;
        self.0.lock().unwrap().push(call.name.to_owned());
        Ok(ToolResult::new(call.id.to_owned(), call.name.to_owned(), false).into())
    }
}

async fn dispatch(
    leaf: &Arc<Leaf>,
    name: &str,
    context: &ToolDispatchContext,
) -> Result<ToolDispatchOutcome, ToolError> {
    let args = serde_json::value::RawValue::from_string("{}".to_owned()).unwrap();
    let call = ToolCallView {
        id: name,
        name,
        args: &args,
    };
    let resolution = ToolExecutionResolutionContext::new(
        ToolDeadlineChain::new(vec![ToolDeadlineContributor::finite(
            ToolDeadlineOwner::DirectCaller,
            BOUND,
        )])
        .unwrap(),
    );
    let plan = resolve_tool_execution_plan_fenced(leaf, call, context, &resolution).unwrap();
    tokio::time::timeout(
        BOUND,
        dispatch_tool_execution_plan_fenced(leaf, call, context, &plan),
    )
    .await
    .expect("bounded reviewer dispatch")
}

async fn scenario(source_access: SourceAccess, model_permission: ModelPermission, allowed: bool) {
    scenario_with_outcome_fault(source_access, model_permission, allowed, false).await;
}

async fn scenario_with_outcome_fault(
    source_access: SourceAccess,
    model_permission: ModelPermission,
    allowed: bool,
    fail_model_outcome: bool,
) {
    let server = Server::start().await;
    let binding = AuthBindingRef {
        realm: RealmId::parse("review_test").unwrap(),
        binding: BindingId::parse("default_openai").unwrap(),
        profile: None,
        origin: BindingOrigin::Configured,
    };
    let mut config = Config::default();
    let mut realm = RealmConfigSection::from_inline_api_keys(&[("openai", "test-key")]);
    realm.backend.get_mut("default_openai").unwrap().base_url = Some(server.base_url.clone());
    config.realm.insert("review_test".to_owned(), realm);
    let mut identity = identity(MODEL);
    identity.auth_binding = Some(binding.clone());
    let candidate = Arc::new(Mutex::new(None));
    let policy = Arc::new(TestPolicy {
        permission: model_permission,
        binding,
        endpoint: format!("{}/v1/responses", server.base_url),
        candidate: candidate.clone(),
        model_prepares: AtomicUsize::new(0),
        model_entries: Arc::new(AtomicUsize::new(0)),
        model_outcomes: Arc::new(AtomicUsize::new(0)),
        fail_model_outcome,
    });
    // These are explicit test-owner coordinates, not proof of a live runtime
    // admission. Public fenced dispatch must preserve the complete scope.
    let reaches_model = allowed || fail_model_outcome;
    let scope = if reaches_model {
        let input_id = meerkat_core::InputId::new();
        OperationExecutionScope::RuntimeInput {
            owner_session_id: meerkat_core::SessionId::new(),
            runtime_epoch_id: meerkat_core::RuntimeEpochId::new(),
            submitted_input_id: input_id.clone(),
            canonical_input_id: input_id,
        }
    } else {
        OperationExecutionScope::Domain
    };
    let work = WorkAuthorizationContext::new(policy.clone(), scope);
    let source = Arc::new(TestSource {
        access: source_access,
        calls: AtomicUsize::new(0),
        work: work.clone(),
        candidate,
    });
    let factory = crate::AgentFactory::minimal().without_provider_auth_persistence();
    if reaches_model {
        let inherited = factory
            .request_policy_for_llm_identity(
                &config,
                &identity,
                meerkat_core::ToolCategoryOverride::Inherit,
            )
            .unwrap();
        assert!(
            matches!(inherited.provider_tool_defaults,
            Some(meerkat_core::lifecycle::run_primitive::ProviderTag::OpenAi(tag))
                if tag.web_search.is_some()),
            "fixture must otherwise enable a provider-hosted tool"
        );
    }
    let selected_source: Arc<dyn OperationReviewContextSource> = match source_access {
        SourceAccess::NativeUnavailable => Arc::new(NativeOperationReviewContextSource),
        _ => source.clone(),
    };
    let reviewer = ModelOperationReviewer::build(
        &factory,
        &config,
        ModelReviewerConfig::new(identity, 73).unwrap(),
        selected_source,
    )
    .await
    .expect("real configured factory reviewer");
    let observations = Arc::new(Observations::default());
    let review = Arc::new(
        BoundOperationReview::new(Arc::new(reviewer), ApprovalService::new(), BOUND)
            .with_observer(observations.clone()),
    );
    let context = ToolDispatchContext::default()
        .with_work_authorization(Some(work))
        .with_operation_review(Some(review));
    let leaf = Arc::new(Leaf::default());
    let outcome = dispatch(&leaf, REVIEWED, &context).await;
    assert_eq!(outcome.is_ok(), allowed);
    if fail_model_outcome {
        assert!(
            matches!(outcome, Err(ToolError::OperationObservationUnavailable)),
            "failed required outcome observation must not become denial or ReviewerFailed"
        );
    } else if !allowed {
        assert!(matches!(
            outcome,
            Err(ToolError::ReviewUnavailable {
                kind: meerkat_core::ReviewUnavailableKind::ReviewerFailed,
            })
        ));
    }
    dispatch(&leaf, SIBLING, &context)
        .await
        .expect("R1 sibling still enters");
    assert_eq!(
        source.calls.load(Ordering::SeqCst),
        usize::from(!matches!(source_access, SourceAccess::NativeUnavailable)),
        "R1 sibling does not read review context; native source never falls back to a host stub"
    );
    let effects = leaf.0.lock().unwrap();
    assert_eq!(
        effects.as_slice(),
        if allowed {
            &[REVIEWED, SIBLING][..]
        } else {
            &[SIBLING][..]
        }
    );
    let requests = server.requests.lock().unwrap();
    assert_eq!(
        requests.len(),
        usize::from(reaches_model),
        "one request, no retry or fallback"
    );
    assert_eq!(
        policy.model_entries.load(Ordering::SeqCst),
        usize::from(reaches_model)
    );
    assert_eq!(
        policy.model_outcomes.load(Ordering::SeqCst),
        usize::from(reaches_model),
        "counts attempted outcome observations, not durable audit records"
    );
    assert_eq!(
        policy.model_prepares.load(Ordering::SeqCst),
        usize::from(matches!(source_access, SourceAccess::Allowed))
    );
    if reaches_model {
        let observations = observations.0.lock().unwrap();
        let statuses = observations
            .iter()
            .map(|item| (item.status, item.retirement))
            .collect::<Vec<_>>();
        if fail_model_outcome {
            assert_eq!(
                statuses,
                vec![
                    (ReviewAttemptStatus::Pending, None),
                    (
                        ReviewAttemptStatus::Retired,
                        Some(ReviewRetirementReason::Abandoned)
                    ),
                ],
                "no Allowed or Used may follow the observation failure"
            );
        } else {
            assert_eq!(
                statuses,
                vec![
                    (ReviewAttemptStatus::Pending, None),
                    (ReviewAttemptStatus::Allowed, None),
                    (ReviewAttemptStatus::Used, None),
                ]
            );
        }
        for item in observations.iter() {
            assert_eq!(
                item.attempt, observations[0].attempt,
                "exact original review attempt"
            );
            assert_eq!(
                item.tool_call_id.as_ref(),
                REVIEWED,
                "the R1 sibling creates no review attempt or retirement"
            );
        }
    }
    if let Some((path, body)) = requests.first() {
        assert_eq!(path, "/v1/responses");
        assert_eq!(body["model"], MODEL);
        assert_eq!(body["max_output_tokens"], 73);
        assert!(
            body.get("tools")
                .is_none_or(|tools| tools.as_array().is_some_and(Vec::is_empty))
        );
        let wire = body.to_string();
        assert!(wire.contains("Test owner context"));
        assert!(wire.contains(REVIEWED));
    }
}

#[tokio::test]
async fn missing_or_denied_source_sends_no_model_request_and_preserves_sibling() {
    for access in [SourceAccess::Unavailable, SourceAccess::Denied] {
        scenario(access, ModelPermission::Allowed, false).await;
    }
}

#[tokio::test]
async fn oversized_encoded_context_is_refused_without_truncation_or_transport() {
    scenario(
        SourceAccess::OversizeEncodedContext,
        ModelPermission::Allowed,
        false,
    )
    .await;
}

#[tokio::test]
async fn unsupported_native_context_owner_has_no_host_source_or_model_fallback() {
    scenario(
        SourceAccess::NativeUnavailable,
        ModelPermission::Allowed,
        false,
    )
    .await;
}

#[tokio::test]
async fn denied_model_or_processor_sends_no_request_and_preserves_sibling() {
    for permission in [
        ModelPermission::ModelDenied,
        ModelPermission::ProcessorDenied,
    ] {
        scenario(SourceAccess::Allowed, permission, false).await;
    }
}

#[tokio::test]
async fn permitted_source_and_model_send_one_factory_request_and_enter_once() {
    scenario(SourceAccess::Allowed, ModelPermission::Allowed, true).await;
}

#[tokio::test]
async fn reviewer_http_outcome_observation_failure_preserves_infrastructure_and_sibling() {
    // Actual factory/OpenAI HTTP transport into the review owner, with one
    // test-policy observation fault and the unchanged positive control.
    // HttpResponse is recorded at headers, not after SSE completion. This is
    // not native ingress, SQLite failure, or Agent/controller continuation.
    for fail_model_outcome in [true, false] {
        scenario_with_outcome_fault(
            SourceAccess::Allowed,
            ModelPermission::Allowed,
            !fail_model_outcome,
            fail_model_outcome,
        )
        .await;
    }
}

struct HeldSource {
    work: WorkAuthorizationContext,
    started: AtomicBool,
    dropped: AtomicUsize,
}

#[async_trait]
impl OperationReviewContextSource for HeldSource {
    async fn read_context(
        &self,
        candidate: &ReviewCandidate<'_>,
    ) -> Result<ReviewContextMaterial, ReviewerFailure> {
        struct ReadLifetime<'a>(&'a AtomicUsize);
        impl Drop for ReadLifetime<'_> {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        assert!(candidate.work_authorization().same_context(&self.work));
        let _lifetime = ReadLifetime(&self.dropped);
        self.started.store(true, Ordering::SeqCst);
        futures::future::pending().await
    }
}

#[derive(Default)]
struct Observations(Mutex<Vec<ReviewObservation>>);

impl OperationReviewObserver for Observations {
    fn observe(&self, observation: ReviewObservation) {
        self.0.lock().unwrap().push(observation);
    }
}

#[tokio::test]
async fn owner_deadline_or_outer_drop_disposes_context_read_without_detached_model_work() {
    for deadline_expires in [true, false] {
        let server = Server::start().await;
        let binding = AuthBindingRef {
            realm: RealmId::parse("review_test").unwrap(),
            binding: BindingId::parse("default_openai").unwrap(),
            profile: None,
            origin: BindingOrigin::Configured,
        };
        let mut config = Config::default();
        let mut realm = RealmConfigSection::from_inline_api_keys(&[("openai", "test-key")]);
        realm.backend.get_mut("default_openai").unwrap().base_url = Some(server.base_url.clone());
        config.realm.insert("review_test".to_owned(), realm);
        let mut identity = identity(MODEL);
        identity.auth_binding = Some(binding.clone());
        let policy = Arc::new(TestPolicy {
            permission: ModelPermission::Allowed,
            binding,
            endpoint: format!("{}/v1/responses", server.base_url),
            candidate: Arc::new(Mutex::new(None)),
            model_prepares: AtomicUsize::new(0),
            model_entries: Arc::new(AtomicUsize::new(0)),
            model_outcomes: Arc::new(AtomicUsize::new(0)),
            fail_model_outcome: false,
        });
        let work = WorkAuthorizationContext::new(policy.clone(), OperationExecutionScope::Domain);
        let source = Arc::new(HeldSource {
            work: work.clone(),
            started: AtomicBool::new(false),
            dropped: AtomicUsize::new(0),
        });
        let reviewer = ModelOperationReviewer::build(
            &crate::AgentFactory::minimal().without_provider_auth_persistence(),
            &config,
            ModelReviewerConfig::new(identity, 73).unwrap(),
            source.clone(),
        )
        .await
        .unwrap();
        let observations = Arc::new(Observations::default());
        let review = Arc::new(
            BoundOperationReview::new(
                Arc::new(reviewer),
                ApprovalService::new(),
                Duration::from_secs(1),
            )
            .with_observer(observations.clone()),
        );
        let context = ToolDispatchContext::default()
            .with_work_authorization(Some(work))
            .with_operation_review(Some(review));
        let leaf = Arc::new(Leaf::default());

        // Poll the actual fenced dispatch into the held read before moving the
        // existing owner's clock. No timer-only sleep or detached read task.
        tokio::time::pause();
        let mut pending = Box::pin(dispatch(&leaf, REVIEWED, &context));
        assert!(pending.as_mut().now_or_never().is_none());
        assert!(source.started.load(Ordering::SeqCst));
        let retirement = if deadline_expires {
            tokio::time::advance(Duration::from_secs(2)).await;
            assert!(matches!(
                pending.await,
                Err(ToolError::ReviewUnavailable {
                    kind: meerkat_core::ReviewUnavailableKind::DeadlineExpired,
                })
            ));
            ReviewRetirementReason::DeadlineExpired
        } else {
            drop(pending);
            ReviewRetirementReason::Abandoned
        };
        tokio::time::resume();

        assert_eq!(source.dropped.load(Ordering::SeqCst), 1);
        let statuses = observations
            .0
            .lock()
            .unwrap()
            .iter()
            .map(|item| (item.status, item.retirement))
            .collect::<Vec<_>>();
        assert_eq!(
            statuses.as_slice(),
            &[
                (ReviewAttemptStatus::Pending, None),
                (ReviewAttemptStatus::Retired, Some(retirement)),
            ]
        );
        dispatch(&leaf, SIBLING, &context)
            .await
            .expect("unrelated R1 work proceeds");
        assert_eq!(leaf.0.lock().unwrap().as_slice(), &[SIBLING]);
        assert!(server.requests.lock().unwrap().is_empty());
        assert_eq!(policy.model_prepares.load(Ordering::SeqCst), 0);
        assert_eq!(policy.model_entries.load(Ordering::SeqCst), 0);
    }
}
