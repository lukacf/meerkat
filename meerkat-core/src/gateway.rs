//! Tool gateway for composing multiple tool dispatchers
//!
//! The [`ToolGateway`] combines multiple tool dispatchers into a single unified
//! dispatcher. This enables composing core tool dispatchers (shell, task, MCP)
//! with infrastructure-provided tools (comms) without coupling them together.
//!
//! # Example
//!
//! ```text
//! use meerkat_core::{ToolGateway, ToolGatewayBuilder, AgentToolDispatcher};
//!
//! // Compose multiple dispatchers
//! let gateway = ToolGatewayBuilder::new()
//!     .add_dispatcher(base_dispatcher)
//!     .add_dispatcher(comms_dispatcher)
//!     .build()?;
//! ```

use crate::AgentToolDispatcher;
use crate::agent::{DetachedOpCompletion, ExternalToolUpdate};
use crate::error::ToolError;
use crate::event::ExternalToolDelta;
#[cfg(all(target_arch = "wasm32", test))]
use crate::tokio;
use crate::tool_catalog::{ToolCatalogCapabilities, ToolCatalogEntry, ToolUnavailableReason};
use crate::types::{ToolCallView, ToolDef, ToolName};
use async_trait::async_trait;
use std::sync::Arc;

/// Entry for a dispatcher in the gateway.
struct DispatcherEntry {
    dispatcher: Arc<dyn AgentToolDispatcher>,
}

/// A tool dispatcher that composes multiple dispatchers into one.
///
/// The gateway validates the initial child catalogs for collisions, then uses
/// live child catalogs/lists for routing and identity admission. This lets
/// dynamic dispatchers surface newly connected tools without rebuilding the
/// gateway while preserving first-dispatcher-wins behavior for live duplicates.
///
/// ## Dynamic Visibility
///
/// Some tools may have dynamic availability based on runtime conditions.
/// The gateway handles this by:
/// - Only returning available tools from `tools()`
/// - Returning `ToolError::Unavailable` for hidden tools on dispatch
pub struct ToolGateway {
    /// Dispatcher entries in stable precedence order.
    entries: Vec<DispatcherEntry>,
}

impl std::fmt::Debug for ToolGateway {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolGateway")
            .field(
                "tools",
                &self
                    .tool_catalog()
                    .iter()
                    .map(|entry| entry.tool.name.as_str())
                    .collect::<Vec<_>>(),
            )
            .finish_non_exhaustive()
    }
}

impl ToolGateway {
    /// Create a new gateway with a base dispatcher and optional overlay.
    ///
    pub fn new(
        base: Arc<dyn AgentToolDispatcher>,
        overlay: Option<Arc<dyn AgentToolDispatcher>>,
    ) -> Result<Self, ToolError> {
        let mut builder = ToolGatewayBuilder::new().add_dispatcher(base);
        if let Some(o) = overlay {
            builder = builder.add_dispatcher(o);
        }
        builder.build()
    }

    fn live_catalog_for_dispatcher(
        dispatcher: &dyn AgentToolDispatcher,
    ) -> Arc<[ToolCatalogEntry]> {
        if dispatcher.tool_catalog_capabilities().exact_catalog {
            return dispatcher.tool_catalog();
        }
        dispatcher
            .tools()
            .iter()
            .map(|tool| ToolCatalogEntry::session_inline(Arc::clone(tool), true))
            .collect::<Vec<_>>()
            .into()
    }

    fn route_not_found_as_unavailable(name: &str, err: ToolError) -> ToolError {
        match err {
            ToolError::NotFound { name: err_name } if err_name == name => {
                ToolError::unavailable(name, ToolUnavailableReason::NotCurrentlyCallable)
            }
            other => other,
        }
    }
}

/// Builder for constructing a [`ToolGateway`].
///
/// Use this when you need to compose more than two dispatchers.
pub struct ToolGatewayBuilder {
    dispatchers: Vec<Arc<dyn AgentToolDispatcher>>,
}

impl Default for ToolGatewayBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolGatewayBuilder {
    /// Create a new empty builder.
    pub fn new() -> Self {
        Self {
            dispatchers: Vec::new(),
        }
    }

    /// Add a dispatcher with default availability (always).
    pub fn add_dispatcher(mut self, dispatcher: Arc<dyn AgentToolDispatcher>) -> Self {
        self.dispatchers.push(dispatcher);
        self
    }

    /// Optionally add a dispatcher if present.
    pub fn maybe_add_dispatcher(self, dispatcher: Option<Arc<dyn AgentToolDispatcher>>) -> Self {
        match dispatcher {
            Some(d) => self.add_dispatcher(d),
            None => self,
        }
    }

    /// Build the gateway, validating that there are no tool name collisions.
    ///
    /// Returns an error if any two dispatchers provide tools with the same name.
    /// All tools are checked for collisions regardless of their availability.
    pub fn build(self) -> Result<ToolGateway, ToolError> {
        let mut entries: Vec<DispatcherEntry> = Vec::new();
        let mut seen: std::collections::HashSet<ToolName> = std::collections::HashSet::new();

        for dispatcher in self.dispatchers {
            let frozen_catalog = ToolGateway::live_catalog_for_dispatcher(dispatcher.as_ref());
            for entry in frozen_catalog.iter() {
                let name = entry.tool.tool_name();
                if !seen.insert(name.clone()) {
                    return Err(ToolError::Other(format!(
                        "tool name collision in gateway: '{}'",
                        entry.tool.name.as_str()
                    )));
                }
            }
            entries.push(DispatcherEntry { dispatcher });
        }

        Ok(ToolGateway { entries })
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl AgentToolDispatcher for ToolGateway {
    /// Returns only tools whose owning catalog says they are currently callable.
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        self.tool_catalog()
            .iter()
            .filter(|entry| entry.callability.is_callable())
            .map(|entry| Arc::clone(&entry.tool))
            .collect::<Vec<_>>()
            .into()
    }

    /// Dispatch a tool call.
    ///
    /// Returns:
    /// - `ToolError::NotFound` if the tool doesn't exist
    /// - `ToolError::Unavailable` if the tool exists but is hidden or the
    ///   selected owner cannot route it anymore
    /// - The tool result if execution succeeds
    async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<crate::ops::ToolDispatchOutcome, ToolError> {
        for entry in &self.entries {
            if entry.dispatcher.tool_catalog_capabilities().exact_catalog {
                if let Some(catalog_entry) = entry
                    .dispatcher
                    .tool_catalog()
                    .iter()
                    .find(|entry| entry.tool.name == call.name)
                {
                    if let Some(reason) = catalog_entry.callability.unavailable_reason() {
                        return Err(ToolError::unavailable(call.name, reason));
                    }
                    return entry
                        .dispatcher
                        .dispatch(call)
                        .await
                        .map_err(|err| Self::route_not_found_as_unavailable(call.name, err));
                }
            } else if entry
                .dispatcher
                .tools()
                .iter()
                .any(|tool| tool.name == call.name)
            {
                return entry
                    .dispatcher
                    .dispatch(call)
                    .await
                    .map_err(|err| Self::route_not_found_as_unavailable(call.name, err));
            }
        }
        Err(ToolError::not_found(call.name))
    }

    async fn dispatch_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &crate::ToolDispatchContext,
    ) -> Result<crate::ops::ToolDispatchOutcome, ToolError> {
        for entry in &self.entries {
            if entry.dispatcher.tool_catalog_capabilities().exact_catalog {
                if let Some(catalog_entry) = entry
                    .dispatcher
                    .tool_catalog()
                    .iter()
                    .find(|entry| entry.tool.name == call.name)
                {
                    if let Some(reason) = catalog_entry.callability.unavailable_reason() {
                        return Err(ToolError::unavailable(call.name, reason));
                    }
                    return entry
                        .dispatcher
                        .dispatch_with_context(call, context)
                        .await
                        .map_err(|err| Self::route_not_found_as_unavailable(call.name, err));
                }
            } else if entry
                .dispatcher
                .tools()
                .iter()
                .any(|tool| tool.name == call.name)
            {
                return entry
                    .dispatcher
                    .dispatch_with_context(call, context)
                    .await
                    .map_err(|err| Self::route_not_found_as_unavailable(call.name, err));
            }
        }
        Err(ToolError::not_found(call.name))
    }

    fn tool_catalog_capabilities(&self) -> ToolCatalogCapabilities {
        ToolCatalogCapabilities {
            exact_catalog: self
                .entries
                .iter()
                .all(|entry| entry.dispatcher.tool_catalog_capabilities().exact_catalog),
            may_require_catalog_control_plane: self.entries.iter().any(|entry| {
                entry
                    .dispatcher
                    .tool_catalog_capabilities()
                    .may_require_catalog_control_plane
            }),
        }
    }

    fn pending_catalog_sources(&self) -> Arc<[String]> {
        let mut pending = std::collections::BTreeSet::new();
        for entry in &self.entries {
            let sources = entry.dispatcher.pending_catalog_sources();
            pending.extend(sources.iter().cloned());
        }
        pending.into_iter().collect::<Vec<_>>().into()
    }

    fn tool_catalog(&self) -> Arc<[ToolCatalogEntry]> {
        let mut seen = std::collections::HashSet::new();
        let mut result = Vec::new();
        for entry in &self.entries {
            for catalog_entry in Self::live_catalog_for_dispatcher(entry.dispatcher.as_ref()).iter()
            {
                if seen.insert(catalog_entry.tool.name.clone()) {
                    result.push(catalog_entry.clone());
                }
            }
        }
        result.into()
    }

    fn capabilities(&self) -> crate::agent::DispatcherCapabilities {
        let mut caps = crate::agent::DispatcherCapabilities::default();
        for entry in &self.entries {
            let c = entry.dispatcher.capabilities();
            caps.ops_lifecycle |= c.ops_lifecycle;
        }
        caps
    }

    fn bind_ops_lifecycle(
        self: Arc<Self>,
        registry: Arc<dyn crate::ops_lifecycle::OpsLifecycleRegistry>,
        owner_bridge_session_id: crate::types::SessionId,
    ) -> Result<crate::agent::BindOutcome, crate::agent::OpsLifecycleBindError> {
        let owned = Arc::try_unwrap(self)
            .map_err(|_| crate::agent::OpsLifecycleBindError::SharedOwnership)?;

        let mut builder = ToolGatewayBuilder::new();
        let mut any_bound = false;
        for entry in owned.entries {
            if entry.dispatcher.capabilities().ops_lifecycle
                && Arc::strong_count(&entry.dispatcher) == 1
            {
                let outcome = entry
                    .dispatcher
                    .bind_ops_lifecycle(Arc::clone(&registry), owner_bridge_session_id.clone())?;
                if outcome.was_bound() {
                    any_bound = true;
                }
                builder = builder.add_dispatcher(outcome.into_dispatcher());
            } else {
                builder = builder.add_dispatcher(entry.dispatcher);
            }
        }

        let gateway = builder
            .build()
            .map_err(|_| crate::agent::OpsLifecycleBindError::Unsupported)?;
        let d: Arc<dyn AgentToolDispatcher> = Arc::new(gateway);
        Ok(if any_bound {
            crate::agent::BindOutcome::Bound(d)
        } else {
            crate::agent::BindOutcome::Skipped(d)
        })
    }

    fn completion_enrichment(
        &self,
    ) -> Option<Arc<dyn crate::completion_feed::CompletionEnrichmentProvider>> {
        self.entries
            .iter()
            .find_map(|e| e.dispatcher.completion_enrichment())
    }

    fn external_tool_surface_snapshot(&self) -> Option<crate::ExternalToolSurfaceSnapshot> {
        self.entries
            .iter()
            .find_map(|entry| entry.dispatcher.external_tool_surface_snapshot())
    }

    fn bind_mcp_server_lifecycle_handle(
        &self,
        handle: Arc<dyn crate::handles::McpServerLifecycleHandle>,
    ) {
        for entry in &self.entries {
            entry
                .dispatcher
                .bind_mcp_server_lifecycle_handle(Arc::clone(&handle));
        }
    }

    fn bind_external_tool_surface_handle(
        &self,
        handle: Arc<dyn crate::handles::ExternalToolSurfaceHandle>,
    ) {
        for entry in &self.entries {
            entry
                .dispatcher
                .bind_external_tool_surface_handle(Arc::clone(&handle));
        }
    }

    /// Aggregate external updates across all dispatcher entries.
    ///
    /// Deduplicates by server name for pending, by `(server, operation, status)`
    /// for notices. First-seen wins, stable order.
    async fn poll_external_updates(&self) -> ExternalToolUpdate {
        let mut all_notices: Vec<ExternalToolDelta> = Vec::new();
        let mut all_pending: Vec<String> = Vec::new();
        let mut seen_pending: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut seen_notices: std::collections::HashSet<(
            String,
            String,
            String,
            bool,
            Option<u32>,
        )> = std::collections::HashSet::new();
        let mut seen_bg_job_ids: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        let mut all_bg_completions: Vec<DetachedOpCompletion> = Vec::new();

        for entry in &self.entries {
            let update = entry.dispatcher.poll_external_updates().await;
            for notice in update.notices {
                let key = (
                    notice.target.clone(),
                    format!("{:?}", notice.operation),
                    notice.status_text(),
                    notice.persisted,
                    notice.applied_at_turn,
                );
                if seen_notices.insert(key) {
                    all_notices.push(notice);
                }
            }
            for pending in update.pending {
                if seen_pending.insert(pending.clone()) {
                    all_pending.push(pending);
                }
            }
            for bg in update.background_completions {
                if seen_bg_job_ids.insert(bg.job_id.clone()) {
                    all_bg_completions.push(bg);
                }
            }
        }

        ExternalToolUpdate {
            notices: all_notices,
            pending: all_pending,
            background_completions: all_bg_completions,
        }
    }
}

// ---------------------------------------------------------------------------
// DynamicToolComposite
// ---------------------------------------------------------------------------

/// Composes multiple dispatchers with live tool list delegation.
///
/// Like [`ToolGateway`], this composite calls `tools()` on each child dispatcher every time,
/// enabling children with dynamic tool lists (e.g. callback tool dispatchers
/// backed by a shared registry) to surface additions/removals between turns.
///
/// First-dispatcher-wins on name collision (consistent with `ToolGateway`).
pub struct DynamicToolComposite {
    dispatchers: Vec<Arc<dyn AgentToolDispatcher>>,
}

impl DynamicToolComposite {
    pub fn new(dispatchers: Vec<Arc<dyn AgentToolDispatcher>>) -> Self {
        Self { dispatchers }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl AgentToolDispatcher for DynamicToolComposite {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        if self.tool_catalog_capabilities().exact_catalog {
            return self
                .tool_catalog()
                .iter()
                .filter(|entry| entry.currently_callable())
                .map(|entry| Arc::clone(&entry.tool))
                .collect::<Vec<_>>()
                .into();
        }

        let mut seen = std::collections::HashSet::new();
        let mut result = Vec::new();
        for d in &self.dispatchers {
            for t in d.tools().iter() {
                if seen.insert(t.name.clone()) {
                    result.push(Arc::clone(t));
                }
            }
        }
        result.into()
    }

    async fn dispatch(
        &self,
        call: crate::types::ToolCallView<'_>,
    ) -> Result<crate::ops::ToolDispatchOutcome, crate::error::ToolError> {
        if self.tool_catalog_capabilities().exact_catalog {
            for d in &self.dispatchers {
                if let Some(entry) = d
                    .tool_catalog()
                    .iter()
                    .find(|entry| entry.tool.name == call.name)
                {
                    if let Some(reason) = entry.callability.unavailable_reason() {
                        return Err(crate::error::ToolError::unavailable(call.name, reason));
                    }
                    return d.dispatch(call).await.map_err(|err| {
                        ToolGateway::route_not_found_as_unavailable(call.name, err)
                    });
                }
            }
            return Err(crate::error::ToolError::not_found(call.name));
        }

        for d in &self.dispatchers {
            if d.tools().iter().any(|t| t.name == call.name) {
                return d
                    .dispatch(call)
                    .await
                    .map_err(|err| ToolGateway::route_not_found_as_unavailable(call.name, err));
            }
        }
        Err(crate::error::ToolError::not_found(call.name))
    }

    async fn dispatch_with_context(
        &self,
        call: crate::types::ToolCallView<'_>,
        context: &crate::ToolDispatchContext,
    ) -> Result<crate::ops::ToolDispatchOutcome, crate::error::ToolError> {
        if self.tool_catalog_capabilities().exact_catalog {
            for d in &self.dispatchers {
                if let Some(entry) = d
                    .tool_catalog()
                    .iter()
                    .find(|entry| entry.tool.name == call.name)
                {
                    if let Some(reason) = entry.callability.unavailable_reason() {
                        return Err(crate::error::ToolError::unavailable(call.name, reason));
                    }
                    return d.dispatch_with_context(call, context).await.map_err(|err| {
                        ToolGateway::route_not_found_as_unavailable(call.name, err)
                    });
                }
            }
            return Err(crate::error::ToolError::not_found(call.name));
        }

        for d in &self.dispatchers {
            if d.tools().iter().any(|t| t.name == call.name) {
                return d
                    .dispatch_with_context(call, context)
                    .await
                    .map_err(|err| ToolGateway::route_not_found_as_unavailable(call.name, err));
            }
        }
        Err(crate::error::ToolError::not_found(call.name))
    }

    async fn poll_external_updates(&self) -> ExternalToolUpdate {
        let mut all_notices = Vec::new();
        let mut all_pending = Vec::new();
        for d in &self.dispatchers {
            let update = d.poll_external_updates().await;
            all_notices.extend(update.notices);
            all_pending.extend(update.pending);
        }
        ExternalToolUpdate {
            notices: all_notices,
            pending: all_pending,
            background_completions: Vec::new(),
        }
    }

    fn capabilities(&self) -> crate::agent::DispatcherCapabilities {
        let mut caps = crate::agent::DispatcherCapabilities::default();
        for d in &self.dispatchers {
            let c = d.capabilities();
            caps.ops_lifecycle |= c.ops_lifecycle;
        }
        caps
    }

    fn completion_enrichment(
        &self,
    ) -> Option<Arc<dyn crate::completion_feed::CompletionEnrichmentProvider>> {
        self.dispatchers
            .iter()
            .find_map(|d| d.completion_enrichment())
    }

    fn tool_catalog_capabilities(&self) -> ToolCatalogCapabilities {
        ToolCatalogCapabilities {
            exact_catalog: self
                .dispatchers
                .iter()
                .all(|dispatcher| dispatcher.tool_catalog_capabilities().exact_catalog),
            may_require_catalog_control_plane: self.dispatchers.iter().any(|dispatcher| {
                dispatcher
                    .tool_catalog_capabilities()
                    .may_require_catalog_control_plane
            }),
        }
    }

    fn pending_catalog_sources(&self) -> Arc<[String]> {
        let mut pending = std::collections::BTreeSet::new();
        for dispatcher in &self.dispatchers {
            let sources = dispatcher.pending_catalog_sources();
            pending.extend(sources.iter().cloned());
        }
        pending.into_iter().collect::<Vec<_>>().into()
    }

    fn tool_catalog(&self) -> Arc<[ToolCatalogEntry]> {
        if !self.tool_catalog_capabilities().exact_catalog {
            return self
                .tools()
                .iter()
                .map(|tool| ToolCatalogEntry::session_inline(Arc::clone(tool), true))
                .collect::<Vec<_>>()
                .into();
        }

        let mut seen = std::collections::HashSet::new();
        let mut result = Vec::new();
        for dispatcher in &self.dispatchers {
            for entry in dispatcher.tool_catalog().iter() {
                if seen.insert(entry.tool.name.clone()) {
                    result.push(entry.clone());
                }
            }
        }
        result.into()
    }

    fn external_tool_surface_snapshot(&self) -> Option<crate::ExternalToolSurfaceSnapshot> {
        self.dispatchers
            .iter()
            .find_map(|dispatcher| dispatcher.external_tool_surface_snapshot())
    }

    fn bind_mcp_server_lifecycle_handle(
        &self,
        handle: Arc<dyn crate::handles::McpServerLifecycleHandle>,
    ) {
        for dispatcher in &self.dispatchers {
            dispatcher.bind_mcp_server_lifecycle_handle(Arc::clone(&handle));
        }
    }

    fn bind_external_tool_surface_handle(
        &self,
        handle: Arc<dyn crate::handles::ExternalToolSurfaceHandle>,
    ) {
        for dispatcher in &self.dispatchers {
            dispatcher.bind_external_tool_surface_handle(Arc::clone(&handle));
        }
    }

    fn bind_ops_lifecycle(
        self: Arc<Self>,
        registry: Arc<dyn crate::ops_lifecycle::OpsLifecycleRegistry>,
        owner_bridge_session_id: crate::types::SessionId,
    ) -> Result<crate::agent::BindOutcome, crate::agent::OpsLifecycleBindError> {
        let owned = Arc::try_unwrap(self)
            .map_err(|_| crate::agent::OpsLifecycleBindError::SharedOwnership)?;
        let mut rebound = Vec::with_capacity(owned.dispatchers.len());
        let mut any_bound = false;
        for d in owned.dispatchers {
            if d.capabilities().ops_lifecycle && Arc::strong_count(&d) == 1 {
                let outcome =
                    d.bind_ops_lifecycle(Arc::clone(&registry), owner_bridge_session_id.clone())?;
                if outcome.was_bound() {
                    any_bound = true;
                }
                rebound.push(outcome.into_dispatcher());
            } else {
                rebound.push(d);
            }
        }
        let d: Arc<dyn AgentToolDispatcher> = Arc::new(DynamicToolComposite::new(rebound));
        Ok(if any_bound {
            crate::agent::BindOutcome::Bound(d)
        } else {
            crate::agent::BindOutcome::Skipped(d)
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::tool_catalog::ToolCallability;
    use crate::types::ToolResult;
    use serde_json::Value;
    use serde_json::json;
    use std::sync::atomic::{AtomicBool, Ordering};

    async fn dispatch_json(
        gateway: &ToolGateway,
        name: &str,
        args: serde_json::Value,
    ) -> Result<Value, ToolError> {
        let args_raw =
            serde_json::value::RawValue::from_string(args.to_string()).expect("valid args json");
        let call = ToolCallView {
            id: "test-1",
            name,
            args: &args_raw,
        };
        let outcome = gateway.dispatch(call).await?;
        serde_json::from_str(&outcome.result.text_content())
            .map_err(|e| ToolError::execution_failed(e.to_string()))
    }

    fn empty_object_schema() -> Value {
        let mut obj = serde_json::Map::new();
        obj.insert("type".to_string(), Value::String("object".to_string()));
        obj.insert(
            "properties".to_string(),
            Value::Object(serde_json::Map::new()),
        );
        obj.insert("required".to_string(), Value::Array(Vec::new()));
        Value::Object(obj)
    }

    /// A simple mock dispatcher for testing
    struct MockDispatcher {
        tools: Arc<[Arc<ToolDef>]>,
        prefix: String,
    }

    impl MockDispatcher {
        fn new(prefix: &str, tool_names: &[&str]) -> Self {
            let tools: Arc<[Arc<ToolDef>]> = tool_names
                .iter()
                .map(|name| {
                    Arc::new(ToolDef {
                        name: (*name).into(),
                        description: format!("{prefix} tool: {name}"),
                        input_schema: empty_object_schema(),
                        provenance: None,
                    })
                })
                .collect::<Vec<_>>()
                .into();
            Self {
                tools,
                prefix: prefix.to_string(),
            }
        }
    }

    struct ExactMockDispatcher {
        tools: Arc<[Arc<ToolDef>]>,
        catalog: Arc<[crate::ToolCatalogEntry]>,
        prefix: String,
    }

    impl ExactMockDispatcher {
        fn with_callability(prefix: &str, entries: &[(&str, bool)]) -> Self {
            let catalog: Vec<crate::ToolCatalogEntry> = entries
                .iter()
                .map(|(name, currently_callable)| {
                    crate::ToolCatalogEntry::session_inline(
                        Arc::new(ToolDef {
                            name: (*name).into(),
                            description: format!("{prefix} tool: {name}"),
                            input_schema: empty_object_schema(),
                            provenance: None,
                        }),
                        *currently_callable,
                    )
                })
                .collect();
            let tools: Arc<[Arc<ToolDef>]> = catalog
                .iter()
                .filter(|entry| entry.currently_callable())
                .map(|entry| Arc::clone(&entry.tool))
                .collect::<Vec<_>>()
                .into();
            Self {
                tools,
                catalog: catalog.into(),
                prefix: prefix.to_string(),
            }
        }

        fn with_unavailable_reason(
            prefix: &str,
            name: &str,
            reason: ToolUnavailableReason,
        ) -> Self {
            let tool = Arc::new(ToolDef {
                name: name.into(),
                description: format!("{prefix} tool: {name}"),
                input_schema: empty_object_schema(),
                provenance: None,
            });
            let catalog = Arc::from([crate::ToolCatalogEntry::session_inline_with_callability(
                Arc::clone(&tool),
                ToolCallability::unavailable(reason),
            )]);
            Self {
                tools: Vec::<Arc<ToolDef>>::new().into(),
                catalog,
                prefix: prefix.to_string(),
            }
        }
    }

    struct LiveExactMockDispatcher {
        tool: Arc<ToolDef>,
        prefix: String,
        callable: Arc<AtomicBool>,
    }

    struct MutableMockDispatcher {
        tools: std::sync::Mutex<Vec<Arc<ToolDef>>>,
        prefix: String,
    }

    struct UnroutableExactDispatcher {
        tool: Arc<ToolDef>,
    }

    struct ContextAwareDispatcher {
        tool: Arc<ToolDef>,
        exact_catalog: bool,
    }

    impl ContextAwareDispatcher {
        fn new(tool_name: &str, exact_catalog: bool) -> Self {
            Self {
                tool: Arc::new(ToolDef {
                    name: tool_name.into(),
                    description: "context aware tool".into(),
                    input_schema: empty_object_schema(),
                    provenance: None,
                }),
                exact_catalog,
            }
        }
    }

    impl LiveExactMockDispatcher {
        fn new(prefix: &str, tool_name: &str, callable: Arc<AtomicBool>) -> Self {
            Self {
                tool: Arc::new(ToolDef {
                    name: tool_name.into(),
                    description: format!("{prefix} tool: {tool_name}"),
                    input_schema: empty_object_schema(),
                    provenance: None,
                }),
                prefix: prefix.to_string(),
                callable,
            }
        }

        fn current_entry(&self) -> crate::ToolCatalogEntry {
            crate::ToolCatalogEntry::session_inline(
                Arc::clone(&self.tool),
                self.callable.load(Ordering::SeqCst),
            )
        }
    }

    impl MutableMockDispatcher {
        fn new(prefix: &str, names: &[&str]) -> Self {
            Self {
                tools: std::sync::Mutex::new(
                    names
                        .iter()
                        .map(|name| {
                            Arc::new(ToolDef {
                                name: (*name).into(),
                                description: format!("{prefix} tool: {name}"),
                                input_schema: empty_object_schema(),
                                provenance: None,
                            })
                        })
                        .collect(),
                ),
                prefix: prefix.to_string(),
            }
        }

        fn add_tool(&self, name: &str) {
            self.tools.lock().unwrap().push(Arc::new(ToolDef {
                name: name.into(),
                description: format!("{} tool: {}", self.prefix, name),
                input_schema: empty_object_schema(),
                provenance: None,
            }));
        }

        fn remove_tool(&self, name: &str) {
            self.tools.lock().unwrap().retain(|tool| tool.name != name);
        }
    }

    impl UnroutableExactDispatcher {
        fn new(name: &str) -> Self {
            Self {
                tool: Arc::new(ToolDef {
                    name: name.into(),
                    description: format!("{name} tool"),
                    input_schema: empty_object_schema(),
                    provenance: None,
                }),
            }
        }
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl AgentToolDispatcher for ExactMockDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            Arc::clone(&self.tools)
        }

        fn tool_catalog_capabilities(&self) -> crate::ToolCatalogCapabilities {
            crate::ToolCatalogCapabilities {
                exact_catalog: true,
                may_require_catalog_control_plane: false,
            }
        }

        fn tool_catalog(&self) -> Arc<[crate::ToolCatalogEntry]> {
            Arc::clone(&self.catalog)
        }

        async fn dispatch(
            &self,
            call: ToolCallView<'_>,
        ) -> Result<crate::ops::ToolDispatchOutcome, ToolError> {
            let Some(entry) = self
                .catalog
                .iter()
                .find(|entry| entry.tool.name == call.name)
            else {
                return Err(ToolError::not_found(call.name));
            };
            if let Some(reason) = entry.callability.unavailable_reason() {
                return Err(ToolError::unavailable(call.name, reason));
            }
            Ok(ToolResult::new(
                call.id.to_string(),
                json!({"source": self.prefix, "tool": call.name}).to_string(),
                false,
            )
            .into())
        }
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl AgentToolDispatcher for LiveExactMockDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            if self.callable.load(Ordering::SeqCst) {
                Arc::from([Arc::clone(&self.tool)])
            } else {
                Arc::new([])
            }
        }

        fn tool_catalog_capabilities(&self) -> crate::ToolCatalogCapabilities {
            crate::ToolCatalogCapabilities {
                exact_catalog: true,
                may_require_catalog_control_plane: false,
            }
        }

        fn tool_catalog(&self) -> Arc<[crate::ToolCatalogEntry]> {
            Arc::from([self.current_entry()])
        }

        async fn dispatch(
            &self,
            call: ToolCallView<'_>,
        ) -> Result<crate::ops::ToolDispatchOutcome, ToolError> {
            if self.tool.name != call.name {
                return Err(ToolError::not_found(call.name));
            }
            if !self.callable.load(Ordering::SeqCst) {
                return Err(ToolError::unavailable(
                    call.name,
                    ToolUnavailableReason::NotCurrentlyCallable,
                ));
            }
            Ok(ToolResult::new(
                call.id.to_string(),
                json!({"source": self.prefix, "tool": call.name}).to_string(),
                false,
            )
            .into())
        }
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl AgentToolDispatcher for MockDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            Arc::clone(&self.tools)
        }

        async fn dispatch(
            &self,
            call: ToolCallView<'_>,
        ) -> Result<crate::ops::ToolDispatchOutcome, ToolError> {
            if self.tools.iter().any(|t| t.name == call.name) {
                Ok(ToolResult::new(
                    call.id.to_string(),
                    json!({"source": self.prefix, "tool": call.name}).to_string(),
                    false,
                )
                .into())
            } else {
                Err(ToolError::not_found(call.name))
            }
        }
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl AgentToolDispatcher for MutableMockDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            self.tools.lock().unwrap().clone().into()
        }

        async fn dispatch(
            &self,
            call: ToolCallView<'_>,
        ) -> Result<crate::ops::ToolDispatchOutcome, ToolError> {
            if self
                .tools
                .lock()
                .unwrap()
                .iter()
                .any(|t| t.name == call.name)
            {
                Ok(ToolResult::new(
                    call.id.to_string(),
                    json!({"source": self.prefix, "tool": call.name}).to_string(),
                    false,
                )
                .into())
            } else {
                Err(ToolError::not_found(call.name))
            }
        }
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl AgentToolDispatcher for UnroutableExactDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            Arc::from([Arc::clone(&self.tool)])
        }

        fn tool_catalog_capabilities(&self) -> crate::ToolCatalogCapabilities {
            crate::ToolCatalogCapabilities {
                exact_catalog: true,
                may_require_catalog_control_plane: false,
            }
        }

        fn tool_catalog(&self) -> Arc<[crate::ToolCatalogEntry]> {
            Arc::from([crate::ToolCatalogEntry::session_inline(
                Arc::clone(&self.tool),
                true,
            )])
        }

        async fn dispatch(
            &self,
            call: ToolCallView<'_>,
        ) -> Result<crate::ops::ToolDispatchOutcome, ToolError> {
            Err(ToolError::not_found(call.name))
        }
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl AgentToolDispatcher for ContextAwareDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            Arc::from([Arc::clone(&self.tool)])
        }

        fn tool_catalog_capabilities(&self) -> crate::ToolCatalogCapabilities {
            crate::ToolCatalogCapabilities {
                exact_catalog: self.exact_catalog,
                may_require_catalog_control_plane: false,
            }
        }

        fn tool_catalog(&self) -> Arc<[crate::ToolCatalogEntry]> {
            Arc::from([crate::ToolCatalogEntry::session_inline(
                Arc::clone(&self.tool),
                true,
            )])
        }

        async fn dispatch(
            &self,
            call: ToolCallView<'_>,
        ) -> Result<crate::ops::ToolDispatchOutcome, ToolError> {
            Ok(ToolResult::new(
                call.id.to_string(),
                json!({"saw_context_image": false}).to_string(),
                false,
            )
            .into())
        }

        async fn dispatch_with_context(
            &self,
            call: ToolCallView<'_>,
            context: &crate::ToolDispatchContext,
        ) -> Result<crate::ops::ToolDispatchOutcome, ToolError> {
            Ok(ToolResult::new(
                call.id.to_string(),
                json!({"saw_context_image": context.current_turn_image(0).is_some()}).to_string(),
                false,
            )
            .into())
        }
    }

    #[test]
    fn test_gateway_merges_tools() {
        let base = Arc::new(MockDispatcher::new("base", &["task_create", "task_list"]));
        let overlay = Arc::new(MockDispatcher::new("comms", &["send", "peers"]));

        let gateway = ToolGateway::new(base, Some(overlay)).unwrap();

        let tools = gateway.tools();
        let tool_names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(tool_names.len(), 4);
        assert!(tool_names.contains(&"task_create"));
        assert!(tool_names.contains(&"task_list"));
        assert!(tool_names.contains(&"send"));
        assert!(tool_names.contains(&"peers"));
    }

    #[test]
    fn test_gateway_no_overlay() {
        let base = Arc::new(MockDispatcher::new("base", &["task_create", "task_list"]));

        let gateway = ToolGateway::new(base, None).unwrap();

        assert_eq!(gateway.tools().len(), 2);
    }

    #[test]
    fn test_gateway_collision_error() {
        let base = Arc::new(MockDispatcher::new("base", &["task_create", "send"]));
        let overlay = Arc::new(MockDispatcher::new("comms", &["send", "peers"]));

        let result = ToolGateway::new(base, Some(overlay));

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("send"));
        assert!(err.to_string().contains("collision"));
    }

    #[tokio::test]
    async fn test_gateway_routes_to_base() {
        let base = Arc::new(MockDispatcher::new("base", &["task_create"]));
        let overlay = Arc::new(MockDispatcher::new("comms", &["send"]));

        let gateway = ToolGateway::new(base, Some(overlay)).unwrap();

        let result = dispatch_json(&gateway, "task_create", json!({}))
            .await
            .unwrap();
        assert_eq!(result["source"], "base");
        assert_eq!(result["tool"], "task_create");
    }

    #[tokio::test]
    async fn test_gateway_routes_to_overlay() {
        let base = Arc::new(MockDispatcher::new("base", &["task_create"]));
        let overlay = Arc::new(MockDispatcher::new("comms", &["send"]));

        let gateway = ToolGateway::new(base, Some(overlay)).unwrap();

        let result = dispatch_json(&gateway, "send", json!({})).await.unwrap();
        assert_eq!(result["source"], "comms");
        assert_eq!(result["tool"], "send");
    }

    #[tokio::test]
    async fn test_gateway_not_found() {
        let base = Arc::new(MockDispatcher::new("base", &["task_create"]));

        let gateway = ToolGateway::new(base, None).unwrap();

        let result = dispatch_json(&gateway, "unknown_tool", json!({})).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ToolError::NotFound { .. }));
    }

    #[test]
    fn test_builder_multiple_dispatchers() {
        let base = Arc::new(MockDispatcher::new("base", &["task_create"]));
        let comms = Arc::new(MockDispatcher::new("comms", &["send"]));
        let shell = Arc::new(MockDispatcher::new("shell", &["run_command"]));

        let gateway = ToolGatewayBuilder::new()
            .add_dispatcher(base)
            .add_dispatcher(comms)
            .add_dispatcher(shell)
            .build()
            .unwrap();

        assert_eq!(gateway.tools().len(), 3);
    }

    #[tokio::test]
    async fn gateway_exact_catalog_uses_live_callability_snapshot_for_all_paths() {
        let callable = Arc::new(AtomicBool::new(false));
        let base = Arc::new(MockDispatcher::new("base", &["task_create"]));
        let dynamic = Arc::new(LiveExactMockDispatcher::new(
            "dynamic",
            "send",
            Arc::clone(&callable),
        ));

        let gateway = ToolGatewayBuilder::new()
            .add_dispatcher(base)
            .add_dispatcher(dynamic)
            .build()
            .unwrap();

        let tools = gateway.tools();
        assert_eq!(
            tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            vec!["task_create"]
        );
        let catalog = gateway.tool_catalog();
        assert!(
            !catalog
                .iter()
                .find(|entry| entry.tool.name == "send")
                .expect("send catalog entry")
                .currently_callable()
        );
        let result = dispatch_json(&gateway, "send", json!({})).await;
        assert!(matches!(result, Err(ToolError::Unavailable { .. })));

        callable.store(true, Ordering::SeqCst);
        let tools = gateway.tools();
        assert_eq!(
            tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            vec!["task_create", "send"]
        );
        let catalog = gateway.tool_catalog();
        assert!(
            catalog
                .iter()
                .find(|entry| entry.tool.name == "send")
                .expect("send catalog entry")
                .currently_callable()
        );
        let result = dispatch_json(&gateway, "send", json!({})).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn gateway_admits_live_child_tool_identity_updates() {
        let dynamic = Arc::new(MutableMockDispatcher::new("dynamic", &["initial"]));
        let gateway = ToolGatewayBuilder::new()
            .add_dispatcher(dynamic.clone())
            .build()
            .unwrap();

        let initial_names: Vec<_> = gateway
            .tools()
            .iter()
            .map(|tool| tool.name.to_string())
            .collect();
        assert_eq!(initial_names, vec!["initial".to_string()]);

        dynamic.add_tool("late");

        let live_names: Vec<_> = gateway
            .tools()
            .iter()
            .map(|tool| tool.name.to_string())
            .collect();
        assert_eq!(
            live_names,
            vec!["initial".to_string(), "late".to_string()],
            "gateway should admit live tool identities from child dispatchers"
        );

        let catalog_names: Vec<_> = gateway
            .tool_catalog()
            .iter()
            .map(|entry| entry.tool.name.to_string())
            .collect();
        assert_eq!(
            catalog_names,
            vec!["initial".to_string(), "late".to_string()]
        );

        let result = dispatch_json(&gateway, "late", json!({})).await;
        assert!(result.is_ok());

        dynamic.remove_tool("initial");
        assert_eq!(
            gateway
                .tools()
                .iter()
                .map(|tool| tool.name.to_string())
                .collect::<Vec<_>>(),
            vec!["late".to_string()]
        );
        let catalog = gateway.tool_catalog();
        assert_eq!(catalog.len(), 1);
        assert_eq!(catalog[0].tool.name, "late");
        assert!(catalog[0].currently_callable());

        let result = dispatch_json(&gateway, "initial", json!({})).await;
        assert!(matches!(result, Err(ToolError::NotFound { .. })));
    }

    #[tokio::test]
    async fn gateway_fails_closed_when_visible_identity_is_not_routable() {
        let visible = Arc::new(UnroutableExactDispatcher::new("visible"));
        let gateway = ToolGatewayBuilder::new()
            .add_dispatcher(visible)
            .build()
            .unwrap();

        assert_eq!(
            gateway
                .tools()
                .iter()
                .map(|tool| tool.name.to_string())
                .collect::<Vec<_>>(),
            vec!["visible".to_string()]
        );

        let err = dispatch_json(&gateway, "visible", json!({}))
            .await
            .unwrap_err();
        let reason = match &err {
            ToolError::Unavailable { reason, .. } => Some(*reason),
            _ => None,
        };
        assert_eq!(reason, Some(ToolUnavailableReason::NotCurrentlyCallable));
    }

    #[tokio::test]
    async fn test_gateway_unavailable_dispatch() {
        let base = Arc::new(MockDispatcher::new("base", &["task_create"]));
        let exact = Arc::new(ExactMockDispatcher::with_callability(
            "exact",
            &[("send", false)],
        ));

        let gateway = ToolGatewayBuilder::new()
            .add_dispatcher(base)
            .add_dispatcher(exact)
            .build()
            .unwrap();

        let result = dispatch_json(&gateway, "send", json!({})).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, ToolError::Unavailable { .. }));
        assert!(err.to_string().contains("not currently callable"));
    }

    #[tokio::test]
    async fn gateway_preserves_typed_unavailable_reason() {
        let exact = Arc::new(ExactMockDispatcher::with_unavailable_reason(
            "exact",
            "peers",
            ToolUnavailableReason::NoPeersConfigured,
        ));
        let gateway = ToolGatewayBuilder::new()
            .add_dispatcher(exact)
            .build()
            .unwrap();

        let err = dispatch_json(&gateway, "peers", json!({}))
            .await
            .unwrap_err();

        let reason = match &err {
            ToolError::Unavailable { reason, .. } => Some(*reason),
            _ => None,
        };
        assert_eq!(reason, Some(ToolUnavailableReason::NoPeersConfigured));
        assert!(err.to_string().contains("no peers configured"));
    }

    #[test]
    fn test_collision_detection_ignores_callability() {
        let base = Arc::new(MockDispatcher::new("base", &["send"]));
        let exact = Arc::new(ExactMockDispatcher::with_callability(
            "exact",
            &[("send", false)],
        ));

        let result = ToolGatewayBuilder::new()
            .add_dispatcher(base)
            .add_dispatcher(exact)
            .build();

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("collision"));
    }

    #[test]
    fn test_builder_maybe_add() {
        let base = Arc::new(MockDispatcher::new("base", &["task_create"]));

        // None case
        let gateway = ToolGatewayBuilder::new()
            .add_dispatcher(base.clone())
            .maybe_add_dispatcher(None)
            .build()
            .unwrap();
        assert_eq!(gateway.tools().len(), 1);

        // Some case
        let overlay = Arc::new(MockDispatcher::new("comms", &["send"]));
        let gateway = ToolGatewayBuilder::new()
            .add_dispatcher(base)
            .maybe_add_dispatcher(Some(overlay))
            .build()
            .unwrap();
        assert_eq!(gateway.tools().len(), 2);
    }

    /// Mock dispatcher that returns a pre-built ExternalToolUpdate from poll_external_updates.
    struct MockBgDispatcher {
        update: ExternalToolUpdate,
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl AgentToolDispatcher for MockBgDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            Arc::new([])
        }

        async fn dispatch(
            &self,
            _call: ToolCallView<'_>,
        ) -> Result<crate::ops::ToolDispatchOutcome, ToolError> {
            Err(ToolError::not_found(""))
        }

        async fn poll_external_updates(&self) -> ExternalToolUpdate {
            self.update.clone()
        }
    }

    /// CHOKE-003-IT-B: ToolGateway deduplicates background_completions by job_id.
    ///
    /// Two dispatchers return the same job_id. After poll_external_updates,
    /// only one DetachedOpCompletion should appear (deduped by job_id).
    /// This test is expected to FAIL until Phase 2 adds dedup logic.
    #[tokio::test]
    async fn choke_003_gateway_dedups_background_completions_by_job_id() {
        use crate::agent::DetachedOpCompletion;
        use crate::ops_lifecycle::{OperationKind, OperationStatus};

        let completion = DetachedOpCompletion {
            job_id: "j_123".into(),
            kind: OperationKind::BackgroundToolOp,
            status: OperationStatus::Completed,
            terminal_outcome: None,
            display_name: "sleep 2".into(),
            detail: "exit_code: 0".into(),
            elapsed_ms: Some(2000),
        };

        let update = ExternalToolUpdate {
            notices: Vec::new(),
            pending: Vec::new(),
            background_completions: vec![completion.clone()],
        };

        let d1: Arc<dyn AgentToolDispatcher> = Arc::new(MockBgDispatcher {
            update: update.clone(),
        });
        let d2: Arc<dyn AgentToolDispatcher> = Arc::new(MockBgDispatcher { update });

        let gateway = ToolGatewayBuilder::new()
            .add_dispatcher(d1)
            .add_dispatcher(d2)
            .build()
            .unwrap();

        let result = gateway.poll_external_updates().await;
        assert_eq!(
            result.background_completions.len(),
            1,
            "gateway must dedup background_completions by job_id; got {} entries",
            result.background_completions.len()
        );
        assert_eq!(result.background_completions[0].job_id, "j_123");
    }

    #[test]
    fn gateway_exact_catalog_tracks_unavailable_winners() {
        let base = Arc::new(ExactMockDispatcher::with_callability(
            "base",
            &[("alpha", true)],
        ));
        let overlay = Arc::new(ExactMockDispatcher::with_callability(
            "overlay",
            &[("beta", false)],
        ));

        let gateway = ToolGateway::new(base, Some(overlay)).expect("gateway should build");

        assert!(
            gateway.tool_catalog_capabilities().exact_catalog,
            "gateway should be exact when every child is exact"
        );

        let visible_names: Vec<_> = gateway
            .tools()
            .iter()
            .map(|tool| tool.name.to_string())
            .collect();
        assert_eq!(visible_names, vec!["alpha".to_string()]);

        let catalog = gateway.tool_catalog();
        let catalog_names: Vec<_> = catalog
            .iter()
            .map(|entry| entry.tool.name.to_string())
            .collect();
        assert_eq!(catalog_names, vec!["alpha".to_string(), "beta".to_string()]);
        assert!(
            !catalog
                .iter()
                .find(|entry| entry.tool.name == "beta")
                .expect("beta catalog entry")
                .currently_callable(),
            "exact catalog should retain unavailable winners"
        );
    }

    #[test]
    fn gateway_exact_catalog_is_disabled_by_non_exact_child() {
        let exact = Arc::new(ExactMockDispatcher::with_callability(
            "exact",
            &[("alpha", true)],
        ));
        let non_exact = Arc::new(MockDispatcher::new("legacy", &["beta"]));

        let gateway = ToolGateway::new(exact, Some(non_exact)).expect("gateway should build");

        assert!(
            !gateway.tool_catalog_capabilities().exact_catalog,
            "gateway should disable deferred catalogs when any child is non-exact"
        );
    }

    #[test]
    fn dynamic_tool_composite_exact_catalog_keeps_first_winner_even_when_unavailable() {
        let first = Arc::new(ExactMockDispatcher::with_callability(
            "first",
            &[("shared", false)],
        ));
        let second = Arc::new(ExactMockDispatcher::with_callability(
            "second",
            &[("shared", true), ("other", true)],
        ));
        let composite = DynamicToolComposite::new(vec![first, second]);

        assert!(
            composite.tool_catalog_capabilities().exact_catalog,
            "dynamic composite should be exact when every child is exact"
        );

        let visible_names: Vec<_> = composite
            .tools()
            .iter()
            .map(|tool| tool.name.to_string())
            .collect();
        assert_eq!(
            visible_names,
            vec!["other".to_string()],
            "a later visible collision loser must not become the exported winner"
        );

        let catalog = composite.tool_catalog();
        assert_eq!(catalog.len(), 2);
        assert!(
            !catalog
                .iter()
                .find(|entry| entry.tool.name == "shared")
                .expect("shared entry")
                .currently_callable()
        );
    }

    #[tokio::test]
    async fn tool_gateway_dispatch_with_context_delegates_turn_context() {
        let dispatcher = Arc::new(ContextAwareDispatcher::new("inspect_context", true));
        let gateway = ToolGatewayBuilder::new()
            .add_dispatcher(dispatcher)
            .build()
            .expect("gateway should build");
        let args_raw = serde_json::value::RawValue::from_string("{}".to_string()).unwrap();
        let call = ToolCallView {
            id: "ctx-1",
            name: "inspect_context",
            args: &args_raw,
        };
        let context = crate::ToolDispatchContext::from_current_turn_input(
            &crate::ContentInput::Blocks(vec![crate::ContentBlock::Image {
                media_type: "image/png".into(),
                data: "abc".into(),
            }]),
        );

        let outcome = gateway.dispatch_with_context(call, &context).await.unwrap();
        let payload: Value = serde_json::from_str(&outcome.result.text_content()).unwrap();
        assert_eq!(payload["saw_context_image"], true);
    }

    #[tokio::test]
    async fn dynamic_tool_composite_dispatch_with_context_delegates_turn_context() {
        let dispatcher = Arc::new(ContextAwareDispatcher::new("inspect_context", false));
        let composite = DynamicToolComposite::new(vec![dispatcher]);
        let args_raw = serde_json::value::RawValue::from_string("{}".to_string()).unwrap();
        let call = ToolCallView {
            id: "ctx-1",
            name: "inspect_context",
            args: &args_raw,
        };
        let context = crate::ToolDispatchContext::from_current_turn_input(
            &crate::ContentInput::Blocks(vec![crate::ContentBlock::Image {
                media_type: "image/png".into(),
                data: "abc".into(),
            }]),
        );

        let outcome = composite
            .dispatch_with_context(call, &context)
            .await
            .unwrap();
        let payload: Value = serde_json::from_str(&outcome.result.text_content()).unwrap();
        assert_eq!(payload["saw_context_image"], true);
    }
}
