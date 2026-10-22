//! Governed service calls: provider calls that no runtime input carries.
//!
//! A scheduled host runnable (for example a collector that reads a provider
//! through a connector-bound MCP server) runs host code with no agent turn,
//! so no admitted run supplies its work authorization. A connector-bound
//! call without one is refused (`meerkat::ConnectorMcpCallContextProvider`).
//! This module is the native seam for that work: the trusted host presents
//! a [`ServiceCallRequest`] naming the service original it enrolled at
//! operator-reviewed setup, and the machine's installed native host admits
//! it (see [`NativeWorkAuthorizationHost::service_call_authorization`]).
//! Every call made through the returned [`ServiceCall`] then runs the
//! ordinary fenced dispatch: prepared from the work's current owners (the
//! work owner, the operator's grant selected for exactly the operation's
//! MCP server, the operation owner), checked again at the call's entry, and
//! audited. Nothing here grants anything, and no run's authority is
//! borrowed.
//!
//! [`NativeWorkAuthorizationHost::service_call_authorization`]:
//! crate::input_authority::NativeWorkAuthorizationHost::service_call_authorization

use std::sync::Arc;

use meerkat_core::types::ToolCallView;
use meerkat_core::{
    AgentToolDispatcher, OperationAuthorizationError, ToolDispatchContext, ToolDispatchOutcome,
    ToolError, ToolExecutionResolutionContext,
};

use super::MeerkatMachine;
use crate::input_authority::ServiceCallRequest;

impl MeerkatMachine {
    /// Admit `request`, one schedule occurrence's governed service calls,
    /// through this machine's installed native host.
    ///
    /// # Errors
    /// `Unavailable` on an ungoverned machine, on a host or owner without
    /// service call support, or when an owner cannot say now; `Refused`
    /// (`Denied`) for an original that is not enrolled or is cancelled, and
    /// (`MalformedFacts`) for a request whose association is not the
    /// enrolled record.
    pub fn prepare_service_call(
        &self,
        request: Arc<ServiceCallRequest>,
    ) -> Result<ServiceCall, OperationAuthorizationError> {
        // An ungoverned machine has no native host, and so no service call:
        // no ungoverned context is ever handed out for one.
        let attachment = self
            .native_work_authorization_host
            .get()
            .ok_or(OperationAuthorizationError::Unavailable)?;
        self.shared
            .require_governed_execution_custody()
            .map_err(|_| OperationAuthorizationError::Unavailable)?;
        let work = attachment.host().service_call_authorization(request)?;
        Ok(ServiceCall {
            context: ToolDispatchContext::default().with_work_authorization(Some(work)),
        })
    }
}

/// One admitted schedule occurrence's governed service calls.
///
/// Not `Clone`: the runnable that prepared it owns it for its one
/// occurrence and drops it when the occurrence's run returns. Core builds
/// and keeps the dispatch context; the host's dispatcher receives it during
/// each dispatch, as every dispatcher does, and a context it keeps
/// authorizes no other call, because each entry needs a prepared check
/// current for that exact call.
pub struct ServiceCall {
    context: ToolDispatchContext,
}

impl ServiceCall {
    /// Make one call through `dispatcher` (for example the
    /// `McpRouterAdapter` the connector-bound server is staged on) under
    /// this occurrence's work authorization: the fenced plan resolution and
    /// dispatch every governed tool call takes. The governed decision is
    /// prepared and checked before any dispatcher code runs, so a refused
    /// operation never reaches the connector credential owner.
    ///
    /// # Errors
    /// `AuthorizationRefused` for a governed refusal, and the dispatcher's
    /// own errors (a connector call-context refusal included); nothing is
    /// sent on any refusal.
    pub async fn dispatch<T: AgentToolDispatcher + ?Sized + 'static>(
        &self,
        dispatcher: &Arc<T>,
        call: ToolCallView<'_>,
        resolution: ToolExecutionResolutionContext,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        let plan = meerkat_core::resolve_tool_execution_plan_fenced(
            dispatcher,
            call,
            &self.context,
            &resolution,
        )?;
        meerkat_core::dispatch_tool_execution_plan_fenced(dispatcher, call, &self.context, &plan)
            .await
    }
}

impl std::fmt::Debug for ServiceCall {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ServiceCall([REDACTED])")
    }
}
