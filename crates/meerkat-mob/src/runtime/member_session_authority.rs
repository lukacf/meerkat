//! The mob runtime's receiver of every local member session's live mob
//! operator authority.
//!
//! Every member build this runtime provisions in a local session carries its
//! external tools inside a receiver for that session, whatever the member's
//! profile says about mob tools: the receiver forwards everything to the
//! tools it wraps, and lists none of its own. The agent factory binds the
//! session's final, sealed mob operator authority to the session's external
//! tools (`AgentToolDispatcher::bind_session_mob_tool_authority`), so the
//! receiver gets it, the authority-only binding of a creator whose mob tools
//! are disabled included, and this runtime holds the live authority of each
//! of its member sessions. [`MemberSessionAuthorities`] keeps them by
//! session. A receiver that wraps no tools (its build has none of its own)
//! only observes that binding (`AgentToolDispatcher::observes_bindings_only`):
//! the agent factory binds it, then composes the build as one with no
//! external tools, so the receiver changes nothing in the session's surface.
//!
//! An agent creator's commit check reads only this record
//! ([`MemberSessionAuthorities::admit_identity_creation`], reached through
//! `MobHandle::admit_member_identity_creation`): it takes no authority from
//! its caller, so a caller holding another session's sealed authority, a
//! wider one included, cannot lend it. A session this runtime did not build,
//! one whose build bound no authority, and one whose tools and agent are gone
//! hold none, and are refused. A host that edits a member build's external
//! tools wraps them, as for the runtime's own member tools; tools it replaces
//! outright leave the session with no recorded authority.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError, RwLock, Weak};

use meerkat_core::agent::{
    AgentToolDispatcher, BindOutcome, DispatcherCapabilities, OpsLifecycleBindError,
    ToolDispatchContext,
};
use meerkat_core::error::ToolError;
use meerkat_core::ops::ToolDispatchOutcome;
use meerkat_core::ops_lifecycle::OpsLifecycleRegistry;
use meerkat_core::service::{LiveMobIncarnation, MobToolAuthorityContext};
use meerkat_core::types::{SessionId, ToolCallView, ToolDef};
use meerkat_runtime::mob_operator_authority::IdentityCreationScopeRefusal;

/// A session's effective mob operator authority handle, as the agent factory
/// binds it.
type SessionAuthorityHandle = Arc<RwLock<MobToolAuthorityContext>>;

/// The live mob operator authority of every local member session this mob
/// runtime built, keyed by session. Process-local and never persisted. It
/// holds each session's handle weakly: an entry answers only while the
/// session holds that handle, through its tools, which hold its receiver, or
/// its agent, which holds the same handle.
#[derive(Default)]
pub struct MemberSessionAuthorities {
    sessions: Mutex<HashMap<SessionId, Weak<RwLock<MobToolAuthorityContext>>>>,
}

impl MemberSessionAuthorities {
    /// The receiver one member build of `session_id` carries: it wraps the
    /// build's external tools (`inner`, none when the build has none, and
    /// then the receiver only observes the build's binding).
    pub(crate) fn receiver(
        self: &Arc<Self>,
        session_id: SessionId,
        inner: Option<Arc<dyn AgentToolDispatcher>>,
    ) -> MemberSessionAuthorityReceiver {
        let wraps_tools = inner.is_some();
        MemberSessionAuthorityReceiver {
            inner: inner.unwrap_or_else(|| {
                Arc::new(meerkat_core::DynamicToolComposite::new(Vec::new()))
                    as Arc<dyn AgentToolDispatcher>
            }),
            wraps_tools,
            authorities: Arc::clone(self),
            session_id,
            bound: Mutex::new(None),
        }
    }

    /// Record what a build of `session_id` bound: its authority, or `None`
    /// when the session holds no mob operator authority. The latest build of
    /// a session wins.
    fn record(&self, session_id: &SessionId, authority: Option<&SessionAuthorityHandle>) {
        let mut sessions = self.sessions.lock().unwrap_or_else(PoisonError::into_inner);
        // Sessions whose tools are gone answer nothing: drop their entries.
        sessions.retain(|_, handle| handle.strong_count() > 0);
        match authority {
            Some(handle) => {
                sessions.insert(session_id.clone(), Arc::downgrade(handle));
            }
            None => {
                sessions.remove(session_id);
            }
        }
    }

    /// A snapshot of the live authority the latest build of `session_id`
    /// bound, or `None`: no build of that session bound one, or the session's
    /// tools are gone.
    pub(crate) fn snapshot(&self, session_id: &SessionId) -> Option<MobToolAuthorityContext> {
        let handle = self
            .sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(session_id)
            .and_then(Weak::upgrade)?;
        let context = handle
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        Some(context)
    }

    /// The identity creation check of an agent creator's commit: whether the
    /// member session `origin_session` may create identities from `template`
    /// in the mob `mob_id` as it runs at `mob_incarnation`. It reads only
    /// `origin_session`'s own live authority, recorded here from that
    /// session's build, and asks the generated MeerkatMachine verdict
    /// (`meerkat_runtime::mob_operator_authority::admit_identity_creation_from_template`)
    /// over it. A session with no record is refused like a session with no
    /// generated authority.
    pub(crate) fn admit_identity_creation(
        &self,
        origin_session: &SessionId,
        mob_id: &str,
        mob_incarnation: LiveMobIncarnation,
        template: &str,
    ) -> Result<(), IdentityCreationScopeRefusal> {
        let authority = self.snapshot(origin_session);
        meerkat_runtime::mob_operator_authority::admit_identity_creation_from_template(
            authority.as_ref(),
            mob_id,
            mob_incarnation,
            template,
        )
    }
}

/// One member build's external tools, wrapped for the session the build is
/// for. Every tool, catalog entry, call and binding is the wrapped tools'
/// own: the receiver only records the authority the agent factory binds to
/// the session's external tools in its runtime's
/// [`MemberSessionAuthorities`], and keeps that handle alive for as long as
/// the session's tools hold it. A receiver that wraps no tools only observes
/// that binding: the agent factory composes its build as one with no
/// external tools, and the session's agent keeps the handle alive.
pub(crate) struct MemberSessionAuthorityReceiver {
    inner: Arc<dyn AgentToolDispatcher>,
    /// Whether the build had external tools for this receiver to wrap.
    wraps_tools: bool,
    authorities: Arc<MemberSessionAuthorities>,
    session_id: SessionId,
    bound: Mutex<Option<SessionAuthorityHandle>>,
}

#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
impl AgentToolDispatcher for MemberSessionAuthorityReceiver {
    async fn resolve_tool_application(
        &self,
        source_tool: &str,
        request: &meerkat_core::ToolApplicationRequest,
        invocation: &serde_json::Value,
        context: &ToolDispatchContext,
    ) -> Result<meerkat_core::tool_application::ToolApplicationResolution, ToolError> {
        self.inner
            .resolve_tool_application(source_tool, request, invocation, context)
            .await
    }

    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        self.inner.tools()
    }

    fn tool_catalog_capabilities(&self) -> meerkat_core::ToolCatalogCapabilities {
        self.inner.tool_catalog_capabilities()
    }

    fn tool_catalog(&self) -> Arc<[meerkat_core::ToolCatalogEntry]> {
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
        self.inner.dispatch(call).await
    }

    async fn dispatch_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        self.inner.dispatch_with_context(call, context).await
    }

    async fn dispatch_resolved_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        plan: &meerkat_core::ResolvedToolExecutionPlan,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        self.inner
            .dispatch_resolved_with_context(call, context, plan)
            .await
    }

    async fn poll_external_updates(&self) -> meerkat_core::ExternalToolUpdate {
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
        registry: Arc<dyn OpsLifecycleRegistry>,
        owner_bridge_session_id: SessionId,
    ) -> Result<BindOutcome, OpsLifecycleBindError> {
        let Self {
            inner,
            wraps_tools,
            authorities,
            session_id,
            bound,
        } = Arc::try_unwrap(self).map_err(|_| OpsLifecycleBindError::SharedOwnership)?;
        let outcome = inner.bind_ops_lifecycle(registry, owner_bridge_session_id)?;
        let was_bound = outcome.was_bound();
        let receiver = Arc::new(Self {
            inner: outcome.into_dispatcher(),
            wraps_tools,
            authorities,
            session_id,
            bound,
        });
        Ok(if was_bound {
            BindOutcome::Bound(receiver)
        } else {
            BindOutcome::Skipped(receiver)
        })
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

    fn bind_session_mob_tool_authority(&self, authority: Option<SessionAuthorityHandle>) {
        self.authorities
            .record(&self.session_id, authority.as_ref());
        self.inner
            .bind_session_mob_tool_authority(authority.clone());
        *self.bound.lock().unwrap_or_else(PoisonError::into_inner) = authority;
    }

    /// A receiver that wraps no tools only observes the binding above, so the
    /// agent factory composes its build as one with no external tools.
    fn observes_bindings_only(&self) -> bool {
        !self.wraps_tools || self.inner.observes_bindings_only()
    }
}
