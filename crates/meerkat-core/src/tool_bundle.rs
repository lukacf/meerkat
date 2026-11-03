//! The registration owner of a host's Rust tool bundle.
//!
//! A bundle registered by name (`tools.rust_bundles`) is wrapped once by its
//! registration. It always attaches its registration owner witness to every
//! plan it resolves and revalidates that witness at dispatch; the witness
//! carries the typed [`ToolOperationTarget::RegisteredToolBundleOperation`]
//! only when the bundle's own plan names no operation target. An
//! operation policy owner maps the typed `(bundle, operation)` fact to its
//! existing actions; authority is never derived from a tool name, and the
//! bundle gains no permission class of its own.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::agent::{
    AgentToolDispatcher, BindOutcome, DispatcherCapabilities, ExternalToolUpdate,
    OpsLifecycleBindError, ToolDispatchContext,
};
use crate::error::ToolError;
use crate::ops::ToolDispatchOutcome;
use crate::types::{ToolCallView, ToolDef};
use crate::{ToolCatalogCapabilities, ToolCatalogEntry, ToolOperationTarget};

static NEXT_BUNDLE_EXECUTION_AUTHORITY_ID: AtomicU64 = AtomicU64::new(1);

/// A registered Rust tool bundle's dispatcher, wrapped by its registration.
///
/// Every plan it resolves carries this instance's registration witness
/// (always), with the bundle operation target only when the inner target is
/// absent; a resolved plan dispatches only when this same instance resolved
/// it for exactly that operation and its binding is still current.
/// Everything else forwards to the bundle unchanged.
pub struct RegisteredToolBundleDispatcher {
    bundle: String,
    inner: Arc<dyn AgentToolDispatcher>,
    execution_authority_id: u64,
    account: Arc<crate::OperationAccount>,
}

impl RegisteredToolBundleDispatcher {
    /// Wrap `inner` as the bundle registered under `bundle`. Its operations'
    /// account is unavailable until the registration declares one
    /// ([`Self::with_account`]).
    pub fn new(bundle: impl Into<String>, inner: Arc<dyn AgentToolDispatcher>) -> Self {
        Self {
            bundle: bundle.into(),
            inner,
            execution_authority_id: NEXT_BUNDLE_EXECUTION_AUTHORITY_ID
                .fetch_add(1, Ordering::Relaxed),
            account: Arc::new(crate::OperationAccount::Unavailable),
        }
    }

    /// The registering host's declaration of the account every operation of
    /// this bundle acts with: an account it selected, or that they act with
    /// none. It is stated on each operation target this registration
    /// produces, never inferred from the bundle's name, tools or
    /// credentials.
    #[must_use]
    pub fn with_account(mut self, account: crate::OperationAccount) -> Self {
        self.account = Arc::new(account);
        self
    }

    /// The registration name this wrapper attributes operations to.
    pub fn bundle(&self) -> &str {
        &self.bundle
    }

    /// The typed target this registration produces for `operation`.
    pub fn operation_target(&self, operation: &str) -> ToolOperationTarget {
        ToolOperationTarget::RegisteredToolBundleOperation {
            bundle: self.bundle.clone(),
            operation: operation.to_string(),
            account: Arc::clone(&self.account),
        }
    }

    fn execution_authority_key(&self) -> String {
        format!("tool-bundle:{}", self.execution_authority_id)
    }

    /// The plan was resolved by this instance for exactly this operation,
    /// and its binding is still current.
    fn owner_witness_matches(
        &self,
        call: ToolCallView<'_>,
        plan: &crate::ResolvedToolExecutionPlan,
    ) -> bool {
        let Ok(live) = self.execution_binding_fingerprint(call.name) else {
            return false;
        };
        let key = self.execution_authority_key();
        let expected = self.attached_target(plan, &key, call.name);
        plan.owner_witness(&key).is_some_and(|witness| {
            witness.owner_key() == call.name
                && witness.binding_fingerprint() == &live
                && witness.operation_target() == expected.as_ref()
        })
    }

    /// The bundle target this registration attaches to `plan`: none when the
    /// bundle's own plan already names an operation target (an MCP or
    /// catalog owner inside the bundle), which is never erased or made
    /// ambiguous.
    fn attached_target(
        &self,
        plan: &crate::ResolvedToolExecutionPlan,
        own_key: &str,
        operation: &str,
    ) -> Option<ToolOperationTarget> {
        let inner_target = plan.owner_witnesses().iter().any(|witness| {
            witness.authority_key() != own_key && witness.operation_target().is_some()
        });
        (!inner_target).then(|| self.operation_target(operation))
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
impl AgentToolDispatcher for RegisteredToolBundleDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        self.inner.tools()
    }

    fn tool_catalog_capabilities(&self) -> ToolCatalogCapabilities {
        self.inner.tool_catalog_capabilities()
    }

    fn tool_catalog(&self) -> Arc<[ToolCatalogEntry]> {
        self.inner.tool_catalog()
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

    /// The bundle's own live binding, attributed to this registration.
    fn execution_binding_fingerprint(
        &self,
        tool_name: &str,
    ) -> Result<crate::EphemeralToolBindingFingerprint, crate::ToolExecutionResolutionError> {
        Ok(self
            .inner
            .execution_binding_fingerprint(tool_name)?
            .with_live_authority(self.execution_authority_id as usize, 0))
    }

    fn resolve_execution_plan(
        &self,
        call: ToolCallView<'_>,
        dispatch_context: &ToolDispatchContext,
        resolution_context: &crate::ToolExecutionResolutionContext,
    ) -> Result<crate::ResolvedToolExecutionPlan, crate::ToolExecutionResolutionError> {
        let before = self.execution_binding_fingerprint(call.name)?;
        let plan = self
            .inner
            .resolve_execution_plan(call, dispatch_context, resolution_context)?;
        if self.execution_binding_fingerprint(call.name)? != before {
            return Err(crate::ToolExecutionResolutionError::Unavailable {
                tool_name: call.name.to_string(),
                reason: crate::ToolUnavailableReason::ExecutionOwnerChanged,
            });
        }
        // The typed bundle fact, produced and revalidated only by this
        // registration: operation policy maps it, never the tool name.
        let key = self.execution_authority_key();
        let mut witness = crate::ToolExecutionOwnerWitness::new(key.clone(), call.name, before)?;
        if let Some(target) = self.attached_target(&plan, &key, call.name) {
            witness = witness.with_operation_target(target);
        }
        plan.with_owner_witness(witness)
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

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        self.inner.dispatch(call).await
    }

    async fn dispatch_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        self.inner.dispatch_with_context(call, context).await
    }

    /// A resolved plan runs only when this registration resolved it for
    /// exactly this operation and the bundle's binding is unchanged.
    async fn dispatch_resolved_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        plan: &crate::ResolvedToolExecutionPlan,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        if !self.owner_witness_matches(call, plan) {
            return Err(ToolError::unavailable(
                call.name,
                crate::ToolUnavailableReason::ExecutionOwnerChanged,
            ));
        }
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
        // Consume the registration and rebind its own inner, then keep the
        // same registration around the returned dispatcher: binding never
        // strips the bundle's owner witness.
        let owned = Arc::try_unwrap(self).map_err(|_| OpsLifecycleBindError::SharedOwnership)?;
        if owned.inner.capabilities().ops_lifecycle && Arc::strong_count(&owned.inner) != 1 {
            return Err(OpsLifecycleBindError::SharedOwnership);
        }
        let Self {
            bundle,
            inner,
            execution_authority_id,
            account,
        } = owned;
        let rewrap = |inner: Arc<dyn AgentToolDispatcher>| -> Arc<dyn AgentToolDispatcher> {
            Arc::new(Self {
                bundle: bundle.clone(),
                inner,
                execution_authority_id,
                account: Arc::clone(&account),
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
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::types::ToolResult;

    struct Bundle(Arc<[Arc<ToolDef>]>, std::sync::atomic::AtomicU64);

    #[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
    impl AgentToolDispatcher for Bundle {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            Arc::clone(&self.0)
        }

        fn execution_binding_epoch(&self, _tool_name: &str) -> u64 {
            self.1.load(Ordering::SeqCst)
        }

        fn capabilities(&self) -> DispatcherCapabilities {
            DispatcherCapabilities {
                ops_lifecycle: true,
            }
        }

        /// The ordinary exclusive-inner contract: bind only when uniquely
        /// owned, returning the rebound dispatcher.
        fn bind_ops_lifecycle(
            self: Arc<Self>,
            _registry: Arc<dyn crate::ops_lifecycle::OpsLifecycleRegistry>,
            _owner: crate::types::SessionId,
        ) -> Result<BindOutcome, OpsLifecycleBindError> {
            let owned =
                Arc::try_unwrap(self).map_err(|_| OpsLifecycleBindError::SharedOwnership)?;
            Ok(BindOutcome::Bound(Arc::new(Bundle(
                owned.0,
                std::sync::atomic::AtomicU64::new(owned.1.load(Ordering::SeqCst)),
            ))))
        }

        async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
            Ok(ToolDispatchOutcome::sync_result(ToolResult::new(
                call.id.to_string(),
                call.name.to_string(),
                false,
            )))
        }
    }

    fn tool(name: &str) -> Arc<ToolDef> {
        Arc::new(ToolDef {
            name: name.into(),
            description: String::new(),
            input_schema: serde_json::json!({"type": "object"}),
            provenance: None,
            audience: Default::default(),
        })
    }

    fn call<'a>(name: &'a str, args: &'a serde_json::value::RawValue) -> ToolCallView<'a> {
        ToolCallView {
            id: "call-1",
            name,
            args,
        }
    }

    #[tokio::test]
    async fn bundle_plans_carry_and_require_their_registration_operation() {
        let bundle = Arc::new(Bundle(
            vec![tool("read_mail"), tool("send_draft")].into(),
            std::sync::atomic::AtomicU64::new(0),
        ));
        let inner: Arc<dyn AgentToolDispatcher> = bundle.clone();
        let registered = RegisteredToolBundleDispatcher::new("mail", Arc::clone(&inner));
        let other = RegisteredToolBundleDispatcher::new("mail", Arc::clone(&inner));
        let context = ToolDispatchContext::default();
        let resolution = crate::ToolExecutionResolutionContext::new(
            crate::ToolDeadlineChain::new(vec![crate::ToolDeadlineContributor::finite(
                crate::ToolDeadlineOwner::CoreToolDispatch,
                std::time::Duration::from_mins(10),
            )])
            .expect("deadline chain"),
        );
        let args = serde_json::value::RawValue::from_string("{}".to_string()).unwrap();
        let read = call("read_mail", &args);
        let send = call("send_draft", &args);

        let plan = registered
            .resolve_execution_plan(read, &context, &resolution)
            .expect("bundle plan");
        let targets: Vec<_> = plan
            .owner_witnesses()
            .iter()
            .filter_map(|witness| witness.operation_target())
            .collect();
        assert_eq!(
            targets,
            vec![&ToolOperationTarget::RegisteredToolBundleOperation {
                bundle: "mail".into(),
                operation: "read_mail".into(),
                account: Arc::new(crate::OperationAccount::Unavailable),
            }]
        );
        assert!(
            registered
                .dispatch_resolved_with_context(read, &context, &plan)
                .await
                .is_ok()
        );

        let owner_changed = |result: Result<ToolDispatchOutcome, ToolError>, name: &str| {
            assert_eq!(
                result.expect_err(name),
                ToolError::unavailable(name, crate::ToolUnavailableReason::ExecutionOwnerChanged)
            );
        };
        // Another registration instance never runs this one's plan.
        owner_changed(
            other
                .dispatch_resolved_with_context(read, &context, &plan)
                .await,
            "read_mail",
        );
        // An intentional replacement under the same stable name (`other`)
        // resolves a fresh plan with the identical typed target, and that
        // fresh plan runs on it, never on the replaced registration: the
        // target names only the stable bundle and operation, and the live
        // per-instance witness decides where a plan runs.
        let fresh = other
            .resolve_execution_plan(read, &context, &resolution)
            .expect("fresh plan");
        assert!(
            fresh
                .owner_witnesses()
                .iter()
                .filter_map(|witness| witness.operation_target())
                .eq(targets.iter().copied())
        );
        assert!(
            other
                .dispatch_resolved_with_context(read, &context, &fresh)
                .await
                .is_ok()
        );
        owner_changed(
            registered
                .dispatch_resolved_with_context(read, &context, &fresh)
                .await,
            "read_mail",
        );
        // A plan for one operation never runs another.
        owner_changed(
            registered
                .dispatch_resolved_with_context(send, &context, &plan)
                .await,
            "send_draft",
        );
        // The bundle's own plan, without the registration's witness, is refused.
        let bare = inner
            .resolve_execution_plan(read, &context, &resolution)
            .expect("bare plan");
        owner_changed(
            registered
                .dispatch_resolved_with_context(read, &context, &bare)
                .await,
            "read_mail",
        );
        // A changed inner binding refuses a plan resolved before it.
        bundle.1.store(1, Ordering::SeqCst);
        owner_changed(
            registered
                .dispatch_resolved_with_context(read, &context, &plan)
                .await,
            "read_mail",
        );

        // Through the real composite and root fence the plan works.
        let root: Arc<dyn AgentToolDispatcher> =
            Arc::new(crate::DynamicToolComposite::new(vec![Arc::new(
                RegisteredToolBundleDispatcher::new("mail", inner),
            )
                as Arc<dyn AgentToolDispatcher>]));
        let routed = crate::resolve_tool_execution_plan_fenced(&root, read, &context, &resolution)
            .expect("root plan");
        assert!(
            routed
                .owner_witnesses()
                .iter()
                .filter_map(|witness| witness.operation_target())
                .eq([&ToolOperationTarget::RegisteredToolBundleOperation {
                    bundle: "mail".into(),
                    operation: "read_mail".into(),
                    account: Arc::new(crate::OperationAccount::Unavailable),
                }])
        );
        assert!(
            crate::dispatch_tool_execution_plan_fenced(&root, read, &context, &routed)
                .await
                .is_ok()
        );
    }

    /// The registration's declared account is stated on every operation
    /// target it produces; a registration that declared none states it
    /// unavailable, never "no account".
    #[tokio::test]
    async fn the_registrations_declared_account_rides_on_its_targets() {
        let inner: Arc<dyn AgentToolDispatcher> = Arc::new(Bundle(
            vec![tool("read_mail")].into(),
            std::sync::atomic::AtomicU64::new(0),
        ));
        let account = crate::OperationAccount::selected("https://mail.example", "alice");
        let declared = RegisteredToolBundleDispatcher::new("mail", Arc::clone(&inner))
            .with_account(account.clone());
        let none = RegisteredToolBundleDispatcher::new("mail", Arc::clone(&inner))
            .with_account(crate::OperationAccount::DeclaredNone);
        let undeclared = RegisteredToolBundleDispatcher::new("mail", inner);
        let context = ToolDispatchContext::default();
        let resolution = crate::ToolExecutionResolutionContext::new(
            crate::ToolDeadlineChain::new(vec![crate::ToolDeadlineContributor::finite(
                crate::ToolDeadlineOwner::CoreToolDispatch,
                std::time::Duration::from_mins(10),
            )])
            .expect("deadline chain"),
        );
        let args = serde_json::value::RawValue::from_string("{}".to_string()).unwrap();
        let read = call("read_mail", &args);
        for (registration, expected) in [
            (&declared, account),
            (&none, crate::OperationAccount::DeclaredNone),
            (&undeclared, crate::OperationAccount::Unavailable),
        ] {
            let plan = registration
                .resolve_execution_plan(read, &context, &resolution)
                .expect("plan");
            let accounts: Vec<_> = plan
                .owner_witnesses()
                .iter()
                .filter_map(|witness| witness.operation_target())
                .map(|target| target.account().cloned())
                .collect();
            assert_eq!(accounts, vec![Some(expected)]);
            assert!(
                registration
                    .dispatch_resolved_with_context(read, &context, &plan)
                    .await
                    .is_ok()
            );
        }
    }

    #[tokio::test]
    async fn lifecycle_binding_keeps_the_registration() {
        let registered: Arc<dyn AgentToolDispatcher> = Arc::new(
            RegisteredToolBundleDispatcher::new(
                "mail",
                Arc::new(Bundle(
                    vec![tool("read_mail")].into(),
                    std::sync::atomic::AtomicU64::new(0),
                )),
            )
            .with_account(crate::OperationAccount::DeclaredNone),
        );
        let outcome = registered
            .bind_ops_lifecycle(
                Arc::new(crate::tool_execution_policy::tests::UnsupportedOpsRegistry),
                crate::types::SessionId::new(),
            )
            .expect("a uniquely owned registration binds its exclusive inner");
        assert!(outcome.was_bound());
        let bound = outcome.into_dispatcher();
        let context = ToolDispatchContext::default();
        let resolution = crate::ToolExecutionResolutionContext::new(
            crate::ToolDeadlineChain::new(vec![crate::ToolDeadlineContributor::finite(
                crate::ToolDeadlineOwner::CoreToolDispatch,
                std::time::Duration::from_mins(10),
            )])
            .expect("deadline chain"),
        );
        let args = serde_json::value::RawValue::from_string("{}".to_string()).unwrap();
        let read = call("read_mail", &args);
        let plan = bound
            .resolve_execution_plan(read, &context, &resolution)
            .expect("the bound registration still resolves");
        assert!(
            plan.owner_witnesses()
                .iter()
                .filter_map(|witness| witness.operation_target())
                .eq([&ToolOperationTarget::RegisteredToolBundleOperation {
                    bundle: "mail".into(),
                    operation: "read_mail".into(),
                    account: Arc::new(crate::OperationAccount::DeclaredNone),
                }]),
            "binding never strips the registration's target or its declared account"
        );
        assert!(
            bound
                .dispatch_resolved_with_context(read, &context, &plan)
                .await
                .is_ok()
        );

        // A shared registration is not rebound.
        let shared: Arc<dyn AgentToolDispatcher> = Arc::new(RegisteredToolBundleDispatcher::new(
            "mail",
            Arc::new(Bundle(
                vec![tool("read_mail")].into(),
                std::sync::atomic::AtomicU64::new(0),
            )),
        ));
        let _other_owner = Arc::clone(&shared);
        assert!(matches!(
            shared.bind_ops_lifecycle(
                Arc::new(crate::tool_execution_policy::tests::UnsupportedOpsRegistry),
                crate::types::SessionId::new(),
            ),
            Err(OpsLifecycleBindError::SharedOwnership)
        ));
    }

    /// A bundle whose own owner names an operation target (here an MCP
    /// server operation).
    struct TargetedBundle(Bundle);

    #[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
    impl AgentToolDispatcher for TargetedBundle {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            self.0.tools()
        }

        fn resolve_execution_plan(
            &self,
            call: ToolCallView<'_>,
            dispatch_context: &ToolDispatchContext,
            resolution_context: &crate::ToolExecutionResolutionContext,
        ) -> Result<crate::ResolvedToolExecutionPlan, crate::ToolExecutionResolutionError> {
            let plan = self
                .0
                .resolve_execution_plan(call, dispatch_context, resolution_context)?;
            plan.with_owner_witness(
                crate::ToolExecutionOwnerWitness::new(
                    "inner-owner",
                    call.name,
                    self.execution_binding_fingerprint(call.name)?,
                )?
                .with_operation_target(ToolOperationTarget::McpServerOperation {
                    server: "alpha".into(),
                    connection_generation: 1,
                    raw_operation: call.name.into(),
                    connector: crate::McpConnectorApproval::NoConnector,
                    account: Arc::new(crate::OperationAccount::DeclaredNone),
                }),
            )
        }

        async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
            self.0.dispatch(call).await
        }
    }

    #[tokio::test]
    async fn an_inner_operation_target_is_preserved_not_made_ambiguous() {
        let registered = RegisteredToolBundleDispatcher::new(
            "mail",
            Arc::new(TargetedBundle(Bundle(
                vec![tool("read_mail")].into(),
                std::sync::atomic::AtomicU64::new(0),
            ))),
        );
        let context = ToolDispatchContext::default();
        let resolution = crate::ToolExecutionResolutionContext::new(
            crate::ToolDeadlineChain::new(vec![crate::ToolDeadlineContributor::finite(
                crate::ToolDeadlineOwner::CoreToolDispatch,
                std::time::Duration::from_mins(10),
            )])
            .expect("deadline chain"),
        );
        let args = serde_json::value::RawValue::from_string("{}".to_string()).unwrap();
        let read = call("read_mail", &args);
        let plan = registered
            .resolve_execution_plan(read, &context, &resolution)
            .expect("plan");
        assert!(
            plan.owner_witnesses()
                .iter()
                .filter_map(|witness| witness.operation_target())
                .eq([&ToolOperationTarget::McpServerOperation {
                    server: "alpha".into(),
                    connection_generation: 1,
                    raw_operation: "read_mail".into(),
                    connector: crate::McpConnectorApproval::NoConnector,
                    account: Arc::new(crate::OperationAccount::DeclaredNone),
                }]),
            "the inner target stays the plan's only target"
        );
        assert_eq!(
            plan.owner_witnesses().len(),
            2,
            "the registration witness is attached all the same"
        );
        assert!(
            registered
                .dispatch_resolved_with_context(read, &context, &plan)
                .await
                .is_ok()
        );
    }
}
