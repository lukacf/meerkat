//! A trusted host's checked route to raw operations of one MCP server.
//!
//! A server's access ceiling keeps operations out of the router's projection
//! and refuses them at its connection, whatever the caller. A trusted host
//! with a checked registration of its own (for example a wrapper that runs
//! the host's own review before a call) can still need some of them. A
//! checked route offers exactly the operations the host selected, under the
//! server's genuine definitions, on the same connection. It runs a call only
//! when all of these hold, at its entry and again at the transport handoff,
//! after every preparation await:
//! - the route is open;
//! - the dispatch carries actual bound work and a prepared authorization for
//!   exactly this call and plan (a matching runtime identity alone, which
//!   any caller can set, never admits), and it is the route's own session's;
//! - this route instance resolved the plan for exactly that operation, and
//!   its binding and operation target (server registration, connection
//!   generation, connector approval, raw operation) are still the live ones;
//! - the plan never entered through the route before: each plan the route
//!   resolves is one occurrence, named in its witness, and enters once, so
//!   its replay is refused here without consulting any host state. A fresh
//!   resolution is a new occurrence (a provider can repeat a call id across
//!   turns), entering only under its own prepared authorization and review.
//!
//! The plan's single operation target is a
//! [`meerkat_core::ToolOperationTarget::CheckedMcpServerOperation`] naming
//! the route, so the work needs a grant selected for exactly that route to
//! that server: the server's ordinary grant never covers it, and the
//! identical raw operation through the ordinary projection is still refused
//! by the ceiling and by that ordinary grant. That grant is route authority
//! only: it is not proof that the host's own checks (its proposal, approval
//! or consumption) ran. Everything else (call accounting, call-context and
//! connector preparation, the final native authorization check and the
//! reviewed entry at the transport handoff) is the router's ordinary call
//! path. Hand the route only to the host's own checked wrapper: it is never
//! a session tool surface of its own.

use std::collections::{BTreeSet, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock as StdRwLock};

use async_trait::async_trait;
use meerkat_core::error::ToolError;
use meerkat_core::types::{SessionId, ToolCallView, ToolDef};
use meerkat_core::{
    AgentToolDispatcher, ToolCatalogCapabilities, ToolCatalogEntry, ToolDispatchContext,
};

use crate::McpRouterAdapter;
use crate::router::{CheckedEntry, CheckedOperation, McpRouter, call_request_digest};

static NEXT_CHECKED_ROUTE_AUTHORITY_ID: AtomicU64 = AtomicU64::new(1);

/// What one checked route covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedMcpRouteSpec {
    /// The stable name the trusted host registers the route under. The
    /// work's grant names it.
    pub route: String,
    /// The configured name of the MCP server the route reaches.
    pub server: String,
    /// Exactly the raw operations of that server the route offers.
    pub operations: BTreeSet<String>,
}

/// A refused checked route.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum CheckedMcpRouteError {
    #[error(
        "a checked MCP route needs a route name, a server name and at least one operation, \
         none of them empty"
    )]
    Incomplete,
}

/// A trusted host's checked route over one adapter's router (see the
/// module documentation). Created by [`McpRouterAdapter::checked_route`].
pub struct CheckedMcpRoute {
    adapter: Arc<McpRouterAdapter>,
    spec: CheckedMcpRouteSpec,
    session: SessionId,
    execution_authority_id: u64,
    closed: AtomicBool,
    /// The occurrence the next resolved plan names in its witness.
    next_occurrence: AtomicU64,
    /// The occurrences that entered through the route: each enters once.
    entered: Mutex<HashSet<u64>>,
    catalog_cache: StdRwLock<Arc<[ToolCatalogEntry]>>,
}

impl std::fmt::Debug for CheckedMcpRoute {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CheckedMcpRoute")
            .field("spec", &self.spec)
            .field("session", &self.session)
            .field("open", &self.is_open())
            .finish_non_exhaustive()
    }
}

impl CheckedMcpRoute {
    pub(crate) fn new(
        adapter: Arc<McpRouterAdapter>,
        session: SessionId,
        spec: CheckedMcpRouteSpec,
    ) -> Result<Self, CheckedMcpRouteError> {
        if spec.route.is_empty()
            || spec.server.is_empty()
            || spec.operations.is_empty()
            || spec.operations.iter().any(String::is_empty)
        {
            return Err(CheckedMcpRouteError::Incomplete);
        }
        Ok(Self {
            adapter,
            spec,
            session,
            execution_authority_id: NEXT_CHECKED_ROUTE_AUTHORITY_ID.fetch_add(1, Ordering::Relaxed),
            closed: AtomicBool::new(false),
            next_occurrence: AtomicU64::new(1),
            entered: Mutex::new(HashSet::new()),
            catalog_cache: StdRwLock::new(Arc::from([])),
        })
    }

    /// The stable name the route is registered under.
    pub fn route(&self) -> &str {
        &self.spec.route
    }

    /// The server the route reaches.
    pub fn server(&self) -> &str {
        &self.spec.server
    }

    /// The raw operations the route offers.
    pub fn operations(&self) -> &BTreeSet<String> {
        &self.spec.operations
    }

    /// The session the route serves.
    pub fn session(&self) -> &SessionId {
        &self.session
    }

    /// Whether the route still offers and runs its operations.
    pub fn is_open(&self) -> bool {
        !self.closed.load(Ordering::Acquire) && !self.adapter.is_closing()
    }

    /// Close the route for good: it offers nothing more, and no plan it
    /// resolved enters, including one resolved or already dispatched before
    /// the close whose transport handoff has not happened yet. An effect
    /// that already entered keeps its custody: closing never cancels it.
    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.set_catalog_cache(Arc::from([]));
    }

    fn authority_prefix(&self) -> String {
        format!("checked-mcp-route:{}:", self.execution_authority_id)
    }

    /// The witness authority key of one occurrence of this instance.
    fn authority_key(&self, occurrence: u64) -> String {
        format!("{}{occurrence}", self.authority_prefix())
    }

    /// The occurrence this instance minted for `plan`, with its witness: the
    /// plan's only witness under this instance's key.
    fn witnessed_occurrence<'p>(
        &self,
        plan: &'p meerkat_core::ResolvedToolExecutionPlan,
    ) -> Option<(u64, &'p meerkat_core::ToolExecutionOwnerWitness)> {
        let prefix = self.authority_prefix();
        let mut own = plan.owner_witnesses().iter().filter_map(|witness| {
            let occurrence = witness.authority_key().strip_prefix(prefix.as_str())?;
            Some((occurrence, witness))
        });
        let (occurrence, witness) = own.next()?;
        if own.next().is_some() {
            return None;
        }
        let minted = self.next_occurrence.load(Ordering::Acquire);
        let occurrence = occurrence
            .parse::<u64>()
            .ok()
            .filter(|occurrence| (1..minted).contains(occurrence))?;
        Some((occurrence, witness))
    }

    fn has_entered(&self, occurrence: u64) -> bool {
        self.entered
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(&occurrence)
    }

    fn offered(&self, router: &McpRouter) -> Vec<CheckedOperation> {
        if !self.is_open() {
            return Vec::new();
        }
        router.checked_operations(&self.spec.server, &self.spec.operations)
    }

    fn offered_named(&self, router: &McpRouter, name: &str) -> Option<CheckedOperation> {
        self.offered(router)
            .into_iter()
            .find(|offered| offered.tool.name.as_str() == name)
    }

    /// The catalog form of an offered definition: deferred with its
    /// discovery provenance, as the router's projection lists MCP tools.
    fn catalog_entry(tool: &Arc<ToolDef>) -> ToolCatalogEntry {
        match tool.provenance.clone() {
            Some(provenance) => {
                ToolCatalogEntry::session_deferred(Arc::clone(tool), true, provenance)
            }
            None => ToolCatalogEntry::session_inline(Arc::clone(tool), true),
        }
    }

    fn catalog_of(&self, router: &McpRouter) -> Arc<[ToolCatalogEntry]> {
        self.offered(router)
            .iter()
            .map(|offered| Self::catalog_entry(&offered.tool))
            .collect::<Vec<_>>()
            .into()
    }

    /// The live binding of one offered entry, attributed to this route.
    fn live_fingerprint(
        &self,
        entry: &ToolCatalogEntry,
    ) -> meerkat_core::EphemeralToolBindingFingerprint {
        meerkat_core::ephemeral_tool_catalog_binding_fingerprint(entry).with_live_authority(
            self.execution_authority_id as usize,
            self.execution_binding_epoch(entry.tool.name.as_str()),
        )
    }

    fn set_catalog_cache(&self, catalog: Arc<[ToolCatalogEntry]>) {
        match self.catalog_cache.write() {
            Ok(mut guard) => *guard = catalog,
            Err(poisoned) => *poisoned.into_inner() = catalog,
        }
    }

    fn cached_catalog(&self) -> Arc<[ToolCatalogEntry]> {
        match self.catalog_cache.read() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }
}

#[async_trait]
impl AgentToolDispatcher for CheckedMcpRoute {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        self.tool_catalog()
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
        if !self.is_open() {
            return Arc::from([]);
        }
        match self.adapter.router_lock().try_read() {
            Ok(router) => match router.as_ref() {
                Some(router) => {
                    let catalog = self.catalog_of(router);
                    self.set_catalog_cache(catalog.clone());
                    catalog
                }
                // The router was taken for shutdown: nothing is offered.
                None => Arc::from([]),
            },
            Err(_) => self.cached_catalog(),
        }
    }

    fn tool_mutation_class(&self, tool_name: &str) -> meerkat_core::ToolMutationClass {
        self.adapter
            .router_lock()
            .try_read()
            .ok()
            .and_then(|guard| {
                let router = guard.as_ref()?;
                let offered = self.offered_named(router, tool_name)?;
                Some(router.checked_mutation_class(&self.spec.server, &offered.raw_operation))
            })
            .unwrap_or(meerkat_core::ToolMutationClass::Unknown)
    }

    /// The router's call path carries review to the transport handoff, as
    /// for the projection's own routes.
    fn review_entry_support(
        &self,
        tool_name: &str,
    ) -> meerkat_core::approval::review::ReviewEntrySupport {
        if self
            .tool_catalog()
            .iter()
            .any(|entry| entry.tool.name.as_str() == tool_name)
        {
            meerkat_core::approval::review::ReviewEntrySupport::ConsumesAtEntry
        } else {
            meerkat_core::approval::review::ReviewEntrySupport::Unsupported
        }
    }

    /// The adapter's own generation: every replacement it observes is one
    /// for the route too.
    fn execution_binding_epoch(&self, tool_name: &str) -> u64 {
        AgentToolDispatcher::execution_binding_epoch(self.adapter.as_ref(), tool_name)
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
            .find(|entry| entry.tool.name.as_str() == tool_name)
            .ok_or_else(|| meerkat_core::ToolExecutionResolutionError::NotFound {
                tool_name: tool_name.to_string(),
            })?;
        Ok(self.live_fingerprint(entry))
    }

    /// Resolve under one router read: the offered definition, its execution
    /// contract, and this route's witness, naming a new occurrence, of
    /// exactly the checked operation target the call resolves to now.
    fn resolve_execution_plan(
        &self,
        call: ToolCallView<'_>,
        _dispatch_context: &ToolDispatchContext,
        resolution_context: &meerkat_core::ToolExecutionResolutionContext,
    ) -> Result<meerkat_core::ResolvedToolExecutionPlan, meerkat_core::ToolExecutionResolutionError>
    {
        let not_found = || meerkat_core::ToolExecutionResolutionError::NotFound {
            tool_name: call.name.to_string(),
        };
        let unavailable = || meerkat_core::ToolExecutionResolutionError::Unavailable {
            tool_name: call.name.to_string(),
            reason: meerkat_core::ToolUnavailableReason::TemporarilyUnavailable,
        };
        if !self.is_open() {
            return Err(not_found());
        }
        let guard = self
            .adapter
            .router_lock()
            .try_read()
            .map_err(|_| unavailable())?;
        let router = guard.as_ref().ok_or_else(unavailable)?;
        let offered = self
            .offered_named(router, call.name)
            .ok_or_else(not_found)?;
        let entry = Self::catalog_entry(&offered.tool);
        if let Some(reason) = entry.callability.unavailable_reason() {
            return Err(meerkat_core::ToolExecutionResolutionError::Unavailable {
                tool_name: call.name.to_string(),
                reason,
            });
        }
        let target = router
            .checked_operation_target(&self.spec.route, &self.spec.server, &offered.raw_operation)
            .ok_or_else(unavailable)?;
        let digest = call_request_digest(&target, call);
        let occurrence = self.next_occurrence.fetch_add(1, Ordering::AcqRel);
        let witness = meerkat_core::ToolExecutionOwnerWitness::new(
            self.authority_key(occurrence),
            call.name,
            self.live_fingerprint(&entry),
        )
        .map_err(meerkat_core::ToolExecutionResolutionError::from)?
        .with_operation_target(target);
        let witness = match digest {
            Some(digest) => witness.with_request_digest(digest),
            None => witness,
        };
        entry
            .execution
            .resolve_default(resolution_context.deadlines().clone())
            .map_err(meerkat_core::ToolExecutionResolutionError::from)?
            .with_owner_witness(witness)
    }

    /// A checked route runs only a plan it resolved itself.
    async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, ToolError> {
        Err(ToolError::unavailable(
            call.name,
            meerkat_core::ToolUnavailableReason::ExecutionOwnerChanged,
        ))
    }

    /// A checked route runs only a plan it resolved itself.
    async fn dispatch_with_context(
        &self,
        call: ToolCallView<'_>,
        _context: &ToolDispatchContext,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, ToolError> {
        Err(ToolError::unavailable(
            call.name,
            meerkat_core::ToolUnavailableReason::ExecutionOwnerChanged,
        ))
    }

    /// Run `plan` only while the route is open, under actual bound work and
    /// a prepared authorization for exactly this call and plan in the
    /// route's own session, when this route instance resolved the plan for
    /// exactly this operation, the plan's occurrence never entered through
    /// it, and its binding and checked operation target are still the live
    /// ones under the router read held through the call. Everything after
    /// that is the router's ordinary call path, admitted by the route's
    /// selection instead of the access ceiling, with the same conjuncts
    /// asked again at the transport handoff.
    async fn dispatch_resolved_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        plan: &meerkat_core::ResolvedToolExecutionPlan,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, ToolError> {
        let changed = || {
            ToolError::unavailable(
                call.name,
                meerkat_core::ToolUnavailableReason::ExecutionOwnerChanged,
            )
        };
        if !self.is_open() {
            return Err(changed());
        }
        // The ceiling bypass needs actual bound work: a runtime identity
        // alone, which any caller can set, never admits.
        if context.origin_session_id() != Some(&self.session)
            || !context.is_prepared_for(call, plan)
        {
            return Err(ToolError::access_denied(call.name));
        }
        // Only a plan this instance resolved, and only once: its replay is
        // refused.
        let Some((occurrence, witness)) = self.witnessed_occurrence(plan) else {
            return Err(changed());
        };
        if self.has_entered(occurrence) {
            return Err(changed());
        }
        let guard = self.adapter.router_lock().read().await;
        let Some(router) = guard.as_ref() else {
            return Err(changed());
        };
        let Some(offered) = self.offered_named(router, call.name) else {
            return Err(changed());
        };
        let live = self.live_fingerprint(&Self::catalog_entry(&offered.tool));
        let target = router.checked_operation_target(
            &self.spec.route,
            &self.spec.server,
            &offered.raw_operation,
        );
        let Some(
            target @ meerkat_core::ToolOperationTarget::CheckedMcpServerOperation {
                connection_generation,
                ..
            },
        ) = &target
        else {
            return Err(changed());
        };
        let witnessed = witness.owner_key() == call.name
            && witness.binding_fingerprint() == &live
            && witness.operation_target() == Some(target)
            && witness.request_digest() == call_request_digest(target, call).as_ref();
        if !witnessed {
            return Err(changed());
        }
        let entry = RouteEntry {
            route: self,
            plan,
            occurrence,
            connection_generation: *connection_generation,
        };
        router
            .dispatch_checked(
                &entry,
                &self.spec.server,
                &offered.raw_operation,
                call,
                context,
            )
            .await
    }
}

/// The route's own conjunct for one dispatched plan, asked by the router at
/// the transport handoff, after every preparation await.
struct RouteEntry<'a> {
    route: &'a CheckedMcpRoute,
    plan: &'a meerkat_core::ResolvedToolExecutionPlan,
    occurrence: u64,
    connection_generation: u64,
}

impl CheckedEntry for RouteEntry<'_> {
    fn operations(&self) -> &BTreeSet<String> {
        &self.route.spec.operations
    }

    fn reserve_entry(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        connection_generation: u64,
    ) -> Result<(), ToolError> {
        let changed = || {
            ToolError::unavailable(
                call.name,
                meerkat_core::ToolUnavailableReason::ExecutionOwnerChanged,
            )
        };
        if !self.route.is_open() || connection_generation != self.connection_generation {
            return Err(changed());
        }
        if context.origin_session_id() != Some(&self.route.session)
            || !context.is_prepared_for(call, self.plan)
        {
            return Err(ToolError::access_denied(call.name));
        }
        // The occurrence's single entry: a second one, concurrent or a
        // replay, is refused.
        if !self
            .route
            .entered
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(self.occurrence)
        {
            return Err(changed());
        }
        Ok(())
    }

    fn release_entry(&self) {
        self.route
            .entered
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.occurrence);
    }
}

#[cfg(test)]
#[path = "checked_route_tests.rs"]
mod tests;
