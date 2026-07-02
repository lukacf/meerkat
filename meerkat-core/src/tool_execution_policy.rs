//! Call-level tool execution authorization.
//!
//! [`ToolExecutionPolicy`] is the sealed, resolved form of the per-launch
//! [`crate::ops::ToolAccessPolicy`] vocabulary. `Inherit` is **not**
//! resolvable at this seam: the spawn chain resolves `Inherit` to the
//! parent's effective policy before the execution gate is constructed, so an
//! unresolved `Inherit` here is a wiring fault and fails closed with a typed
//! error.
//!
//! [`ExecutionPolicyGatedDispatcher`] enforces the resolved policy at
//! dispatch time while leaving the LLM-visible tool list
//! (`tools()`/`tool_catalog()`) byte-identical, so gating never changes the
//! prompt-cache prefix. A denied call surfaces as an ordinary
//! `access_denied` [`ToolError`] which the agent loop converts into an
//! `is_error` tool result via `terminal_tool_outcome_for_error` — the run
//! continues. Provider-native server tools never traverse
//! [`AgentToolDispatcher`] and cannot be gated here; hosts that need them
//! gated must disable the native capability on gated builds.

use crate::agent::{
    AgentToolDispatcher, BindOutcome, DispatcherCapabilities, ExternalToolUpdate,
    OpsLifecycleBindError, ToolDispatchContext,
};
use crate::error::ToolError;
use crate::ops::ToolAccessPolicy;
use crate::tool_catalog::{ToolCatalogCapabilities, ToolCatalogEntry};
use crate::types::{ToolCallView, ToolDef, ToolNameSet};
use async_trait::async_trait;
use std::sync::Arc;

/// Error resolving a [`ToolAccessPolicy`] into a [`ToolExecutionPolicy`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ToolExecutionPolicyError {
    /// `Inherit` reached the execution seam unresolved.
    ///
    /// The spawn chain owns `Inherit` resolution (a child inherits the
    /// parent's effective policy; host/operator launches with no parent
    /// resolve to unrestricted). By the time a policy is turned into an
    /// execution gate it must be a concrete allow/deny shape.
    #[error(
        "tool access policy 'inherit' is unresolved at the execution seam; \
         the spawn chain must resolve it to the parent's effective policy \
         before the dispatch gate is built"
    )]
    UnresolvedInherit,
}

impl ToolExecutionPolicyError {
    /// Stable machine-readable error code for wire surfaces.
    pub fn error_code(&self) -> &'static str {
        match self {
            Self::UnresolvedInherit => "tool_execution_policy_unresolved_inherit",
        }
    }
}

/// Sealed resolved form of a per-launch tool access policy.
///
/// Constructed only through [`ToolExecutionPolicy::unrestricted`] or the
/// fallible [`ToolExecutionPolicy::resolve`]; the shape is private so no
/// caller can mint a policy that skipped `Inherit` resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolExecutionPolicy {
    kind: ToolExecutionPolicyKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ToolExecutionPolicyKind {
    /// Every tool call is permitted (host authority; no gating applied).
    Unrestricted,
    /// Only the named tools may execute.
    AllowList(ToolNameSet),
    /// The named tools may not execute.
    DenyList(ToolNameSet),
}

impl ToolExecutionPolicy {
    /// A policy that permits every tool call.
    #[must_use]
    pub fn unrestricted() -> Self {
        Self {
            kind: ToolExecutionPolicyKind::Unrestricted,
        }
    }

    /// Resolve the per-launch [`ToolAccessPolicy`] vocabulary into the sealed
    /// execution form.
    ///
    /// `AllowList`/`DenyList` carry over directly. `Inherit` fails with
    /// [`ToolExecutionPolicyError::UnresolvedInherit`] — the spawn chain must
    /// have replaced it with the parent's effective policy before this point.
    pub fn resolve(policy: ToolAccessPolicy) -> Result<Self, ToolExecutionPolicyError> {
        match policy {
            ToolAccessPolicy::Inherit => Err(ToolExecutionPolicyError::UnresolvedInherit),
            ToolAccessPolicy::AllowList(names) => Ok(Self {
                kind: ToolExecutionPolicyKind::AllowList(names),
            }),
            ToolAccessPolicy::DenyList(names) => Ok(Self {
                kind: ToolExecutionPolicyKind::DenyList(names),
            }),
        }
    }

    /// Whether this policy permits every tool call.
    #[must_use]
    pub fn is_unrestricted(&self) -> bool {
        matches!(self.kind, ToolExecutionPolicyKind::Unrestricted)
    }

    /// Whether a call to the named tool is permitted by this policy.
    #[must_use]
    pub fn permits(&self, name: &str) -> bool {
        match &self.kind {
            ToolExecutionPolicyKind::Unrestricted => true,
            ToolExecutionPolicyKind::AllowList(names) => names.contains(name),
            ToolExecutionPolicyKind::DenyList(names) => !names.contains(name),
        }
    }
}

/// A tool dispatcher that gates execution behind a [`ToolExecutionPolicy`]
/// while forwarding the entire [`AgentToolDispatcher`] surface unchanged.
///
/// Unlike [`crate::agent::FilteredToolDispatcher`] (a list-changing
/// visibility filter), this wrapper leaves `tools()`/`tool_catalog()`
/// byte-identical and denies only inside `dispatch`/`dispatch_with_context`,
/// so the denial reaches the transcript as an ordinary `is_error` tool
/// result and the run continues. It explicitly forwards
/// `bind_mcp_server_lifecycle_handle` and `bind_external_tool_surface_handle`
/// (which `FilteredToolDispatcher` does not) so wrapping never strips MCP
/// DSL lifecycle mirroring from the inner dispatcher.
pub struct ExecutionPolicyGatedDispatcher<T: AgentToolDispatcher + ?Sized> {
    inner: Arc<T>,
    policy: ToolExecutionPolicy,
}

impl<T: AgentToolDispatcher + ?Sized> ExecutionPolicyGatedDispatcher<T> {
    /// Wrap `inner` with the resolved execution policy.
    pub fn new(inner: Arc<T>, policy: ToolExecutionPolicy) -> Self {
        Self { inner, policy }
    }

    /// Deny verdict for a call, preserving the `not_found` vs `access_denied`
    /// distinction: a name the inner dispatcher does not know is `not_found`
    /// (the tool genuinely does not exist), a known name blocked by policy is
    /// `access_denied` (precedent: `FilteredToolDispatcher`'s dispatch-deny
    /// arm).
    fn denial_error(&self, name: &str) -> ToolError {
        let inner_knows_tool = if self.inner.tool_catalog_capabilities().exact_catalog {
            self.inner
                .tool_catalog()
                .iter()
                .any(|entry| entry.tool.name == name)
        } else {
            self.inner.tools().iter().any(|tool| tool.name == name)
        };
        if inner_knows_tool {
            ToolError::access_denied(name)
        } else {
            ToolError::not_found(name)
        }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl<T: AgentToolDispatcher + ?Sized + 'static> AgentToolDispatcher
    for ExecutionPolicyGatedDispatcher<T>
{
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        self.inner.tools()
    }

    fn tool_catalog_capabilities(&self) -> ToolCatalogCapabilities {
        self.inner.tool_catalog_capabilities()
    }

    fn tool_catalog(&self) -> Arc<[ToolCatalogEntry]> {
        self.inner.tool_catalog()
    }

    fn pending_catalog_sources(&self) -> Arc<[String]> {
        self.inner.pending_catalog_sources()
    }

    async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<crate::ops::ToolDispatchOutcome, ToolError> {
        if !self.policy.permits(call.name) {
            return Err(self.denial_error(call.name));
        }
        self.inner.dispatch(call).await
    }

    async fn dispatch_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
    ) -> Result<crate::ops::ToolDispatchOutcome, ToolError> {
        if !self.policy.permits(call.name) {
            return Err(self.denial_error(call.name));
        }
        self.inner.dispatch_with_context(call, context).await
    }

    async fn poll_external_updates(&self) -> ExternalToolUpdate {
        self.inner.poll_external_updates().await
    }

    fn external_tool_surface_snapshot(&self) -> Option<crate::ExternalToolSurfaceSnapshot> {
        self.inner.external_tool_surface_snapshot()
    }

    fn capabilities(&self) -> DispatcherCapabilities {
        self.inner.capabilities()
    }

    fn bind_ops_lifecycle(
        self: Arc<Self>,
        registry: Arc<dyn crate::ops_lifecycle::OpsLifecycleRegistry>,
        owner_bridge_session_id: crate::types::SessionId,
    ) -> Result<BindOutcome, OpsLifecycleBindError> {
        let owned = Arc::try_unwrap(self).map_err(|_| OpsLifecycleBindError::SharedOwnership)?;
        if Arc::strong_count(&owned.inner) == 1 {
            let outcome = owned
                .inner
                .bind_ops_lifecycle(registry, owner_bridge_session_id)?;
            let bound = outcome.was_bound();
            let inner = outcome.into_dispatcher();
            let gated = Arc::new(ExecutionPolicyGatedDispatcher::new(inner, owned.policy));
            Ok(if bound {
                BindOutcome::Bound(gated)
            } else {
                BindOutcome::Skipped(gated)
            })
        } else {
            Ok(BindOutcome::Skipped(Arc::new(
                ExecutionPolicyGatedDispatcher {
                    inner: owned.inner,
                    policy: owned.policy,
                },
            )))
        }
    }

    fn completion_enrichment(
        &self,
    ) -> Option<Arc<dyn crate::completion_feed::CompletionEnrichmentProvider>> {
        self.inner.completion_enrichment()
    }

    fn bind_mcp_server_lifecycle_handle(
        &self,
        handle: Arc<dyn crate::handles::McpServerLifecycleHandle>,
    ) {
        self.inner.bind_mcp_server_lifecycle_handle(handle);
    }

    fn bind_external_tool_surface_handle(
        &self,
        handle: Arc<dyn crate::handles::ExternalToolSurfaceHandle>,
    ) {
        self.inner.bind_external_tool_surface_handle(handle);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::handles::{
        DslTransitionError, ExternalToolSurfaceHandle, ExternalToolSurfaceInput,
        ExternalToolSurfaceTransition, McpServerLifecycleHandle, SurfaceDiagnosticSnapshot,
        SurfaceSnapshot,
    };
    use crate::ops_lifecycle::{
        OperationCompletionWatch, OperationLifecycleSnapshot, OperationPeerHandle,
        OperationProgressUpdate, OpsLifecycleError, OpsLifecycleRegistry,
    };
    use crate::tool_scope::ExternalToolSurfaceGlobalPhase;
    use crate::types::ToolResult;
    use std::collections::BTreeSet;
    use std::sync::Mutex;

    fn tool_def(name: &str) -> Arc<ToolDef> {
        Arc::new(ToolDef::new(
            name,
            format!("test tool {name}"),
            serde_json::json!({ "type": "object" }),
        ))
    }

    fn empty_args() -> Box<serde_json::value::RawValue> {
        serde_json::value::RawValue::from_string("{}".to_string()).expect("valid args json")
    }

    /// Inner test dispatcher that records dispatched names and supports the
    /// optional bind surfaces so forwarding can be asserted.
    struct SpyDispatcher {
        tools: Arc<[Arc<ToolDef>]>,
        dispatched: Mutex<Vec<String>>,
        ops_bound: Mutex<bool>,
        mcp_handles_bound: Mutex<usize>,
        surface_handles_bound: Mutex<usize>,
    }

    impl SpyDispatcher {
        fn new(names: &[&str]) -> Self {
            Self {
                tools: names.iter().map(|name| tool_def(name)).collect(),
                dispatched: Mutex::new(Vec::new()),
                ops_bound: Mutex::new(false),
                mcp_handles_bound: Mutex::new(0),
                surface_handles_bound: Mutex::new(0),
            }
        }

        fn dispatched(&self) -> Vec<String> {
            self.dispatched.lock().unwrap().clone()
        }
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl AgentToolDispatcher for SpyDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            Arc::clone(&self.tools)
        }

        async fn dispatch(
            &self,
            call: ToolCallView<'_>,
        ) -> Result<crate::ops::ToolDispatchOutcome, ToolError> {
            self.dispatched.lock().unwrap().push(call.name.to_string());
            Ok(crate::ops::ToolDispatchOutcome::from(ToolResult::new(
                call.id.to_string(),
                "ok".to_string(),
                false,
            )))
        }

        fn capabilities(&self) -> DispatcherCapabilities {
            DispatcherCapabilities {
                ops_lifecycle: true,
            }
        }

        fn bind_ops_lifecycle(
            self: Arc<Self>,
            _registry: Arc<dyn OpsLifecycleRegistry>,
            _owner_bridge_session_id: crate::types::SessionId,
        ) -> Result<BindOutcome, OpsLifecycleBindError> {
            *self.ops_bound.lock().unwrap() = true;
            Ok(BindOutcome::Bound(self))
        }

        fn bind_mcp_server_lifecycle_handle(&self, _handle: Arc<dyn McpServerLifecycleHandle>) {
            *self.mcp_handles_bound.lock().unwrap() += 1;
        }

        fn bind_external_tool_surface_handle(&self, _handle: Arc<dyn ExternalToolSurfaceHandle>) {
            *self.surface_handles_bound.lock().unwrap() += 1;
        }
    }

    /// Fully-unsupported registry stub: the spy dispatcher never calls into
    /// the registry, so every method fails loud if it ever does.
    struct UnsupportedOpsRegistry;

    fn unsupported(op: &str) -> OpsLifecycleError {
        OpsLifecycleError::Unsupported(op.into())
    }

    impl OpsLifecycleRegistry for UnsupportedOpsRegistry {
        fn register_operation(
            &self,
            _spec: crate::ops_lifecycle::OperationSpec,
        ) -> Result<(), OpsLifecycleError> {
            Err(unsupported("register_operation"))
        }

        fn provisioning_succeeded(
            &self,
            _id: &crate::ops::OperationId,
        ) -> Result<(), OpsLifecycleError> {
            Err(unsupported("provisioning_succeeded"))
        }

        fn provisioning_failed(
            &self,
            _id: &crate::ops::OperationId,
            _error: String,
        ) -> Result<(), OpsLifecycleError> {
            Err(unsupported("provisioning_failed"))
        }

        fn peer_ready(
            &self,
            _id: &crate::ops::OperationId,
            _peer: OperationPeerHandle,
        ) -> Result<(), OpsLifecycleError> {
            Err(unsupported("peer_ready"))
        }

        fn register_watcher(
            &self,
            _id: &crate::ops::OperationId,
        ) -> Result<OperationCompletionWatch, OpsLifecycleError> {
            Err(unsupported("register_watcher"))
        }

        fn report_progress(
            &self,
            _id: &crate::ops::OperationId,
            _update: OperationProgressUpdate,
        ) -> Result<(), OpsLifecycleError> {
            Err(unsupported("report_progress"))
        }

        fn complete_operation(
            &self,
            _id: &crate::ops::OperationId,
            _result: crate::ops::OperationResult,
        ) -> Result<(), OpsLifecycleError> {
            Err(unsupported("complete_operation"))
        }

        fn fail_operation(
            &self,
            _id: &crate::ops::OperationId,
            _error: String,
        ) -> Result<(), OpsLifecycleError> {
            Err(unsupported("fail_operation"))
        }

        fn abort_provisioning(
            &self,
            _id: &crate::ops::OperationId,
            _reason: Option<String>,
        ) -> Result<(), OpsLifecycleError> {
            Err(unsupported("abort_provisioning"))
        }

        fn cancel_operation(
            &self,
            _id: &crate::ops::OperationId,
            _reason: Option<String>,
        ) -> Result<(), OpsLifecycleError> {
            Err(unsupported("cancel_operation"))
        }

        fn request_retire(&self, _id: &crate::ops::OperationId) -> Result<(), OpsLifecycleError> {
            Err(unsupported("request_retire"))
        }

        fn mark_retired(&self, _id: &crate::ops::OperationId) -> Result<(), OpsLifecycleError> {
            Err(unsupported("mark_retired"))
        }

        fn snapshot(
            &self,
            _id: &crate::ops::OperationId,
        ) -> Result<Option<OperationLifecycleSnapshot>, OpsLifecycleError> {
            Err(unsupported("snapshot"))
        }

        fn list_operations(&self) -> Result<Vec<OperationLifecycleSnapshot>, OpsLifecycleError> {
            Err(unsupported("list_operations"))
        }

        fn terminate_owner(&self, _reason: String) -> Result<(), OpsLifecycleError> {
            Err(unsupported("terminate_owner"))
        }
    }

    struct NoopMcpLifecycleHandle;

    impl McpServerLifecycleHandle for NoopMcpLifecycleHandle {
        fn apply_connect_pending(&self, _server_id: &str) -> Result<(), DslTransitionError> {
            Ok(())
        }

        fn apply_connected(&self, _server_id: &str) -> Result<(), DslTransitionError> {
            Ok(())
        }

        fn apply_failed(&self, _server_id: &str, _error: &str) -> Result<(), DslTransitionError> {
            Ok(())
        }

        fn apply_disconnected(&self, _server_id: &str) -> Result<(), DslTransitionError> {
            Ok(())
        }

        fn apply_reload(&self, _server_id: &str) -> Result<(), DslTransitionError> {
            Ok(())
        }

        fn pending_server_ids(&self) -> BTreeSet<String> {
            BTreeSet::new()
        }
    }

    /// Rejecting external tool-surface handle stub — the spy dispatcher only
    /// records the bind, so no method is ever exercised.
    struct RejectingSurfaceHandle;

    impl RejectingSurfaceHandle {
        fn reject(context: &'static str) -> DslTransitionError {
            DslTransitionError::guard_rejected(context, "test stub rejects all surface inputs")
        }

        fn empty_snapshot() -> SurfaceDiagnosticSnapshot {
            SurfaceDiagnosticSnapshot {
                surface_phase: ExternalToolSurfaceGlobalPhase::Operating,
                known_surfaces: BTreeSet::new(),
                visible_surfaces: BTreeSet::new(),
                snapshot_epoch: 0,
                snapshot_aligned_epoch: 0,
                has_pending_or_staged: false,
                entries: Vec::new(),
            }
        }
    }

    impl ExternalToolSurfaceHandle for RejectingSurfaceHandle {
        fn apply_surface_input(
            &self,
            _input: ExternalToolSurfaceInput,
        ) -> Result<ExternalToolSurfaceTransition, DslTransitionError> {
            Err(Self::reject("RejectingSurfaceHandle::apply_surface_input"))
        }

        fn register(&self, _surface_id: String) -> Result<(), DslTransitionError> {
            Err(Self::reject("RejectingSurfaceHandle::register"))
        }

        fn stage_add(&self, _surface_id: String, _now_ms: u64) -> Result<(), DslTransitionError> {
            Err(Self::reject("RejectingSurfaceHandle::stage_add"))
        }

        fn stage_remove(
            &self,
            _surface_id: String,
            _now_ms: u64,
        ) -> Result<(), DslTransitionError> {
            Err(Self::reject("RejectingSurfaceHandle::stage_remove"))
        }

        fn stage_reload(
            &self,
            _surface_id: String,
            _now_ms: u64,
        ) -> Result<(), DslTransitionError> {
            Err(Self::reject("RejectingSurfaceHandle::stage_reload"))
        }

        fn apply_boundary(
            &self,
            _surface_id: String,
            _now_ms: u64,
            _staged_intent_sequence: u64,
            _applied_at_turn: u64,
        ) -> Result<(), DslTransitionError> {
            Err(Self::reject("RejectingSurfaceHandle::apply_boundary"))
        }

        fn mark_pending_succeeded(
            &self,
            _surface_id: String,
            _pending_task_sequence: u64,
            _staged_intent_sequence: u64,
        ) -> Result<(), DslTransitionError> {
            Err(Self::reject(
                "RejectingSurfaceHandle::mark_pending_succeeded",
            ))
        }

        fn mark_pending_failed(
            &self,
            _surface_id: String,
            _pending_task_sequence: u64,
            _staged_intent_sequence: u64,
            _cause: crate::tool_scope::ExternalToolSurfaceFailureCause,
        ) -> Result<(), DslTransitionError> {
            Err(Self::reject("RejectingSurfaceHandle::mark_pending_failed"))
        }

        fn call_started(&self, _surface_id: String) -> Result<(), DslTransitionError> {
            Err(Self::reject("RejectingSurfaceHandle::call_started"))
        }

        fn call_finished(&self, _surface_id: String) -> Result<(), DslTransitionError> {
            Err(Self::reject("RejectingSurfaceHandle::call_finished"))
        }

        fn finalize_removal_clean(&self, _surface_id: String) -> Result<(), DslTransitionError> {
            Err(Self::reject(
                "RejectingSurfaceHandle::finalize_removal_clean",
            ))
        }

        fn finalize_removal_forced(&self, _surface_id: String) -> Result<(), DslTransitionError> {
            Err(Self::reject(
                "RejectingSurfaceHandle::finalize_removal_forced",
            ))
        }

        fn snapshot_aligned(&self, _epoch: u64) -> Result<(), DslTransitionError> {
            Err(Self::reject("RejectingSurfaceHandle::snapshot_aligned"))
        }

        fn shutdown_surface(&self) -> Result<(), DslTransitionError> {
            Err(Self::reject("RejectingSurfaceHandle::shutdown_surface"))
        }

        fn surface_snapshot(&self, _surface_id: &str) -> Option<SurfaceSnapshot> {
            None
        }

        fn diagnostic_snapshot(&self) -> SurfaceDiagnosticSnapshot {
            Self::empty_snapshot()
        }

        fn visible_surfaces(&self) -> BTreeSet<String> {
            BTreeSet::new()
        }

        fn removing_surfaces(&self) -> BTreeSet<String> {
            BTreeSet::new()
        }

        fn pending_surfaces(&self) -> BTreeSet<String> {
            BTreeSet::new()
        }

        fn has_pending_or_staged(&self) -> bool {
            false
        }

        fn snapshot_epoch(&self) -> u64 {
            0
        }

        fn snapshot_aligned_epoch(&self) -> u64 {
            0
        }
    }

    fn allow_list(names: &[&str]) -> ToolExecutionPolicy {
        ToolExecutionPolicy::resolve(ToolAccessPolicy::AllowList(names.iter().copied().collect()))
            .expect("allow list resolves")
    }

    fn deny_list(names: &[&str]) -> ToolExecutionPolicy {
        ToolExecutionPolicy::resolve(ToolAccessPolicy::DenyList(names.iter().copied().collect()))
            .expect("deny list resolves")
    }

    async fn dispatch_named<T: AgentToolDispatcher + ?Sized>(
        dispatcher: &T,
        name: &str,
    ) -> Result<crate::ops::ToolDispatchOutcome, ToolError> {
        let args = empty_args();
        let call = ToolCallView {
            id: "call-1",
            name,
            args: &args,
        };
        dispatcher.dispatch(call).await
    }

    async fn dispatch_named_with_context<T: AgentToolDispatcher + ?Sized>(
        dispatcher: &T,
        name: &str,
    ) -> Result<crate::ops::ToolDispatchOutcome, ToolError> {
        let args = empty_args();
        let call = ToolCallView {
            id: "call-1",
            name,
            args: &args,
        };
        dispatcher
            .dispatch_with_context(call, &ToolDispatchContext::default())
            .await
    }

    // ── Resolver ─────────────────────────────────────────────────────────

    #[test]
    fn resolve_inherit_fails_closed_with_typed_error() {
        let err = ToolExecutionPolicy::resolve(ToolAccessPolicy::Inherit)
            .expect_err("inherit must not resolve at the execution seam");
        assert_eq!(err, ToolExecutionPolicyError::UnresolvedInherit);
        assert_eq!(err.error_code(), "tool_execution_policy_unresolved_inherit");
    }

    #[test]
    fn resolve_allow_and_deny_lists_carry_over() {
        assert!(allow_list(&["a"]).permits("a"));
        assert!(!allow_list(&["a"]).permits("b"));
        assert!(!deny_list(&["a"]).permits("a"));
        assert!(deny_list(&["a"]).permits("b"));
        assert!(ToolExecutionPolicy::unrestricted().permits("anything"));
        assert!(ToolExecutionPolicy::unrestricted().is_unrestricted());
        assert!(!allow_list(&["a"]).is_unrestricted());
        assert!(!deny_list(&["a"]).is_unrestricted());
    }

    // ── List preservation ────────────────────────────────────────────────

    #[test]
    fn gated_dispatcher_preserves_tools_and_catalog_byte_identically() {
        let inner = Arc::new(SpyDispatcher::new(&["alpha", "beta", "gamma"]));
        let inner_tools = inner.tools();
        let inner_catalog = inner.tool_catalog();
        let gated = ExecutionPolicyGatedDispatcher::new(Arc::clone(&inner), allow_list(&["alpha"]));

        let gated_tools = gated.tools();
        assert_eq!(gated_tools.len(), inner_tools.len());
        for (gated_tool, inner_tool) in gated_tools.iter().zip(inner_tools.iter()) {
            // Same Arc — content AND identity preserved, so the LLM-visible
            // list (and the prompt-cache prefix derived from it) is unchanged.
            assert!(Arc::ptr_eq(gated_tool, inner_tool));
        }
        let gated_catalog = gated.tool_catalog();
        assert_eq!(gated_catalog.len(), inner_catalog.len());
        for (gated_entry, inner_entry) in gated_catalog.iter().zip(inner_catalog.iter()) {
            assert!(Arc::ptr_eq(&gated_entry.tool, &inner_entry.tool));
        }
        assert_eq!(
            gated.tool_catalog_capabilities(),
            inner.tool_catalog_capabilities()
        );
        assert_eq!(gated.capabilities(), inner.capabilities());
    }

    // ── Allow/deny matrices ──────────────────────────────────────────────

    #[tokio::test]
    async fn allow_list_matrix_permits_listed_denies_rest() {
        let inner = Arc::new(SpyDispatcher::new(&["alpha", "beta"]));
        let gated = ExecutionPolicyGatedDispatcher::new(Arc::clone(&inner), allow_list(&["alpha"]));

        let outcome = dispatch_named(&gated, "alpha")
            .await
            .expect("allow-listed tool must dispatch");
        assert!(!outcome.result.is_error);

        let err = dispatch_named(&gated, "beta")
            .await
            .expect_err("non-listed known tool must be denied");
        assert_eq!(err, ToolError::access_denied("beta"));
        assert_eq!(err.error_code(), "access_denied");

        // Unknown name blocked by policy stays not_found — the gate must not
        // claim a policy denial for a tool that does not exist.
        let err = dispatch_named(&gated, "missing")
            .await
            .expect_err("unknown tool must not dispatch");
        assert_eq!(err, ToolError::not_found("missing"));

        assert_eq!(inner.dispatched(), vec!["alpha".to_string()]);
    }

    #[tokio::test]
    async fn deny_list_matrix_denies_listed_permits_rest() {
        let inner = Arc::new(SpyDispatcher::new(&["alpha", "beta"]));
        let gated = ExecutionPolicyGatedDispatcher::new(Arc::clone(&inner), deny_list(&["beta"]));

        dispatch_named_with_context(&gated, "alpha")
            .await
            .expect("non-denied tool must dispatch");

        let err = dispatch_named_with_context(&gated, "beta")
            .await
            .expect_err("deny-listed tool must be denied");
        assert_eq!(err, ToolError::access_denied("beta"));

        // Deny-listed name unknown to the inner dispatcher: not_found (the
        // tool does not exist; the policy verdict cannot invent it).
        let gated_ghost =
            ExecutionPolicyGatedDispatcher::new(Arc::clone(&inner), deny_list(&["ghost"]));
        let err = dispatch_named(&gated_ghost, "ghost")
            .await
            .expect_err("unknown deny-listed tool must not dispatch");
        assert_eq!(err, ToolError::not_found("ghost"));

        // Unknown name permitted by policy forwards to inner, which reports
        // its own truth (the spy dispatches anything, proving forwarding).
        dispatch_named(&gated, "gamma")
            .await
            .expect("policy-permitted unknown name forwards to inner");
        assert_eq!(
            inner.dispatched(),
            vec!["alpha".to_string(), "gamma".to_string()]
        );
    }

    #[tokio::test]
    async fn memory_search_denied_when_absent_from_allow_list() {
        let inner = Arc::new(SpyDispatcher::new(&["memory_search", "read_file"]));
        let gated =
            ExecutionPolicyGatedDispatcher::new(Arc::clone(&inner), allow_list(&["read_file"]));

        let err = dispatch_named(&gated, "memory_search")
            .await
            .expect_err("memory_search absent from allow list must be denied");
        assert_eq!(err, ToolError::access_denied("memory_search"));
        assert!(inner.dispatched().is_empty());
    }

    // ── Bind survival ────────────────────────────────────────────────────

    #[tokio::test]
    async fn bind_ops_lifecycle_rewrap_keeps_gate_and_registry_binding() {
        let inner = Arc::new(SpyDispatcher::new(&["alpha", "beta"]));
        let gated: Arc<ExecutionPolicyGatedDispatcher<SpyDispatcher>> = Arc::new(
            ExecutionPolicyGatedDispatcher::new(Arc::clone(&inner), allow_list(&["alpha"])),
        );
        // Drop the local inner handle so the wrapper holds the only strong
        // reference and the re-wrap dance can take ownership.
        let inner_probe = Arc::downgrade(&inner);
        drop(inner);

        let outcome = gated
            .bind_ops_lifecycle(
                Arc::new(UnsupportedOpsRegistry),
                crate::types::SessionId::new(),
            )
            .expect("bind must succeed through the gate");
        assert!(outcome.was_bound(), "inner binding must be applied");
        let rebound = outcome.into_dispatcher();

        let inner_alive = inner_probe
            .upgrade()
            .expect("inner dispatcher must survive rebind");
        assert!(
            *inner_alive.ops_bound.lock().unwrap(),
            "ops registry binding must reach the inner dispatcher"
        );

        // The gate must survive the re-wrap: denied tool stays denied.
        let err = dispatch_named_with_context(rebound.as_ref(), "beta")
            .await
            .expect_err("gate must survive bind_ops_lifecycle re-wrap");
        assert_eq!(err, ToolError::access_denied("beta"));
        dispatch_named_with_context(rebound.as_ref(), "alpha")
            .await
            .expect("allow-listed tool must still dispatch after re-wrap");
    }

    #[test]
    fn bind_ops_lifecycle_shared_wrapper_reports_shared_ownership() {
        let inner = Arc::new(SpyDispatcher::new(&["alpha"]));
        let gated: Arc<ExecutionPolicyGatedDispatcher<SpyDispatcher>> = Arc::new(
            ExecutionPolicyGatedDispatcher::new(Arc::clone(&inner), allow_list(&["alpha"])),
        );
        let extra_handle = Arc::clone(&gated);
        let err = match gated.bind_ops_lifecycle(
            Arc::new(UnsupportedOpsRegistry),
            crate::types::SessionId::new(),
        ) {
            Ok(_) => panic!("shared wrapper ownership must refuse rebind"),
            Err(err) => err,
        };
        assert_eq!(err, OpsLifecycleBindError::SharedOwnership);
        drop(extra_handle);
    }

    #[test]
    fn both_handle_binds_reach_inner_dispatcher() {
        let inner = Arc::new(SpyDispatcher::new(&["alpha"]));
        let gated = ExecutionPolicyGatedDispatcher::new(Arc::clone(&inner), allow_list(&["alpha"]));

        gated.bind_mcp_server_lifecycle_handle(Arc::new(NoopMcpLifecycleHandle));
        gated.bind_external_tool_surface_handle(Arc::new(RejectingSurfaceHandle));

        assert_eq!(
            *inner.mcp_handles_bound.lock().unwrap(),
            1,
            "bind_mcp_server_lifecycle_handle must forward to inner"
        );
        assert_eq!(
            *inner.surface_handles_bound.lock().unwrap(),
            1,
            "bind_external_tool_surface_handle must forward to inner"
        );
    }
}
