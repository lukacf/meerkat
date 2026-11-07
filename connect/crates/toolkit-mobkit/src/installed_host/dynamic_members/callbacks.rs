//! A native bundle registration for exactly the validated class callbacks.
//! Other overlay tools keep their own dispatcher and targets unchanged.
use super::*;
use meerkat_core::agent::{
    BindOutcome, DispatcherCapabilities, ExternalToolUpdate, OpsLifecycleBindError,
    ToolDispatchContext,
};
use meerkat_core::types::ToolCallView;
use meerkat_core::{AgentToolDispatcher, RegisteredToolBundleDispatcher};
use std::sync::Arc;

pub(crate) fn bundle(
    realm: &str,
    owner: &str,
    admission: &MemberClassAdmission,
) -> Result<String, HostLifetimeError> {
    use sha2::{Digest, Sha256};
    let bytes = toolkit_package::canonical_json(&(
        "toolkit.class-callback.v1",
        realm,
        owner,
        &admission.id,
        &admission.class,
        &admission.instance,
    ))
    .map_err(|_| HostLifetimeError::InvalidSelection)?;
    Ok(format!("toolkit.class/{:x}", Sha256::digest(bytes)))
}

pub(super) struct ClassCallbackDispatcher {
    inner: Arc<dyn AgentToolDispatcher>,
    registered: Arc<dyn AgentToolDispatcher>,
    names: Vec<String>,
    bundle: String,
    account: meerkat_core::OperationAccount,
}
impl ClassCallbackDispatcher {
    pub(super) fn new(
        inner: Arc<dyn AgentToolDispatcher>,
        bundle: String,
        class: &CompiledMemberClass,
    ) -> Result<Self, HostLifetimeError> {
        let account = class
            .callback_account
            .as_ref()
            .map(|a| a.native())
            .transpose()?
            .unwrap_or(meerkat_core::OperationAccount::Unavailable);
        Ok(Self::wrap(
            inner,
            bundle,
            class.callbacks.iter().map(|c| c.name.clone()).collect(),
            account,
        ))
    }
    fn wrap(
        inner: Arc<dyn AgentToolDispatcher>,
        bundle: String,
        names: Vec<String>,
        account: meerkat_core::OperationAccount,
    ) -> Self {
        let registered = Arc::new(
            RegisteredToolBundleDispatcher::new(bundle.clone(), inner.clone())
                .with_account(account.clone()),
        );
        Self {
            inner,
            registered,
            names,
            bundle,
            account,
        }
    }
    fn selected(&self, name: &str) -> &dyn AgentToolDispatcher {
        if self.names.iter().any(|n| n == name) {
            self.registered.as_ref()
        } else {
            self.inner.as_ref()
        }
    }
}
#[async_trait::async_trait]
impl AgentToolDispatcher for ClassCallbackDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        self.inner.tools()
    }
    fn tool_catalog_capabilities(&self) -> meerkat_core::ToolCatalogCapabilities {
        self.inner.tool_catalog_capabilities()
    }
    fn tool_catalog(&self) -> Arc<[meerkat_core::ToolCatalogEntry]> {
        self.inner.tool_catalog()
    }
    fn tool_mutation_class(&self, name: &str) -> meerkat_core::ToolMutationClass {
        self.inner.tool_mutation_class(name)
    }
    fn live_bridge_effect_kind(&self, name: &str) -> meerkat_core::LiveBridgeEffectKind {
        self.inner.live_bridge_effect_kind(name)
    }
    fn review_entry_support(
        &self,
        name: &str,
    ) -> meerkat_core::approval::review::ReviewEntrySupport {
        self.inner.review_entry_support(name)
    }
    fn execution_binding_epoch(&self, name: &str) -> u64 {
        self.selected(name).execution_binding_epoch(name)
    }
    fn execution_binding_fingerprint(
        &self,
        name: &str,
    ) -> Result<
        meerkat_core::EphemeralToolBindingFingerprint,
        meerkat_core::ToolExecutionResolutionError,
    > {
        self.selected(name).execution_binding_fingerprint(name)
    }
    fn resolve_execution_plan(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        resolution: &meerkat_core::ToolExecutionResolutionContext,
    ) -> Result<meerkat_core::ResolvedToolExecutionPlan, meerkat_core::ToolExecutionResolutionError>
    {
        self.selected(call.name)
            .resolve_execution_plan(call, context, resolution)
    }
    fn validate_resolved_execution_plan(
        &self,
        call: ToolCallView<'_>,
        resolution: &meerkat_core::ToolExecutionResolutionContext,
        plan: &meerkat_core::ResolvedToolExecutionPlan,
    ) -> Result<(), meerkat_core::ToolExecutionResolutionError> {
        self.selected(call.name)
            .validate_resolved_execution_plan(call, resolution, plan)
    }
    fn pending_catalog_sources(&self) -> Arc<[String]> {
        self.inner.pending_catalog_sources()
    }
    async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, meerkat_core::ToolError> {
        self.selected(call.name).dispatch(call).await
    }
    async fn dispatch_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, meerkat_core::ToolError> {
        self.selected(call.name)
            .dispatch_with_context(call, context)
            .await
    }
    async fn dispatch_resolved_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        plan: &meerkat_core::ResolvedToolExecutionPlan,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, meerkat_core::ToolError> {
        self.selected(call.name)
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
        session: meerkat_core::SessionId,
    ) -> Result<BindOutcome, OpsLifecycleBindError> {
        // The real gateway callback dispatcher has no lifecycle binding.
        // Preserve this registration when there is nothing to bind.
        if !self.inner.capabilities().ops_lifecycle {
            return Ok(BindOutcome::Skipped(self));
        }
        let owned = Arc::try_unwrap(self).map_err(|_| OpsLifecycleBindError::SharedOwnership)?;
        let Self {
            inner,
            registered,
            names,
            bundle,
            account,
        } = owned;
        drop(registered);
        if inner.capabilities().ops_lifecycle && Arc::strong_count(&inner) != 1 {
            return Err(OpsLifecycleBindError::SharedOwnership);
        }
        if Arc::strong_count(&inner) == 1 {
            let outcome = inner.bind_ops_lifecycle(registry, session)?;
            let bound = outcome.was_bound();
            // A changed inner is a new selective registration. Native owns
            // its new witness and refuses any plan from before the rebind.
            let wrapped = Arc::new(Self::wrap(
                outcome.into_dispatcher(),
                bundle,
                names,
                account,
            ));
            Ok(if bound {
                BindOutcome::Bound(wrapped)
            } else {
                BindOutcome::Skipped(wrapped)
            })
        } else {
            Ok(BindOutcome::Skipped(Arc::new(Self::wrap(
                inner, bundle, names, account,
            ))))
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
}

#[cfg(test)]
mod tests;
