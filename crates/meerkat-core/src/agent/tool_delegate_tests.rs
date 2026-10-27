//! The typed delegate of a governed dispatch's own builtin web_search call
//! is minted only through the real fenced dispatch of exactly that call, and
//! only when the call's plan carries exactly the builtin web_search target.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;
use crate::authorization::{
    AuthorizationOperation, OperationAuthorizationError, OperationObservation,
    OperationObservationError, OperationRefusalKind, OperationReviewTier,
    PreparedAuthorizationBinding, PreparedOperationAuthorization, PreparedOperationCheck,
    ToolDelegateAttribution, ToolDelegateRole, WorkAuthorization,
};
use crate::tool_execution::{BuiltinToolOperation, CatalogOperation, ToolOperationTarget};
use crate::types::ToolResult;
use std::sync::Mutex;

struct Allowed;
impl PreparedOperationAuthorization for Allowed {
    fn review_tier(&self) -> OperationReviewTier {
        OperationReviewTier::R1
    }

    fn check_current(
        &self,
        _binding: &PreparedAuthorizationBinding,
    ) -> Result<(), OperationAuthorizationError> {
        Ok(())
    }

    fn observe(
        &self,
        _binding: &PreparedAuthorizationBinding,
        _observation: OperationObservation,
    ) -> Result<(), OperationObservationError> {
        Ok(())
    }
}

struct AllowAll;
impl WorkAuthorization for AllowAll {
    fn prepare(
        &self,
        _binding: &PreparedAuthorizationBinding,
    ) -> Result<Arc<dyn PreparedOperationAuthorization>, OperationAuthorizationError> {
        Ok(Arc::new(Allowed))
    }
}

type Minted = Result<Option<ToolDelegateAttribution>, crate::ToolError>;

/// What one dispatch saw: the delegate of the call it was handed, the
/// delegate of an equal-looking call, and the dispatch's own prepared
/// binding.
struct Seen {
    own: Minted,
    lookalike: Minted,
    parent: Option<PreparedAuthorizationBinding>,
}

/// One `web_search` tool whose own plan carries `targets`, each under an
/// owner key of its own.
struct TargetOwner {
    targets: Vec<ToolOperationTarget>,
    seen: Mutex<Vec<Seen>>,
}

fn owner(targets: Vec<ToolOperationTarget>) -> Arc<TargetOwner> {
    Arc::new(TargetOwner {
        targets,
        seen: Mutex::new(Vec::new()),
    })
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl AgentToolDispatcher for TargetOwner {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::from([Arc::new(ToolDef::new(
            "web_search",
            "fixture search",
            serde_json::json!({"type": "object"}),
        ))])
    }

    fn resolve_execution_plan(
        &self,
        call: ToolCallView<'_>,
        _dispatch_context: &ToolDispatchContext,
        resolution_context: &crate::ToolExecutionResolutionContext,
    ) -> Result<crate::ResolvedToolExecutionPlan, crate::ToolExecutionResolutionError> {
        let mut plan = crate::ToolExecutionContract::default()
            .resolve_default(resolution_context.deadlines().clone())?;
        for (index, target) in self.targets.iter().enumerate() {
            plan = plan.with_owner_witness(
                crate::ToolExecutionOwnerWitness::new(
                    format!("fixture-owner-{index}"),
                    call.name,
                    self.execution_binding_fingerprint(call.name)?,
                )?
                .with_operation_target(target.clone()),
            )?;
        }
        Ok(plan)
    }

    async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<crate::ops::ToolDispatchOutcome, crate::error::ToolError> {
        Ok(ToolResult::new(call.id.to_string(), "ungoverned".to_string(), false).into())
    }

    async fn dispatch_resolved_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        _plan: &crate::ResolvedToolExecutionPlan,
    ) -> Result<crate::ops::ToolDispatchOutcome, crate::error::ToolError> {
        let id = call.id.to_owned();
        let name = call.name.to_owned();
        let args = serde_json::value::RawValue::from_string(call.args.get().to_owned())
            .expect("raw arguments");
        let lookalike = ToolCallView {
            id: &id,
            name: &name,
            args: &args,
        };
        let seen = Seen {
            own: context.tool_delegate_attribution(call, ToolDelegateRole::HostedSearchInference),
            lookalike: context
                .tool_delegate_attribution(lookalike, ToolDelegateRole::HostedSearchInference),
            parent: context
                .prepared_authorization()
                .map(|check| check.binding().clone()),
        };
        self.seen.lock().unwrap().push(seen);
        Ok(ToolResult::new(call.id.to_string(), "searched".to_string(), false).into())
    }
}

/// One real fenced dispatch of a `web_search` call through `owner`.
async fn dispatch_once(owner: &Arc<TargetOwner>, context: &ToolDispatchContext) -> Seen {
    let args = serde_json::value::RawValue::from_string(r#"{"query":"fixture"}"#.to_string())
        .expect("raw arguments");
    let call = ToolCallView {
        id: "search-call",
        name: "web_search",
        args: &args,
    };
    let resolution = crate::ToolExecutionResolutionContext::new(
        crate::ToolDeadlineChain::new(vec![crate::ToolDeadlineContributor::finite(
            crate::ToolDeadlineOwner::CoreToolDispatch,
            std::time::Duration::from_mins(1),
        )])
        .expect("deadline chain"),
    );
    let plan = resolve_tool_execution_plan_fenced(owner, call, context, &resolution)
        .expect("the owner's plan");
    dispatch_tool_execution_plan_fenced(owner, call, context, &plan)
        .await
        .expect("dispatched");
    owner.seen.lock().unwrap().pop().expect("one dispatch")
}

fn builtin_web_search() -> ToolOperationTarget {
    ToolOperationTarget::BuiltinOperation {
        operation: BuiltinToolOperation::WebSearch,
    }
}

fn malformed(result: &Minted) -> bool {
    matches!(
        result,
        Err(crate::ToolError::AuthorizationRefused { refusal })
            if refusal.kind() == OperationRefusalKind::MalformedFacts
    )
}

/// Ungoverned dispatch has no delegate. A governed dispatch of the builtin
/// web_search call mints the delegate of exactly the binding it prepared,
/// and only for the borrowed call it handed its leaf. A governed context
/// without its prepared check, a check without its work, a call whose plan
/// names no target, another owner's target (the catalog control, a bundle
/// operation named web_search) or two targets are malformed facts.
#[tokio::test]
async fn a_tool_delegate_is_minted_only_from_the_dispatch_own_builtin_web_search_call() {
    let work = crate::WorkAuthorizationContext::new(
        Arc::new(AllowAll),
        crate::OperationExecutionScope::Domain,
    );
    let governed = ToolDispatchContext::default().with_work_authorization(Some(work.clone()));

    let ungoverned = dispatch_once(
        &owner(vec![builtin_web_search()]),
        &ToolDispatchContext::default(),
    )
    .await;
    assert!(
        matches!(ungoverned.own, Ok(None)),
        "ungoverned: no delegate"
    );
    assert!(ungoverned.parent.is_none());

    let Seen {
        own,
        lookalike,
        parent,
    } = dispatch_once(&owner(vec![builtin_web_search()]), &governed).await;
    let parent = parent.expect("the governed dispatch prepared its own call");
    let attribution = own.expect("minted").expect("governed");
    assert!(attribution.parent_binding().same_operation(&parent));
    assert!(attribution.work_authorization().same_context(&work));
    assert_eq!(attribution.role(), ToolDelegateRole::HostedSearchInference);
    assert!(matches!(
        &parent.facts().operation,
        AuthorizationOperation::Tool(tool) if tool.call_id.as_ref() == "search-call"
    ));
    assert!(
        malformed(&lookalike),
        "an equal-looking call is not the dispatch's own call"
    );

    let args = serde_json::value::RawValue::from_string("{}".to_string()).expect("raw arguments");
    let call = ToolCallView {
        id: "search-call",
        name: "web_search",
        args: &args,
    };
    assert!(
        malformed(
            &governed.tool_delegate_attribution(call, ToolDelegateRole::HostedSearchInference)
        ),
        "work without its prepared check"
    );
    let check = PreparedOperationCheck::prepare(work.clone(), parent.clone()).expect("prepared");
    assert!(
        malformed(
            &ToolDispatchContext::default()
                .with_prepared_authorization(check)
                .tool_delegate_attribution(call, ToolDelegateRole::HostedSearchInference)
        ),
        "a prepared check without its work"
    );

    let bundle = ToolOperationTarget::RegisteredToolBundleOperation {
        bundle: "host".into(),
        operation: "web_search".into(),
        account: Arc::new(crate::OperationAccount::Unavailable),
    };
    for (label, targets) in [
        ("no target", Vec::new()),
        (
            "the catalog control",
            vec![ToolOperationTarget::CatalogControl {
                operation: CatalogOperation::Search,
            }],
        ),
        ("a bundle operation named web_search", vec![bundle]),
        (
            "two targets",
            vec![builtin_web_search(), builtin_web_search()],
        ),
    ] {
        let seen = dispatch_once(&owner(targets), &governed).await;
        assert!(
            seen.parent.is_some(),
            "{label}: the call itself was prepared"
        );
        assert!(malformed(&seen.own), "{label}");
    }
}
