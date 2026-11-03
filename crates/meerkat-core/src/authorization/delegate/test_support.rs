//! Test support: a delegate minted the only way production mints one, by a
//! real governed fenced dispatch of a builtin call whose plan carries the
//! builtin target, as the builtin's registration owner states it.

use std::sync::{Arc, Mutex};

use super::{ToolDelegateAttribution, ToolDelegateRole};
use crate::agent::{
    AgentToolDispatcher, ToolDispatchContext, dispatch_tool_execution_plan_fenced,
    resolve_tool_execution_plan_fenced,
};
use crate::authorization::WorkAuthorizationContext;
use crate::tool_execution::ToolOperationTarget;
use crate::types::{ToolCallView, ToolDef, ToolResult};
use crate::{RunId, ToolError};

type Minted = Result<Option<ToolDelegateAttribution>, ToolError>;

/// One `web_search` call whose plan carries the builtin target; its leaf
/// asks the dispatch for its own delegate.
struct BuiltinSearchCall {
    role: ToolDelegateRole,
    minted: Mutex<Option<Minted>>,
}

#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
impl AgentToolDispatcher for BuiltinSearchCall {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::from([Arc::new(ToolDef::new(
            "web_search",
            "builtin web search",
            serde_json::json!({"type": "object"}),
        ))])
    }

    fn resolve_execution_plan(
        &self,
        call: ToolCallView<'_>,
        _dispatch_context: &ToolDispatchContext,
        resolution_context: &crate::ToolExecutionResolutionContext,
    ) -> Result<crate::ResolvedToolExecutionPlan, crate::ToolExecutionResolutionError> {
        crate::ToolExecutionContract::default()
            .resolve_default(resolution_context.deadlines().clone())?
            .with_owner_witness(
                crate::ToolExecutionOwnerWitness::new(
                    "builtin-tools:test-support",
                    call.name,
                    self.execution_binding_fingerprint(call.name)?,
                )?
                .with_operation_target(ToolOperationTarget::BuiltinOperation {
                    operation: self.role.parent_operation(),
                }),
            )
    }

    async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<crate::ops::ToolDispatchOutcome, ToolError> {
        Ok(ToolResult::new(call.id.to_string(), "ungoverned".to_string(), false).into())
    }

    async fn dispatch_resolved_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        _plan: &crate::ResolvedToolExecutionPlan,
    ) -> Result<crate::ops::ToolDispatchOutcome, ToolError> {
        let minted = context.tool_delegate_attribution(call, self.role);
        if let Ok(mut slot) = self.minted.lock() {
            *slot = Some(minted);
        }
        Ok(ToolResult::new(call.id.to_string(), "searched".to_string(), false).into())
    }
}

/// Test support: one real governed fenced dispatch of a builtin `web_search`
/// call under `work` (for the staged run `run_id`, when it has one), and the
/// delegate its leaf mints for `role` from its own call. A call the work
/// refuses is that refusal, before any leaf.
#[doc(hidden)]
pub async fn __test_mint_tool_delegate(
    work: &WorkAuthorizationContext,
    run_id: Option<RunId>,
    role: ToolDelegateRole,
) -> Result<ToolDelegateAttribution, ToolError> {
    let owner = Arc::new(BuiltinSearchCall {
        role,
        minted: Mutex::new(None),
    });
    let mut context = ToolDispatchContext::default().with_work_authorization(Some(work.clone()));
    if let Some(run_id) = run_id {
        context.bind_run_id(run_id);
    }
    let args = serde_json::value::RawValue::from_string(r#"{"query":"test search"}"#.to_string())
        .map_err(|error| ToolError::execution_failed(error.to_string()))?;
    let call = ToolCallView {
        id: "test-search-call",
        name: "web_search",
        args: &args,
    };
    let resolution = crate::ToolExecutionResolutionContext::new(
        crate::ToolDeadlineChain::new(vec![crate::ToolDeadlineContributor::finite(
            crate::ToolDeadlineOwner::CoreToolDispatch,
            std::time::Duration::from_mins(1),
        )])
        .map_err(|error| ToolError::execution_failed(error.to_string()))?,
    );
    let plan = resolve_tool_execution_plan_fenced(&owner, call, &context, &resolution)?;
    dispatch_tool_execution_plan_fenced(&owner, call, &context, &plan).await?;
    let minted = owner
        .minted
        .lock()
        .map_err(|_| ToolError::execution_failed("delegate slot poisoned"))?
        .take()
        .ok_or_else(|| ToolError::execution_failed("the call never reached its leaf"))?;
    minted?.ok_or_else(|| ToolError::execution_failed("an ungoverned call has no delegate"))
}
