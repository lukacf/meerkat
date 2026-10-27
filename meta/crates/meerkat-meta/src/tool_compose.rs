//! Composition for external-tool dispatchers.
//!
//! `build.external_tools` is a single slot, and assigning it wholesale is a
//! recurring defect class: a later installer silently discards whatever an
//! earlier one put there (the agent-memory recorder's `memory` tool has been
//! clobbered this way twice — by an example SessionHook, fixed in f8d11e57,
//! and by the rpc_gateway callback build path, a downstream app "Bug D"). Installers
//! MUST compose over the existing slot value instead of assigning.
//!
//! [`ComposedExternalTools`] is the canonical way to do that: the `primary`
//! dispatcher (the tools being installed) wins name collisions; anything it
//! does not advertise falls through to the `fallback` (whatever was already
//! in the slot). BOTH dispatch entry points forward — the
//! `ToolDispatchContext` carries per-turn authority witnesses (workgraph
//! attention projections), and relying on the trait's default
//! `dispatch_with_context` silently drops it, which was a verified
//! attention-scope bypass in an earlier wrapper.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use meerkat_core::agent::OpsLifecycleBindError;
use meerkat_core::types::{ToolCallView, ToolDef};
use meerkat_core::{
    AgentToolDispatcher, BindOutcome, DispatcherCapabilities, ToolCatalogCapabilities,
    ToolCatalogEntry, ToolDispatchOutcome, ToolError,
};

/// Two external-tool dispatchers behind one slot: `primary` wins name
/// collisions, unknown calls fall through to `fallback`.
pub struct ComposedExternalTools {
    primary: Arc<dyn AgentToolDispatcher>,
    fallback: Arc<dyn AgentToolDispatcher>,
    execution_authority_id: usize,
}

static NEXT_EXECUTION_AUTHORITY_ID: AtomicUsize = AtomicUsize::new(1);

impl ComposedExternalTools {
    /// Compose `primary` over an optional pre-existing dispatcher. With no
    /// fallback this is the identity — callers can use it unconditionally on
    /// the slot they are about to fill.
    ///
    /// Name collisions are legal (primary wins) but never silent: a
    /// pre-installed tool that stops being dispatchable is the same defect
    /// class as the clobber this type exists to prevent, just scoped to one
    /// name — and it is exactly how a host tool named `memory` disables the
    /// agent-memory recorder while every layer still reports the tool
    /// present.
    pub fn over(
        primary: Arc<dyn AgentToolDispatcher>,
        fallback: Option<Arc<dyn AgentToolDispatcher>>,
    ) -> Arc<dyn AgentToolDispatcher> {
        match fallback {
            None => primary,
            Some(fallback) => {
                let composed = Self {
                    primary,
                    fallback,
                    execution_authority_id: NEXT_EXECUTION_AUTHORITY_ID
                        .fetch_add(1, Ordering::Relaxed),
                };
                let shadowed = composed.shadowed_fallback_names();
                if !shadowed.is_empty() {
                    tracing::warn!(
                        shadowed = %shadowed.join(", "),
                        "external-tool composition shadows pre-installed tools: the \
                         installing dispatcher wins these names and the pre-installed \
                         implementations become unreachable. If 'memory' is listed, the \
                         agent-memory recorder is disabled for this member — rename the \
                         host tool or disable agent_memory.recorder_tool"
                    );
                }
                Arc::new(composed)
            }
        }
    }

    /// Fallback tool names the primary also advertises — present in the
    /// merged catalog, but their pre-installed implementations are
    /// unreachable through this composition.
    fn shadowed_fallback_names(&self) -> Vec<String> {
        let primary = self.primary.tools();
        self.fallback
            .tools()
            .iter()
            .filter(|tool| primary.iter().any(|existing| existing.name == tool.name))
            .map(|tool| tool.name.to_string())
            .collect()
    }

    fn primary_advertises(&self, name: &str) -> bool {
        self.primary
            .tools()
            .iter()
            .any(|tool| tool.name.as_ref() == name)
    }

    fn owner(&self, name: &str) -> (&'static str, &Arc<dyn AgentToolDispatcher>) {
        if self.primary_advertises(name) {
            ("primary", &self.primary)
        } else {
            ("fallback", &self.fallback)
        }
    }

    fn execution_authority_key(&self) -> String {
        format!(
            "meta:composed-external-tools:{}",
            self.execution_authority_id
        )
    }
}

#[async_trait::async_trait]
impl AgentToolDispatcher for ComposedExternalTools {
    async fn resolve_tool_application(
        &self,
        source_tool: &str,
        request: &meerkat_core::ToolApplicationRequest,
        invocation: &serde_json::Value,
        context: &meerkat_core::ToolDispatchContext,
    ) -> Result<meerkat_core::tool_application::ToolApplicationResolution, ToolError> {
        let (route, owner) = self.owner(source_tool);
        let changed = || {
            ToolError::unavailable(
                source_tool,
                meerkat_core::ToolUnavailableReason::ExecutionOwnerChanged,
            )
        };
        let before = owner
            .execution_binding_fingerprint(source_tool)
            .map_err(|_| changed())?;
        let mut result = owner
            .resolve_tool_application(source_tool, request, invocation, context)
            .await?;
        if self.owner(source_tool).0 != route
            || owner.execution_binding_fingerprint(source_tool).as_ref() != Ok(&before)
        {
            return Err(changed());
        }
        // A resolved app action belongs to the same leaf as its source. A
        // colliding host tool must not receive another leaf's native binding.
        if let meerkat_core::tool_application::ToolApplicationResolution::Call {
            name,
            binding,
            ..
        } = &mut result
        {
            let (target_route, target_owner) = self.owner(name);
            if target_route != route {
                return Err(ToolError::access_denied(name.as_str()));
            }
            let witness = meerkat_core::ToolExecutionOwnerWitness::new(
                self.execution_authority_key(),
                target_route,
                target_owner
                    .execution_binding_fingerprint(name)
                    .map_err(ToolError::from)?,
            )
            .map_err(|_| changed())?;
            *binding = binding
                .clone()
                .with_owner_witness(witness)
                .map_err(ToolError::from)?;
        }
        Ok(result)
    }

    fn tool_mutation_class(&self, tool_name: &str) -> meerkat_core::ToolMutationClass {
        self.owner(tool_name).1.tool_mutation_class(tool_name)
    }

    fn live_bridge_effect_kind(&self, tool_name: &str) -> meerkat_core::LiveBridgeEffectKind {
        self.owner(tool_name).1.live_bridge_effect_kind(tool_name)
    }

    fn review_entry_support(
        &self,
        tool_name: &str,
    ) -> meerkat_core::approval::review::ReviewEntrySupport {
        self.owner(tool_name).1.review_entry_support(tool_name)
    }

    fn execution_binding_epoch(&self, tool_name: &str) -> u64 {
        self.owner(tool_name).1.execution_binding_epoch(tool_name)
    }

    fn execution_binding_fingerprint(
        &self,
        tool_name: &str,
    ) -> Result<
        meerkat_core::EphemeralToolBindingFingerprint,
        meerkat_core::ToolExecutionResolutionError,
    > {
        let (route, owner) = self.owner(tool_name);
        let child = owner.execution_binding_fingerprint(tool_name)?;
        Ok(child.with_live_authority(self.execution_authority_id, u64::from(route == "primary")))
    }

    fn resolve_execution_plan(
        &self,
        call: ToolCallView<'_>,
        context: &meerkat_core::ToolDispatchContext,
        resolution_context: &meerkat_core::ToolExecutionResolutionContext,
    ) -> Result<meerkat_core::ResolvedToolExecutionPlan, meerkat_core::ToolExecutionResolutionError>
    {
        let (route, owner) = self.owner(call.name);
        let before = owner.execution_binding_fingerprint(call.name)?;
        let plan = owner.resolve_execution_plan(call, context, resolution_context)?;
        if self.owner(call.name).0 != route
            || owner.execution_binding_fingerprint(call.name).as_ref() != Ok(&before)
        {
            return Err(meerkat_core::ToolExecutionResolutionError::Unavailable {
                tool_name: call.name.to_string(),
                reason: meerkat_core::ToolUnavailableReason::ExecutionOwnerChanged,
            });
        }
        plan.with_owner_witness(meerkat_core::ToolExecutionOwnerWitness::new(
            self.execution_authority_key(),
            route,
            before,
        )?)
    }

    fn validate_resolved_execution_plan(
        &self,
        call: ToolCallView<'_>,
        context: &meerkat_core::ToolExecutionResolutionContext,
        plan: &meerkat_core::ResolvedToolExecutionPlan,
    ) -> Result<(), meerkat_core::ToolExecutionResolutionError> {
        self.owner(call.name)
            .1
            .validate_resolved_execution_plan(call, context, plan)
    }

    async fn dispatch_resolved_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &meerkat_core::ToolDispatchContext,
        plan: &meerkat_core::ResolvedToolExecutionPlan,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        let (route, owner) = self.owner(call.name);
        let changed = || {
            ToolError::unavailable(
                call.name,
                meerkat_core::ToolUnavailableReason::ExecutionOwnerChanged,
            )
        };
        let witness = plan
            .owner_witness(&self.execution_authority_key())
            .ok_or_else(changed)?;
        if witness.owner_key() != route
            || owner.execution_binding_fingerprint(call.name).as_ref()
                != Ok(witness.binding_fingerprint())
        {
            return Err(changed());
        }
        owner
            .dispatch_resolved_with_context(call, context, plan)
            .await
    }

    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        let primary = self.primary.tools();
        let mut merged: Vec<Arc<ToolDef>> = primary.iter().cloned().collect();
        for tool in self.fallback.tools().iter() {
            if !primary.iter().any(|existing| existing.name == tool.name) {
                merged.push(Arc::clone(tool));
            }
        }
        merged.into()
    }

    /// The merged catalog is exact only when both halves are: the composition
    /// adds no names of its own, so exactness is the conjunction. Leaving this
    /// on the trait default made every member whose build composed host tools
    /// over a pre-installed dispatcher read as `NonExactCatalog` downstream.
    fn tool_catalog_capabilities(&self) -> ToolCatalogCapabilities {
        let primary = self.primary.tool_catalog_capabilities();
        let fallback = self.fallback.tool_catalog_capabilities();
        ToolCatalogCapabilities {
            exact_catalog: primary.exact_catalog && fallback.exact_catalog,
            may_require_catalog_control_plane: primary.may_require_catalog_control_plane
                || fallback.may_require_catalog_control_plane,
        }
    }

    /// Primary entries verbatim, then every fallback entry whose name the
    /// primary does not advertise - the same precedence `tools()` applies.
    fn tool_catalog(&self) -> Arc<[ToolCatalogEntry]> {
        let mut merged: Vec<ToolCatalogEntry> = self.primary.tool_catalog().to_vec();
        for entry in self.fallback.tool_catalog().iter() {
            if !self.primary_advertises(&entry.tool.name) {
                merged.push(entry.clone());
            }
        }
        merged.into()
    }

    /// A composition supports ops-lifecycle binding when either half does: it
    /// adds no tools of its own, so hiding a half's capability would leave
    /// that half's ops (detached jobs, async operations) unbound from the
    /// session's registry while every layer still reported it present.
    fn capabilities(&self) -> DispatcherCapabilities {
        let primary = self.primary.capabilities();
        let fallback = self.fallback.capabilities();
        DispatcherCapabilities {
            ops_lifecycle: primary.ops_lifecycle || fallback.ops_lifecycle,
        }
    }

    /// The session's mob operator authority reaches both halves: either may
    /// hold a mob runtime's own operator tools for the member (the existing
    /// overlay a composition falls back to usually does).
    fn bind_session_mob_tool_authority(
        &self,
        authority: Option<Arc<std::sync::RwLock<meerkat_core::service::MobToolAuthorityContext>>>,
    ) {
        self.primary
            .bind_session_mob_tool_authority(authority.clone());
        self.fallback.bind_session_mob_tool_authority(authority);
    }

    /// Rebind each half that supports ops-lifecycle binding, keeping the
    /// composition's precedence. Binding needs exclusive ownership all the way
    /// down: a composition or an ops-capable half still shared elsewhere
    /// cannot be rebound, and leaving it unbound would silently drop its owner
    /// session and registry, so the bind is refused with `SharedOwnership`
    /// (the rule meerkat's own tool gateway applies).
    fn bind_ops_lifecycle(
        self: Arc<Self>,
        registry: Arc<dyn meerkat_core::ops_lifecycle::OpsLifecycleRegistry>,
        owner_bridge_session_id: meerkat_core::types::SessionId,
    ) -> Result<BindOutcome, OpsLifecycleBindError> {
        let owned = Arc::try_unwrap(self).map_err(|_| OpsLifecycleBindError::SharedOwnership)?;
        let mut any_bound = false;
        let mut bind_half = |half: Arc<dyn AgentToolDispatcher>| {
            if !half.capabilities().ops_lifecycle {
                return Ok(half);
            }
            if Arc::strong_count(&half) != 1 {
                return Err(OpsLifecycleBindError::SharedOwnership);
            }
            let outcome =
                half.bind_ops_lifecycle(Arc::clone(&registry), owner_bridge_session_id.clone())?;
            any_bound |= outcome.was_bound();
            Ok(outcome.into_dispatcher())
        };
        let primary = bind_half(owned.primary)?;
        let fallback = bind_half(owned.fallback)?;
        let rebound: Arc<dyn AgentToolDispatcher> = Arc::new(Self {
            primary,
            fallback,
            execution_authority_id: owned.execution_authority_id,
        });
        Ok(if any_bound {
            BindOutcome::Bound(rebound)
        } else {
            BindOutcome::Skipped(rebound)
        })
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        if self.primary_advertises(call.name) {
            self.primary.dispatch(call).await
        } else {
            self.fallback.dispatch(call).await
        }
    }

    async fn dispatch_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &meerkat_core::ToolDispatchContext,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        if self.primary_advertises(call.name) {
            self.primary.dispatch_with_context(call, context).await
        } else {
            self.fallback.dispatch_with_context(call, context).await
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Probe {
        names: Vec<&'static str>,
        exact: bool,
        dispatched: AtomicUsize,
        contexted: AtomicUsize,
    }

    impl Probe {
        fn new(names: Vec<&'static str>) -> Arc<Self> {
            Self::with_exactness(names, true)
        }

        fn with_exactness(names: Vec<&'static str>, exact: bool) -> Arc<Self> {
            Arc::new(Self {
                names,
                exact,
                dispatched: AtomicUsize::new(0),
                contexted: AtomicUsize::new(0),
            })
        }
    }

    #[async_trait::async_trait]
    impl AgentToolDispatcher for Probe {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            self.names
                .iter()
                .map(|name| {
                    Arc::new(ToolDef {
                        audience: Default::default(),
                        name: (*name).into(),
                        description: String::new(),
                        input_schema: json!({"type": "object"}),
                        provenance: None,
                    })
                })
                .collect::<Vec<_>>()
                .into()
        }

        fn tool_catalog_capabilities(&self) -> ToolCatalogCapabilities {
            ToolCatalogCapabilities {
                exact_catalog: self.exact,
                may_require_catalog_control_plane: false,
            }
        }

        async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
            self.dispatched.fetch_add(1, Ordering::SeqCst);
            Err(ToolError::not_found(call.name))
        }

        async fn dispatch_with_context(
            &self,
            call: ToolCallView<'_>,
            _context: &meerkat_core::ToolDispatchContext,
        ) -> Result<ToolDispatchOutcome, ToolError> {
            self.contexted.fetch_add(1, Ordering::SeqCst);
            Err(ToolError::not_found(call.name))
        }
    }

    fn call<'a>(name: &'a str, args: &'a serde_json::value::RawValue) -> ToolCallView<'a> {
        ToolCallView {
            id: "call-1",
            name,
            args,
        }
    }

    #[tokio::test]
    async fn application_resolution_preserves_selected_source_raw_data_and_native_context() {
        use crate::tool_application_test_support::{
            ApplicationProbe, application_request, native_context,
        };
        let primary = ApplicationProbe::with_action(
            vec!["primary_view", "primary_refresh"],
            "primary_refresh",
        );
        let fallback = ApplicationProbe::new(vec!["fallback_view", "app_refresh"]);
        let composed = ComposedExternalTools::over(primary.clone(), Some(fallback.clone()));
        let request = application_request();
        let context = native_context(request.clone());
        let invocation =
            json!({"registration": "original-physical", "result": {"_meta": {"ui_only": [1,2,3]}}});
        for (source, owner) in [("primary_view", &primary), ("fallback_view", &fallback)] {
            let meerkat_core::tool_application::ToolApplicationResolution::Call {
                name,
                binding,
                project_result,
            } = composed
                .resolve_tool_application(source, &request, &invocation, &context)
                .await
                .unwrap()
            else {
                panic!("expected call")
            };
            assert_eq!(name, owner.action_name);
            assert_eq!(binding.extension, request.extension);
            assert_eq!(
                binding.payload,
                json!({"source": source, "request": request, "invocation": invocation})
            );
            assert_eq!(
                owner.context_address.load(Ordering::SeqCst),
                std::ptr::from_ref(&context) as usize
            );
            let mut result =
                meerkat_core::ToolResult::new("result".into(), "fallback".into(), false);
            result
                .host_metadata
                .insert("opaque".into(), invocation.clone());
            assert_eq!(
                project_result(&result).unwrap(),
                json!({"opaque": invocation})
            );
        }
        assert_eq!(primary.resolved.load(Ordering::SeqCst), 1);
        assert_eq!(fallback.resolved.load(Ordering::SeqCst), 1);

        let args = serde_json::value::RawValue::from_string("{}".into()).unwrap();
        let call = call("app_refresh", &args);
        let resolution = crate::tool_application_test_support::resolution();
        let plan = composed
            .resolve_execution_plan(call, &context, &resolution)
            .unwrap();
        assert!(plan.owner_witness("test:app-leaf").is_some());
        composed
            .dispatch_resolved_with_context(call, &context, &plan)
            .await
            .unwrap();
        assert_eq!(fallback.dispatched.load(Ordering::SeqCst), 1);
        assert_eq!(
            fallback.context_address.load(Ordering::SeqCst),
            std::ptr::from_ref(&context) as usize
        );
    }

    #[tokio::test]
    async fn application_shadowing_never_falls_back_after_primary_refusal() {
        use crate::tool_application_test_support::{
            ApplicationProbe, application_request, native_context,
        };
        let fallback = ApplicationProbe::new(vec!["shared"]);
        let composed =
            ComposedExternalTools::over(Probe::new(vec!["shared"]), Some(fallback.clone()));
        let request = application_request();
        assert!(matches!(
            composed
                .resolve_tool_application(
                    "shared",
                    &request,
                    &json!({}),
                    &native_context(request.clone())
                )
                .await,
            Err(ToolError::AccessDenied { .. })
        ));
        assert_eq!(fallback.resolved.load(Ordering::SeqCst), 0);
        assert_eq!(
            composed.tool_mutation_class("shared"),
            meerkat_core::ToolMutationClass::Unknown
        );
        assert_eq!(
            composed.review_entry_support("shared"),
            meerkat_core::approval::review::ReviewEntrySupport::Unsupported
        );

        let source = ApplicationProbe::new(vec!["view", "app_refresh"]);
        let composed =
            ComposedExternalTools::over(Probe::new(vec!["app_refresh"]), Some(source.clone()));
        assert!(matches!(
            composed
                .resolve_tool_application(
                    "view",
                    &request,
                    &json!({}),
                    &native_context(request.clone())
                )
                .await,
            Err(ToolError::AccessDenied { .. })
        ));
        assert_eq!(source.resolved.load(Ordering::SeqCst), 1);
        assert_eq!(source.dispatched.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn application_resolution_refuses_stable_route_with_changed_leaf_binding() {
        use crate::tool_application_test_support::{
            ApplicationProbe, application_request, native_context,
        };
        let source = ApplicationProbe::new(vec!["view", "app_refresh"]);
        source.block.store(true, Ordering::SeqCst);
        let composed = ComposedExternalTools::over(source.clone(), Some(Probe::new(vec!["host"])));
        let request = application_request();
        let context = native_context(request.clone());
        let invocation = json!({"physical": "original"});
        let pending = composed.resolve_tool_application("view", &request, &invocation, &context);
        tokio::pin!(pending);
        tokio::select! {
            _ = &mut pending => panic!("resolution completed before release"),
            permit = source.entered.acquire() => permit.unwrap().forget(),
        }
        source.epoch.fetch_add(1, Ordering::SeqCst);
        source.release.add_permits(1);
        assert!(matches!(
            pending.await,
            Err(ToolError::Unavailable {
                reason: meerkat_core::ToolUnavailableReason::ExecutionOwnerChanged,
                ..
            })
        ));
        assert_eq!(source.dispatched.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn application_selection_cannot_be_captured_before_execution_plan_resolution() {
        use crate::tool_application_test_support::{
            ApplicationProbe, application_request, native_context, resolution,
        };
        let primary = ApplicationProbe::new(vec!["app_refresh"]);
        primary.active.store(false, Ordering::SeqCst);
        let fallback = ApplicationProbe::new(vec!["view", "app_refresh"]);
        let composed = ComposedExternalTools::over(primary.clone(), Some(fallback.clone()));
        let request = application_request();
        let context = native_context(request.clone());
        let meerkat_core::tool_application::ToolApplicationResolution::Call {
            name, binding, ..
        } = composed
            .resolve_tool_application("view", &request, &json!({"physical":"original"}), &context)
            .await
            .unwrap()
        else {
            panic!("expected app action")
        };
        let args = serde_json::value::RawValue::from_string("{}".into()).unwrap();
        let call = call(&name, &args);
        let original_plan = composed
            .resolve_execution_plan(call, &context, &resolution())
            .unwrap();
        binding
            .validate_execution_plan(&name, &original_plan)
            .unwrap();

        // The source stays on fallback, but a new primary action is published
        // after app resolution and before the ordinary fresh plan is selected.
        primary.active.store(true, Ordering::SeqCst);
        let captured_plan = composed
            .resolve_execution_plan(call, &context, &resolution())
            .unwrap();
        assert!(matches!(
            binding.validate_execution_plan(&name, &captured_plan),
            Err(meerkat_core::ToolExecutionResolutionError::Unavailable {
                reason: meerkat_core::ToolUnavailableReason::ExecutionOwnerChanged,
                ..
            })
        ));
        assert_eq!(primary.dispatched.load(Ordering::SeqCst), 0);
        assert_eq!(fallback.dispatched.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn application_execution_retains_leaf_facts_and_refuses_changed_winner_or_binding() {
        use crate::tool_application_test_support::{
            ApplicationProbe, application_request, native_context, resolution,
        };
        let primary = ApplicationProbe::new(vec!["shared"]);
        let fallback = ApplicationProbe::new(vec!["shared"]);
        let composed = ComposedExternalTools::over(primary.clone(), Some(fallback.clone()));
        assert_eq!(
            composed.tool_mutation_class("shared"),
            meerkat_core::ToolMutationClass::ReadOnly
        );
        assert_eq!(
            composed.live_bridge_effect_kind("shared"),
            meerkat_core::LiveBridgeEffectKind::ReadOnlyMemorySnapshot
        );
        assert_eq!(
            composed.review_entry_support("shared"),
            meerkat_core::approval::review::ReviewEntrySupport::ConsumesAtEntry
        );
        let context = native_context(application_request());
        let args = serde_json::value::RawValue::from_string("{}".into()).unwrap();
        let call = call("shared", &args);
        let fingerprint = composed.execution_binding_fingerprint("shared").unwrap();
        let plan = composed
            .resolve_execution_plan(call, &context, &resolution())
            .unwrap();
        primary.epoch.fetch_add(1, Ordering::SeqCst);
        assert_ne!(
            composed.execution_binding_fingerprint("shared").unwrap(),
            fingerprint
        );
        assert!(
            composed
                .dispatch_resolved_with_context(call, &context, &plan)
                .await
                .is_err()
        );
        let plan = composed
            .resolve_execution_plan(call, &context, &resolution())
            .unwrap();
        primary.active.store(false, Ordering::SeqCst);
        assert!(
            composed
                .dispatch_resolved_with_context(call, &context, &plan)
                .await
                .is_err()
        );
        assert_eq!(primary.dispatched.load(Ordering::SeqCst), 0);
        assert_eq!(fallback.dispatched.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn tools_merge_with_primary_winning_name_collisions() {
        let primary = Probe::new(vec!["shared", "python_tool"]);
        let fallback = Probe::new(vec!["shared", "memory"]);
        let composed = ComposedExternalTools::over(primary, Some(fallback));
        let names: Vec<String> = composed
            .tools()
            .iter()
            .map(|tool| tool.name.to_string())
            .collect();
        assert_eq!(names, vec!["shared", "python_tool", "memory"]);
    }

    #[tokio::test]
    async fn catalog_exactness_is_the_conjunction_and_the_catalog_follows_precedence() {
        let composed = ComposedExternalTools::over(
            Probe::new(vec!["shared", "python_tool"]),
            Some(Probe::new(vec!["shared", "memory"])),
        );
        assert!(composed.tool_catalog_capabilities().exact_catalog);
        let names: Vec<String> = composed
            .tool_catalog()
            .iter()
            .map(|entry| entry.tool.name.to_string())
            .collect();
        assert_eq!(names, vec!["shared", "python_tool", "memory"]);

        for (primary_exact, fallback_exact) in [(false, true), (true, false), (false, false)] {
            let composed = ComposedExternalTools::over(
                Probe::with_exactness(vec!["python_tool"], primary_exact),
                Some(Probe::with_exactness(vec!["memory"], fallback_exact)),
            );
            assert!(
                !composed.tool_catalog_capabilities().exact_catalog,
                "one non-exact half must make the composition non-exact"
            );
        }
    }

    #[tokio::test]
    async fn dispatch_routes_by_advertised_name_on_both_entry_points() {
        let primary = Probe::new(vec!["python_tool"]);
        let fallback = Probe::new(vec!["memory"]);
        let composed = ComposedExternalTools::over(
            Arc::clone(&primary) as _,
            Some(Arc::clone(&fallback) as _),
        );
        let args = serde_json::value::RawValue::from_string("{}".to_string()).expect("raw");

        let _ = composed.dispatch(call("python_tool", &args)).await;
        assert_eq!(primary.dispatched.load(Ordering::SeqCst), 1);
        assert_eq!(fallback.dispatched.load(Ordering::SeqCst), 0);

        let _ = composed.dispatch(call("memory", &args)).await;
        assert_eq!(fallback.dispatched.load(Ordering::SeqCst), 1);

        // The context entry point must forward AS the context entry point —
        // the trait default would drop the ToolDispatchContext (verified
        // witness-loss class).
        let _ = composed
            .dispatch_with_context(
                call("memory", &args),
                &meerkat_core::ToolDispatchContext::default(),
            )
            .await;
        assert_eq!(fallback.contexted.load(Ordering::SeqCst), 1);
        assert_eq!(
            fallback.dispatched.load(Ordering::SeqCst),
            1,
            "context calls must not degrade to plain dispatch"
        );
    }

    #[tokio::test]
    async fn shadowed_fallback_names_name_exactly_the_unreachable_tools() {
        // "shared" collides (primary wins, fallback copy unreachable);
        // "memory" survives untouched. The composition-time warn is driven by
        // this list, so pinning it pins the observability contract: a host
        // tool shadowing the agent-memory recorder is loud, never silent.
        let composed = ComposedExternalTools {
            primary: Probe::new(vec!["shared", "python_tool"]),
            fallback: Probe::new(vec!["shared", "memory"]),
            execution_authority_id: 0,
        };
        assert_eq!(composed.shadowed_fallback_names(), vec!["shared"]);

        let recorder_shadowed = ComposedExternalTools {
            primary: Probe::new(vec!["memory"]),
            fallback: Probe::new(vec!["memory"]),
            execution_authority_id: 0,
        };
        assert_eq!(recorder_shadowed.shadowed_fallback_names(), vec!["memory"]);

        let disjoint = ComposedExternalTools {
            primary: Probe::new(vec!["python_tool"]),
            fallback: Probe::new(vec!["memory"]),
            execution_authority_id: 0,
        };
        assert!(disjoint.shadowed_fallback_names().is_empty());
    }

    #[tokio::test]
    async fn no_fallback_is_the_identity() {
        let primary = Probe::new(vec!["only"]);
        let composed = ComposedExternalTools::over(Arc::clone(&primary) as _, None);
        assert!(Arc::ptr_eq(
            &(Arc::clone(&primary) as Arc<dyn AgentToolDispatcher>),
            &composed
        ));
    }

    type BoundTo = (
        Arc<dyn meerkat_core::ops_lifecycle::OpsLifecycleRegistry>,
        meerkat_core::types::SessionId,
    );

    /// A context-entry dispatch as an ops-capable half saw it: the tool name,
    /// the context's runtime origin session, and its `witness` turn metadata.
    type ContextDispatch = (
        String,
        Option<meerkat_core::types::SessionId>,
        Option<serde_json::Value>,
    );

    /// Everything an ops-capable half observed. It lives outside the half, so
    /// a test holds no extra handle to the half itself.
    #[derive(Default)]
    struct OpsObservations {
        bound: Option<BoundTo>,
        plain_dispatches: Vec<String>,
        context_dispatches: Vec<ContextDispatch>,
    }

    /// Ops-capable half for the binding tests.
    struct OpsHalf {
        names: Vec<&'static str>,
        seen: Arc<std::sync::Mutex<OpsObservations>>,
    }

    impl OpsHalf {
        fn observed(
            names: Vec<&'static str>,
        ) -> (
            Arc<dyn AgentToolDispatcher>,
            Arc<std::sync::Mutex<OpsObservations>>,
        ) {
            let seen = Arc::new(std::sync::Mutex::new(OpsObservations::default()));
            let half = Arc::new(Self {
                names,
                seen: Arc::clone(&seen),
            });
            (half, seen)
        }
    }

    #[async_trait::async_trait]
    impl AgentToolDispatcher for OpsHalf {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            self.names
                .iter()
                .map(|name| {
                    Arc::new(ToolDef {
                        audience: Default::default(),
                        name: (*name).into(),
                        description: String::new(),
                        input_schema: json!({"type": "object"}),
                        provenance: None,
                    })
                })
                .collect::<Vec<_>>()
                .into()
        }

        async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
            self.seen
                .lock()
                .expect("ops lock")
                .plain_dispatches
                .push(call.name.to_string());
            Err(ToolError::not_found(call.name))
        }

        async fn dispatch_with_context(
            &self,
            call: ToolCallView<'_>,
            context: &meerkat_core::ToolDispatchContext,
        ) -> Result<ToolDispatchOutcome, ToolError> {
            self.seen
                .lock()
                .expect("ops lock")
                .context_dispatches
                .push((
                    call.name.to_string(),
                    context.origin_session_id().cloned(),
                    context.turn_metadata("witness").cloned(),
                ));
            Err(ToolError::not_found(call.name))
        }

        fn capabilities(&self) -> DispatcherCapabilities {
            DispatcherCapabilities {
                ops_lifecycle: true,
            }
        }

        fn bind_ops_lifecycle(
            self: Arc<Self>,
            registry: Arc<dyn meerkat_core::ops_lifecycle::OpsLifecycleRegistry>,
            owner_bridge_session_id: meerkat_core::types::SessionId,
        ) -> Result<BindOutcome, OpsLifecycleBindError> {
            let this = Arc::try_unwrap(self).map_err(|_| OpsLifecycleBindError::SharedOwnership)?;
            this.seen.lock().expect("ops lock").bound = Some((registry, owner_bridge_session_id));
            Ok(BindOutcome::Bound(Arc::new(this)))
        }
    }

    fn registry() -> Arc<dyn meerkat_core::ops_lifecycle::OpsLifecycleRegistry> {
        Arc::new(meerkat_runtime::RuntimeOpsLifecycleRegistry::new())
    }

    /// The half was bound to exactly `registry`, with `owner` as owner.
    fn assert_bound_to(
        seen: &std::sync::Mutex<OpsObservations>,
        registry: &Arc<dyn meerkat_core::ops_lifecycle::OpsLifecycleRegistry>,
        owner: &meerkat_core::types::SessionId,
    ) {
        let (bound_registry, bound_owner) = seen
            .lock()
            .expect("ops lock")
            .bound
            .clone()
            .expect("the ops-capable half was bound");
        assert!(Arc::ptr_eq(&bound_registry, registry), "the given registry");
        assert_eq!(&bound_owner, owner);
    }

    fn tool_names(dispatcher: &Arc<dyn AgentToolDispatcher>) -> Vec<String> {
        dispatcher
            .tools()
            .iter()
            .map(|tool| tool.name.to_string())
            .collect()
    }

    #[tokio::test]
    async fn ops_rebinding_preserves_application_selected_owner_and_context() {
        use crate::tool_application_test_support::{
            ApplicationProbe, application_request, native_context, resolution,
        };
        let source = ApplicationProbe::new(vec!["view", "app_refresh"]);
        let (ops, seen) = OpsHalf::observed(vec!["ops_tool"]);
        let composed = ComposedExternalTools::over(source.clone(), Some(ops));
        let request = application_request();
        let context = native_context(request.clone());
        let invocation = json!({"physical": "original", "_meta": {"private": [1, 2]}});
        let meerkat_core::tool_application::ToolApplicationResolution::Call {
            name, binding, ..
        } = composed
            .resolve_tool_application("view", &request, &invocation, &context)
            .await
            .expect("native app resolution")
        else {
            panic!("expected app action");
        };
        let registry = registry();
        let owner = meerkat_core::types::SessionId::new();
        let rebound = composed
            .bind_ops_lifecycle(Arc::clone(&registry), owner.clone())
            .expect("exclusive ops lifecycle binding")
            .into_dispatcher();
        assert_bound_to(&seen, &registry, &owner);
        let args = serde_json::value::RawValue::from_string("{}".into()).expect("raw args");
        let call = call(&name, &args);
        let plan = rebound
            .resolve_execution_plan(call, &context, &resolution())
            .expect("selected execution plan after binding");
        binding
            .validate_execution_plan(&name, &plan)
            .expect("same selected execution owner survives binding");
        rebound
            .dispatch_resolved_with_context(call, &context, &plan)
            .await
            .expect("dispatch to original app owner");
        assert_eq!(source.dispatched.load(Ordering::SeqCst), 1);
        assert_eq!(
            source.context_address.load(Ordering::SeqCst),
            std::ptr::from_ref(&context) as usize
        );
        assert_eq!(
            binding.payload,
            json!({"source": "view", "request": request, "invocation": invocation})
        );
    }

    /// The composition reports an ops-capable FALLBACK and rebinds it,
    /// keeping the other half and the precedence: binding a composed slot
    /// must reach the ops-capable dispatcher behind it, not stop at the
    /// wrapper.
    #[test]
    fn a_composition_reports_and_binds_its_ops_capable_fallback() {
        let (ops, seen) = OpsHalf::observed(vec!["ops_tool"]);
        let composed = ComposedExternalTools::over(Probe::new(vec!["weather"]), Some(ops));
        assert!(composed.capabilities().ops_lifecycle);
        let registry = registry();
        let owner = meerkat_core::types::SessionId::new();
        let outcome = composed
            .bind_ops_lifecycle(Arc::clone(&registry), owner.clone())
            .expect("an exclusively owned composition binds");
        assert!(outcome.was_bound());
        assert_bound_to(&seen, &registry, &owner);
        assert_eq!(
            tool_names(&outcome.into_dispatcher()),
            vec!["weather".to_string(), "ops_tool".to_string()]
        );
    }

    /// The same for an ops-capable PRIMARY over a non-ops fallback: the
    /// capability is reported from the primary alone, and the primary is the
    /// half that binds.
    #[test]
    fn a_composition_reports_and_binds_its_ops_capable_primary() {
        let (ops, seen) = OpsHalf::observed(vec!["ops_tool"]);
        let composed = ComposedExternalTools::over(ops, Some(Probe::new(vec!["weather"])));
        assert!(composed.capabilities().ops_lifecycle);
        let registry = registry();
        let owner = meerkat_core::types::SessionId::new();
        let outcome = composed
            .bind_ops_lifecycle(Arc::clone(&registry), owner.clone())
            .expect("an exclusively owned composition binds");
        assert!(outcome.was_bound());
        assert_bound_to(&seen, &registry, &owner);
        assert_eq!(
            tool_names(&outcome.into_dispatcher()),
            vec!["ops_tool".to_string(), "weather".to_string()]
        );
    }

    /// With BOTH halves ops-capable, both bind to the same registry and owner,
    /// and the rebound composition keeps its routing: the primary still wins
    /// a colliding name, and the context entry point still forwards the
    /// caller's (non-default) context to whichever half owns the name.
    #[tokio::test]
    async fn a_composition_binds_both_ops_halves_and_keeps_its_routing() {
        let (primary, primary_seen) = OpsHalf::observed(vec!["shared", "primary_ops"]);
        let (fallback, fallback_seen) = OpsHalf::observed(vec!["shared", "fallback_ops"]);
        let composed = ComposedExternalTools::over(primary, Some(fallback));
        assert!(composed.capabilities().ops_lifecycle);
        let registry = registry();
        let owner = meerkat_core::types::SessionId::new();
        let outcome = composed
            .bind_ops_lifecycle(Arc::clone(&registry), owner.clone())
            .expect("an exclusively owned composition binds");
        assert!(outcome.was_bound());
        assert_bound_to(&primary_seen, &registry, &owner);
        assert_bound_to(&fallback_seen, &registry, &owner);
        let rebound = outcome.into_dispatcher();
        assert_eq!(
            tool_names(&rebound),
            vec![
                "shared".to_string(),
                "primary_ops".to_string(),
                "fallback_ops".to_string()
            ]
        );

        let args = serde_json::value::RawValue::from_string("{}".to_string()).expect("raw");
        let _ = rebound.dispatch(call("shared", &args)).await;

        let origin = meerkat_core::types::SessionId::new();
        let context = meerkat_core::ToolDispatchContext::default()
            .with_runtime_identity(origin.clone(), None)
            .with_turn_metadata(std::collections::BTreeMap::from([(
                "witness".to_string(),
                json!("attention-7"),
            )]));
        let _ = rebound
            .dispatch_with_context(call("fallback_ops", &args), &context)
            .await;
        let _ = rebound
            .dispatch_with_context(call("shared", &args), &context)
            .await;

        let expected = |name: &str| {
            (
                name.to_string(),
                Some(origin.clone()),
                Some(json!("attention-7")),
            )
        };
        let primary_seen = primary_seen.lock().expect("ops lock");
        let fallback_seen = fallback_seen.lock().expect("ops lock");
        assert_eq!(
            primary_seen.plain_dispatches,
            vec!["shared".to_string()],
            "the primary wins the colliding name after the rebind"
        );
        assert_eq!(primary_seen.context_dispatches, vec![expected("shared")]);
        assert!(
            fallback_seen.plain_dispatches.is_empty(),
            "context calls must not degrade to plain dispatch, and the shadowed copy is never reached"
        );
        assert_eq!(
            fallback_seen.context_dispatches,
            vec![expected("fallback_ops")]
        );
    }

    /// A shared ops-capable half (either one) or a shared composition refuses
    /// typed instead of running with a half silently unbound.
    #[test]
    fn a_shared_ops_half_or_composition_refuses_with_shared_ownership() {
        let (ops, seen) = OpsHalf::observed(vec!["ops_tool"]);
        let retained_fallback = Arc::clone(&ops);
        let composed = ComposedExternalTools::over(Probe::new(vec!["weather"]), Some(ops));
        assert!(matches!(
            composed.bind_ops_lifecycle(registry(), meerkat_core::types::SessionId::new()),
            Err(OpsLifecycleBindError::SharedOwnership)
        ));
        assert!(seen.lock().expect("ops lock").bound.is_none());
        drop(retained_fallback);

        let (ops, seen) = OpsHalf::observed(vec!["ops_tool"]);
        let retained_primary = Arc::clone(&ops);
        let composed = ComposedExternalTools::over(ops, Some(Probe::new(vec!["weather"])));
        assert!(matches!(
            composed.bind_ops_lifecycle(registry(), meerkat_core::types::SessionId::new()),
            Err(OpsLifecycleBindError::SharedOwnership)
        ));
        assert!(seen.lock().expect("ops lock").bound.is_none());
        drop(retained_primary);

        let (ops, seen) = OpsHalf::observed(vec!["ops_tool"]);
        let composed = ComposedExternalTools::over(Probe::new(vec!["weather"]), Some(ops));
        let retained_composition = Arc::clone(&composed);
        assert!(matches!(
            composed.bind_ops_lifecycle(registry(), meerkat_core::types::SessionId::new()),
            Err(OpsLifecycleBindError::SharedOwnership)
        ));
        assert!(seen.lock().expect("ops lock").bound.is_none());
        drop(retained_composition);
    }

    /// With no ops-capable half there is nothing to bind: the composition
    /// reports no capability and a bind is skipped, not refused.
    #[test]
    fn a_composition_without_ops_halves_skips_binding() {
        let composed =
            ComposedExternalTools::over(Probe::new(vec!["weather"]), Some(Probe::new(vec!["x"])));
        assert!(!composed.capabilities().ops_lifecycle);
        let outcome = composed
            .bind_ops_lifecycle(registry(), meerkat_core::types::SessionId::new())
            .expect("nothing to bind is not a refusal");
        assert!(!outcome.was_bound());
    }

    /// The session's mob operator authority reaches both halves; the
    /// existing overlay a composition falls back to may hold a mob
    /// runtime's own operator tools for the member.
    #[test]
    fn a_composition_binds_the_session_mob_authority_on_both_halves() {
        use crate::tool_application_test_support::{MobAuthorityBindProbe, session_mob_authority};
        let primary = Arc::new(MobAuthorityBindProbe::default());
        let fallback = Arc::new(MobAuthorityBindProbe::default());
        let composed = ComposedExternalTools::over(primary.clone(), Some(fallback.clone()));
        let authority = session_mob_authority();
        composed.bind_session_mob_tool_authority(Some(Arc::clone(&authority)));
        for half in [&primary, &fallback] {
            assert!(half.last_bound_is(Some(&authority)));
        }
        composed.bind_session_mob_tool_authority(None);
        for half in [&primary, &fallback] {
            assert!(half.last_bound_is(None));
            assert_eq!(half.bind_count(), 2);
        }
    }
}
