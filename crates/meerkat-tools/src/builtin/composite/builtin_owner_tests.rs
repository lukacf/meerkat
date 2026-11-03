//! The composite as the registration owner of a builtin that declares a typed
//! operation: a resolved plan of that builtin runs only on the exact
//! registration that resolved it.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;
use crate::builtin::MemoryTaskStore;
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};

/// This crate's directory, read when the test runs: cargo, nextest (also
/// from an archive) and the Bazel test rule set CARGO_MANIFEST_DIR for the
/// test process, so an archived test binary never carries the builder's path.
fn manifest_dir() -> PathBuf {
    PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR")
            .expect("CARGO_MANIFEST_DIR is set for test processes"),
    )
}

/// Counts the searches it runs and answers each as completed.
struct CountingSearch(Arc<AtomicUsize>);

#[async_trait]
impl meerkat_llm_core::WebSearchExecutor for CountingSearch {
    async fn execute_web_search(
        &self,
        request: meerkat_core::web_search::WebSearchRequest,
    ) -> Result<meerkat_core::web_search::WebSearchResult, meerkat_llm_core::LlmError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(meerkat_core::web_search::WebSearchResult {
            status: meerkat_core::WebSearchStatus::Completed,
            query: request.query,
            provider: None,
            model: None,
            answer: Some("counted".to_string()),
            evidence: Vec::new(),
            native_events: Vec::new(),
            error: None,
            checked_at: chrono::Utc::now(),
        })
    }
}

fn web_search_composite(searches: &Arc<AtomicUsize>) -> CompositeDispatcher {
    let mut dispatcher = CompositeDispatcher::new(
        Arc::new(MemoryTaskStore::new()),
        &BuiltinToolConfig::default(),
        Some(manifest_dir()),
        None,
        None,
        None,
    )
    .expect("composite dispatcher");
    dispatcher.register_web_search_tool(
        Arc::new(CountingSearch(Arc::clone(searches))),
        ToolCategoryOverride::Enable,
    );
    dispatcher
}

fn deadlines() -> meerkat_core::ToolDeadlineChain {
    meerkat_core::ToolDeadlineChain::new(vec![meerkat_core::ToolDeadlineContributor::finite(
        meerkat_core::ToolDeadlineOwner::CoreToolDispatch,
        std::time::Duration::from_mins(10),
    )])
    .expect("deadline chain")
}

fn owner_changed(result: &Result<ToolDispatchOutcome, ToolError>) -> bool {
    matches!(
        result,
        Err(ToolError::Unavailable {
            reason: meerkat_core::ToolUnavailableReason::ExecutionOwnerChanged,
            ..
        })
    )
}

/// A web_search plan runs only on the composite registration that resolved
/// it. Another composite's plan, a plan whose witness names any other owner
/// key, and a plan resolved before this same composite replaced its
/// web_search executor (identical name, schema and contract) are an owner
/// change, and no executor runs. A plan resolved on the current registration
/// runs on its executor only.
#[tokio::test]
async fn a_builtin_target_is_revalidated_by_the_composite_that_resolved_it() {
    let args =
        serde_json::value::RawValue::from_string(json!({"query": "fixed query"}).to_string())
            .unwrap();
    let call = ToolCallView {
        id: "search-call",
        name: "web_search",
        args: &args,
    };
    let context = ToolDispatchContext::default();
    let resolution = meerkat_core::ToolExecutionResolutionContext::new(deadlines());
    let first = Arc::new(AtomicUsize::new(0));
    let other = Arc::new(AtomicUsize::new(0));
    let mut a = web_search_composite(&first);
    let b = web_search_composite(&other);
    let a_plan = a
        .resolve_execution_plan(call, &context, &resolution)
        .expect("a resolves its own web_search plan");

    // Another composite instance never runs a's plan.
    let result = b
        .dispatch_resolved_with_context(call, &context, &a_plan)
        .await;
    assert!(owner_changed(&result), "a's plan on b: {result:?}");

    // A plan whose witness names another owner key never runs either.
    let forged = meerkat_core::ToolExecutionContract::default()
        .resolve_default(deadlines())
        .expect("plan")
        .with_owner_witness(
            meerkat_core::ToolExecutionOwnerWitness::new(
                "builtin-tools:0",
                "web_search",
                b.execution_binding_fingerprint("web_search")
                    .expect("fingerprint"),
            )
            .expect("witness"),
        )
        .expect("forged witness");
    let result = b
        .dispatch_resolved_with_context(call, &context, &forged)
        .await;
    assert!(
        owner_changed(&result),
        "a witness under another key: {result:?}"
    );
    assert_eq!(
        (first.load(Ordering::SeqCst), other.load(Ordering::SeqCst)),
        (0, 0),
        "no executor ran for a plan of another owner"
    );

    // The same composite replaces its executor with identical metadata: the
    // plan it resolved before the replacement is an owner change.
    let replacement = Arc::new(AtomicUsize::new(0));
    a.register_web_search_tool(
        Arc::new(CountingSearch(Arc::clone(&replacement))),
        ToolCategoryOverride::Enable,
    );
    let result = a
        .dispatch_resolved_with_context(call, &context, &a_plan)
        .await;
    assert!(
        owner_changed(&result),
        "a plan resolved before the replacement: {result:?}"
    );
    assert_eq!(
        (
            first.load(Ordering::SeqCst),
            replacement.load(Ordering::SeqCst)
        ),
        (0, 0),
        "neither the replaced nor the replacing executor ran"
    );

    // A plan resolved on the current registration runs on its executor.
    let current = a
        .resolve_execution_plan(call, &context, &resolution)
        .expect("current plan");
    a.dispatch_resolved_with_context(call, &context, &current)
        .await
        .expect("the current registration runs its own plan");
    assert_eq!(
        (
            first.load(Ordering::SeqCst),
            replacement.load(Ordering::SeqCst),
            other.load(Ordering::SeqCst)
        ),
        (0, 1, 0)
    );
}

/// web_search's plan carries exactly one operation target: the typed builtin
/// operation the tool itself declares, under this composite's own owner key
/// and the call's own name. A builtin that declares no operation keeps a
/// plan without any witness.
#[test]
fn web_search_plans_carry_the_composite_owned_builtin_target() {
    let composite = web_search_composite(&Arc::new(AtomicUsize::new(0)));
    let context = ToolDispatchContext::default();
    let resolution = meerkat_core::ToolExecutionResolutionContext::new(deadlines());
    let args = serde_json::value::RawValue::from_string(json!({"query": "q"}).to_string())
        .expect("raw arguments");
    let plan = composite
        .resolve_execution_plan(
            ToolCallView {
                id: "search-call",
                name: "web_search",
                args: &args,
            },
            &context,
            &resolution,
        )
        .expect("web_search plan");
    let expected = meerkat_core::ToolOperationTarget::BuiltinOperation {
        operation: meerkat_core::BuiltinToolOperation::WebSearch,
    };
    let targeted: Vec<_> = plan
        .owner_witnesses()
        .iter()
        .filter(|witness| witness.operation_target().is_some())
        .collect();
    assert_eq!(targeted.len(), 1, "exactly one owner names a target");
    assert_eq!(
        targeted[0].authority_key(),
        format!("builtin-tools:{}", composite.execution_authority_id)
    );
    assert_eq!(targeted[0].owner_key(), "web_search");
    assert_eq!(targeted[0].operation_target(), Some(&expected));
    assert_eq!(plan.single_operation_target(), Ok(Some(&expected)));

    let empty = serde_json::value::RawValue::from_string("{}".to_string()).expect("raw");
    let datetime = composite
        .resolve_execution_plan(
            ToolCallView {
                id: "time-call",
                name: "datetime",
                args: &empty,
            },
            &context,
            &resolution,
        )
        .expect("datetime plan");
    assert!(
        datetime.owner_witnesses().is_empty(),
        "a builtin without a declared operation keeps its plan"
    );
    assert_eq!(datetime.single_operation_target(), Ok(None));
    assert_eq!(
        crate::builtin::web_search::WebSearchTool::new(Arc::new(
            crate::builtin::web_search::EmptyWebSearchExecutor
        ))
        .builtin_operation(),
        Some(meerkat_core::BuiltinToolOperation::WebSearch)
    );
    assert_eq!(
        crate::builtin::utility::DateTimeTool::new().builtin_operation(),
        None
    );
}

struct Allowed;
impl meerkat_core::authorization::PreparedOperationAuthorization for Allowed {
    fn review_tier(&self) -> meerkat_core::authorization::OperationReviewTier {
        meerkat_core::authorization::OperationReviewTier::R1
    }

    fn check_current(
        &self,
        _binding: &meerkat_core::authorization::PreparedAuthorizationBinding,
    ) -> Result<(), meerkat_core::OperationAuthorizationError> {
        Ok(())
    }
}

/// Admits every operation: only the delegate's own rules are under test.
struct AllowAll;
impl meerkat_core::authorization::WorkAuthorization for AllowAll {
    fn prepare(
        &self,
        _binding: &meerkat_core::authorization::PreparedAuthorizationBinding,
    ) -> Result<
        Arc<dyn meerkat_core::authorization::PreparedOperationAuthorization>,
        meerkat_core::OperationAuthorizationError,
    > {
        Ok(Arc::new(Allowed))
    }
}

fn map_tool_error(error: BuiltinToolError) -> ToolError {
    match error {
        BuiltinToolError::EntryRefused(error) => *error,
        BuiltinToolError::OperationRefused { refusal } => {
            ToolError::AuthorizationRefused { refusal }
        }
        other => ToolError::ExecutionFailed {
            message: other.to_string(),
        },
    }
}

/// Counts every search it is asked for, on either seam, and answers each
/// as completed: a helper request would be visible here whether or not it
/// carried an authorization.
struct AnySeamSearch(Arc<AtomicUsize>);

#[async_trait]
impl meerkat_llm_core::WebSearchExecutor for AnySeamSearch {
    async fn execute_web_search(
        &self,
        request: meerkat_core::web_search::WebSearchRequest,
    ) -> Result<meerkat_core::web_search::WebSearchResult, meerkat_llm_core::LlmError> {
        CountingSearch(Arc::clone(&self.0))
            .execute_web_search(request)
            .await
    }

    async fn execute_web_search_authorized(
        &self,
        request: meerkat_core::web_search::WebSearchRequest,
        _authorization: Option<meerkat_core::LlmRequestAuthorization>,
    ) -> Result<meerkat_core::web_search::WebSearchResult, meerkat_llm_core::LlmError> {
        self.execute_web_search(request).await
    }
}

/// A host dispatcher that serves its own real `WebSearchTool` under the
/// name web_search.
struct HostSearch(crate::builtin::web_search::WebSearchTool);

#[async_trait]
impl AgentToolDispatcher for HostSearch {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::from([Arc::new(self.0.def())])
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
        let args: Value = serde_json::from_str(call.args.get()).expect("arguments");
        self.0
            .call_with_context(call, args, context)
            .await
            .map_err(map_tool_error)?;
        Ok(ToolDispatchOutcome::new(
            ToolResult::new(call.id.to_string(), "host search".to_string(), false),
            Vec::new(),
            Vec::new(),
        ))
    }
}

/// A builtin named web_search that declares no operation: a real
/// `WebSearchTool` behind another registration.
struct Lookalike(crate::builtin::web_search::WebSearchTool);

#[async_trait]
impl BuiltinTool for Lookalike {
    fn name(&self) -> &'static str {
        "web_search"
    }

    fn def(&self) -> ToolDef {
        self.0.def()
    }

    fn default_enabled(&self) -> bool {
        true
    }

    async fn call(&self, args: Value) -> Result<ToolOutput, BuiltinToolError> {
        self.0.call(args).await
    }

    async fn call_with_context(
        &self,
        call: ToolCallView<'_>,
        args: Value,
        context: &ToolDispatchContext,
    ) -> Result<ToolOutput, BuiltinToolError> {
        self.0.call_with_context(call, args, context).await
    }
}

/// One real governed fenced dispatch of a web_search call through
/// `dispatcher`: its plan and the dispatch result.
async fn governed_search(
    dispatcher: &Arc<CompositeDispatcher>,
) -> (
    meerkat_core::ResolvedToolExecutionPlan,
    Result<ToolDispatchOutcome, ToolError>,
) {
    let work = meerkat_core::WorkAuthorizationContext::new(
        Arc::new(AllowAll),
        meerkat_core::exact_operation::OperationExecutionScope::Domain,
    );
    let context = ToolDispatchContext::default().with_work_authorization(Some(work));
    let args =
        serde_json::value::RawValue::from_string(json!({"query": "fixed query"}).to_string())
            .expect("raw arguments");
    let call = ToolCallView {
        id: "search-call",
        name: "web_search",
        args: &args,
    };
    let resolution = meerkat_core::ToolExecutionResolutionContext::new(deadlines());
    let plan = meerkat_core::agent::resolve_tool_execution_plan_fenced(
        dispatcher,
        call,
        &context,
        &resolution,
    )
    .expect("plan");
    let result =
        meerkat_core::agent::dispatch_tool_execution_plan_fenced(dispatcher, call, &context, &plan)
            .await;
    (plan, result)
}

/// A tool merely named web_search, from any owner but the composite's own
/// declared builtin, has no builtin target: a host dispatcher's tool, an
/// undeclared builtin and a registered bundle's web_search operation. Each
/// serves a real `WebSearchTool` here, whose governed call then refuses as
/// malformed facts before its executor runs.
#[tokio::test]
async fn a_tool_named_web_search_from_another_owner_has_no_builtin_target() {
    let host_tool = |searches: &Arc<AtomicUsize>| {
        crate::builtin::web_search::WebSearchTool::new(Arc::new(AnySeamSearch(Arc::clone(
            searches,
        ))))
    };
    let composite_over = |external: Option<Arc<dyn AgentToolDispatcher>>| {
        CompositeDispatcher::new(
            Arc::new(MemoryTaskStore::new()),
            &BuiltinToolConfig::default(),
            Some(manifest_dir()),
            None,
            external,
            None,
        )
        .expect("composite dispatcher")
    };
    let searches = Arc::new(AtomicUsize::new(0));
    let host = composite_over(Some(Arc::new(HostSearch(host_tool(&searches)))));
    let mut lookalike = composite_over(None);
    lookalike
        .builtin_tools
        .push(Arc::new(Lookalike(host_tool(&searches))));
    lookalike.allowed_tools.insert("web_search".to_string());
    let bundle = composite_over(Some(Arc::new(
        meerkat_core::RegisteredToolBundleDispatcher::new(
            "host",
            Arc::new(HostSearch(host_tool(&searches))),
        ),
    )));
    for (label, dispatcher) in [
        ("a host dispatcher's tool", host),
        ("an undeclared builtin", lookalike),
        ("a registered bundle's operation", bundle),
    ] {
        let (plan, result) = governed_search(&Arc::new(dispatcher)).await;
        assert!(
            !plan.owner_witnesses().iter().any(|witness| matches!(
                witness.operation_target(),
                Some(meerkat_core::ToolOperationTarget::BuiltinOperation { .. })
            )),
            "{label}: no builtin target"
        );
        assert!(
            matches!(
                &result,
                Err(ToolError::AuthorizationRefused { refusal })
                    if refusal.kind()
                        == meerkat_core::authorization::OperationRefusalKind::MalformedFacts
            ),
            "{label}: {result:?}"
        );
    }
    assert_eq!(searches.load(Ordering::SeqCst), 0, "no executor ran");
}

/// A plan carrying this composite's builtin witness runs only on that
/// builtin, whatever owns the name now. Re-registering web_search as
/// disabled hands the name to an external tool that also advertises it: the
/// plan the builtin resolved before is an owner change and never reaches the
/// external tool, whose own current plan still runs on it.
#[tokio::test]
async fn a_builtin_plan_never_runs_on_the_next_owner_of_its_name() {
    let builtin_searches = Arc::new(AtomicUsize::new(0));
    let external_searches = Arc::new(AtomicUsize::new(0));
    let external: Arc<dyn AgentToolDispatcher> =
        Arc::new(HostSearch(crate::builtin::web_search::WebSearchTool::new(
            Arc::new(AnySeamSearch(Arc::clone(&external_searches))),
        )));
    let mut composite = CompositeDispatcher::new(
        Arc::new(MemoryTaskStore::new()),
        &BuiltinToolConfig::default(),
        Some(manifest_dir()),
        None,
        Some(external),
        None,
    )
    .expect("composite dispatcher");
    composite.register_web_search_tool(
        Arc::new(CountingSearch(Arc::clone(&builtin_searches))),
        ToolCategoryOverride::Enable,
    );
    let args =
        serde_json::value::RawValue::from_string(json!({"query": "fixed query"}).to_string())
            .expect("raw arguments");
    let call = ToolCallView {
        id: "search-call",
        name: "web_search",
        args: &args,
    };
    let context = ToolDispatchContext::default();
    let resolution = meerkat_core::ToolExecutionResolutionContext::new(deadlines());
    let builtin_target = meerkat_core::ToolOperationTarget::BuiltinOperation {
        operation: meerkat_core::BuiltinToolOperation::WebSearch,
    };
    let old = composite
        .resolve_execution_plan(call, &context, &resolution)
        .expect("the enabled builtin owns web_search");
    assert_eq!(old.single_operation_target(), Ok(Some(&builtin_target)));

    composite.register_web_search_tool(
        Arc::new(CountingSearch(Arc::clone(&builtin_searches))),
        ToolCategoryOverride::Disable,
    );
    let current = composite
        .resolve_execution_plan(call, &context, &resolution)
        .expect("the external tool now owns web_search");
    assert_eq!(
        current.single_operation_target(),
        Ok(None),
        "the external tool's plan carries no builtin target"
    );

    let result = composite
        .dispatch_resolved_with_context(call, &context, &old)
        .await;
    assert!(
        owner_changed(&result),
        "the builtin's plan on the name's next owner: {result:?}"
    );
    assert_eq!(
        (
            builtin_searches.load(Ordering::SeqCst),
            external_searches.load(Ordering::SeqCst)
        ),
        (0, 0),
        "no executor ran for the builtin's plan"
    );

    composite
        .dispatch_resolved_with_context(call, &context, &current)
        .await
        .expect("the external tool runs its own plan");
    assert_eq!(
        (
            builtin_searches.load(Ordering::SeqCst),
            external_searches.load(Ordering::SeqCst)
        ),
        (0, 1)
    );
}

/// Allows every operation at one owner review tier.
struct Tiered(meerkat_core::authorization::OperationReviewTier);
impl meerkat_core::authorization::PreparedOperationAuthorization for Tiered {
    fn review_tier(&self) -> meerkat_core::authorization::OperationReviewTier {
        self.0
    }

    fn check_current(
        &self,
        _binding: &meerkat_core::authorization::PreparedAuthorizationBinding,
    ) -> Result<(), meerkat_core::OperationAuthorizationError> {
        Ok(())
    }
}

struct TieredWork(meerkat_core::authorization::OperationReviewTier);
impl meerkat_core::authorization::WorkAuthorization for TieredWork {
    fn prepare(
        &self,
        _binding: &meerkat_core::authorization::PreparedAuthorizationBinding,
    ) -> Result<
        Arc<dyn meerkat_core::authorization::PreparedOperationAuthorization>,
        meerkat_core::OperationAuthorizationError,
    > {
        Ok(Arc::new(Tiered(self.0)))
    }
}

/// A reviewer that counts every review it is asked for.
#[derive(Default)]
struct CountingReviewer(AtomicUsize);

#[async_trait]
impl meerkat_core::approval::review::OperationReviewer for CountingReviewer {
    async fn review(
        &self,
        _candidate: &meerkat_core::approval::review::ReviewCandidate<'_>,
    ) -> Result<
        meerkat_core::approval::review::ReviewVerdict,
        meerkat_core::approval::review::ReviewerFailure,
    > {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(meerkat_core::approval::review::ReviewVerdict::Allow)
    }
}

/// A human consent route that counts every time it is reached.
#[derive(Default)]
struct CountingConsentRoute(AtomicUsize);

impl meerkat_core::approval::consent::HumanConsentRoute for CountingConsentRoute {
    fn present(
        &self,
        _candidate: &meerkat_core::approval::consent::ConsentPresentationCandidate<'_>,
    ) -> Result<
        meerkat_core::approval::consent::ConsentPresentation,
        meerkat_core::approval::consent::ConsentRouteUnavailable,
    > {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(meerkat_core::approval::consent::ConsentRouteUnavailable)
    }

    fn eligibility(
        &self,
        _approver: &meerkat_core::approval::consent::AuthenticatedApprover,
        _approval: &meerkat_core::ApprovalRecord,
    ) -> Result<
        Arc<dyn meerkat_core::approval::consent::ConsentEligibility>,
        meerkat_core::approval::consent::ConsentIneligible,
    > {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(meerkat_core::approval::consent::ConsentIneligible)
    }

    fn notice(&self, _notice: meerkat_core::approval::consent::ConsentNotice) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

/// How a required review settles one governed web_search call.
#[derive(Debug, Clone, Copy)]
enum Settles {
    Unavailable(meerkat_core::ReviewUnavailableKind),
    Unsatisfied(meerkat_core::ReviewUnsatisfiedKind),
}

/// web_search has no reviewed entry point, so a review its owner requires
/// settles as typed local feedback and the search never runs. R2 is
/// `reviewer_missing` without a reviewer and `unsupported_entry` with one;
/// R3 is `human_consent_required` without a human consent route (with or
/// without a reviewer) and `unsupported_entry` with one. No reviewer or
/// consent route is asked. R1 runs the search under the same composition.
#[tokio::test]
async fn a_required_review_settles_web_search_as_local_feedback_per_tier() {
    use meerkat_core::authorization::OperationReviewTier::{R1, R2, R3};
    use meerkat_core::{ReviewUnavailableKind, ReviewUnsatisfiedKind};
    let searches = Arc::new(AtomicUsize::new(0));
    let reviewer = Arc::new(CountingReviewer::default());
    let route = Arc::new(CountingConsentRoute::default());
    let review = |route: Option<Arc<CountingConsentRoute>>| {
        let review = meerkat_core::approval::review::BoundOperationReview::new(
            reviewer.clone(),
            meerkat_core::ApprovalService::new(),
            std::time::Duration::from_secs(30),
        );
        Some(Arc::new(match route {
            Some(route) => review.with_human_consent(route, std::time::Duration::from_mins(1)),
            None => review,
        }))
    };
    let mut composite = CompositeDispatcher::new(
        Arc::new(MemoryTaskStore::new()),
        &BuiltinToolConfig::default(),
        Some(manifest_dir()),
        None,
        None,
        None,
    )
    .expect("composite dispatcher");
    composite.register_web_search_tool(
        Arc::new(AnySeamSearch(Arc::clone(&searches))),
        ToolCategoryOverride::Enable,
    );
    let dispatcher = Arc::new(composite);
    let args =
        serde_json::value::RawValue::from_string(json!({"query": "fixed query"}).to_string())
            .expect("raw arguments");
    let call = ToolCallView {
        id: "search-call",
        name: "web_search",
        args: &args,
    };
    let resolution = meerkat_core::ToolExecutionResolutionContext::new(deadlines());
    let dispatch = |tier, review| {
        let context = ToolDispatchContext::default()
            .with_work_authorization(Some(meerkat_core::WorkAuthorizationContext::new(
                Arc::new(TieredWork(tier)),
                meerkat_core::exact_operation::OperationExecutionScope::Domain,
            )))
            .with_operation_review(review);
        let dispatcher = Arc::clone(&dispatcher);
        let resolution = resolution.clone();
        async move {
            let plan = meerkat_core::agent::resolve_tool_execution_plan_fenced(
                &dispatcher,
                call,
                &context,
                &resolution,
            )
            .expect("plan");
            meerkat_core::agent::dispatch_tool_execution_plan_fenced(
                &dispatcher,
                call,
                &context,
                &plan,
            )
            .await
        }
    };
    for (label, tier, review, settles) in [
        (
            "R2 without a reviewer",
            R2,
            None,
            Settles::Unavailable(ReviewUnavailableKind::ReviewerMissing),
        ),
        (
            "R2 with a reviewer",
            R2,
            review(None),
            Settles::Unavailable(ReviewUnavailableKind::UnsupportedEntry),
        ),
        (
            "R3 without any review",
            R3,
            None,
            Settles::Unsatisfied(ReviewUnsatisfiedKind::HumanConsentRequired),
        ),
        (
            "R3 with a reviewer and no human consent route",
            R3,
            review(None),
            Settles::Unsatisfied(ReviewUnsatisfiedKind::HumanConsentRequired),
        ),
        (
            "R3 with a human consent route",
            R3,
            review(Some(route.clone())),
            Settles::Unavailable(ReviewUnavailableKind::UnsupportedEntry),
        ),
    ] {
        let result = dispatch(tier, review).await;
        let settled = match settles {
            Settles::Unavailable(expected) => matches!(
                &result,
                Err(ToolError::ReviewUnavailable { kind }) if *kind == expected
            ),
            Settles::Unsatisfied(expected) => matches!(
                &result,
                Err(ToolError::ReviewUnsatisfied { kind, .. }) if *kind == expected
            ),
        };
        assert!(settled, "{label}: expected {settles:?}, got {result:?}");
    }
    assert_eq!(
        (
            searches.load(Ordering::SeqCst),
            reviewer.0.load(Ordering::SeqCst),
            route.0.load(Ordering::SeqCst)
        ),
        (0, 0, 0),
        "no search ran and no reviewer or consent route was asked"
    );

    dispatch(R1, review(Some(route.clone())))
        .await
        .expect("R1 runs the search");
    assert_eq!(searches.load(Ordering::SeqCst), 1);
}

/// The direct registration-identity control: a plan resolved on web_search
/// registration A never runs after the same composite replaced A with B
/// under the same name, schema and typed target. The old plan is an owner
/// change and neither A's nor B's executor runs; B's own plan runs on B.
/// The other-instance and changed-target controls do not cover this case.
#[tokio::test]
async fn a_plan_of_a_replaced_registration_never_runs_on_its_replacement() {
    let a_searches = Arc::new(AtomicUsize::new(0));
    let b_searches = Arc::new(AtomicUsize::new(0));
    let mut composite = web_search_composite(&a_searches);
    let args =
        serde_json::value::RawValue::from_string(json!({"query": "fixed query"}).to_string())
            .expect("raw arguments");
    let call = ToolCallView {
        id: "search-call",
        name: "web_search",
        args: &args,
    };
    let context = ToolDispatchContext::default();
    let resolution = meerkat_core::ToolExecutionResolutionContext::new(deadlines());
    let advertised = |composite: &CompositeDispatcher| {
        composite
            .tools()
            .iter()
            .find(|tool| tool.name == "web_search")
            .map(|tool| serde_json::to_value(&**tool).expect("tool definition"))
            .expect("web_search is advertised")
    };
    let a_definition = advertised(&composite);
    let a_plan = composite
        .resolve_execution_plan(call, &context, &resolution)
        .expect("registration A resolves its plan");

    composite.register_web_search_tool(
        Arc::new(CountingSearch(Arc::clone(&b_searches))),
        ToolCategoryOverride::Enable,
    );
    assert_eq!(
        advertised(&composite),
        a_definition,
        "B has A's name and schema"
    );
    let b_plan = composite
        .resolve_execution_plan(call, &context, &resolution)
        .expect("registration B resolves its plan");
    assert_eq!(
        b_plan.single_operation_target(),
        a_plan.single_operation_target(),
        "and A's typed target"
    );

    let result = composite
        .dispatch_resolved_with_context(call, &context, &a_plan)
        .await;
    assert!(
        owner_changed(&result),
        "A's plan after the replacement: {result:?}"
    );
    assert_eq!(
        (
            a_searches.load(Ordering::SeqCst),
            b_searches.load(Ordering::SeqCst)
        ),
        (0, 0),
        "neither A's nor B's executor ran"
    );

    composite
        .dispatch_resolved_with_context(call, &context, &b_plan)
        .await
        .expect("B runs its own plan");
    assert_eq!(
        (
            a_searches.load(Ordering::SeqCst),
            b_searches.load(Ordering::SeqCst)
        ),
        (0, 1)
    );
}

/// Registering skill tools changes which owners the composite's names can
/// resolve to, so it advances the registration generation like every
/// builtin registration change: a web_search plan resolved before it is an
/// owner change and its executor never runs; a plan resolved after it runs.
#[cfg(feature = "skills")]
#[tokio::test]
async fn registering_skill_tools_advances_the_registration_generation() {
    let searches = Arc::new(AtomicUsize::new(0));
    let mut composite = web_search_composite(&searches);
    let args =
        serde_json::value::RawValue::from_string(json!({"query": "fixed query"}).to_string())
            .expect("raw arguments");
    let call = ToolCallView {
        id: "search-call",
        name: "web_search",
        args: &args,
    };
    let context = ToolDispatchContext::default();
    let resolution = meerkat_core::ToolExecutionResolutionContext::new(deadlines());
    let before = composite
        .resolve_execution_plan(call, &context, &resolution)
        .expect("the plan before the skill registration");

    composite.register_skill_tools(super::tests::stub_skill_tool_set());
    let result = composite
        .dispatch_resolved_with_context(call, &context, &before)
        .await;
    assert!(
        owner_changed(&result),
        "a plan resolved before the skill registration: {result:?}"
    );
    assert_eq!(searches.load(Ordering::SeqCst), 0, "no executor ran");

    let after = composite
        .resolve_execution_plan(call, &context, &resolution)
        .expect("the plan after the skill registration");
    composite
        .dispatch_resolved_with_context(call, &context, &after)
        .await
        .expect("a plan resolved after it runs");
    assert_eq!(searches.load(Ordering::SeqCst), 1);
}
