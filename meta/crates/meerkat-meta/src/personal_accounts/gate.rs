//! The owner check: a dispatcher over each member's MCP adapter.
//!
//! A composition with personal servers composes every MCP adapter its member
//! factories build ([`PersonalMcpAccounts::composer`]), over any composer a
//! host installed. The returned [`PersonalAccountGate`] forwards everything
//! to the adapter (or the host's dispatcher) and checks each path that
//! reaches a server: every tool call (resolved or not) and every tool
//! application (an MCP app's resource read or tool call), before the router.
//! The server is named by the resolved plan's operation target or the app
//! invocation's registration when there is one, and by the tool's MCP
//! provenance in every case. A refused use ends as a policy denial with the
//! refusal's kind, and nothing is sent. The catalog paths (`tools`,
//! `tool_catalog`, the deferred catalog sources) only read what the
//! connection listed, so they reach no server.
//!
//! The agent's grant gate wraps the whole composed dispatcher, so it
//! authorizes a call first; the router's per-call preparation runs after
//! this check.

use std::sync::Arc;

use async_trait::async_trait;
use meerkat::{McpAdapterComposeRefusal, McpAdapterComposer};
use meerkat_core::agent::{
    BindOutcome, DispatcherCapabilities, ExternalToolUpdate, OpsLifecycleBindError,
};
use meerkat_core::ops::ToolDispatchOutcome;
use meerkat_core::types::{SessionId, ToolCallView, ToolDef};
use meerkat_core::{
    AgentToolDispatcher, ToolCatalogCapabilities, ToolCatalogEntry, ToolDispatchContext, ToolError,
};

use super::{PersonalAccountRefusal, PersonalMcpAccounts, PrivateOwner};

/// The composer [`PersonalMcpAccounts::composer`] installs.
struct PersonalAccountComposer {
    accounts: Arc<PersonalMcpAccounts>,
    host: Option<Arc<dyn McpAdapterComposer>>,
}

impl McpAdapterComposer for PersonalAccountComposer {
    fn compose(
        &self,
        adapter: &Arc<meerkat_mcp::McpRouterAdapter>,
        session_id: &SessionId,
        member: Option<&meerkat_core::MobMemberBinding>,
    ) -> Result<Arc<dyn AgentToolDispatcher>, McpAdapterComposeRefusal> {
        let inner = match &self.host {
            Some(host) => host.compose(adapter, session_id, member)?,
            None => Arc::clone(adapter) as Arc<dyn AgentToolDispatcher>,
        };
        let owner = self.accounts.take_plan(session_id);
        Ok(Arc::new(PersonalAccountGate {
            inner,
            accounts: Arc::clone(&self.accounts),
            owner,
        }))
    }
}

impl PersonalMcpAccounts {
    /// The MCP adapter composer of a member factory: every construction's
    /// adapter, composed by `host` first when a host installed a composer,
    /// behind the owner check.
    pub fn composer(
        self: &Arc<Self>,
        host: Option<Arc<dyn McpAdapterComposer>>,
    ) -> Arc<dyn McpAdapterComposer> {
        self.mark_gate_installed();
        Arc::new(PersonalAccountComposer {
            accounts: Arc::clone(self),
            host,
        })
    }
}

/// The owner check over one member's MCP adapter (see the module docs).
pub(crate) struct PersonalAccountGate {
    inner: Arc<dyn AgentToolDispatcher>,
    accounts: Arc<PersonalMcpAccounts>,
    /// The member's private owner, as its build established it; `None` for a
    /// member private to no person.
    owner: Option<PrivateOwner>,
}

impl PersonalAccountGate {
    #[cfg(test)]
    pub(crate) fn new(
        inner: Arc<dyn AgentToolDispatcher>,
        accounts: Arc<PersonalMcpAccounts>,
        owner: Option<PrivateOwner>,
    ) -> Self {
        Self {
            inner,
            accounts,
            owner,
        }
    }

    /// Check one use of `servers` (the servers a call names) for a run with
    /// `context`: every personal server among them must be the member
    /// owner's own use.
    fn check<'a>(
        &self,
        servers: impl IntoIterator<Item = &'a str>,
        context: &ToolDispatchContext,
    ) -> Result<(), PersonalAccountRefusal> {
        let config = self.accounts.config();
        servers
            .into_iter()
            .filter(|server| config.is_personal(server))
            .find_map(|server| {
                self.accounts
                    .refusal(server, self.owner.as_ref(), context.direct_requester())
            })
            .map_or(Ok(()), Err)
    }

    /// The MCP server the inner dispatcher's tool `name` comes from, by its
    /// MCP provenance.
    fn provenance_server(&self, name: &str) -> Option<String> {
        self.inner
            .tools()
            .iter()
            .find(|tool| tool.name.as_str() == name)
            .and_then(|tool| tool.provenance.as_ref())
            .filter(|provenance| provenance.kind == meerkat_core::types::ToolSourceKind::Mcp)
            .map(|provenance| provenance.source_id.as_str().to_string())
    }

    /// The MCP server a resolved plan's operation target names.
    fn plan_server(plan: &meerkat_core::ResolvedToolExecutionPlan) -> Option<String> {
        use meerkat_core::ToolOperationTarget::{CheckedMcpServerOperation, McpServerOperation};
        match plan.single_operation_target() {
            Ok(Some(
                McpServerOperation { server, .. } | CheckedMcpServerOperation { server, .. },
            )) => Some(server.clone()),
            _ => None,
        }
    }

    /// The MCP server an MCP app invocation registers, when `invocation` is
    /// one.
    fn app_server(invocation: &serde_json::Value) -> Option<String> {
        #[derive(serde::Deserialize)]
        struct Registered {
            registration: meerkat_mcp::apps::McpAppRegistration,
        }
        serde_json::from_value::<Registered>(invocation.clone())
            .ok()
            .map(|registered| registered.registration.server)
    }

    fn checked(
        &self,
        name: &str,
        named: Option<String>,
        context: &ToolDispatchContext,
    ) -> Result<(), ToolError> {
        let servers = [named, self.provenance_server(name)];
        self.check(servers.iter().flatten().map(String::as_str), context)
            .map_err(|refusal| {
                tracing::info!(
                    kind = refusal.kind(),
                    server = refusal.server(),
                    tool = name,
                    "a personal account use was refused"
                );
                refusal.to_tool_error()
            })
    }
}

#[async_trait]
impl AgentToolDispatcher for PersonalAccountGate {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        self.inner.tools()
    }

    async fn resolve_tool_application(
        &self,
        source_tool: &str,
        request: &meerkat_core::ToolApplicationRequest,
        invocation: &serde_json::Value,
        context: &ToolDispatchContext,
    ) -> Result<meerkat_core::tool_application::ToolApplicationResolution, ToolError> {
        self.checked(source_tool, Self::app_server(invocation), context)?;
        self.inner
            .resolve_tool_application(source_tool, request, invocation, context)
            .await
    }

    fn tool_catalog_capabilities(&self) -> ToolCatalogCapabilities {
        self.inner.tool_catalog_capabilities()
    }

    fn tool_catalog(&self) -> Arc<[ToolCatalogEntry]> {
        self.inner.tool_catalog()
    }

    fn tool_mutation_class(&self, tool_name: &str) -> meerkat_core::ToolMutationClass {
        self.inner.tool_mutation_class(tool_name)
    }

    fn live_bridge_effect_kind(&self, tool_name: &str) -> meerkat_core::LiveBridgeEffectKind {
        self.inner.live_bridge_effect_kind(tool_name)
    }

    fn review_entry_support(
        &self,
        tool_name: &str,
    ) -> meerkat_core::approval::review::ReviewEntrySupport {
        self.inner.review_entry_support(tool_name)
    }

    fn execution_binding_epoch(&self, tool_name: &str) -> u64 {
        self.inner.execution_binding_epoch(tool_name)
    }

    fn execution_binding_fingerprint(
        &self,
        tool_name: &str,
    ) -> Result<
        meerkat_core::EphemeralToolBindingFingerprint,
        meerkat_core::ToolExecutionResolutionError,
    > {
        self.inner.execution_binding_fingerprint(tool_name)
    }

    fn resolve_execution_plan(
        &self,
        call: ToolCallView<'_>,
        dispatch_context: &ToolDispatchContext,
        resolution_context: &meerkat_core::ToolExecutionResolutionContext,
    ) -> Result<meerkat_core::ResolvedToolExecutionPlan, meerkat_core::ToolExecutionResolutionError>
    {
        self.inner
            .resolve_execution_plan(call, dispatch_context, resolution_context)
    }

    fn validate_resolved_execution_plan(
        &self,
        call: ToolCallView<'_>,
        resolution_context: &meerkat_core::ToolExecutionResolutionContext,
        plan: &meerkat_core::ResolvedToolExecutionPlan,
    ) -> Result<(), meerkat_core::ToolExecutionResolutionError> {
        self.inner
            .validate_resolved_execution_plan(call, resolution_context, plan)
    }

    fn pending_catalog_sources(&self) -> Arc<[String]> {
        self.inner.pending_catalog_sources()
    }

    fn adjudicates_tool_grants(&self) -> bool {
        self.inner.adjudicates_tool_grants()
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
        self.checked(call.name, None, context)?;
        self.inner.dispatch_with_context(call, context).await
    }

    async fn dispatch_resolved_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        plan: &meerkat_core::ResolvedToolExecutionPlan,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        self.checked(call.name, Self::plan_server(plan), context)?;
        self.inner
            .dispatch_resolved_with_context(call, context, plan)
            .await
    }

    async fn poll_external_updates(&self) -> ExternalToolUpdate {
        self.inner.poll_external_updates().await
    }

    fn external_tool_surface_snapshot(&self) -> Option<meerkat_core::ExternalToolSurfaceSnapshot> {
        self.inner.external_tool_surface_snapshot()
    }

    fn capabilities(&self) -> DispatcherCapabilities {
        self.inner.capabilities()
    }

    fn bind_ops_lifecycle(
        self: Arc<Self>,
        registry: Arc<dyn meerkat_core::ops_lifecycle::OpsLifecycleRegistry>,
        owner_bridge_session_id: SessionId,
    ) -> Result<BindOutcome, OpsLifecycleBindError> {
        // Rebind the inner dispatcher and keep the check around whatever it
        // returns: binding never strips the owner check.
        let owned = Arc::try_unwrap(self).map_err(|_| OpsLifecycleBindError::SharedOwnership)?;
        let Self {
            inner,
            accounts,
            owner,
        } = owned;
        let rewrap = |inner: Arc<dyn AgentToolDispatcher>| -> Arc<dyn AgentToolDispatcher> {
            Arc::new(Self {
                inner,
                accounts: Arc::clone(&accounts),
                owner: owner.clone(),
            })
        };
        if Arc::strong_count(&inner) == 1 {
            let outcome = inner.bind_ops_lifecycle(registry, owner_bridge_session_id)?;
            let bound = outcome.was_bound();
            let dispatcher = rewrap(outcome.into_dispatcher());
            Ok(if bound {
                BindOutcome::Bound(dispatcher)
            } else {
                BindOutcome::Skipped(dispatcher)
            })
        } else {
            Ok(BindOutcome::Skipped(rewrap(inner)))
        }
    }

    fn completion_enrichment(
        &self,
    ) -> Option<Arc<dyn meerkat_core::completion_feed::CompletionEnrichmentProvider>> {
        self.inner.completion_enrichment()
    }

    fn bind_mcp_server_lifecycle_handle(
        &self,
        handle: Arc<dyn meerkat_core::handles::McpServerLifecycleHandle>,
    ) {
        self.inner.bind_mcp_server_lifecycle_handle(handle);
    }

    fn bind_external_tool_surface_handle(
        &self,
        handle: Arc<dyn meerkat_core::handles::ExternalToolSurfaceHandle>,
    ) {
        self.inner.bind_external_tool_surface_handle(handle);
    }

    fn bind_session_mob_tool_authority(
        &self,
        authority: Option<Arc<std::sync::RwLock<meerkat_core::service::MobToolAuthorityContext>>>,
    ) {
        self.inner.bind_session_mob_tool_authority(authority);
    }

    fn observes_bindings_only(&self) -> bool {
        self.inner.observes_bindings_only()
    }
}
