//! Required operation review at the MCP local transport handoff (ADR-001).
//! A revoke or an expiry while the call-context provider is still preparing
//! must hand nothing off and spend nothing, while a permitted sibling
//! proceeds. Expiry runs on a paused virtual clock, never a wall timer.

use super::*;
use meerkat_core::ToolDispatchOutcome;
use meerkat_core::approval::review::{
    BoundOperationReview, OperationReviewObserver, OperationReviewTier, OperationReviewer,
    ReviewAttemptStatus, ReviewCandidate, ReviewObservation, ReviewRetirementReason, ReviewVerdict,
    ReviewerFailure,
};
use meerkat_core::authorization::{
    AuthorizationOperation, OperationAuthorizationError, OperationRefusalKind, OperationRefused,
    PreparedAuthorizationBinding, PreparedOperationAuthorization, WorkAuthorization,
    WorkAuthorizationContext,
};
use meerkat_core::exact_operation::OperationExecutionScope;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::Notify;

const REVIEW_DEADLINE: Duration = Duration::from_secs(30);

/// Native owner: R2 for the call id "reviewed", explicit R1 otherwise. A
/// narrow revocation refuses only the named call; every owner edit bumps the
/// generation so retained decisions re-prepare.
#[derive(Default)]
struct Owner {
    generation: AtomicU64,
    revoked_call: std::sync::Mutex<Option<String>>,
}

impl Owner {
    fn revoke(&self, call_id: &str) {
        *self.revoked_call.lock().unwrap() = Some(call_id.to_string());
        self.generation.fetch_add(1, Ordering::SeqCst);
    }
}

struct Work(Arc<Owner>);

struct Decision {
    owner: Arc<Owner>,
    generation: u64,
    tier: OperationReviewTier,
}

impl WorkAuthorization for Work {
    fn prepare(
        &self,
        binding: &PreparedAuthorizationBinding,
    ) -> Result<Arc<dyn PreparedOperationAuthorization>, OperationAuthorizationError> {
        let AuthorizationOperation::Tool(tool) = &binding.facts().operation else {
            return Err(OperationRefused::new(OperationRefusalKind::Denied).into());
        };
        if self.0.revoked_call.lock().unwrap().as_deref() == Some(tool.call_id.as_ref()) {
            return Err(OperationRefused::new(OperationRefusalKind::Denied).into());
        }
        let tier = if tool.call_id.as_ref() == "reviewed" {
            OperationReviewTier::R2
        } else {
            OperationReviewTier::R1
        };
        Ok(Arc::new(Decision {
            owner: Arc::clone(&self.0),
            generation: self.0.generation.load(Ordering::SeqCst),
            tier,
        }))
    }
}

impl PreparedOperationAuthorization for Decision {
    fn review_tier(&self) -> OperationReviewTier {
        self.tier
    }

    fn check_current(
        &self,
        _binding: &PreparedAuthorizationBinding,
    ) -> Result<(), OperationAuthorizationError> {
        if self.owner.generation.load(Ordering::SeqCst) == self.generation {
            Ok(())
        } else {
            Err(OperationRefused::new(OperationRefusalKind::ReprepareRequired).into())
        }
    }
}

struct AllowReviewer;

#[async_trait]
impl OperationReviewer for AllowReviewer {
    async fn review(&self, _: &ReviewCandidate<'_>) -> Result<ReviewVerdict, ReviewerFailure> {
        Ok(ReviewVerdict::Allow)
    }
}

#[derive(Default)]
struct Recording(std::sync::Mutex<Vec<ReviewObservation>>);

impl OperationReviewObserver for Recording {
    fn observe(&self, observation: ReviewObservation) {
        self.0.lock().unwrap().push(observation);
    }
}

impl Recording {
    fn statuses(&self) -> Vec<(ReviewAttemptStatus, Option<ReviewRetirementReason>)> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .map(|observation| (observation.status, observation.retirement))
            .collect()
    }
}

/// Call-context provider that pauses preparation of the reviewed call.
struct PausingProvider {
    selected: McpServerConfig,
    entered: Notify,
    release: Notify,
}

#[async_trait]
impl McpCallContextProvider for PausingProvider {
    async fn prepare(
        &self,
        target: McpCallTarget<'_>,
        call: ToolCallView<'_>,
        _: &ToolDispatchContext,
    ) -> Result<Option<McpCallContext>, McpCallContextError> {
        if target.config != &self.selected {
            return Ok(None);
        }
        if call.id == "reviewed" {
            self.entered.notify_one();
            self.release.notified().await;
        }
        Ok(Some(McpCallContext::new(
            serde_json::Map::from_iter([(PRIVATE_KEY.into(), json!({"prepared": call.id}))]),
            (),
        )))
    }

    fn tool_mutation_class(&self, _: &McpServerConfig, _: &str) -> ToolMutationClass {
        ToolMutationClass::Unknown
    }
}

fn governed(owner: &Arc<Owner>, observer: &Arc<Recording>) -> ToolDispatchContext {
    let review = Arc::new(
        BoundOperationReview::new(
            Arc::new(AllowReviewer),
            meerkat_core::ApprovalService::new(),
            REVIEW_DEADLINE,
        )
        .with_observer(observer.clone()),
    );
    ToolDispatchContext::default()
        .with_runtime_identity(SessionId::new(), None)
        .with_work_authorization(Some(WorkAuthorizationContext::new(
            Arc::new(Work(Arc::clone(owner))),
            OperationExecutionScope::Domain,
        )))
        .with_operation_review(Some(review))
}

async fn fenced(
    adapter: &Arc<McpRouterAdapter>,
    call: ToolCallView<'_>,
    context: &ToolDispatchContext,
) -> Result<ToolDispatchOutcome, ToolError> {
    let plan = meerkat_core::agent::resolve_tool_execution_plan_fenced(
        adapter,
        call,
        context,
        &resolution(),
    )
    .unwrap();
    meerkat_core::agent::dispatch_tool_execution_plan_fenced(adapter, call, context, &plan).await
}

async fn run_paused_preparation_case(expire: bool) {
    let fixture = Fixture::start().await;
    let provider = Arc::new(PausingProvider {
        selected: fixture.config.clone(),
        entered: Notify::new(),
        release: Notify::new(),
    });
    let router = fixture
        .router(Some(provider.clone()), fixture.config.clone())
        .await;
    let adapter = Arc::new(McpRouterAdapter::new(router));
    let owner = Arc::new(Owner::default());
    let observer = Arc::new(Recording::default());
    let context = governed(&owner, &observer);
    let result = AssertUnwindSafe(async {
        let args = serde_json::value::to_raw_value(&json!({})).unwrap();
        let reviewed = ToolCallView {
            id: "reviewed",
            name: "read",
            args: &args,
        };
        let mut dispatch = Box::pin(fenced(&adapter, reviewed, &context));
        tokio::select! {
            _ = &mut dispatch => panic!("dispatch skipped call-context preparation"),
            () = provider.entered.notified() => {}
            () = tokio::time::sleep(LIMIT) => panic!("preparation never started"),
        }
        if expire {
            // The retained allow's absolute deadline passes during preparation.
            tokio::time::pause();
            tokio::time::advance(REVIEW_DEADLINE + Duration::from_millis(1)).await;
            tokio::time::resume();
        } else {
            owner.revoke("reviewed");
        }
        provider.release.notify_one();
        let error = tokio::time::timeout(LIMIT, dispatch)
            .await
            .unwrap()
            .unwrap_err();
        if expire {
            assert_eq!(
                error,
                ToolError::ReviewUnavailable {
                    kind: meerkat_core::ReviewUnavailableKind::DeadlineExpired,
                }
            );
        } else {
            assert!(
                matches!(&error, ToolError::AuthorizationRefused { refusal } if refusal.kind() == OperationRefusalKind::Denied),
                "{error:?}"
            );
        }
        assert!(
            fixture.server.requests.lock().unwrap().is_empty(),
            "nothing was handed to the transport"
        );
        let statuses = observer.statuses();
        assert!(
            !statuses.contains(&(ReviewAttemptStatus::Used, None)),
            "the allow was never spent: {statuses:?}"
        );
        // A permitted R1 sibling still reaches the server.
        let sibling = ToolCallView {
            id: "sibling",
            name: "read",
            args: &args,
        };
        fenced(&adapter, sibling, &context)
            .await
            .expect("permitted sibling proceeds");
        assert_eq!(fixture.server.requests.lock().unwrap().len(), 1);
    })
    .catch_unwind()
    .await;
    provider.release.notify_one();
    drop(context);
    Arc::try_unwrap(adapter).ok().unwrap().shutdown().await;
    fixture.finish(result).await;
}

#[tokio::test]
async fn revoke_during_mcp_preparation_hands_off_nothing_and_spends_nothing() {
    run_paused_preparation_case(false).await;
}

#[tokio::test]
async fn expiry_during_mcp_preparation_hands_off_nothing_and_spends_nothing() {
    run_paused_preparation_case(true).await;
}
