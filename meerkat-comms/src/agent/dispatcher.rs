//! CommsToolDispatcher - Implements AgentToolDispatcher for comms tools.

use crate::mcp::tools::{
    RuntimeCommsCommandHandle, ToolContext, comms_tool_unavailable_reason,
    handle_tools_call_with_context, tools_list,
};
use crate::{Router, TrustedPeersView};
use async_trait::async_trait;
use meerkat_core::AgentToolDispatcher;
use meerkat_core::ToolCallArguments;
use meerkat_core::ToolCallability;
use meerkat_core::ToolCatalogCapabilities;
use meerkat_core::ToolCatalogEntry;
use meerkat_core::ToolDispatchContext;
use meerkat_core::ToolDispatchOutcome;
use meerkat_core::agent::ExternalToolUpdate;
use meerkat_core::error::ToolError;
use meerkat_core::types::{ToolCallView, ToolDef, ToolProvenance, ToolResult, ToolSourceKind};
use std::sync::Arc;

/// Tool dispatcher that provides comms tools.
pub struct CommsToolDispatcher<T: AgentToolDispatcher = NoOpDispatcher> {
    tool_context: ToolContext,
    inner: Option<Arc<T>>,
    tool_defs: Arc<[Arc<ToolDef>]>,
}

impl CommsToolDispatcher<NoOpDispatcher> {
    pub fn new(router: Arc<Router>, trusted_peers: TrustedPeersView) -> Self {
        let tool_context = ToolContext {
            router,
            trusted_peers,
            runtime: None,
        };
        let tool_defs: Arc<[Arc<ToolDef>]> = comms_tool_defs().into();
        Self {
            tool_context,
            inner: None,
            tool_defs,
        }
    }

    pub fn new_with_runtime(
        router: Arc<Router>,
        trusted_peers: TrustedPeersView,
        runtime: Arc<dyn meerkat_core::agent::CommsRuntime>,
    ) -> Self {
        let tool_context = ToolContext {
            router,
            trusted_peers,
            runtime: Some(RuntimeCommsCommandHandle::new(runtime)),
        };
        let tool_defs: Arc<[Arc<ToolDef>]> = comms_tool_defs().into();
        Self {
            tool_context,
            inner: None,
            tool_defs,
        }
    }
}

impl<T: AgentToolDispatcher> CommsToolDispatcher<T> {
    pub fn with_inner(router: Arc<Router>, trusted_peers: TrustedPeersView, inner: Arc<T>) -> Self {
        let tool_context = ToolContext {
            router,
            trusted_peers,
            runtime: None,
        };
        let tool_defs: Arc<[Arc<ToolDef>]> = comms_tool_defs().into();
        Self {
            tool_context,
            inner: Some(inner),
            tool_defs,
        }
    }
}

pub struct NoOpDispatcher;

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl AgentToolDispatcher for NoOpDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::from([])
    }
    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        Err(ToolError::NotFound {
            name: call.name.into(),
        })
    }
}

fn is_comms_tool(name: &str) -> bool {
    matches!(
        name,
        "send_message" | "reply_to_peer" | "send_request" | "send_response" | "peers"
    )
}

fn callability_for_context(ctx: &ToolContext, name: &str) -> ToolCallability {
    comms_tool_unavailable_reason(ctx, name)
        .map_or_else(ToolCallability::callable, ToolCallability::unavailable)
}

fn resolve_comms_execution_plan(
    tool_context: &ToolContext,
    call: ToolCallView<'_>,
    resolution_context: &meerkat_core::ToolExecutionResolutionContext,
) -> Result<meerkat_core::ResolvedToolExecutionPlan, meerkat_core::ToolExecutionResolutionError> {
    if let Some(reason) = callability_for_context(tool_context, call.name).unavailable_reason() {
        return Err(meerkat_core::ToolExecutionResolutionError::Unavailable {
            tool_name: call.name.to_string(),
            reason,
        });
    }
    meerkat_core::ToolExecutionContract::default()
        .resolve_default(resolution_context.deadlines().clone())
        .map_err(Into::into)
}

/// Canonical JSON-to-ToolDef conversion for comms tools.
pub fn comms_tool_defs() -> Vec<Arc<ToolDef>> {
    tools_list()
        .into_iter()
        .map(|t| {
            Arc::new(ToolDef {
                name: t["name"].as_str().unwrap_or_default().into(),
                description: t["description"].as_str().unwrap_or_default().to_string(),
                input_schema: t["inputSchema"].clone(),
                provenance: Some(ToolProvenance {
                    kind: ToolSourceKind::Comms,
                    source_id: "comms".into(),
                }),
            })
        })
        .collect()
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl<T: AgentToolDispatcher + 'static> AgentToolDispatcher for CommsToolDispatcher<T> {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        if let Some(inner) = &self.inner {
            let mut tools = self
                .tool_defs
                .iter()
                .filter(|tool| {
                    callability_for_context(&self.tool_context, &tool.name).is_callable()
                })
                .cloned()
                .collect::<Vec<_>>();
            tools.extend(
                inner
                    .tools()
                    .iter()
                    .filter(|tool| !is_comms_tool(&tool.name))
                    .map(Arc::clone),
            );
            tools.into()
        } else {
            self.tool_defs
                .iter()
                .filter(|tool| {
                    callability_for_context(&self.tool_context, &tool.name).is_callable()
                })
                .cloned()
                .collect::<Vec<_>>()
                .into()
        }
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
        if is_comms_tool(call.name) {
            if let Some(reason) =
                callability_for_context(&self.tool_context, call.name).unavailable_reason()
            {
                return Err(ToolError::unavailable(call.name, reason));
            }
            // Parse tool arguments through the object-guarded typed boundary.
            // Non-object / non-JSON args fail closed with a typed
            // `InvalidArguments` error rather than falling back to a
            // `Value::String` that would silently reach the comms handler.
            let args = ToolCallArguments::from_raw_json(call.args)
                .map_err(|err| ToolError::invalid_arguments(call.name, err.to_string()))?;
            let result = handle_tools_call_with_context(
                &self.tool_context,
                call.name,
                args.as_value(),
                context,
            )
            .await
            .map_err(|e| ToolError::ExecutionFailed { message: e })?;
            Ok(ToolResult::new(call.id.to_string(), result.to_string(), false).into())
        } else if let Some(inner) = &self.inner {
            inner.dispatch_with_context(call, context).await
        } else {
            Err(ToolError::NotFound {
                name: call.name.into(),
            })
        }
    }

    async fn poll_external_updates(&self) -> ExternalToolUpdate {
        if let Some(inner) = &self.inner {
            inner.poll_external_updates().await
        } else {
            ExternalToolUpdate::default()
        }
    }

    fn tool_catalog_capabilities(&self) -> ToolCatalogCapabilities {
        let inner = self
            .inner
            .as_ref()
            .map(|dispatcher| dispatcher.tool_catalog_capabilities());
        ToolCatalogCapabilities {
            exact_catalog: inner.is_none_or(|capabilities| capabilities.exact_catalog),
            may_require_catalog_control_plane: inner
                .is_some_and(|capabilities| capabilities.may_require_catalog_control_plane),
        }
    }

    fn pending_catalog_sources(&self) -> Arc<[String]> {
        self.inner
            .as_ref()
            .map(|dispatcher| dispatcher.pending_catalog_sources())
            .unwrap_or_else(|| Arc::from([]))
    }

    fn execution_binding_fingerprint(
        &self,
        tool_name: &str,
    ) -> Result<
        meerkat_core::EphemeralToolBindingFingerprint,
        meerkat_core::ToolExecutionResolutionError,
    > {
        let catalog = self.tool_catalog();
        let entry = catalog
            .iter()
            .find(|entry| entry.tool.name == tool_name)
            .ok_or_else(|| meerkat_core::ToolExecutionResolutionError::NotFound {
                tool_name: tool_name.to_string(),
            })?;
        let fingerprint = meerkat_core::ephemeral_tool_catalog_binding_fingerprint(entry)
            .with_live_authority(0, 0);
        if is_comms_tool(tool_name) {
            Ok(fingerprint)
        } else if let Some(inner) = &self.inner {
            Ok(fingerprint.with_dependency(&inner.execution_binding_fingerprint(tool_name)?))
        } else {
            Err(meerkat_core::ToolExecutionResolutionError::NotFound {
                tool_name: tool_name.to_string(),
            })
        }
    }

    fn resolve_execution_plan(
        &self,
        call: ToolCallView<'_>,
        dispatch_context: &ToolDispatchContext,
        resolution_context: &meerkat_core::ToolExecutionResolutionContext,
    ) -> Result<meerkat_core::ResolvedToolExecutionPlan, meerkat_core::ToolExecutionResolutionError>
    {
        if is_comms_tool(call.name) {
            resolve_comms_execution_plan(&self.tool_context, call, resolution_context)
        } else if let Some(inner) = &self.inner {
            inner.resolve_execution_plan(call, dispatch_context, resolution_context)
        } else {
            Err(meerkat_core::ToolExecutionResolutionError::NotFound {
                tool_name: call.name.to_string(),
            })
        }
    }

    async fn dispatch_resolved_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        plan: &meerkat_core::ResolvedToolExecutionPlan,
    ) -> Result<meerkat_core::ToolDispatchOutcome, ToolError> {
        if is_comms_tool(call.name) {
            if plan.mode() != meerkat_core::ToolExecutionMode::Fast {
                return Err(ToolError::unavailable(
                    call.name,
                    meerkat_core::ToolUnavailableReason::ExecutionModeOwnerUnavailable,
                ));
            }
            self.dispatch_with_context(call, context).await
        } else if let Some(inner) = &self.inner {
            inner
                .dispatch_resolved_with_context(call, context, plan)
                .await
        } else {
            Err(ToolError::not_found(call.name))
        }
    }

    fn tool_catalog(&self) -> Arc<[ToolCatalogEntry]> {
        let mut catalog = self
            .tool_defs
            .iter()
            .map(|tool| {
                ToolCatalogEntry::session_inline_with_callability(
                    Arc::clone(tool),
                    callability_for_context(&self.tool_context, &tool.name),
                )
            })
            .collect::<Vec<_>>();
        if let Some(inner) = &self.inner {
            if inner.tool_catalog_capabilities().exact_catalog {
                catalog.extend(
                    inner
                        .tool_catalog()
                        .iter()
                        .filter(|entry| !is_comms_tool(&entry.tool.name))
                        .cloned(),
                );
            } else {
                catalog.extend(
                    inner
                        .tools()
                        .iter()
                        .filter(|tool| !is_comms_tool(&tool.name))
                        .map(|tool| ToolCatalogEntry::session_inline(Arc::clone(tool), true)),
                );
            }
        }
        catalog.into()
    }
}

pub struct DynCommsToolDispatcher {
    tool_context: ToolContext,
    inner: Arc<dyn AgentToolDispatcher>,
    tool_defs: Arc<[Arc<ToolDef>]>,
}

impl DynCommsToolDispatcher {
    pub fn new(
        router: Arc<Router>,
        trusted_peers: TrustedPeersView,
        inner: Arc<dyn AgentToolDispatcher>,
    ) -> Self {
        let tool_context = ToolContext {
            router,
            trusted_peers,
            runtime: None,
        };
        let tool_defs: Arc<[Arc<ToolDef>]> = comms_tool_defs().into();
        Self {
            tool_context,
            inner,
            tool_defs,
        }
    }

    pub fn new_with_runtime(
        router: Arc<Router>,
        trusted_peers: TrustedPeersView,
        runtime: Arc<dyn meerkat_core::agent::CommsRuntime>,
        inner: Arc<dyn AgentToolDispatcher>,
    ) -> Self {
        let tool_context = ToolContext {
            router,
            trusted_peers,
            runtime: Some(RuntimeCommsCommandHandle::new(runtime)),
        };
        let tool_defs: Arc<[Arc<ToolDef>]> = comms_tool_defs().into();
        Self {
            tool_context,
            inner,
            tool_defs,
        }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl AgentToolDispatcher for DynCommsToolDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        let mut tools = self
            .tool_defs
            .iter()
            .filter(|tool| callability_for_context(&self.tool_context, &tool.name).is_callable())
            .cloned()
            .collect::<Vec<_>>();
        tools.extend(
            self.inner
                .tools()
                .iter()
                .filter(|tool| !is_comms_tool(&tool.name))
                .map(Arc::clone),
        );
        tools.into()
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
        if is_comms_tool(call.name) {
            if let Some(reason) =
                callability_for_context(&self.tool_context, call.name).unavailable_reason()
            {
                return Err(ToolError::unavailable(call.name, reason));
            }
            // Parse tool arguments through the object-guarded typed boundary.
            // Non-object / non-JSON args fail closed with a typed
            // `InvalidArguments` error rather than falling back to a
            // `Value::String` that would silently reach the comms handler.
            let args = ToolCallArguments::from_raw_json(call.args)
                .map_err(|err| ToolError::invalid_arguments(call.name, err.to_string()))?;
            let result = handle_tools_call_with_context(
                &self.tool_context,
                call.name,
                args.as_value(),
                context,
            )
            .await
            .map_err(|e| ToolError::ExecutionFailed { message: e })?;
            Ok(ToolResult::new(call.id.to_string(), result.to_string(), false).into())
        } else {
            self.inner.dispatch_with_context(call, context).await
        }
    }

    async fn poll_external_updates(&self) -> ExternalToolUpdate {
        self.inner.poll_external_updates().await
    }

    fn tool_catalog_capabilities(&self) -> ToolCatalogCapabilities {
        let inner = self.inner.tool_catalog_capabilities();
        ToolCatalogCapabilities {
            exact_catalog: inner.exact_catalog,
            may_require_catalog_control_plane: inner.may_require_catalog_control_plane,
        }
    }

    fn pending_catalog_sources(&self) -> Arc<[String]> {
        self.inner.pending_catalog_sources()
    }

    fn execution_binding_fingerprint(
        &self,
        tool_name: &str,
    ) -> Result<
        meerkat_core::EphemeralToolBindingFingerprint,
        meerkat_core::ToolExecutionResolutionError,
    > {
        let catalog = self.tool_catalog();
        let entry = catalog
            .iter()
            .find(|entry| entry.tool.name == tool_name)
            .ok_or_else(|| meerkat_core::ToolExecutionResolutionError::NotFound {
                tool_name: tool_name.to_string(),
            })?;
        let fingerprint = meerkat_core::ephemeral_tool_catalog_binding_fingerprint(entry)
            .with_live_authority(0, 0);
        if is_comms_tool(tool_name) {
            Ok(fingerprint)
        } else {
            Ok(fingerprint.with_dependency(&self.inner.execution_binding_fingerprint(tool_name)?))
        }
    }

    fn resolve_execution_plan(
        &self,
        call: ToolCallView<'_>,
        dispatch_context: &ToolDispatchContext,
        resolution_context: &meerkat_core::ToolExecutionResolutionContext,
    ) -> Result<meerkat_core::ResolvedToolExecutionPlan, meerkat_core::ToolExecutionResolutionError>
    {
        if is_comms_tool(call.name) {
            resolve_comms_execution_plan(&self.tool_context, call, resolution_context)
        } else {
            self.inner
                .resolve_execution_plan(call, dispatch_context, resolution_context)
        }
    }

    async fn dispatch_resolved_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        plan: &meerkat_core::ResolvedToolExecutionPlan,
    ) -> Result<meerkat_core::ToolDispatchOutcome, ToolError> {
        if is_comms_tool(call.name) {
            if plan.mode() != meerkat_core::ToolExecutionMode::Fast {
                return Err(ToolError::unavailable(
                    call.name,
                    meerkat_core::ToolUnavailableReason::ExecutionModeOwnerUnavailable,
                ));
            }
            self.dispatch_with_context(call, context).await
        } else {
            self.inner
                .dispatch_resolved_with_context(call, context, plan)
                .await
        }
    }

    fn tool_catalog(&self) -> Arc<[ToolCatalogEntry]> {
        let mut catalog = self
            .tool_defs
            .iter()
            .map(|tool| {
                ToolCatalogEntry::session_inline_with_callability(
                    Arc::clone(tool),
                    callability_for_context(&self.tool_context, &tool.name),
                )
            })
            .collect::<Vec<_>>();
        if self.inner.tool_catalog_capabilities().exact_catalog {
            catalog.extend(
                self.inner
                    .tool_catalog()
                    .iter()
                    .filter(|entry| !is_comms_tool(&entry.tool.name))
                    .cloned(),
            );
        } else {
            catalog.extend(
                self.inner
                    .tools()
                    .iter()
                    .filter(|tool| !is_comms_tool(&tool.name))
                    .map(|tool| ToolCatalogEntry::session_inline(Arc::clone(tool), true)),
            );
        }
        catalog.into()
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::inbox::Inbox;
    use crate::trust::TrustStore;
    use meerkat_core::ToolCatalogDeferredEligibility;
    use parking_lot::RwLock;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct ExactDeferredDispatcher {
        tool: Arc<ToolDef>,
        polled: AtomicBool,
    }

    struct ContextAwareDispatcher {
        tool: Arc<ToolDef>,
    }

    struct HybridExecutionDispatcher {
        catalog: Arc<[ToolCatalogEntry]>,
    }

    struct DuplicateCommsDispatcher {
        catalog: Arc<[ToolCatalogEntry]>,
        dispatched: AtomicBool,
    }

    impl ExactDeferredDispatcher {
        fn new() -> Self {
            Self {
                tool: Arc::new(ToolDef {
                    name: "secret_lookup".into(),
                    description: "Look up a secret".to_string(),
                    input_schema: serde_json::json!({"type": "object"}),
                    provenance: None,
                }),
                polled: AtomicBool::new(false),
            }
        }
    }

    impl ContextAwareDispatcher {
        fn new() -> Self {
            Self {
                tool: Arc::new(ToolDef {
                    name: "inspect_context".into(),
                    description: "Inspect dispatch context".to_string(),
                    input_schema: serde_json::json!({"type": "object"}),
                    provenance: None,
                }),
            }
        }
    }

    impl HybridExecutionDispatcher {
        fn new() -> Self {
            let tool = Arc::new(ToolDef {
                name: "hybrid_lookup".into(),
                description: "Run inline or detach from typed arguments".to_string(),
                input_schema: serde_json::json!({"type": "object"}),
                provenance: None,
            });
            let detached = meerkat_core::DetachedToolExecutionPolicy::new(
                meerkat_core::RunnerIdentity::new("test.hybrid_lookup", "v1")
                    .expect("valid runner identity"),
                meerkat_core::RestartClass::NonResumable,
                meerkat_core::IdempotencyScope::InteractionAndArguments,
                std::time::Duration::from_secs(10),
            )
            .expect("valid detached policy");
            let execution = meerkat_core::ToolExecutionContract::new(
                std::collections::BTreeSet::from([
                    meerkat_core::ToolExecutionMode::Fast,
                    meerkat_core::ToolExecutionMode::Detached,
                ]),
                meerkat_core::ToolExecutionMode::Fast,
                None,
                Some(detached),
            )
            .expect("valid hybrid execution contract");
            Self {
                catalog: Arc::from([
                    ToolCatalogEntry::session_inline(tool, true).with_execution_contract(execution)
                ]),
            }
        }
    }

    impl DuplicateCommsDispatcher {
        fn new() -> Self {
            let tool = Arc::new(ToolDef {
                name: "peers".into(),
                description: "Inner duplicate that must never win".to_string(),
                input_schema: serde_json::json!({"type": "object"}),
                provenance: Some(ToolProvenance {
                    kind: ToolSourceKind::Callback,
                    source_id: "inner-duplicate".into(),
                }),
            });
            let detached = meerkat_core::DetachedToolExecutionPolicy::new(
                meerkat_core::RunnerIdentity::new("test.inner_peers", "v1")
                    .expect("valid runner identity"),
                meerkat_core::RestartClass::NonResumable,
                meerkat_core::IdempotencyScope::ToolCall,
                std::time::Duration::from_secs(10),
            )
            .expect("valid detached policy");
            let execution = meerkat_core::ToolExecutionContract::new(
                std::collections::BTreeSet::from([meerkat_core::ToolExecutionMode::Detached]),
                meerkat_core::ToolExecutionMode::Detached,
                None,
                Some(detached),
            )
            .expect("valid detached execution contract");
            Self {
                catalog: Arc::from([
                    ToolCatalogEntry::session_inline(tool, true).with_execution_contract(execution)
                ]),
                dispatched: AtomicBool::new(false),
            }
        }
    }

    #[async_trait]
    impl AgentToolDispatcher for ExactDeferredDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            vec![Arc::clone(&self.tool)].into()
        }

        async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
            Ok(ToolResult::new(call.id.to_string(), "secret".to_string(), false).into())
        }

        async fn poll_external_updates(&self) -> ExternalToolUpdate {
            self.polled.store(true, Ordering::SeqCst);
            ExternalToolUpdate::default()
        }

        fn tool_catalog_capabilities(&self) -> ToolCatalogCapabilities {
            ToolCatalogCapabilities {
                exact_catalog: true,
                may_require_catalog_control_plane: true,
            }
        }

        fn tool_catalog(&self) -> Arc<[ToolCatalogEntry]> {
            vec![ToolCatalogEntry::session_deferred(
                Arc::clone(&self.tool),
                true,
                meerkat_core::types::ToolProvenance {
                    kind: meerkat_core::types::ToolSourceKind::Callback,
                    source_id: "secret_lookup".into(),
                },
            )]
            .into()
        }
    }

    #[async_trait]
    impl AgentToolDispatcher for ContextAwareDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            vec![Arc::clone(&self.tool)].into()
        }

        async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
            Ok(ToolResult::new(
                call.id.to_string(),
                serde_json::json!({"saw_context_image": false}).to_string(),
                false,
            )
            .into())
        }

        async fn dispatch_with_context(
            &self,
            call: ToolCallView<'_>,
            context: &ToolDispatchContext,
        ) -> Result<ToolDispatchOutcome, ToolError> {
            let saw_context_image = context
                .current_turn()
                .and_then(|turn| turn.image_ref(0))
                .and_then(|image_ref| context.current_turn_image(image_ref))
                .is_some();
            Ok(ToolResult::new(
                call.id.to_string(),
                serde_json::json!({"saw_context_image": saw_context_image}).to_string(),
                false,
            )
            .into())
        }
    }

    #[async_trait]
    impl AgentToolDispatcher for HybridExecutionDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            self.catalog
                .iter()
                .map(|entry| Arc::clone(&entry.tool))
                .collect::<Vec<_>>()
                .into()
        }

        fn tool_catalog_capabilities(&self) -> ToolCatalogCapabilities {
            ToolCatalogCapabilities {
                exact_catalog: true,
                may_require_catalog_control_plane: false,
            }
        }

        fn tool_catalog(&self) -> Arc<[ToolCatalogEntry]> {
            Arc::clone(&self.catalog)
        }

        fn resolve_execution_plan(
            &self,
            call: ToolCallView<'_>,
            _dispatch_context: &ToolDispatchContext,
            resolution_context: &meerkat_core::ToolExecutionResolutionContext,
        ) -> Result<
            meerkat_core::ResolvedToolExecutionPlan,
            meerkat_core::ToolExecutionResolutionError,
        > {
            let arguments: serde_json::Value =
                serde_json::from_str(call.args.get()).map_err(|error| {
                    meerkat_core::ToolExecutionResolutionError::InvalidArguments {
                        tool_name: call.name.to_string(),
                        reason: error.to_string(),
                    }
                })?;
            let mode = if arguments["detach"].as_bool() == Some(true) {
                meerkat_core::ToolExecutionMode::Detached
            } else {
                meerkat_core::ToolExecutionMode::Fast
            };
            self.catalog[0]
                .execution
                .resolve(mode, resolution_context.deadlines().clone())
                .map_err(Into::into)
        }

        async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
            Ok(ToolResult::new(call.id.to_string(), "{}".to_string(), false).into())
        }

        async fn dispatch_resolved_with_context(
            &self,
            call: ToolCallView<'_>,
            _context: &ToolDispatchContext,
            plan: &meerkat_core::ResolvedToolExecutionPlan,
        ) -> Result<ToolDispatchOutcome, ToolError> {
            if plan.mode() != meerkat_core::ToolExecutionMode::Detached {
                return Err(ToolError::execution_failed(
                    "test detached owner received the wrong plan",
                ));
            }
            self.dispatch(call).await
        }
    }

    #[async_trait]
    impl AgentToolDispatcher for DuplicateCommsDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            self.catalog
                .iter()
                .map(|entry| Arc::clone(&entry.tool))
                .collect::<Vec<_>>()
                .into()
        }

        fn tool_catalog_capabilities(&self) -> ToolCatalogCapabilities {
            ToolCatalogCapabilities {
                exact_catalog: true,
                may_require_catalog_control_plane: true,
            }
        }

        fn tool_catalog(&self) -> Arc<[ToolCatalogEntry]> {
            Arc::clone(&self.catalog)
        }

        fn resolve_execution_plan(
            &self,
            _call: ToolCallView<'_>,
            _dispatch_context: &ToolDispatchContext,
            resolution_context: &meerkat_core::ToolExecutionResolutionContext,
        ) -> Result<
            meerkat_core::ResolvedToolExecutionPlan,
            meerkat_core::ToolExecutionResolutionError,
        > {
            self.catalog[0]
                .execution
                .resolve_default(resolution_context.deadlines().clone())
                .map_err(Into::into)
        }

        async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
            self.dispatched.store(true, Ordering::SeqCst);
            Ok(ToolResult::new(
                call.id.to_string(),
                r#"{"owner":"inner"}"#.to_string(),
                false,
            )
            .into())
        }
    }

    fn execution_resolution_context() -> meerkat_core::ToolExecutionResolutionContext {
        meerkat_core::ToolExecutionResolutionContext::new(
            meerkat_core::ToolDeadlineChain::new(vec![
                meerkat_core::ToolDeadlineContributor::finite(
                    meerkat_core::ToolDeadlineOwner::CoreToolDispatch,
                    std::time::Duration::from_secs(600),
                ),
            ])
            .expect("valid test deadline chain"),
        )
    }

    fn test_router() -> (Arc<Router>, TrustedPeersView) {
        let (_inbox, inbox_sender) = Inbox::new();
        let trusted_peers = Arc::new(RwLock::new(TrustStore::default()));
        let router = Arc::new(Router::with_shared_peers(
            crate::identity::Keypair::generate(),
            Arc::clone(&trusted_peers),
            crate::router::CommsConfig::default(),
            inbox_sender,
            false,
        ));
        let view = router.trusted_peers_view();
        (router, view)
    }

    /// `reply_to_peer` routes through the comms dispatch path (never falling
    /// through to an inner dispatcher's NotFound) and fails closed with the
    /// typed no-capability error when the turn carries no reply context.
    #[tokio::test]
    async fn reply_to_peer_routes_as_comms_tool() {
        assert!(is_comms_tool("reply_to_peer"));

        let (router, trusted_peers) = test_router();
        let dispatcher = CommsToolDispatcher::new(router, trusted_peers);
        let args_raw = serde_json::value::RawValue::from_string(
            serde_json::json!({"body": "on it", "handling_mode": "queue"}).to_string(),
        )
        .expect("valid args");
        let call = ToolCallView {
            id: "reply-1",
            name: "reply_to_peer",
            args: &args_raw,
        };

        let result = dispatcher.dispatch(call).await;
        let Err(ToolError::ExecutionFailed { message }) = result else {
            panic!("expected comms-routed execution failure, got {result:?}");
        };
        assert!(
            message.starts_with("no_reply_capability"),
            "expected typed no_reply_capability error, got: {message}"
        );
    }

    #[test]
    fn comms_tool_defs_have_comms_provenance() {
        let defs = comms_tool_defs();
        assert!(!defs.is_empty(), "comms should expose at least one tool");
        for def in &defs {
            let prov = def
                .provenance
                .as_ref()
                .unwrap_or_else(|| panic!("comms tool '{}' is missing provenance", def.name));
            assert_eq!(
                prov.kind,
                ToolSourceKind::Comms,
                "comms tool '{}' should have Comms provenance",
                def.name
            );
            assert_eq!(prov.source_id, "comms");
        }
    }

    #[test]
    fn comms_routing_names_match_the_canonical_local_definitions() {
        for definition in comms_tool_defs() {
            assert!(
                is_comms_tool(&definition.name),
                "canonical local comms tool '{}' must route to the local owner",
                definition.name
            );
        }
        assert!(
            !is_comms_tool("send"),
            "the removed alias must not shadow an inner tool without a canonical local definition"
        );
    }

    #[tokio::test]
    async fn comms_wrapper_preserves_exact_deferred_catalog_contract() {
        let (router, trusted_peers) = test_router();
        let inner = Arc::new(ExactDeferredDispatcher::new());
        let dispatcher = CommsToolDispatcher::with_inner(router, trusted_peers, Arc::clone(&inner));

        let capabilities = dispatcher.tool_catalog_capabilities();
        assert!(capabilities.exact_catalog);
        assert!(capabilities.may_require_catalog_control_plane);

        let catalog = dispatcher.tool_catalog();
        assert!(
            catalog.iter().any(|entry| {
                entry.tool.name == "secret_lookup"
                    && matches!(
                        entry.deferred_eligibility,
                        ToolCatalogDeferredEligibility::DeferredEligible { .. }
                    )
            }),
            "wrapped exact deferred tools must stay deferred in the catalog"
        );

        dispatcher.poll_external_updates().await;
        assert!(inner.polled.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn comms_wrapper_preserves_dispatch_context_for_inner_tools() {
        let (router, trusted_peers) = test_router();
        let dispatcher = CommsToolDispatcher::with_inner(
            router,
            trusted_peers,
            Arc::new(ContextAwareDispatcher::new()),
        );
        let args_raw = serde_json::value::RawValue::from_string("{}".to_string())
            .expect("empty object should be valid raw JSON");
        let call = ToolCallView {
            id: "ctx-1",
            name: "inspect_context",
            args: &args_raw,
        };
        let context = ToolDispatchContext::from_current_turn_input(
            &meerkat_core::ContentInput::Blocks(vec![meerkat_core::ContentBlock::Image {
                media_type: "image/png".into(),
                data: "abc".into(),
            }]),
        );

        let outcome = dispatcher
            .dispatch_with_context(call, &context)
            .await
            .expect("wrapped inner tool should dispatch");
        let payload: serde_json::Value = serde_json::from_str(&outcome.result.text_content())
            .expect("tool result should be JSON");
        assert_eq!(payload["saw_context_image"], true);
    }

    #[tokio::test]
    async fn dyn_comms_wrapper_preserves_dispatch_context_for_inner_tools() {
        let (router, trusted_peers) = test_router();
        let dispatcher = DynCommsToolDispatcher::new(
            router,
            trusted_peers,
            Arc::new(ContextAwareDispatcher::new()),
        );
        let args_raw = serde_json::value::RawValue::from_string("{}".to_string())
            .expect("empty object should be valid raw JSON");
        let call = ToolCallView {
            id: "ctx-1",
            name: "inspect_context",
            args: &args_raw,
        };
        let context = ToolDispatchContext::from_current_turn_input(
            &meerkat_core::ContentInput::Blocks(vec![meerkat_core::ContentBlock::Image {
                media_type: "image/png".into(),
                data: "abc".into(),
            }]),
        );

        let outcome = dispatcher
            .dispatch_with_context(call, &context)
            .await
            .expect("wrapped inner tool should dispatch");
        let payload: serde_json::Value = serde_json::from_str(&outcome.result.text_content())
            .expect("tool result should be JSON");
        assert_eq!(payload["saw_context_image"], true);
    }

    #[tokio::test]
    async fn comms_wrapper_preserves_catalog_and_forwards_hybrid_execution_resolution() {
        let (router, trusted_peers) = test_router();
        let dispatcher = CommsToolDispatcher::with_inner(
            router,
            trusted_peers,
            Arc::new(HybridExecutionDispatcher::new()),
        );
        let catalog = dispatcher.tool_catalog();
        let hybrid = catalog
            .iter()
            .find(|entry| entry.tool.name == "hybrid_lookup")
            .expect("hybrid inner tool remains in exact catalog");
        assert!(
            hybrid
                .execution
                .supported_modes()
                .contains(&meerkat_core::ToolExecutionMode::Detached),
            "wrapper must preserve the exact inner execution contract"
        );

        let args = serde_json::value::RawValue::from_string(r#"{"detach":true}"#.to_string())
            .expect("valid hybrid arguments");
        let call = ToolCallView {
            id: "hybrid-generic",
            name: "hybrid_lookup",
            args: &args,
        };
        let context = ToolDispatchContext::default();
        let plan = dispatcher
            .resolve_execution_plan(call, &context, &execution_resolution_context())
            .expect("allowed inner plan should resolve");

        assert_eq!(plan.mode(), meerkat_core::ToolExecutionMode::Detached);
        dispatcher
            .dispatch_resolved_with_context(call, &context, &plan)
            .await
            .expect("generic comms wrapper forwards the resolved plan");
    }

    #[tokio::test]
    async fn dyn_comms_wrapper_forwards_hybrid_execution_resolution() {
        let (router, trusted_peers) = test_router();
        let dispatcher = DynCommsToolDispatcher::new(
            router,
            trusted_peers,
            Arc::new(HybridExecutionDispatcher::new()),
        );
        assert!(dispatcher.tool_catalog_capabilities().exact_catalog);
        let catalog = dispatcher.tool_catalog();
        let hybrid = catalog
            .iter()
            .find(|entry| entry.tool.name == "hybrid_lookup")
            .expect("hybrid inner tool remains in exact catalog");
        assert!(
            hybrid
                .execution
                .supported_modes()
                .contains(&meerkat_core::ToolExecutionMode::Detached),
            "dynamic wrapper must preserve the exact inner execution contract"
        );
        let args = serde_json::value::RawValue::from_string(r#"{"detach":true}"#.to_string())
            .expect("valid hybrid arguments");
        let call = ToolCallView {
            id: "hybrid-dyn",
            name: "hybrid_lookup",
            args: &args,
        };
        let context = ToolDispatchContext::default();
        let plan = dispatcher
            .resolve_execution_plan(call, &context, &execution_resolution_context())
            .expect("allowed inner plan should resolve");

        assert_eq!(plan.mode(), meerkat_core::ToolExecutionMode::Detached);
        dispatcher
            .dispatch_resolved_with_context(call, &context, &plan)
            .await
            .expect("dynamic comms wrapper forwards the resolved plan");
    }

    #[tokio::test]
    async fn generic_comms_wrapper_keeps_one_local_winner_when_inner_duplicates_name() {
        let (router, trusted_peers) = test_router();
        let inner = Arc::new(DuplicateCommsDispatcher::new());
        let dispatcher = CommsToolDispatcher::with_inner(router, trusted_peers, Arc::clone(&inner));

        assert!(dispatcher.tool_catalog_capabilities().exact_catalog);
        assert!(
            dispatcher
                .tool_catalog_capabilities()
                .may_require_catalog_control_plane
        );
        assert_eq!(
            dispatcher
                .tools()
                .iter()
                .filter(|tool| tool.name == "peers")
                .count(),
            1,
            "provider tools must contain one canonical comms winner"
        );
        let catalog = dispatcher.tool_catalog();
        let peers = catalog
            .iter()
            .filter(|entry| entry.tool.name == "peers")
            .collect::<Vec<_>>();
        assert_eq!(peers.len(), 1, "exact catalog must contain one winner");
        assert_eq!(
            peers[0]
                .tool
                .provenance
                .as_ref()
                .map(|value| value.kind.clone()),
            Some(ToolSourceKind::Comms),
            "the wrapper-owned comms definition must win"
        );
        assert_eq!(
            peers[0].execution.default_mode(),
            meerkat_core::ToolExecutionMode::Fast,
            "the inner duplicate's detached contract must not leak through"
        );

        let args =
            serde_json::value::RawValue::from_string("{}".to_string()).expect("valid arguments");
        let plan = dispatcher
            .resolve_execution_plan(
                ToolCallView {
                    id: "generic-peers-plan",
                    name: "peers",
                    args: &args,
                },
                &ToolDispatchContext::default(),
                &execution_resolution_context(),
            )
            .expect("local peers plan should resolve");
        assert_eq!(plan.mode(), meerkat_core::ToolExecutionMode::Fast);
        dispatcher
            .dispatch(ToolCallView {
                id: "generic-peers-dispatch",
                name: "peers",
                args: &args,
            })
            .await
            .expect("local peers dispatch should succeed");
        assert!(
            !inner.dispatched.load(Ordering::SeqCst),
            "dispatch must follow the same local winner as tools/catalog/resolution"
        );
    }

    #[tokio::test]
    async fn dyn_comms_wrapper_keeps_one_local_winner_when_inner_duplicates_name() {
        let (router, trusted_peers) = test_router();
        let inner = Arc::new(DuplicateCommsDispatcher::new());
        let inner_dispatcher: Arc<dyn AgentToolDispatcher> = inner.clone();
        let dispatcher = DynCommsToolDispatcher::new(router, trusted_peers, inner_dispatcher);

        assert!(dispatcher.tool_catalog_capabilities().exact_catalog);
        assert!(
            dispatcher
                .tool_catalog_capabilities()
                .may_require_catalog_control_plane
        );
        assert_eq!(
            dispatcher
                .tools()
                .iter()
                .filter(|tool| tool.name == "peers")
                .count(),
            1,
            "provider tools must contain one canonical comms winner"
        );
        let catalog = dispatcher.tool_catalog();
        let peers = catalog
            .iter()
            .filter(|entry| entry.tool.name == "peers")
            .collect::<Vec<_>>();
        assert_eq!(peers.len(), 1, "exact catalog must contain one winner");
        assert_eq!(
            peers[0]
                .tool
                .provenance
                .as_ref()
                .map(|value| value.kind.clone()),
            Some(ToolSourceKind::Comms),
            "the wrapper-owned comms definition must win"
        );
        assert_eq!(
            peers[0].execution.default_mode(),
            meerkat_core::ToolExecutionMode::Fast,
            "the inner duplicate's detached contract must not leak through"
        );

        let args =
            serde_json::value::RawValue::from_string("{}".to_string()).expect("valid arguments");
        let plan = dispatcher
            .resolve_execution_plan(
                ToolCallView {
                    id: "dyn-peers-plan",
                    name: "peers",
                    args: &args,
                },
                &ToolDispatchContext::default(),
                &execution_resolution_context(),
            )
            .expect("local peers plan should resolve");
        assert_eq!(plan.mode(), meerkat_core::ToolExecutionMode::Fast);
        dispatcher
            .dispatch(ToolCallView {
                id: "dyn-peers-dispatch",
                name: "peers",
                args: &args,
            })
            .await
            .expect("local peers dispatch should succeed");
        assert!(
            !inner.dispatched.load(Ordering::SeqCst),
            "dispatch must follow the same local winner as tools/catalog/resolution"
        );
    }

    #[tokio::test]
    async fn runtime_less_semantic_send_request_returns_typed_unavailable() {
        let (router, trusted_peers) = test_router();
        let dispatcher = CommsToolDispatcher::new(router, trusted_peers);
        let args_raw = serde_json::value::RawValue::from_string(
            serde_json::json!({
                "peer_id": meerkat_core::comms::PeerId::new(),
                "intent": "review",
                "params": {"file": "main.rs"},
                "handling_mode": "steer"
            })
            .to_string(),
        )
        .expect("valid args");
        let call = ToolCallView {
            id: "test-1",
            name: "send_request",
            args: &args_raw,
        };

        let result = dispatcher.dispatch(call).await;
        assert!(matches!(
            result,
            Err(ToolError::Unavailable {
                reason: meerkat_core::ToolUnavailableReason::RuntimeCommandAuthorityUnavailable,
                ..
            })
        ));
    }

    /// ROW #63 gate: a comms tool call whose arguments are not a JSON object
    /// (string literal / array / number) must fail closed with a typed
    /// `InvalidArguments` error at the dispatcher boundary, never reaching
    /// `handle_tools_call_with_context` with a `Value::String` fallback.
    #[tokio::test]
    async fn non_object_comms_args_return_invalid_arguments() {
        for raw in [r#""just a string""#, "[1, 2, 3]", "42"] {
            let (router, trusted_peers) = test_router();
            let dispatcher = CommsToolDispatcher::new(router, trusted_peers);
            let args_raw = serde_json::value::RawValue::from_string(raw.to_string())
                .expect("raw JSON literal");
            let call = ToolCallView {
                id: "row63-1",
                name: "send_message",
                args: &args_raw,
            };
            let result = dispatcher.dispatch(call).await;
            assert!(
                matches!(result, Err(ToolError::InvalidArguments { .. })),
                "non-object comms args ({raw}) must return InvalidArguments, got {result:?}"
            );
        }
    }

    /// ROW #63 gate (Dyn wrapper): same object-guarded boundary applies to the
    /// `DynCommsToolDispatcher` dispatch path.
    #[tokio::test]
    async fn dyn_non_object_comms_args_return_invalid_arguments() {
        let (router, trusted_peers) = test_router();
        let dispatcher = DynCommsToolDispatcher::new(
            router,
            trusted_peers,
            Arc::new(ContextAwareDispatcher::new()),
        );
        let args_raw = serde_json::value::RawValue::from_string(r#""not an object""#.to_string())
            .expect("raw JSON literal");
        let call = ToolCallView {
            id: "row63-dyn-1",
            name: "send_message",
            args: &args_raw,
        };
        let result = dispatcher.dispatch(call).await;
        assert!(
            matches!(result, Err(ToolError::InvalidArguments { .. })),
            "non-object comms args must return InvalidArguments, got {result:?}"
        );
    }

    #[test]
    fn runtime_less_tools_hide_semantic_request_response_tools() {
        let (router, trusted_peers) = test_router();
        let dispatcher = CommsToolDispatcher::new(router, trusted_peers);
        let tools = dispatcher.tools();
        let names = tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>();

        assert!(names.contains(&"send_message"));
        assert!(names.contains(&"peers"));
        assert!(!names.contains(&"send_request"));
        assert!(!names.contains(&"send_response"));
    }
}
