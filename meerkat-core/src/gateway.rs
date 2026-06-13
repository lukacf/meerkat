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
use crate::agent::ExternalToolUpdate;
use crate::error::ToolError;
use crate::event::ExternalToolDelta;
#[cfg(all(target_arch = "wasm32", test))]
use crate::tokio;
use crate::tool_catalog::{ToolCatalogCapabilities, ToolCatalogEntry, ToolUnavailableReason};
use crate::types::{ToolCallView, ToolDef, ToolName};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;

/// Entry for a dispatcher in the gateway.
struct DispatcherEntry {
    dispatcher: Arc<dyn AgentToolDispatcher>,
}

/// How a single dispatcher entry routes a given tool name. A flat, typed
/// three-way verdict (rather than `Option<Option<…>>`): the name is either not
/// surfaced by this entry, surfaced and callable, or surfaced but currently
/// unavailable for a typed reason.
enum EntryRouting {
    /// The entry does not surface this tool name live.
    NotSurfaced,
    /// The entry surfaces the tool and it is callable.
    Callable,
    /// The entry surfaces the tool but it is currently unavailable.
    Unavailable(ToolUnavailableReason),
}

/// Gateway-level routing verdict for a tool name: the single owning entry to
/// dispatch to, a typed unavailability, or no owner at all. Collisions are not
/// representable here — they fail closed as typed errors during resolution.
enum GatewayRouting<'a> {
    Dispatch(&'a DispatcherEntry),
    Unavailable(ToolUnavailableReason),
    NotFound,
}

/// Dynamic composite routing verdict. It uses dispatcher borrows directly
/// because the dynamic compositor deliberately does not freeze ownership in a
/// build-time routing table.
enum DynamicRouting<'a> {
    Dispatch(&'a dyn AgentToolDispatcher),
    Unavailable(ToolUnavailableReason),
    NotFound,
}

/// A tool dispatcher that composes multiple dispatchers into one.
///
/// The gateway validates the initial child catalogs for collisions and records
/// a typed routing table keyed by [`ToolName`] that owns the
/// `tool name -> owning dispatcher` decision. The build-time collision check
/// already proves uniqueness, so the routing table is the canonical authority
/// for build-known tools (deterministic owner per `ToolName`, stable regardless
/// of insertion order). Dynamic dispatchers that surface newly connected tools
/// after build time fall back to live catalog/list iteration, failing closed
/// when more than one dispatcher surfaces the same name.
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
    /// Typed routing authority: build-known tool name -> owning entry index.
    ///
    /// Populated from the frozen build-time catalogs (uniqueness already
    /// guaranteed by the collision check), so routing for build-known tools is
    /// a typed lookup, not first-wins iteration re-derived on every call.
    routing: HashMap<ToolName, usize>,
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

    /// Resolve where `name` routes, through the typed routing authority.
    ///
    /// Build-known tools resolve to their recorded owner — exclusively. If the
    /// owner no longer surfaces the name, ownership does NOT silently migrate
    /// to another dispatcher that happens to surface the same name live: that
    /// is a typed collision fault. Names not in the routing table (surfaced
    /// dynamically after build time) resolve only when exactly one dispatcher
    /// surfaces them; live duplicates fail closed with the same collision
    /// fault instead of first-dispatcher-wins shell state.
    fn resolve_routing(&self, name: &str) -> Result<GatewayRouting<'_>, ToolError> {
        if let Some(&owner_index) = self.routing.get(name) {
            let Some(owner) = self.entries.get(owner_index) else {
                return Ok(GatewayRouting::NotFound);
            };
            return match Self::entry_routing(owner, name) {
                EntryRouting::Callable => Ok(GatewayRouting::Dispatch(owner)),
                EntryRouting::Unavailable(reason) => Ok(GatewayRouting::Unavailable(reason)),
                EntryRouting::NotSurfaced => {
                    let surfaced_elsewhere = self.entries.iter().enumerate().any(|(idx, other)| {
                        idx != owner_index
                            && !matches!(
                                Self::entry_routing(other, name),
                                EntryRouting::NotSurfaced
                            )
                    });
                    if surfaced_elsewhere {
                        return Err(ToolError::Other(format!(
                            "tool name collision in gateway: '{name}' (build-known owner no \
                             longer surfaces it; ownership does not migrate to another \
                             dispatcher)"
                        )));
                    }
                    Ok(GatewayRouting::NotFound)
                }
            };
        }
        let mut surfaced: Option<(&DispatcherEntry, EntryRouting)> = None;
        for entry in &self.entries {
            match Self::entry_routing(entry, name) {
                EntryRouting::NotSurfaced => {}
                routing => {
                    if surfaced.is_some() {
                        return Err(ToolError::Other(format!(
                            "tool name collision in gateway: '{name}' (surfaced live by \
                             multiple dispatchers)"
                        )));
                    }
                    surfaced = Some((entry, routing));
                }
            }
        }
        Ok(match surfaced {
            Some((entry, EntryRouting::Callable)) => GatewayRouting::Dispatch(entry),
            Some((_, EntryRouting::Unavailable(reason))) => GatewayRouting::Unavailable(reason),
            Some((_, EntryRouting::NotSurfaced)) | None => GatewayRouting::NotFound,
        })
    }

    /// Classify how the owning entry routes `name`: not surfaced, callable, or
    /// unavailable for a typed reason.
    fn entry_routing(entry: &DispatcherEntry, name: &str) -> EntryRouting {
        if entry.dispatcher.tool_catalog_capabilities().exact_catalog {
            match entry
                .dispatcher
                .tool_catalog()
                .iter()
                .find(|catalog_entry| catalog_entry.tool.name == name)
            {
                Some(catalog_entry) => match catalog_entry.callability.unavailable_reason() {
                    Some(reason) => EntryRouting::Unavailable(reason),
                    None => EntryRouting::Callable,
                },
                None => EntryRouting::NotSurfaced,
            }
        } else if entry
            .dispatcher
            .tools()
            .iter()
            .any(|tool| tool.name == name)
        {
            EntryRouting::Callable
        } else {
            EntryRouting::NotSurfaced
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
        let mut routing: HashMap<ToolName, usize> = HashMap::new();

        for dispatcher in self.dispatchers {
            let entry_index = entries.len();
            let frozen_catalog = ToolGateway::live_catalog_for_dispatcher(dispatcher.as_ref());
            for entry in frozen_catalog.iter() {
                let name = entry.tool.tool_name();
                if routing.insert(name, entry_index).is_some() {
                    return Err(ToolError::Other(format!(
                        "tool name collision in gateway: '{}'",
                        entry.tool.name.as_str()
                    )));
                }
            }
            entries.push(DispatcherEntry { dispatcher });
        }

        Ok(ToolGateway { entries, routing })
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
        match self.resolve_routing(call.name)? {
            GatewayRouting::Dispatch(entry) => entry
                .dispatcher
                .dispatch(call)
                .await
                .map_err(|err| Self::route_not_found_as_unavailable(call.name, err)),
            GatewayRouting::Unavailable(reason) => Err(ToolError::unavailable(call.name, reason)),
            GatewayRouting::NotFound => Err(ToolError::not_found(call.name)),
        }
    }

    async fn dispatch_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &crate::ToolDispatchContext,
    ) -> Result<crate::ops::ToolDispatchOutcome, ToolError> {
        match self.resolve_routing(call.name)? {
            GatewayRouting::Dispatch(entry) => entry
                .dispatcher
                .dispatch_with_context(call, context)
                .await
                .map_err(|err| Self::route_not_found_as_unavailable(call.name, err)),
            GatewayRouting::Unavailable(reason) => Err(ToolError::unavailable(call.name, reason)),
            GatewayRouting::NotFound => Err(ToolError::not_found(call.name)),
        }
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
        for (entry_index, entry) in self.entries.iter().enumerate() {
            for catalog_entry in Self::live_catalog_for_dispatcher(entry.dispatcher.as_ref()).iter()
            {
                // Build-known names are listed only from their recorded owner;
                // the typed routing table — not catalog iteration order — owns
                // which dispatcher's entry represents the name.
                if let Some(&owner_index) = self.routing.get(catalog_entry.tool.name.as_str())
                    && owner_index != entry_index
                {
                    continue;
                }
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
        }

        ExternalToolUpdate {
            notices: all_notices,
            pending: all_pending,
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
/// Dynamic duplicate names fail closed. Callers that need legitimate profile
/// priority should express that priority before composing children; the shared
/// dynamic compositor is not a first-child ownership authority.
pub struct DynamicToolComposite {
    dispatchers: Vec<Arc<dyn AgentToolDispatcher>>,
}

impl DynamicToolComposite {
    pub fn new(dispatchers: Vec<Arc<dyn AgentToolDispatcher>>) -> Self {
        Self { dispatchers }
    }

    fn duplicate_tool_error(name: &str) -> ToolError {
        ToolError::Other(format!(
            "tool name collision in dynamic composite: '{name}' surfaced by multiple dispatchers"
        ))
    }

    fn live_catalog_for_dispatcher(
        dispatcher: &dyn AgentToolDispatcher,
    ) -> Arc<[ToolCatalogEntry]> {
        ToolGateway::live_catalog_for_dispatcher(dispatcher)
    }

    fn entry_routing(dispatcher: &dyn AgentToolDispatcher, name: &str) -> EntryRouting {
        Self::live_catalog_for_dispatcher(dispatcher)
            .iter()
            .find(|catalog_entry| catalog_entry.tool.name == name)
            .map(
                |catalog_entry| match catalog_entry.callability.unavailable_reason() {
                    Some(reason) => EntryRouting::Unavailable(reason),
                    None => EntryRouting::Callable,
                },
            )
            .unwrap_or(EntryRouting::NotSurfaced)
    }

    fn resolve_live_routing(&self, name: &str) -> Result<DynamicRouting<'_>, ToolError> {
        let mut surfaced: Option<(&dyn AgentToolDispatcher, EntryRouting)> = None;
        for dispatcher in &self.dispatchers {
            match Self::entry_routing(dispatcher.as_ref(), name) {
                EntryRouting::NotSurfaced => {}
                routing => {
                    if surfaced.is_some() {
                        return Err(Self::duplicate_tool_error(name));
                    }
                    surfaced = Some((dispatcher.as_ref(), routing));
                }
            }
        }
        Ok(match surfaced {
            Some((dispatcher, EntryRouting::Callable)) => DynamicRouting::Dispatch(dispatcher),
            Some((_, EntryRouting::Unavailable(reason))) => DynamicRouting::Unavailable(reason),
            Some((_, EntryRouting::NotSurfaced)) | None => DynamicRouting::NotFound,
        })
    }

    fn catalog_entries(&self) -> Vec<ToolCatalogEntry> {
        let mut counts: HashMap<ToolName, usize> = HashMap::new();
        let mut result = Vec::new();
        for dispatcher in &self.dispatchers {
            for entry in Self::live_catalog_for_dispatcher(dispatcher.as_ref()).iter() {
                *counts.entry(entry.tool.name.clone()).or_default() += 1;
                result.push(entry.clone());
            }
        }
        result.retain(|entry| counts.get(&entry.tool.name).copied().unwrap_or(0) == 1);
        result
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl AgentToolDispatcher for DynamicToolComposite {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        self.catalog_entries()
            .into_iter()
            .filter(|entry| entry.currently_callable())
            .map(|entry| entry.tool)
            .collect::<Vec<_>>()
            .into()
    }

    async fn dispatch(
        &self,
        call: crate::types::ToolCallView<'_>,
    ) -> Result<crate::ops::ToolDispatchOutcome, crate::error::ToolError> {
        match self.resolve_live_routing(call.name)? {
            DynamicRouting::Dispatch(dispatcher) => dispatcher
                .dispatch(call)
                .await
                .map_err(|err| ToolGateway::route_not_found_as_unavailable(call.name, err)),
            DynamicRouting::Unavailable(reason) => {
                Err(crate::error::ToolError::unavailable(call.name, reason))
            }
            DynamicRouting::NotFound => Err(crate::error::ToolError::not_found(call.name)),
        }
    }

    async fn dispatch_with_context(
        &self,
        call: crate::types::ToolCallView<'_>,
        context: &crate::ToolDispatchContext,
    ) -> Result<crate::ops::ToolDispatchOutcome, crate::error::ToolError> {
        match self.resolve_live_routing(call.name)? {
            DynamicRouting::Dispatch(dispatcher) => dispatcher
                .dispatch_with_context(call, context)
                .await
                .map_err(|err| ToolGateway::route_not_found_as_unavailable(call.name, err)),
            DynamicRouting::Unavailable(reason) => {
                Err(crate::error::ToolError::unavailable(call.name, reason))
            }
            DynamicRouting::NotFound => Err(crate::error::ToolError::not_found(call.name)),
        }
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
        self.catalog_entries().into()
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
            let saw_context_image = context
                .current_turn()
                .and_then(|turn| turn.image_ref(0))
                .and_then(|image_ref| context.current_turn_image(image_ref))
                .is_some();
            Ok(ToolResult::new(
                call.id.to_string(),
                json!({"saw_context_image": saw_context_image}).to_string(),
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

    /// Gate: a tool name surfaced live by TWO dispatchers after build time is
    /// a typed collision fault, not first-dispatcher-wins shell state.
    #[tokio::test]
    async fn gateway_fails_closed_on_dynamic_live_duplicate() {
        let a = Arc::new(MutableMockDispatcher::new("a", &["a_only"]));
        let b = Arc::new(MutableMockDispatcher::new("b", &["b_only"]));
        let gateway = ToolGatewayBuilder::new()
            .add_dispatcher(a.clone())
            .add_dispatcher(b.clone())
            .build()
            .unwrap();

        a.add_tool("dup");
        b.add_tool("dup");

        let err = dispatch_json(&gateway, "dup", json!({})).await.unwrap_err();
        assert!(
            matches!(&err, ToolError::Other(message) if message.contains("collision")),
            "live duplicate must fail closed as a typed collision, got {err:?}"
        );
    }

    /// Gate: the typed routing table owns build-known names exclusively. When
    /// the recorded owner stops surfacing a name that another dispatcher now
    /// surfaces live, ownership does not silently migrate — the gateway fails
    /// closed with a typed collision fault.
    #[tokio::test]
    async fn gateway_ownership_does_not_migrate_to_live_duplicate() {
        let owner = Arc::new(MutableMockDispatcher::new("owner", &["shared"]));
        let interloper = Arc::new(MutableMockDispatcher::new("interloper", &["other"]));
        let gateway = ToolGatewayBuilder::new()
            .add_dispatcher(owner.clone())
            .add_dispatcher(interloper.clone())
            .build()
            .unwrap();

        // An interloper surfaces the build-known name live. While the recorded
        // owner still surfaces it, routing stays deterministic on the owner.
        interloper.add_tool("shared");
        let result = dispatch_json(&gateway, "shared", json!({})).await.unwrap();
        assert_eq!(result["source"], "owner");
        // The catalog also lists the name from its recorded owner only.
        let catalog = gateway.tool_catalog();
        let shared_entries: Vec<_> = catalog
            .iter()
            .filter(|entry| entry.tool.name == "shared")
            .collect();
        assert_eq!(shared_entries.len(), 1);
        assert!(shared_entries[0].tool.description.contains("owner"));

        // Owner stops surfacing it: the interloper must not inherit the name.
        owner.remove_tool("shared");
        let err = dispatch_json(&gateway, "shared", json!({}))
            .await
            .unwrap_err();
        assert!(
            matches!(&err, ToolError::Other(message) if message.contains("collision")),
            "ownership migration must fail closed as a typed collision, got {err:?}"
        );
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
    fn dynamic_tool_composite_exact_catalog_omits_duplicate_names() {
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
            "collided tool names must not be exported through first-child ownership"
        );

        let catalog = composite.tool_catalog();
        assert!(
            catalog.iter().all(|entry| entry.tool.name != "shared"),
            "collided tool names must be omitted from the dynamic catalog"
        );
    }

    #[tokio::test]
    async fn dynamic_tool_composite_fails_closed_on_duplicate_dispatch() {
        let first = Arc::new(MockDispatcher::new("first", &["shared"]));
        let second = Arc::new(MockDispatcher::new("second", &["shared"]));
        let composite = DynamicToolComposite::new(vec![first, second]);
        let args_raw = serde_json::value::RawValue::from_string("{}".to_string()).unwrap();
        let call = ToolCallView {
            id: "collision-1",
            name: "shared",
            args: &args_raw,
        };

        let err = composite
            .dispatch(call)
            .await
            .expect_err("duplicate dynamic tool names must fail closed");
        assert!(
            matches!(err, ToolError::Other(ref message) if message.contains("collision")),
            "expected collision error, got {err:?}"
        );
    }

    #[tokio::test]
    async fn gateway_typed_routing_owner_is_stable_regardless_of_insertion_order() {
        // The typed routing table must resolve each ToolName to a single
        // deterministic owning dispatcher, regardless of the order dispatchers
        // were added. `alpha` is owned by "base" and `beta` by "comms" in both
        // orderings; routing identity (the source that answers a dispatch) must
        // not change with insertion order.
        let base_first = {
            let base = Arc::new(MockDispatcher::new("base", &["alpha"]));
            let comms = Arc::new(MockDispatcher::new("comms", &["beta"]));
            ToolGatewayBuilder::new()
                .add_dispatcher(base)
                .add_dispatcher(comms)
                .build()
                .expect("gateway builds")
        };
        let comms_first = {
            let base = Arc::new(MockDispatcher::new("base", &["alpha"]));
            let comms = Arc::new(MockDispatcher::new("comms", &["beta"]));
            ToolGatewayBuilder::new()
                .add_dispatcher(comms)
                .add_dispatcher(base)
                .build()
                .expect("gateway builds")
        };

        // The typed routing table owns the decision: same owner in both
        // orderings (the resolved entry's dispatcher answers the call).
        for name in ["alpha", "beta"] {
            let owner_tools = |gateway: &ToolGateway| -> Vec<String> {
                match gateway.resolve_routing(name) {
                    Ok(GatewayRouting::Dispatch(entry)) => entry
                        .dispatcher
                        .tools()
                        .iter()
                        .map(|t| t.name.to_string())
                        .collect(),
                    _ => Vec::new(),
                }
            };
            let owner_a = owner_tools(&base_first);
            let owner_b = owner_tools(&comms_first);
            assert!(
                !owner_a.is_empty(),
                "`{name}` must resolve to a dispatchable owner"
            );
            assert_eq!(
                owner_a, owner_b,
                "routing owner for `{name}` must be stable across insertion order"
            );
        }

        // And dispatch routes through the typed map to the correct source.
        let alpha = dispatch_json(&comms_first, "alpha", json!({}))
            .await
            .expect("alpha dispatches");
        assert_eq!(alpha["source"], "base");
        let beta = dispatch_json(&base_first, "beta", json!({}))
            .await
            .expect("beta dispatches");
        assert_eq!(beta["source"], "comms");
    }

    #[test]
    fn gateway_typed_routing_table_rejects_duplicate_owner() {
        // Two dispatchers claiming the same name still fail at build: the typed
        // routing table cannot admit two owners for one ToolName.
        let base = Arc::new(MockDispatcher::new("base", &["dup"]));
        let comms = Arc::new(MockDispatcher::new("comms", &["dup"]));
        let result = ToolGatewayBuilder::new()
            .add_dispatcher(base)
            .add_dispatcher(comms)
            .build();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("collision"));
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
