//! A host's opt-in deferral of stock tool families.
//!
//! [`StockDeferralDispatcher`] applies a [`StockToolDeferralPolicy`] to a
//! session's composed tool dispatcher. It changes what
//! [`AgentToolDispatcher::tool_catalog`] reports and nothing else: the tools
//! of a family the policy names are reported as deferred-eligible, so the
//! session's ordinary catalog rules decide whether they are sent in every
//! model request or wait behind `tool_catalog_search` and
//! `tool_catalog_load`.
//!
//! The policy is host build configuration. It is constant for the process
//! and adds no session state: which tools a session requested and loaded
//! stays where it is for every deferred tool, in the session's visibility
//! state.

use crate::agent::{
    AgentToolDispatcher, BindOutcome, DispatcherCapabilities, ExternalToolUpdate,
    OpsLifecycleBindError, ToolDispatchContext,
};
use crate::error::ToolError;
use crate::tool_catalog::{
    StockToolDeferralPolicy, ToolCatalogCapabilities, ToolCatalogDeferredEligibility,
    ToolCatalogEntry, ToolPlaneClass,
};
use crate::tool_execution::ToolExecutionContract;
use crate::types::{ToolCallView, ToolDef, ToolName, ToolProvenance};
use async_trait::async_trait;
use std::collections::HashSet;
use std::sync::Arc;

/// A tool dispatcher that reports the tools of the stock families a
/// [`StockToolDeferralPolicy`] names as deferred-eligible, and forwards the
/// rest of the [`AgentToolDispatcher`] surface to the dispatcher it wraps.
///
/// A catalog entry of the wrapped dispatcher is reported as
/// `DeferredEligible`, owned by the tool's own provenance, when all of these
/// hold:
///
/// - it is a session-plane entry;
/// - it is `InlineOnly` (an entry its source already made deferred-eligible
///   keeps the owner its source gave it);
/// - its tool carries a provenance whose kind the policy names (a tool that
///   won a stock tool's name is judged by its own provenance, not by the
///   name);
/// - its execution contract is the default one (a tool that owns its
///   deadline, streams or can run detached stays in every request, with the
///   declaration it has today);
/// - its name is not one of the pinned names the host passes.
///
/// Every other entry is reported exactly as the wrapped dispatcher reports
/// it. A re-tagged entry keeps its tool definition, plane, callability and
/// execution contract; only its deferred eligibility differs, and with it
/// the entry's [`crate::ToolBindingDigest`].
///
/// Calls, execution plans, bindings and updates go to the wrapped dispatcher
/// unchanged. The catalog is read from it on every call, so a tool's current
/// callability is always the wrapped dispatcher's.
///
/// The wrapper checks nothing about the session. Whether a re-tagged tool
/// then waits is decided by the catalog of the dispatcher a host finally
/// hands to the agent, which the wrapper cannot see. A host that applies it
/// in its own composition applies it only where that final catalog is
/// exact, as the agent factory does: where it is not exact nothing waits,
/// and the wrapper would only change the named tools' catalog entries and
/// binding digests.
pub struct StockDeferralDispatcher {
    inner: Arc<dyn AgentToolDispatcher>,
    policy: StockToolDeferralPolicy,
    pinned: HashSet<ToolName>,
}

impl StockDeferralDispatcher {
    /// Wrap `inner` with `policy`. A tool named in `pinned` is never
    /// re-tagged: it stays in every model request.
    pub fn new<I, N>(
        inner: Arc<dyn AgentToolDispatcher>,
        policy: StockToolDeferralPolicy,
        pinned: I,
    ) -> Self
    where
        I: IntoIterator<Item = N>,
        N: Into<ToolName>,
    {
        Self {
            inner,
            policy,
            pinned: pinned.into_iter().map(Into::into).collect(),
        }
    }

    /// The provenance that owns `entry` behind the deferred catalog when the
    /// policy re-tags it, `None` when the entry is reported as it is.
    fn deferred_owner(&self, entry: &ToolCatalogEntry) -> Option<ToolProvenance> {
        if entry.plane != ToolPlaneClass::Session {
            return None;
        }
        if entry.deferred_eligibility != ToolCatalogDeferredEligibility::InlineOnly {
            return None;
        }
        let provenance = entry.tool.provenance.as_ref()?;
        if !self.policy.allows_source_kind(&provenance.kind) {
            return None;
        }
        if entry.execution != ToolExecutionContract::default() {
            return None;
        }
        if self.pinned.contains(&entry.tool.name) {
            return None;
        }
        Some(provenance.clone())
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl AgentToolDispatcher for StockDeferralDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        self.inner.tools()
    }

    async fn resolve_tool_application(
        &self,
        source_tool: &str,
        request: &crate::ToolApplicationRequest,
        invocation: &serde_json::Value,
        context: &ToolDispatchContext,
    ) -> Result<crate::tool_application::ToolApplicationResolution, ToolError> {
        self.inner
            .resolve_tool_application(source_tool, request, invocation, context)
            .await
    }

    fn tool_catalog_capabilities(&self) -> ToolCatalogCapabilities {
        self.inner.tool_catalog_capabilities()
    }

    fn tool_catalog(&self) -> Arc<[ToolCatalogEntry]> {
        let catalog = self.inner.tool_catalog();
        if !catalog
            .iter()
            .any(|entry| self.deferred_owner(entry).is_some())
        {
            return catalog;
        }
        catalog
            .iter()
            .map(|entry| {
                let mut entry = entry.clone();
                if let Some(provenance) = self.deferred_owner(&entry) {
                    entry.deferred_eligibility =
                        ToolCatalogDeferredEligibility::DeferredEligible { provenance };
                }
                entry
            })
            .collect::<Vec<_>>()
            .into()
    }

    fn tool_mutation_class(&self, tool_name: &str) -> crate::ToolMutationClass {
        self.inner.tool_mutation_class(tool_name)
    }

    fn live_bridge_effect_kind(&self, tool_name: &str) -> crate::LiveBridgeEffectKind {
        self.inner.live_bridge_effect_kind(tool_name)
    }

    fn review_entry_support(&self, tool_name: &str) -> crate::approval::review::ReviewEntrySupport {
        self.inner.review_entry_support(tool_name)
    }

    fn execution_binding_epoch(&self, tool_name: &str) -> u64 {
        self.inner.execution_binding_epoch(tool_name)
    }

    /// This wrapper's own entry for the tool (the re-tagged one where the
    /// policy applies) with the wrapped dispatcher's binding as its
    /// dependency, so a change of either is a change of the binding.
    fn execution_binding_fingerprint(
        &self,
        tool_name: &str,
    ) -> Result<crate::EphemeralToolBindingFingerprint, crate::ToolExecutionResolutionError> {
        let catalog = self.tool_catalog();
        let entry = catalog
            .iter()
            .find(|entry| entry.tool.name == tool_name)
            .ok_or_else(|| crate::ToolExecutionResolutionError::NotFound {
                tool_name: tool_name.to_string(),
            })?;
        Ok(crate::ephemeral_tool_catalog_binding_fingerprint(entry)
            .with_live_authority(0, 0)
            .with_dependency(&self.inner.execution_binding_fingerprint(tool_name)?))
    }

    fn resolve_execution_plan(
        &self,
        call: ToolCallView<'_>,
        dispatch_context: &ToolDispatchContext,
        resolution_context: &crate::ToolExecutionResolutionContext,
    ) -> Result<crate::ResolvedToolExecutionPlan, crate::ToolExecutionResolutionError> {
        self.inner
            .resolve_execution_plan(call, dispatch_context, resolution_context)
    }

    fn validate_resolved_execution_plan(
        &self,
        call: ToolCallView<'_>,
        resolution_context: &crate::ToolExecutionResolutionContext,
        plan: &crate::ResolvedToolExecutionPlan,
    ) -> Result<(), crate::ToolExecutionResolutionError> {
        self.inner
            .validate_resolved_execution_plan(call, resolution_context, plan)
    }

    fn pending_catalog_sources(&self) -> Arc<[String]> {
        self.inner.pending_catalog_sources()
    }

    fn adjudicates_tool_grants(&self) -> bool {
        self.inner.adjudicates_tool_grants()
    }

    async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<crate::ops::ToolDispatchOutcome, ToolError> {
        self.inner.dispatch(call).await
    }

    async fn dispatch_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
    ) -> Result<crate::ops::ToolDispatchOutcome, ToolError> {
        self.inner.dispatch_with_context(call, context).await
    }

    async fn dispatch_resolved_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        plan: &crate::ResolvedToolExecutionPlan,
    ) -> Result<crate::ops::ToolDispatchOutcome, ToolError> {
        self.inner
            .dispatch_resolved_with_context(call, context, plan)
            .await
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
        // A wrapped dispatcher that needs the binding but is still shared
        // cannot be rebound; leaving it unbound would drop its owner session
        // and registry, so the bind is rejected.
        if owned.inner.capabilities().ops_lifecycle && Arc::strong_count(&owned.inner) != 1 {
            return Err(OpsLifecycleBindError::SharedOwnership);
        }
        if Arc::strong_count(&owned.inner) != 1 {
            return Ok(BindOutcome::Skipped(Arc::new(owned)));
        }
        let outcome = owned
            .inner
            .bind_ops_lifecycle(registry, owner_bridge_session_id)?;
        let bound = outcome.was_bound();
        let rebound = Arc::new(Self {
            inner: outcome.into_dispatcher(),
            policy: owned.policy,
            pinned: owned.pinned,
        });
        Ok(if bound {
            BindOutcome::Bound(rebound)
        } else {
            BindOutcome::Skipped(rebound)
        })
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

    fn bind_session_mob_tool_authority(
        &self,
        authority: Option<Arc<std::sync::RwLock<crate::service::MobToolAuthorityContext>>>,
    ) {
        self.inner.bind_session_mob_tool_authority(authority);
    }

    fn observes_bindings_only(&self) -> bool {
        self.inner.observes_bindings_only()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests;
