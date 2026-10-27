//! Identity-first continuity types and contracts for Meta.

pub mod adapters;
pub mod agent_memory;
pub mod bridge;
pub mod contracts;
#[cfg(test)]
pub(crate) mod core_test_bridge;
#[cfg(test)]
pub(crate) mod core_test_members;
pub mod customizer_tools;
mod delivery_scope;
pub mod first_boot_manifest;
pub mod gateway_bridges;
pub(crate) mod held_bootstrap;
pub mod local_lease;
pub mod local_store;
pub mod orchestrator;
pub mod runtime;
mod types;

pub use adapters::{
    ContinuityIncrementalSessionStore, ContinuitySessionStoreAdapter, DiscoveryRosterAdapter,
    EdgeDiscoveryTopologyAdapter, MutableRosterProvider, SessionHookCustomizerAdapter,
    SessionRuntimeState, agent_discovery_to_durable,
};
pub use agent_memory::{
    AgentMemoryConfig, AgentMemoryCustomizer, AgentMemoryError, AgentMemoryForgetResult,
    AgentMemoryLlmWrites, AgentMemoryOperatorScope, AgentMemoryPerTurnInjection,
    AgentMemoryProvider, AgentMemoryRecallFailurePolicy, AgentMemoryRecallRequest,
    AgentMemoryRecord, AgentMemoryRuntimeInjector, AgentMemorySelection, AuthoredWriteReceipt,
    MEMORY_TOOL_NAME, NewAgentMemory,
};
pub use bridge::{
    ActorCallObservation, ActorCommandTimeout, BoundGovernedInput, BridgeAdmissionError,
    BridgeDelivery, BridgeError, BridgeMemberReload, BridgeTurnError, BridgeTurnReceipt,
    CommittedBoundaryRecoverer, CommittedBoundaryRepair, CompactionFloorRegistry,
    ControllerPinSource, GovernedAdmission, GovernedAdmissionRecord, GovernedBindContext,
    GovernedDelivery, GovernedDeliveryCustody, GovernedEntryFence, GovernedEntryOutcome,
    GovernedEntryPermit, GovernedEntryRefusal, GovernedInputBinder, GovernedInputError,
    GovernedSettlement, MemberInspection, MemberRuntimeBinding, MobSessionBridge,
    NativeInputAdmissionSettlement, QueuedMemberInputs, ResumeFallbackReason, ResumeRejectionKind,
    ResumeSessionOutcome, SessionBridge,
};
pub use contracts::*;
pub use customizer_tools::{
    ComposedSpawnMemberCustomizer, CustomizerToolRegistry, CustomizerToolsSpawnCustomizer,
    IdentityCustomizerTools,
};
pub use delivery_scope::{
    DELIVERY_SCOPE_VERSION, DeliveryScope, DeliveryScopeError, ScopeMismatch, ScopedDeliveryError,
    ScopedDispatchReceipt, ScopedRecovery, ScopedRecoveryUnresolved, ScopedRetireError,
};
pub use first_boot_manifest::{
    FIRST_BOOT_MANIFEST_FILE, FIRST_BOOT_MANIFEST_FORMAT, FIRST_BOOT_MANIFEST_VERSION,
    FirstBootManifest, FirstBootManifestError, FirstBootManifestLocation, FirstBootManifestRefusal,
    FirstBootManifestRemoval, FirstBootRow, FirstBootRowKind, FirstBootRowState,
};
pub use gateway_bridges::{
    CallbackBridge, GatewayAgentCustomizer, GatewayContinuityStore, GatewayLeaseProvider,
    GatewayRosterProvider, GatewayTopologyProvider,
};
pub use held_bootstrap::{
    HELD_COMPLETION_OWED_DIR, HeldBootstrapRefusal, HeldBootstrapSettlement, HeldBootstrapStart,
    HeldCompletion, HeldCompletionOutcome, HeldCreatePass, HeldIdentityMember,
};
pub use local_lease::LocalLeaseProvider;
pub use local_store::{
    HEAD_CANONICAL_SCHEMA_VERSION as HEAD_CANONICAL_CONTINUITY_SCHEMA_VERSION, LocalContinuityStore,
};
pub use orchestrator::{
    ReconcileAction, RestoreFlowResult, RestoreOutcome, compute_reconcile_actions,
    lazy_register_flow, restore_flow,
};
pub(crate) use runtime::NotServedHere;
pub use runtime::live_binding_runtime_id;
pub use runtime::{
    GovernedSendOutcome, IdentityFirstRuntimeContext, IdentityRuntime, IdentityRuntimeConfig,
    IdentityRuntimeError, UnresumableReason, WakeHold, wire_cross_mob_by_identity,
};
pub use types::*;

/// An exported identity runtime: the runtime together with the mob's public
/// owner (D67). The identity runtime's own bridge and runtime services are
/// non-owning, because the composition holds them internally; a caller that
/// keeps this view keeps the mob publicly owned, so the mob is never closed
/// for idleness under it.
#[derive(Clone)]
pub struct PublicIdentityRuntime {
    runtime: std::sync::Arc<IdentityRuntime>,
    owner: crate::mob_handle_runtime::MobRuntime,
}

impl PublicIdentityRuntime {
    pub(crate) fn new(
        runtime: std::sync::Arc<IdentityRuntime>,
        owner: crate::mob_handle_runtime::MobRuntime,
    ) -> Self {
        Self { runtime, owner }
    }

    /// The inner runtime, for crate-internal back-edges only: an exported
    /// view never hands out an ownerless `Arc`.
    pub(crate) fn runtime(&self) -> &std::sync::Arc<IdentityRuntime> {
        &self.runtime
    }

    /// The mob's public owner this view carries.
    pub fn owner(&self) -> &crate::mob_handle_runtime::MobRuntime {
        &self.owner
    }

    /// A weak back-link to this view. It keeps neither the identity runtime
    /// nor the mob alive or publicly owned; see
    /// [`WeakPublicIdentityRuntime::upgrade`].
    #[must_use]
    pub fn downgrade(&self) -> WeakPublicIdentityRuntime {
        WeakPublicIdentityRuntime {
            runtime: std::sync::Arc::downgrade(&self.runtime),
            mob_id: self.owner.handle().definition().id.clone(),
        }
    }

    /// The runtime's session bridge, as an exported view carrying the mob's
    /// public owner. The raw bridge is crate-internal:
    ///
    /// ```compile_fail
    /// fn escape(
    ///     view: &meerkat_meta::identity_first::PublicIdentityRuntime,
    /// ) -> Option<std::sync::Arc<dyn meerkat_meta::identity_first::bridge::SessionBridge>> {
    ///     meerkat_meta::identity_first::IdentityRuntime::bridge(view).cloned()
    /// }
    /// ```
    ///
    /// ```
    /// fn owned(
    ///     view: &meerkat_meta::identity_first::PublicIdentityRuntime,
    /// ) -> Option<meerkat_meta::identity_first::PublicSessionBridge> {
    ///     view.bridge()
    /// }
    /// ```
    pub fn bridge(&self) -> Option<PublicSessionBridge> {
        self.runtime.bridge().map(|bridge| {
            PublicSessionBridge::new(std::sync::Arc::clone(bridge), self.owner.clone())
        })
    }

    /// Attach one reviewed MCP server live to `identity`'s current member
    /// session, through that member's own actor between turns. The
    /// attachment carries the native prepared control for that session
    /// (`MeerkatMachine::prepare_mcp_attachment`); the actor's router stages
    /// it with that control, which is asked again at the connection's entry.
    ///
    /// It never waits for a held turn boundary: `Busy` means that boundary
    /// is held (not necessarily an active model turn), so a member cannot
    /// attach to itself from inside its own turn. `Busy` grants nothing:
    /// whether to try again is for the caller's own owner to decide. The
    /// call is not otherwise wait-free: it first reads the identity's status
    /// (the identity table's read lock, which can wait behind a writer), and
    /// the member's actor stages and applies the add before it answers.
    /// `Refused(NotCurrent)` when the identity has no current session or its
    /// session has no live actor; `Unsupported` when the member's session
    /// owner or construction has no live attachment entry.
    ///
    /// The session is read from the identity's status first. A status that
    /// goes stale before the attach (the identity moved to another session)
    /// cannot retarget it: the owner looks up that exact session's live
    /// actor, so a session that is no longer live answers
    /// `Refused(NotCurrent)`, and the prepared control is checked again
    /// against the current registration when the add is applied.
    ///
    /// The receipt's attempt reports that exact attempt's readiness for a
    /// tool (`McpAttachmentAttempt::observe` / `wait`). It answers
    /// `NotCurrent` once the identity is no longer on the attached session
    /// or the attaching actor was replaced, checked before and after each
    /// read. `wait` bounds every step, those status reads included, by its
    /// one deadline; `observe` has no deadline and can wait behind a writer
    /// of the identity table or the router. It is a local observation, never a permit: it extends no
    /// authority and proves no cleanup.
    ///
    /// The attempt's stdio setup diagnostics
    /// (`McpAttachmentAttempt::setup_failure` and the SENSITIVE
    /// `stderr_tail`) are read under the same checks as `observe` and answer
    /// `None` once the identity is no longer on the attached session. They
    /// are for the host and its operator only: Meta does not log them or put
    /// them in any event or payload.
    pub async fn attach_live_mcp_server(
        &self,
        identity: &AgentIdentity,
        attachment: meerkat_core::mcp_attachment::LiveMcpAttachment,
    ) -> Result<
        meerkat_core::mcp_attachment::LiveMcpAttachmentReceipt,
        meerkat_core::mcp_attachment::LiveMcpAttachError,
    > {
        use meerkat_core::mcp_attachment::{LiveMcpAttachError, McpAttachmentRefusal};
        let not_current = || LiveMcpAttachError::Refused(McpAttachmentRefusal::NotCurrent);
        let session_id = self
            .runtime
            .status(identity)
            .await
            .map_err(|_| not_current())?
            .session_id
            .ok_or_else(not_current)?;
        let bridge = self
            .runtime
            .bridge()
            .ok_or(LiveMcpAttachError::Unsupported)?;
        let mut receipt = bridge
            .attach_live_mcp_server(&session_id, attachment)
            .await?;
        // The attempt answers only while this identity is still on the
        // session it was attached to.
        receipt.attempt = receipt.attempt.observed_through(|inner| {
            std::sync::Arc::new(IdentityBoundAttempt::new(
                std::sync::Arc::downgrade(&self.runtime),
                identity.clone(),
                session_id,
                inner,
            ))
        });
        Ok(receipt)
    }
}

/// An attachment attempt observed only while its identity is still on the
/// session it was attached to: the identity's current session is checked
/// before and after every read. Each check reads the identity's status (the
/// identity table's read lock, which can wait behind a writer); `wait`
/// bounds those reads by its deadline, `observe` and the setup diagnostics
/// do not. It holds the identity runtime weakly: a retained attempt adds no
/// strong owner of the runtime (a read upgrades it only for that read). That
/// is an ownership limit, not proof of teardown.
pub(crate) struct IdentityBoundAttempt {
    runtime: std::sync::Weak<IdentityRuntime>,
    identity: AgentIdentity,
    session_id: meerkat_core::types::SessionId,
    inner: std::sync::Arc<dyn meerkat_core::mcp_attachment::McpAttachmentAttemptObserver>,
}

impl IdentityBoundAttempt {
    pub(crate) fn new(
        runtime: std::sync::Weak<IdentityRuntime>,
        identity: AgentIdentity,
        session_id: meerkat_core::types::SessionId,
        inner: std::sync::Arc<dyn meerkat_core::mcp_attachment::McpAttachmentAttemptObserver>,
    ) -> Self {
        Self {
            runtime,
            identity,
            session_id,
            inner,
        }
    }

    async fn current(&self) -> bool {
        let Some(runtime) = self.runtime.upgrade() else {
            return false;
        };
        runtime
            .status(&self.identity)
            .await
            .is_ok_and(|status| status.session_id.as_ref() == Some(&self.session_id))
    }

    /// `read`'s answer while the identity is still on the attached session,
    /// checked before and after the read; `stale` otherwise.
    async fn checked<T>(&self, read: impl std::future::Future<Output = T>, stale: T) -> T {
        if !self.current().await {
            return stale;
        }
        let answer = read.await;
        if self.current().await { answer } else { stale }
    }
}

#[async_trait::async_trait]
impl meerkat_core::mcp_attachment::McpAttachmentAttemptObserver for IdentityBoundAttempt {
    async fn observe(&self, tool: &str) -> meerkat_core::mcp_attachment::McpAttachmentReadiness {
        self.checked(
            self.inner.observe(tool),
            meerkat_core::mcp_attachment::McpAttachmentReadiness::NotCurrent,
        )
        .await
    }

    /// One absolute deadline bounds the whole wait: the identity check
    /// before, the native wait with the remaining budget, and the identity
    /// check after. A check that cannot complete by the deadline never
    /// lets a positive answer through.
    async fn wait(
        &self,
        tool: &str,
        timeout: std::time::Duration,
    ) -> meerkat_core::mcp_attachment::McpAttachmentReadiness {
        use meerkat_core::mcp_attachment::McpAttachmentReadiness;
        let deadline = tokio::time::Instant::now() + timeout;
        match tokio::time::timeout_at(deadline, self.current()).await {
            Ok(true) => {}
            Ok(false) => return McpAttachmentReadiness::NotCurrent,
            Err(_) => return McpAttachmentReadiness::Pending,
        }
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let readiness = self.inner.wait(tool, remaining).await;
        match tokio::time::timeout_at(deadline, self.current()).await {
            Ok(true) => readiness,
            Ok(false) => McpAttachmentReadiness::NotCurrent,
            // Currentness could not be confirmed in time: a negative answer
            // stays truthful, a positive one is not settled.
            Err(_) => match readiness {
                McpAttachmentReadiness::Usable { .. }
                | McpAttachmentReadiness::ToolNotRouted { .. } => McpAttachmentReadiness::Pending,
                other => other,
            },
        }
    }

    /// The attempt's typed stdio setup failure, under the same identity
    /// checks as `observe`: `None` once the identity left the attached
    /// session.
    async fn setup_failure(&self) -> Option<meerkat_core::mcp_attachment::McpStdioSetupFailure> {
        self.checked(self.inner.setup_failure(), None).await
    }

    /// The attempt's stderr tail, under the same identity checks as
    /// `observe`: `None` once the identity left the attached session.
    /// SENSITIVE: for the host and its operator only, never model context
    /// (`McpStdioStderrTail`); it is passed through, never logged or kept.
    async fn stderr_tail(&self) -> Option<meerkat_core::mcp_attachment::McpStdioStderrTail> {
        self.checked(self.inner.stderr_tail(), None).await
    }
}

/// A weak back-link to a [`PublicIdentityRuntime`], for holders that live
/// inside the runtime's own graph (for example a host's governed submitter):
/// it never keeps the identity runtime alive nor the mob publicly owned, and
/// exposes no raw runtime.
///
/// It holds only a `Weak` to the identity runtime and the mob's id. No mob
/// handle: a handle retains the mob's session service, and through the
/// composition's tool slot that reaches the identity runtime again. On
/// upgrade the owner is recovered from the composition itself, through the
/// identity runtime's own non-owning mob handle.
#[derive(Clone)]
pub struct WeakPublicIdentityRuntime {
    runtime: std::sync::Weak<IdentityRuntime>,
    mob_id: meerkat_mob::MobId,
}

impl WeakPublicIdentityRuntime {
    /// The owner-bound public view again, or `None` once the identity
    /// runtime is gone or the mob no longer admits a public owner (its
    /// admission closed). The returned view carries a fresh public owner and
    /// every check of [`PublicIdentityRuntime`], including
    /// `owned_identity_for_member_alias` and `send_governed_with_turn_ticket`.
    /// Its [`PublicIdentityRuntime::owner`] is the mob's ownership token over
    /// the mob handle alone, not the composition's runtime.
    #[must_use]
    pub fn upgrade(&self) -> Option<PublicIdentityRuntime> {
        let runtime = self.runtime.upgrade()?;
        // The identity runtime's originating handle to the mob it was
        // exported with; the id check keeps the pairing exact.
        let handle = runtime.runtime_services().mob_handle()?;
        if handle.definition().id != self.mob_id {
            return None;
        }
        // D67: atomic with the admission close, so `None` once it closed.
        let owner = handle.acquire_owner().ok()?;
        Some(PublicIdentityRuntime::new(
            runtime,
            crate::mob_handle_runtime::MobRuntime::from_handle(owner),
        ))
    }

    /// How many strong references the identity runtime still has.
    #[cfg(test)]
    pub(crate) fn runtime_strong_count(&self) -> usize {
        self.runtime.strong_count()
    }
}

impl std::fmt::Debug for WeakPublicIdentityRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WeakPublicIdentityRuntime")
            .field("runtime_alive", &(self.runtime.strong_count() > 0))
            .finish_non_exhaustive()
    }
}

impl std::ops::Deref for PublicIdentityRuntime {
    type Target = IdentityRuntime;

    fn deref(&self) -> &IdentityRuntime {
        &self.runtime
    }
}

/// An exported identity-first runtime context with the mob's public owner
/// (see [`PublicIdentityRuntime`]).
///
/// The view never yields an ownerless context: borrowing through `Deref`
/// works,
///
/// ```
/// fn borrow(
///     view: &meerkat_meta::identity_first::PublicIdentityFirstContext,
/// ) -> &meerkat_meta::identity_first::IdentityFirstRuntimeContext {
///     view
/// }
/// ```
///
/// but the context is not `Clone`, so a by-value copy does not compile:
///
/// ```compile_fail
/// fn escape(
///     view: &meerkat_meta::identity_first::PublicIdentityFirstContext,
/// ) -> meerkat_meta::identity_first::IdentityFirstRuntimeContext {
///     meerkat_meta::identity_first::IdentityFirstRuntimeContext::clone(&*view)
/// }
/// ```
#[derive(Clone)]
pub struct PublicIdentityFirstContext {
    context: std::sync::Arc<IdentityFirstRuntimeContext>,
    owner: crate::mob_handle_runtime::MobRuntime,
}

impl PublicIdentityFirstContext {
    pub(crate) fn new(
        context: std::sync::Arc<IdentityFirstRuntimeContext>,
        owner: crate::mob_handle_runtime::MobRuntime,
    ) -> Self {
        Self { context, owner }
    }

    pub fn owner(&self) -> &crate::mob_handle_runtime::MobRuntime {
        &self.owner
    }

    /// The context's identity runtime, as an exported (owning) view.
    pub fn runtime(&self) -> PublicIdentityRuntime {
        PublicIdentityRuntime::new(
            std::sync::Arc::clone(&self.context.runtime),
            self.owner.clone(),
        )
    }
}

impl std::ops::Deref for PublicIdentityFirstContext {
    type Target = IdentityFirstRuntimeContext;

    fn deref(&self) -> &IdentityFirstRuntimeContext {
        &self.context
    }
}

/// An exported session bridge with the mob's public owner (see
/// [`PublicIdentityRuntime`]); the runtime's own bridge is non-owning.
#[derive(Clone)]
pub struct PublicSessionBridge {
    bridge: std::sync::Arc<dyn bridge::SessionBridge>,
    owner: crate::mob_handle_runtime::MobRuntime,
}

impl PublicSessionBridge {
    pub(crate) fn new(
        bridge: std::sync::Arc<dyn bridge::SessionBridge>,
        owner: crate::mob_handle_runtime::MobRuntime,
    ) -> Self {
        Self { bridge, owner }
    }

    pub fn owner(&self) -> &crate::mob_handle_runtime::MobRuntime {
        &self.owner
    }
}

impl std::ops::Deref for PublicSessionBridge {
    type Target = dyn bridge::SessionBridge;

    fn deref(&self) -> &Self::Target {
        &*self.bridge
    }
}

#[cfg(test)]
#[allow(clippy::panic, reason = "test assertions")]
mod public_view_forwarding_tests {
    use std::collections::BTreeSet;

    /// Method names declared in the block that starts at `header`.
    fn methods(source: &str, header: &str) -> BTreeSet<String> {
        let start = source
            .find(header)
            .unwrap_or_else(|| panic!("{header} not found"));
        let body = &source[start..];
        let end = body.find("\n}\n").unwrap_or(body.len());
        body[..end]
            .lines()
            .filter_map(|line| {
                let line = line.trim_start();
                let line = line.strip_prefix("async ").unwrap_or(line);
                let rest = line.strip_prefix("fn ")?;
                Some(rest.split(['(', '<']).next()?.trim().to_string())
            })
            .collect()
    }

    /// D67: the exported bridge forwards every SessionBridge method; a
    /// method left to its trait default would silently diverge from the
    /// composition's own bridge.
    #[test]
    fn public_session_bridge_forwards_every_method() {
        let source = include_str!("bridge.rs");
        let declared = methods(source, "pub trait SessionBridge");
        let forwarded = methods(source, "impl SessionBridge for super::PublicSessionBridge");
        assert!(!declared.is_empty());
        assert_eq!(
            declared.difference(&forwarded).collect::<Vec<_>>(),
            Vec::<&String>::new(),
            "SessionBridge methods PublicSessionBridge does not forward"
        );
    }
}
