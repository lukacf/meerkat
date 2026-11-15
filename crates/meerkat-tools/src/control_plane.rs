use async_trait::async_trait;
use meerkat_core::error::ToolError;
use meerkat_core::ops::{SessionEffect, ToolDispatchOutcome};
use meerkat_core::session::{
    DeferredToolLoadAuthority, SessionToolVisibilityState, ToolVisibilityWitness,
};
use meerkat_core::types::{ToolCallView, ToolDef, ToolProvenance, ToolResult, ToolSourceKind};
use meerkat_core::{
    AgentToolDispatcher, ToolCatalogCapabilities, ToolCatalogDeferredEligibility, ToolCatalogEntry,
    ToolCatalogLoadRejectedReason, ToolCatalogLoadResolution, ToolCatalogMode, ToolPlaneClass,
    ToolScope, select_tool_catalog_mode, should_compose_tool_catalog_control_plane,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use crate::schema::schema_for;

use meerkat_core::tool_catalog::{
    TOOL_CATALOG_LOAD_NAME as LOAD_TOOL_NAME, TOOL_CATALOG_SEARCH_NAME as SEARCH_TOOL_NAME,
};
const CONTROL_SOURCE_ID: &str = "control_plane";
static NEXT_CATALOG_CONTROL_AUTHORITY_ID: AtomicU64 = AtomicU64::new(1);

/// Typed refusal raised when the deferred tool-load control plane has no
/// [`ToolScope`] bound.
///
/// Without a bound scope there is no visibility authority to consult, so the
/// control plane refuses to act rather than fall back to default visibility and
/// admit loads under truth it does not actually own. The deferred-load path
/// fails closed on this marker (it emits no `RequestDeferredTools` effect).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoToolScope;

/// Visibility facts resolved from a *bound* [`ToolScope`] (or, in tests, an
/// injected visibility-state override).
///
/// This is produced only when a scope authority is actually present; it replaces
/// the prior `unwrap_or_default()` fallbacks that let the control plane serve
/// default visibility as if it were authoritative.
struct ResolvedLoadVisibility {
    visibility_state: SessionToolVisibilityState,
    visible_tool_names: BTreeSet<meerkat_core::ToolName>,
}

#[derive(Default)]
pub struct CatalogControlVisibilityProvider {
    scope: RwLock<Option<ToolScope>>,
    #[cfg(test)]
    visibility_state_override: RwLock<Option<SessionToolVisibilityState>>,
    /// Test-only: the bound scope's read of the names the model holds fails.
    #[cfg(test)]
    visible_names_unreadable: std::sync::atomic::AtomicBool,
    /// Test-only: the bound scope's filter predicate fails.
    #[cfg(test)]
    filters_unreadable: std::sync::atomic::AtomicBool,
}

impl CatalogControlVisibilityProvider {
    pub fn new() -> Self {
        Self::default()
    }

    /// Stand in for a bound scope that cannot say which names the model
    /// holds (its own state is unreadable while its owner still answers).
    #[cfg(test)]
    fn set_visible_names_unreadable_for_test(&self, unreadable: bool) {
        self.visible_names_unreadable
            .store(unreadable, std::sync::atomic::Ordering::SeqCst);
    }

    /// Stand in for a bound scope whose filter predicate cannot answer.
    #[cfg(test)]
    fn set_filters_unreadable_for_test(&self, unreadable: bool) {
        self.filters_unreadable
            .store(unreadable, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn set_scope(&self, scope: ToolScope) {
        let mut guard = self
            .scope
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *guard = Some(scope);
    }

    #[cfg(test)]
    fn set_visibility_state_for_test(&self, visibility_state: SessionToolVisibilityState) {
        let mut guard = self
            .visibility_state_override
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *guard = Some(visibility_state);
    }

    /// Resolve the visibility facts the deferred-load path needs from a *bound*
    /// scope authority. Returns [`NoToolScope`] when no scope is present so the
    /// caller fails closed instead of serving default visibility.
    ///
    /// In tests, an injected `visibility_state_override` stands in for a bound
    /// scope (the override exists precisely to exercise control-plane behavior
    /// without a live scope), so its presence resolves successfully.
    fn resolve_load_visibility(&self) -> Result<ResolvedLoadVisibility, NoToolScope> {
        #[cfg(test)]
        if let Some(visibility_state) = self
            .visibility_state_override
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
        {
            return Ok(ResolvedLoadVisibility {
                visibility_state,
                visible_tool_names: BTreeSet::new(),
            });
        }

        let guard = self
            .scope
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let scope = guard.as_ref().ok_or(NoToolScope)?;
        // A bound scope whose visibility cannot be projected is treated as no
        // authoritative visibility (fail closed), never as default visibility.
        let visibility_state = scope.visibility_state().map_err(|_| NoToolScope)?;
        // A scope that cannot say which names the model holds is unreadable,
        // not a scope whose model holds none: fail closed here, before any
        // entry is read, so the answer does not depend on the request.
        let visible_tool_names = scope.visible_tool_names().map_err(|_| NoToolScope);
        #[cfg(test)]
        let visible_tool_names = if self
            .visible_names_unreadable
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            Err(NoToolScope)
        } else {
            visible_tool_names
        };
        let visible_tool_names = visible_tool_names?;
        Ok(ResolvedLoadVisibility {
            visibility_state,
            visible_tool_names,
        })
    }

    /// Ask the bound scope authority whether staged session filters allow a
    /// name. Fail-closed: an absent scope or a scope projection failure yields
    /// [`NoToolScope`] so callers refuse instead of composing a local fallback
    /// filter — the control plane is never a fallback visibility authority.
    ///
    /// In tests, an injected `visibility_state_override` stands in for a bound
    /// scope, so the filter is composed from that injected state.
    fn staged_session_filters_allow_name(&self, name: &str) -> Result<bool, NoToolScope> {
        #[cfg(test)]
        if self
            .filters_unreadable
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return Err(NoToolScope);
        }
        #[cfg(test)]
        if let Some(visibility_state) = self
            .visibility_state_override
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
        {
            return Ok(ToolScope::compose(&[
                visibility_state.inherited_base_filter,
                visibility_state.staged_filter,
            ])
            .allows(name));
        }
        let guard = self
            .scope
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let scope = guard.as_ref().ok_or(NoToolScope)?;
        scope
            .staged_session_filters_allow_name(name)
            .map_err(|_| NoToolScope)
    }
}

pub struct CatalogControlDispatcher {
    session_dispatcher: Arc<dyn AgentToolDispatcher>,
    visibility_provider: Arc<CatalogControlVisibilityProvider>,
    tools: Arc<[Arc<ToolDef>]>,
    /// This instance's ephemeral execution owner identity: it binds the
    /// session dispatcher and visibility provider it retains, so a plan
    /// resolved by another instance never dispatches here.
    execution_authority_id: u64,
}

impl CatalogControlDispatcher {
    pub fn mode_for(session_dispatcher: &dyn AgentToolDispatcher) -> ToolCatalogMode {
        select_tool_catalog_mode(session_dispatcher)
    }

    pub fn should_enable_for(session_dispatcher: &dyn AgentToolDispatcher) -> bool {
        matches!(
            Self::mode_for(session_dispatcher),
            ToolCatalogMode::Deferred
        )
    }

    pub fn should_compose_for(session_dispatcher: &dyn AgentToolDispatcher) -> bool {
        if session_dispatcher.tool_catalog_capabilities().exact_catalog {
            let catalog = session_dispatcher.tool_catalog();
            if catalog.iter().any(|entry| {
                entry.plane == ToolPlaneClass::Control
                    && (entry.tool.name == SEARCH_TOOL_NAME || entry.tool.name == LOAD_TOOL_NAME)
            }) {
                return false;
            }
        }
        should_compose_tool_catalog_control_plane(session_dispatcher)
    }

    /// Whether a session built on `session_dispatcher` starts with the
    /// control tools in its model requests, so its system prompt should say
    /// how to use them: the control plane is composed for it, and a source
    /// needs the control tools in every request
    /// ([`meerkat_core::ToolCatalogControlPlane::Always`]) or the catalog
    /// starts deferred (a source is connecting, or its deferrable tools are
    /// over the byte budget). A session that starts inline gets no such
    /// line; when it later defers, the notice that names its deferred tools
    /// says how to load them.
    pub fn should_announce_for(session_dispatcher: &dyn AgentToolDispatcher) -> bool {
        Self::should_compose_for(session_dispatcher)
            && (session_dispatcher.tool_catalog_control_plane()
                == meerkat_core::ToolCatalogControlPlane::Always
                || Self::should_enable_for(session_dispatcher))
    }

    pub fn new(
        session_dispatcher: Arc<dyn AgentToolDispatcher>,
        visibility_provider: Arc<CatalogControlVisibilityProvider>,
    ) -> Self {
        Self {
            session_dispatcher,
            visibility_provider,
            tools: vec![Arc::new(search_tool_def()), Arc::new(load_tool_def())].into(),
            execution_authority_id: NEXT_CATALOG_CONTROL_AUTHORITY_ID
                .fetch_add(1, Ordering::Relaxed),
        }
    }

    fn execution_authority_key(&self) -> String {
        format!("tool-catalog-control:{}", self.execution_authority_id)
    }

    fn catalog_operation(tool_name: &str) -> Option<meerkat_core::CatalogOperation> {
        match tool_name {
            SEARCH_TOOL_NAME => Some(meerkat_core::CatalogOperation::Search),
            LOAD_TOOL_NAME => Some(meerkat_core::CatalogOperation::Load),
            _ => None,
        }
    }

    /// The plan was resolved by this instance for exactly this catalog
    /// operation, and its binding is still current.
    fn owner_witness_matches(
        &self,
        call: ToolCallView<'_>,
        plan: &meerkat_core::ResolvedToolExecutionPlan,
    ) -> bool {
        let Some(operation) = Self::catalog_operation(call.name) else {
            return false;
        };
        let Ok(live) = self.execution_binding_fingerprint(call.name) else {
            return false;
        };
        plan.owner_witness(&self.execution_authority_key())
            .is_some_and(|witness| {
                witness.owner_key() == call.name
                    && witness.binding_fingerprint() == &live
                    && witness.operation_target()
                        == Some(&meerkat_core::ToolOperationTarget::CatalogControl { operation })
            })
    }

    fn exact_session_catalog(&self) -> Option<Arc<[ToolCatalogEntry]>> {
        if !self
            .session_dispatcher
            .tool_catalog_capabilities()
            .exact_catalog
        {
            return None;
        }
        Some(
            self.session_dispatcher
                .tool_catalog()
                .iter()
                .filter(|entry| entry.tool.audience.allows_model())
                .cloned()
                .collect::<Vec<_>>()
                .into(),
        )
    }

    fn request_witness_matches_entry(
        witness: Option<&ToolVisibilityWitness>,
        provenance: &ToolProvenance,
        tool: &ToolDef,
    ) -> bool {
        let Some(witness) = witness else {
            return false;
        };
        let Some(witness_provenance) = witness.last_seen_provenance.as_ref() else {
            return false;
        };
        witness_provenance == provenance && Some(witness_provenance) == tool.provenance.as_ref()
    }

    fn deferred_authority_for_entry(
        provenance: &ToolProvenance,
        tool: &ToolDef,
    ) -> Option<ToolVisibilityWitness> {
        let tool_provenance = tool.provenance.as_ref()?;
        if tool_provenance != provenance {
            return None;
        }
        Some(ToolVisibilityWitness {
            last_seen_provenance: Some(tool_provenance.clone()),
        })
    }

    fn search_results(&self, args: SearchArgs) -> SearchResponse {
        let Some(catalog) = self.exact_session_catalog() else {
            return SearchResponse {
                catalog_exact: false,
                total_matches: 0,
                results: Vec::new(),
                pending_sources: Vec::new(),
                empty_result_status: None,
            };
        };

        let limit = args.limit.unwrap_or(10).clamp(1, 50);
        let query = args.query.unwrap_or_default().to_lowercase();
        let pending_sources = self.session_dispatcher.pending_catalog_sources();

        // Fail closed: without a readable scope there is no telling which
        // entries the session may find, so the answer names none of them. It
        // says so at the result level, the same for every query.
        let scope_unreadable = || SearchResponse {
            catalog_exact: true,
            total_matches: 0,
            results: Vec::new(),
            pending_sources: pending_sources.iter().cloned().collect(),
            empty_result_status: Some(SearchVisibilityStatus::TemporarilyUnavailable),
        };
        let Ok(ResolvedLoadVisibility {
            visible_tool_names, ..
        }) = self.visibility_provider.resolve_load_visibility()
        else {
            return scope_unreadable();
        };
        let filters_unreadable = std::cell::Cell::new(false);

        let results = catalog
            .iter()
            .filter(|entry| entry.plane == ToolPlaneClass::Session)
            .filter(|entry| {
                matches!(
                    entry.deferred_eligibility,
                    ToolCatalogDeferredEligibility::DeferredEligible { .. }
                )
            })
            .filter(|entry| matches_query(entry, &query))
            .filter_map(|entry| {
                let currently_loaded = visible_tool_names.contains(entry.tool.name.as_str());
                let visibility_status = if currently_loaded {
                    SearchVisibilityStatus::Loaded
                } else {
                    match self
                        .visibility_provider
                        .staged_session_filters_allow_name(&entry.tool.name)
                    {
                        // The scope stopped answering for its filters: no
                        // row, and the whole answer fails closed below.
                        Err(NoToolScope) => {
                            filters_unreadable.set(true);
                            return None;
                        }
                        // The session's filters do not allow the tool, so it
                        // is not in this session's catalog, as it is not in
                        // the session's tool list: no row, and so no total
                        // and no limit slot that counts it.
                        Ok(false) => return None,
                        Ok(true) if !entry.currently_callable() => {
                            SearchVisibilityStatus::TemporarilyUnavailable
                        }
                        Ok(true) => SearchVisibilityStatus::Deferred,
                    }
                };

                Some(SearchResultItem {
                    name: entry.tool.name.clone().into(),
                    description: entry.tool.description.clone(),
                    currently_callable: currently_loaded,
                    visibility_status,
                })
            })
            .collect::<Vec<_>>();
        if filters_unreadable.get() {
            return scope_unreadable();
        }

        let empty_result_status = if results.is_empty() && !pending_sources.is_empty() {
            Some(SearchVisibilityStatus::PendingSource)
        } else {
            None
        };

        SearchResponse {
            catalog_exact: true,
            total_matches: results.len(),
            results: results.into_iter().take(limit).collect(),
            pending_sources: pending_sources.iter().cloned().collect(),
            empty_result_status,
        }
    }

    fn load_response(&self, args: LoadArgs) -> (LoadResponse, Option<SessionEffect>) {
        let Some(catalog) = self.exact_session_catalog() else {
            let resolutions = args.names.into_iter().map(unknown_key_resolution).collect();
            return (
                LoadResponse {
                    catalog_exact: false,
                    accepted_names: Vec::new(),
                    noop_names: Vec::new(),
                    resolutions,
                },
                None,
            );
        };

        // Fail closed: a deferred load requires a bound `ToolScope` to consult.
        // Without one there is no visibility authority, so we refuse every name
        // and emit no `RequestDeferredTools` effect rather than accepting loads
        // under default visibility.
        let Ok(ResolvedLoadVisibility {
            visibility_state,
            visible_tool_names,
        }) = self.visibility_provider.resolve_load_visibility()
        else {
            let resolutions = args
                .names
                .into_iter()
                .map(|name| ToolCatalogLoadResolution {
                    name,
                    accepted: false,
                    accepted_noop: false,
                    rejected_reason: Some(ToolCatalogLoadRejectedReason::TemporarilyUnavailable),
                })
                .collect();
            return (
                LoadResponse {
                    catalog_exact: true,
                    accepted_names: Vec::new(),
                    noop_names: Vec::new(),
                    resolutions,
                },
                None,
            );
        };
        let mut accepted_authorities = BTreeMap::new();
        let mut noop_names = BTreeSet::new();
        let mut resolutions = Vec::new();

        for name in args.names {
            let already_visible = visible_tool_names.contains(name.as_str());
            let filters_allow = self
                .visibility_provider
                .staged_session_filters_allow_name(&name);
            // The scope stopped answering for its filters: fail closed with
            // one answer for every name the model does not hold, whether the
            // catalog holds it or not.
            if !already_visible && filters_allow.is_err() {
                resolutions.push(ToolCatalogLoadResolution {
                    name,
                    accepted: false,
                    accepted_noop: false,
                    rejected_reason: Some(ToolCatalogLoadRejectedReason::TemporarilyUnavailable),
                });
                continue;
            }

            let Some(entry) = catalog
                .iter()
                .find(|entry| entry.plane == ToolPlaneClass::Session && entry.tool.name == name)
            else {
                resolutions.push(unknown_key_resolution(name));
                continue;
            };

            // A tool the session's filters do not allow is not in this
            // session's catalog, whatever kind of entry it is: its load
            // answers what a load of a name the catalog does not hold
            // answers, from the same constructor. A tool the model already
            // holds is not hidden from it and keeps its answer until the
            // boundary that removes it from the model's tool list.
            if !already_visible && matches!(filters_allow, Ok(false)) {
                resolutions.push(unknown_key_resolution(name));
                continue;
            }

            match &entry.deferred_eligibility {
                ToolCatalogDeferredEligibility::InlineOnly => {
                    resolutions.push(ToolCatalogLoadResolution {
                        name,
                        accepted: false,
                        accepted_noop: false,
                        rejected_reason: Some(ToolCatalogLoadRejectedReason::NotDeferredEligible),
                    });
                }
                ToolCatalogDeferredEligibility::DeferredEligible { provenance } => {
                    let Some(authority) =
                        Self::deferred_authority_for_entry(provenance, &entry.tool)
                    else {
                        resolutions.push(ToolCatalogLoadResolution {
                            name,
                            accepted: false,
                            accepted_noop: false,
                            rejected_reason: Some(
                                ToolCatalogLoadRejectedReason::TemporarilyUnavailable,
                            ),
                        });
                        continue;
                    };
                    let staged_or_accepted = visibility_state
                        .staged_requested_deferred_names
                        .contains(name.as_str())
                        || accepted_authorities.contains_key(&name);
                    let already_requested = staged_or_accepted
                        && (Self::request_witness_matches_entry(
                            visibility_state.requested_witnesses.get(name.as_str()),
                            provenance,
                            &entry.tool,
                        ) || Self::request_witness_matches_entry(
                            accepted_authorities.get(&name),
                            provenance,
                            &entry.tool,
                        ));
                    if already_requested || already_visible {
                        noop_names.insert(name.clone());
                        resolutions.push(ToolCatalogLoadResolution {
                            name,
                            accepted: true,
                            accepted_noop: true,
                            rejected_reason: None,
                        });
                        continue;
                    }
                    match filters_allow {
                        Ok(true) => {}
                        // Answered above for every tool the model does not
                        // hold, and a tool it holds is a no-op: kept so the
                        // match stays total and fails closed.
                        Ok(false) => {
                            resolutions.push(unknown_key_resolution(name));
                            continue;
                        }
                        // The scope authority could not project filter truth:
                        // fail closed, never compose a local fallback filter.
                        Err(NoToolScope) => {
                            resolutions.push(ToolCatalogLoadResolution {
                                name,
                                accepted: false,
                                accepted_noop: false,
                                rejected_reason: Some(
                                    ToolCatalogLoadRejectedReason::TemporarilyUnavailable,
                                ),
                            });
                            continue;
                        }
                    }
                    if !entry.currently_callable() {
                        resolutions.push(ToolCatalogLoadResolution {
                            name,
                            accepted: false,
                            accepted_noop: false,
                            rejected_reason: Some(
                                ToolCatalogLoadRejectedReason::TemporarilyUnavailable,
                            ),
                        });
                        continue;
                    }

                    accepted_authorities.insert(name.clone(), authority);
                    resolutions.push(ToolCatalogLoadResolution {
                        name,
                        accepted: true,
                        accepted_noop: false,
                        rejected_reason: None,
                    });
                }
            }
        }

        let accepted_names_vec = accepted_authorities.keys().cloned().collect::<Vec<_>>();
        let accepted_authorities = accepted_authorities
            .into_iter()
            .map(|(name, witness)| DeferredToolLoadAuthority::new(name, witness))
            .collect::<Vec<_>>();
        let effect =
            (!accepted_authorities.is_empty()).then_some(SessionEffect::RequestDeferredTools {
                authorities: accepted_authorities,
            });

        (
            LoadResponse {
                catalog_exact: true,
                accepted_names: accepted_names_vec,
                noop_names: noop_names.into_iter().collect(),
                resolutions,
            },
            effect,
        )
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl AgentToolDispatcher for CatalogControlDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::clone(&self.tools)
    }

    fn tool_mutation_class(&self, tool_name: &str) -> meerkat_core::ToolMutationClass {
        match tool_name {
            // Loading changes session visibility only, not the underlying tool's
            // execution authority or any external state.
            SEARCH_TOOL_NAME | LOAD_TOOL_NAME => meerkat_core::ToolMutationClass::ReadOnly,
            _ => meerkat_core::ToolMutationClass::Unknown,
        }
    }

    fn tool_catalog_capabilities(&self) -> ToolCatalogCapabilities {
        ToolCatalogCapabilities {
            exact_catalog: true,
            may_require_catalog_control_plane: false,
        }
    }

    fn tool_catalog(&self) -> Arc<[ToolCatalogEntry]> {
        self.tools
            .iter()
            .map(|tool| ToolCatalogEntry::control_inline(Arc::clone(tool), true))
            .collect::<Vec<_>>()
            .into()
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
            .find(|entry| entry.tool.name == tool_name)
            .ok_or_else(|| meerkat_core::ToolExecutionResolutionError::NotFound {
                tool_name: tool_name.to_string(),
            })?;
        Ok(
            meerkat_core::ephemeral_tool_catalog_binding_fingerprint(entry)
                .with_live_authority(self.execution_authority_id as usize, 0),
        )
    }

    fn resolve_execution_plan(
        &self,
        call: ToolCallView<'_>,
        _dispatch_context: &meerkat_core::ToolDispatchContext,
        resolution_context: &meerkat_core::ToolExecutionResolutionContext,
    ) -> Result<meerkat_core::ResolvedToolExecutionPlan, meerkat_core::ToolExecutionResolutionError>
    {
        let catalog = self.tool_catalog();
        let entry = catalog
            .iter()
            .find(|entry| entry.tool.name == call.name)
            .ok_or_else(|| meerkat_core::ToolExecutionResolutionError::NotFound {
                tool_name: call.name.to_string(),
            })?;
        if let Some(reason) = entry.callability.unavailable_reason() {
            return Err(meerkat_core::ToolExecutionResolutionError::Unavailable {
                tool_name: call.name.to_string(),
                reason,
            });
        }
        let operation = Self::catalog_operation(call.name).ok_or_else(|| {
            meerkat_core::ToolExecutionResolutionError::NotFound {
                tool_name: call.name.to_string(),
            }
        })?;
        // The typed catalog fact, produced and revalidated at dispatch only
        // by this owner: the operation policy owner classifies catalog reads
        // from it, never from a name.
        let witness = meerkat_core::ToolExecutionOwnerWitness::new(
            self.execution_authority_key(),
            call.name,
            self.execution_binding_fingerprint(call.name)?,
        )?
        .with_operation_target(meerkat_core::ToolOperationTarget::CatalogControl { operation });
        entry
            .execution
            .resolve_default(resolution_context.deadlines().clone())?
            .with_owner_witness(witness)
    }

    /// A resolved plan runs only when this instance resolved it for exactly
    /// this catalog operation; the inherited Fast lowering validates no
    /// leaf owner.
    async fn dispatch_resolved_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &meerkat_core::ToolDispatchContext,
        plan: &meerkat_core::ResolvedToolExecutionPlan,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        if !self.owner_witness_matches(call, plan) {
            return Err(ToolError::unavailable(
                call.name,
                meerkat_core::ToolUnavailableReason::ExecutionOwnerChanged,
            ));
        }
        match plan.mode() {
            meerkat_core::ToolExecutionMode::Fast => {
                self.dispatch_with_context(call, context).await
            }
            meerkat_core::ToolExecutionMode::Streaming
            | meerkat_core::ToolExecutionMode::Detached => Err(ToolError::unavailable(
                call.name,
                meerkat_core::ToolUnavailableReason::ExecutionModeOwnerUnavailable,
            )),
        }
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        match call.name {
            SEARCH_TOOL_NAME => {
                let args =
                    call.parse_args::<SearchArgs>()
                        .map_err(|err| ToolError::InvalidArguments {
                            name: call.name.into(),
                            reason: err.to_string(),
                        })?;
                // Serialization failure propagates as a typed ToolError; a
                // structured success must never collapse into an empty
                // successful result.
                let content = serde_json::to_string(&self.search_results(args)).map_err(|err| {
                    ToolError::ExecutionFailed {
                        message: format!("failed to serialize tool_catalog_search result: {err}"),
                    }
                })?;
                let result = ToolResult::new(call.id.to_string(), content, false);
                Ok(ToolDispatchOutcome::sync_result(result))
            }
            LOAD_TOOL_NAME => {
                let args =
                    call.parse_args::<LoadArgs>()
                        .map_err(|err| ToolError::InvalidArguments {
                            name: call.name.into(),
                            reason: err.to_string(),
                        })?;
                let (response, effect) = self.load_response(args);
                // Every name the catalog accepted, newly requested or
                // requested or loaded before: typed data for the owner of
                // the boundary that follows this call.
                let accepted_names = response
                    .accepted_names
                    .iter()
                    .chain(response.noop_names.iter())
                    .map(|name| meerkat_core::ToolName::from(name.as_str()))
                    .collect::<Vec<_>>();
                // Serialization failure propagates as a typed ToolError; a
                // structured success must never collapse into an empty
                // successful result.
                let content =
                    serde_json::to_string(&response).map_err(|err| ToolError::ExecutionFailed {
                        message: format!("failed to serialize tool_catalog_load result: {err}"),
                    })?;
                let mut outcome = ToolDispatchOutcome::sync_result(ToolResult::new(
                    call.id.to_string(),
                    content,
                    false,
                ));
                if let Some(effect) = effect {
                    outcome.session_effects.push(effect);
                }
                if !accepted_names.is_empty() {
                    outcome = outcome.with_deferred_tool_load(
                        meerkat_core::DeferredToolLoad::requested(accepted_names),
                    );
                }
                Ok(outcome)
            }
            _ => Err(ToolError::NotFound {
                name: call.name.into(),
            }),
        }
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct SearchArgs {
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    #[schemars(range(min = 1, max = 50))]
    limit: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct LoadArgs {
    #[schemars(length(min = 1))]
    names: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct SearchResponse {
    catalog_exact: bool,
    total_matches: usize,
    results: Vec<SearchResultItem>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pending_sources: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    empty_result_status: Option<SearchVisibilityStatus>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct SearchResultItem {
    name: String,
    description: String,
    currently_callable: bool,
    visibility_status: SearchVisibilityStatus,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum SearchVisibilityStatus {
    Loaded,
    Deferred,
    PendingSource,
    TemporarilyUnavailable,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct LoadResponse {
    catalog_exact: bool,
    accepted_names: Vec<String>,
    noop_names: Vec<String>,
    resolutions: Vec<ToolCatalogLoadResolution>,
}

/// The load answer for a name the session's catalog does not hold: a name no
/// source provides, and a tool the session's filters do not allow. Both come
/// from this one constructor, so the answer cannot tell them apart.
fn unknown_key_resolution(name: String) -> ToolCatalogLoadResolution {
    ToolCatalogLoadResolution {
        name,
        accepted: false,
        accepted_noop: false,
        rejected_reason: Some(ToolCatalogLoadRejectedReason::UnknownKey),
    }
}

fn matches_query(entry: &ToolCatalogEntry, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }

    let haystack = format!(
        "{} {}",
        entry.tool.name.to_lowercase(),
        entry.tool.description.to_lowercase()
    );
    haystack.contains(query)
}

fn control_provenance() -> Option<ToolProvenance> {
    Some(ToolProvenance {
        kind: ToolSourceKind::Builtin,
        source_id: CONTROL_SOURCE_ID.into(),
        artifact: None,
    })
}

fn search_tool_def() -> ToolDef {
    ToolDef {
        audience: Default::default(),
        name: SEARCH_TOOL_NAME.into(),
        description:
            "Search the deferred session tool catalog by name or description. Returns deferred-eligible session tools without loading them."
                .to_string(),
        input_schema: schema_for::<SearchArgs>(),
        provenance: control_provenance(),
    }
}

fn load_tool_def() -> ToolDef {
    ToolDef {
        audience: Default::default(),
        name: LOAD_TOOL_NAME.into(),
        description:
            "Load one or more deferred session tools by canonical tool name. An accepted tool can be called from your next step, unless the result's `availability` field, or the tool's entry in `tool_availability`, says it cannot be called in this conversation yet."
                .to_string(),
        input_schema: schema_for::<LoadArgs>(),
        provenance: control_provenance(),
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use meerkat_core::ToolFilter;
    use meerkat_core::types::ToolProvenance;
    use serde_json::json;
    use serde_json::value::RawValue;
    use std::collections::{BTreeMap, HashSet};

    struct ExactCatalogDispatcher {
        tools: Arc<[Arc<ToolDef>]>,
        catalog: Arc<[ToolCatalogEntry]>,
        pending_sources: Arc<[String]>,
        may_require_control_plane: bool,
    }

    struct HybridSessionDispatcher {
        catalog: Arc<[ToolCatalogEntry]>,
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl AgentToolDispatcher for ExactCatalogDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            Arc::clone(&self.tools)
        }

        fn tool_catalog_capabilities(&self) -> ToolCatalogCapabilities {
            ToolCatalogCapabilities {
                exact_catalog: true,
                may_require_catalog_control_plane: self.may_require_control_plane,
            }
        }

        fn tool_catalog(&self) -> Arc<[ToolCatalogEntry]> {
            Arc::clone(&self.catalog)
        }

        fn pending_catalog_sources(&self) -> Arc<[String]> {
            Arc::clone(&self.pending_sources)
        }

        async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
            Err(ToolError::NotFound {
                name: call.name.into(),
            })
        }
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl AgentToolDispatcher for HybridSessionDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            self.catalog
                .iter()
                .map(|entry| Arc::clone(&entry.tool))
                .collect::<Vec<_>>()
                .into()
        }

        fn tool_catalog_capabilities(&self) -> ToolCatalogCapabilities {
            ToolCatalogCapabilities {
                exact_catalog: true,
                may_require_catalog_control_plane: false,
            }
        }

        fn tool_catalog(&self) -> Arc<[ToolCatalogEntry]> {
            Arc::clone(&self.catalog)
        }

        fn resolve_execution_plan(
            &self,
            call: ToolCallView<'_>,
            _dispatch_context: &meerkat_core::ToolDispatchContext,
            resolution_context: &meerkat_core::ToolExecutionResolutionContext,
        ) -> Result<
            meerkat_core::ResolvedToolExecutionPlan,
            meerkat_core::ToolExecutionResolutionError,
        > {
            let args: serde_json::Value =
                serde_json::from_str(call.args.get()).map_err(|error| {
                    meerkat_core::ToolExecutionResolutionError::InvalidArguments {
                        tool_name: call.name.to_string(),
                        reason: error.to_string(),
                    }
                })?;
            let mode = if args["detached"] == true {
                meerkat_core::ToolExecutionMode::Detached
            } else {
                meerkat_core::ToolExecutionMode::Fast
            };
            self.catalog[0]
                .execution
                .resolve(mode, resolution_context.deadlines().clone())
                .map_err(Into::into)
        }

        async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
            Err(ToolError::not_found(call.name))
        }
    }

    fn callback_provenance(source_id: &str) -> ToolProvenance {
        ToolProvenance {
            kind: ToolSourceKind::Callback,
            source_id: source_id.into(),
            artifact: None,
        }
    }

    fn hybrid_session_dispatcher() -> Arc<dyn AgentToolDispatcher> {
        let detached = meerkat_core::DetachedToolExecutionPolicy::new(
            meerkat_core::RunnerIdentity::new("test.catalog-session", "v1")
                .expect("runner declaration"),
            meerkat_core::RestartClass::NonResumable,
            meerkat_core::IdempotencyScope::ToolCall,
            std::time::Duration::from_secs(10),
        )
        .expect("detached policy");
        let contract = meerkat_core::ToolExecutionContract::new(
            BTreeSet::from([
                meerkat_core::ToolExecutionMode::Fast,
                meerkat_core::ToolExecutionMode::Detached,
            ]),
            meerkat_core::ToolExecutionMode::Fast,
            None,
            Some(detached),
        )
        .expect("hybrid execution contract");
        Arc::new(HybridSessionDispatcher {
            catalog: vec![
                ToolCatalogEntry::session_inline(
                    session_tool("hybrid_session_tool", "Hybrid session tool"),
                    true,
                )
                .with_execution_contract(contract),
            ]
            .into(),
        })
    }

    #[test]
    fn control_dispatcher_resolves_only_control_owned_names() {
        let control = CatalogControlDispatcher::new(
            hybrid_session_dispatcher(),
            Arc::new(CatalogControlVisibilityProvider::new()),
        );
        let resolution = meerkat_core::ToolExecutionResolutionContext::new(
            meerkat_core::ToolDeadlineChain::new(vec![
                meerkat_core::ToolDeadlineContributor::finite(
                    meerkat_core::ToolDeadlineOwner::CoreToolDispatch,
                    std::time::Duration::from_mins(10),
                ),
            ])
            .expect("deadline chain"),
        );
        let raw = RawValue::from_string(r#"{"detached":true}"#.to_string())
            .expect("raw control arguments");

        let control_plan = control
            .resolve_execution_plan(
                ToolCallView {
                    id: "control-plan",
                    name: LOAD_TOOL_NAME,
                    args: raw.as_ref(),
                },
                &meerkat_core::ToolDispatchContext::default(),
                &resolution,
            )
            .expect("control-owned name should resolve locally");
        assert_eq!(
            control_plan.mode(),
            meerkat_core::ToolExecutionMode::Fast,
            "control arguments must not be interpreted by the hybrid session resolver"
        );
        // The plan carries this owner's typed catalog target, the only
        // classification an operation policy owner may use.
        let targets: Vec<_> = control_plan
            .owner_witnesses()
            .iter()
            .filter_map(|witness| witness.operation_target())
            .collect();
        assert_eq!(
            targets,
            vec![&meerkat_core::ToolOperationTarget::CatalogControl {
                operation: meerkat_core::CatalogOperation::Load,
            }]
        );
        let search_plan = control
            .resolve_execution_plan(
                ToolCallView {
                    id: "control-search-plan",
                    name: SEARCH_TOOL_NAME,
                    args: raw.as_ref(),
                },
                &meerkat_core::ToolDispatchContext::default(),
                &resolution,
            )
            .expect("control-owned search should resolve locally");
        let search_targets: Vec<_> = search_plan
            .owner_witnesses()
            .iter()
            .filter_map(|witness| witness.operation_target())
            .collect();
        assert_eq!(
            search_targets,
            vec![&meerkat_core::ToolOperationTarget::CatalogControl {
                operation: meerkat_core::CatalogOperation::Search,
            }],
            "a search is never tagged as a load"
        );

        let session_error = control
            .resolve_execution_plan(
                ToolCallView {
                    id: "session-plan",
                    name: "hybrid_session_tool",
                    args: raw.as_ref(),
                },
                &meerkat_core::ToolDispatchContext::default(),
                &resolution,
            )
            .expect_err("the control dispatcher does not dispatch session-owned names");
        assert!(matches!(
            session_error,
            meerkat_core::ToolExecutionResolutionError::NotFound { .. }
        ));
    }

    #[tokio::test]
    async fn catalog_plans_dispatch_only_through_their_resolving_instance_and_operation() {
        let deferred = session_tool("deferred_tool", "Deferred tool");
        let tools: Arc<[Arc<ToolDef>]> = vec![Arc::clone(&deferred)].into();
        let session: Arc<dyn AgentToolDispatcher> = Arc::new(ExactCatalogDispatcher {
            tools: Arc::clone(&tools),
            catalog: vec![ToolCatalogEntry::session_deferred(
                Arc::clone(&deferred),
                true,
                callback_provenance("test"),
            )]
            .into(),
            pending_sources: Arc::from([]),
            may_require_control_plane: false,
        });
        let visibility_provider = Arc::new(CatalogControlVisibilityProvider::new());
        visibility_provider.set_scope(generated_visibility_scope(
            Arc::clone(&tools),
            tools.iter().map(|tool| tool.name.clone()).collect(),
        ));
        let control = Arc::new(CatalogControlDispatcher::new(
            Arc::clone(&session),
            Arc::clone(&visibility_provider),
        ));
        // An equal second instance over the same composition.
        let other = CatalogControlDispatcher::new(Arc::clone(&session), visibility_provider);
        // The factory's composition: session tools, then the control plane.
        let root: Arc<dyn AgentToolDispatcher> =
            Arc::new(meerkat_core::DynamicToolComposite::new(vec![
                Arc::clone(&session),
                control.clone() as Arc<dyn AgentToolDispatcher>,
            ]));
        let context = meerkat_core::ToolDispatchContext::default();
        let resolution = meerkat_core::ToolExecutionResolutionContext::new(
            meerkat_core::ToolDeadlineChain::new(vec![
                meerkat_core::ToolDeadlineContributor::finite(
                    meerkat_core::ToolDeadlineOwner::CoreToolDispatch,
                    std::time::Duration::from_mins(10),
                ),
            ])
            .expect("deadline chain"),
        );
        let search = search_call(SEARCH_TOOL_NAME, json!({"query": "deferred"}));
        let load = search_call(LOAD_TOOL_NAME, json!({"names": ["deferred_tool"]}));
        let resolve = |call| {
            meerkat_core::resolve_tool_execution_plan_fenced(&root, call, &context, &resolution)
                .expect("root plan")
        };
        let catalog_targets = |plan: &meerkat_core::ResolvedToolExecutionPlan| {
            plan.owner_witnesses()
                .iter()
                .filter_map(|witness| witness.operation_target().cloned())
                .collect::<Vec<_>>()
        };

        // Genuine search and load, routed through the gateway and the root
        // fence, carry exactly their own operation and execute.
        let search_plan = resolve(search);
        assert_eq!(
            catalog_targets(&search_plan),
            vec![meerkat_core::ToolOperationTarget::CatalogControl {
                operation: meerkat_core::CatalogOperation::Search,
            }]
        );
        let found = meerkat_core::dispatch_tool_execution_plan_fenced(
            &root,
            search,
            &context,
            &search_plan,
        )
        .await
        .expect("genuine search");
        assert!(found.session_effects.is_empty());
        let load_plan = resolve(load);
        assert_eq!(
            catalog_targets(&load_plan),
            vec![meerkat_core::ToolOperationTarget::CatalogControl {
                operation: meerkat_core::CatalogOperation::Load,
            }]
        );
        let loaded =
            meerkat_core::dispatch_tool_execution_plan_fenced(&root, load, &context, &load_plan)
                .await
                .expect("genuine load");
        assert!(matches!(
            loaded.session_effects.as_slice(),
            [SessionEffect::RequestDeferredTools { .. }]
        ));

        let owner_changed =
            |result: Result<ToolDispatchOutcome, ToolError>, name: &str, label: &str| {
                assert_eq!(
                    result.expect_err(label),
                    ToolError::unavailable(
                        name,
                        meerkat_core::ToolUnavailableReason::ExecutionOwnerChanged,
                    ),
                    "{label}"
                );
            };
        // A plan resolved by another instance never dispatches here.
        owner_changed(
            other
                .dispatch_resolved_with_context(load, &context, &load_plan)
                .await,
            LOAD_TOOL_NAME,
            "load plan on a second instance",
        );
        // A search plan cannot run as a load (no load effect), nor the
        // reverse.
        owner_changed(
            control
                .dispatch_resolved_with_context(load, &context, &search_plan)
                .await,
            LOAD_TOOL_NAME,
            "load call with a search plan",
        );
        owner_changed(
            control
                .dispatch_resolved_with_context(search, &context, &load_plan)
                .await,
            SEARCH_TOOL_NAME,
            "search call with a load plan",
        );
        // A plan without this owner's witness is refused.
        let bare = ToolCatalogEntry::control_inline(Arc::new(load_tool_def()), true)
            .execution
            .resolve_default(resolution.deadlines().clone())
            .expect("bare plan");
        owner_changed(
            control
                .dispatch_resolved_with_context(load, &context, &bare)
                .await,
            LOAD_TOOL_NAME,
            "load without a witness",
        );
    }

    fn session_tool(name: &str, description: &str) -> Arc<ToolDef> {
        Arc::new(ToolDef {
            audience: Default::default(),
            name: name.into(),
            description: description.to_string(),
            input_schema: json!({ "type": "object" }),
            provenance: Some(ToolProvenance {
                kind: ToolSourceKind::Callback,
                source_id: "test".into(),
                artifact: None,
            }),
        })
    }

    /// A description that alone puts a deferred-eligible tool over the
    /// deferred catalog's byte budget, so a fixture with it selects the
    /// deferred mode.
    fn over_catalog_budget(text: &str) -> String {
        format!(
            "{text} {}",
            "x".repeat(meerkat_core::tool_catalog::DEFERRED_CATALOG_SCHEMA_VOLUME_THRESHOLD)
        )
    }

    fn session_tool_without_provenance(name: &str, description: &str) -> Arc<ToolDef> {
        Arc::new(ToolDef {
            audience: Default::default(),
            name: name.into(),
            description: description.to_string(),
            input_schema: json!({ "type": "object" }),
            provenance: None,
        })
    }

    fn search_call(name: &'static str, args: serde_json::Value) -> ToolCallView<'static> {
        let raw = RawValue::from_string(args.to_string()).unwrap();
        let raw = Box::leak(raw);
        ToolCallView {
            id: "toolu_search",
            name,
            args: raw,
        }
    }

    fn generated_visibility_scope(
        tools: Arc<[Arc<ToolDef>]>,
        deferred_tool_names: HashSet<meerkat_core::ToolName>,
    ) -> ToolScope {
        generated_visibility_scope_for_model(tools, deferred_tool_names, None, &ToolFilter::All)
    }

    /// A generated visibility scope whose owner is hydrated with a model
    /// profile and the capability base filter that profile implies.
    fn generated_visibility_scope_for_model(
        tools: Arc<[Arc<ToolDef>]>,
        deferred_tool_names: HashSet<meerkat_core::ToolName>,
        model_profile: Option<&meerkat_core::model_profile::ModelProfile>,
        capability_base_filter: &ToolFilter,
    ) -> ToolScope {
        let identity = meerkat_core::SessionLlmIdentity {
            model: "test-model".to_string(),
            provider: meerkat_core::Provider::Anthropic,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        };
        let owner = meerkat_runtime::standalone_tool_visibility_owner(
            &meerkat_core::SessionId::new(),
            &identity,
            model_profile,
            capability_base_filter,
        )
        .expect("generated standalone visibility owner should initialize");
        ToolScope::new_with_visibility_owner(tools, HashSet::new(), deferred_tool_names, owner)
            .expect("generated visibility test owner should accept catalog authority")
    }

    #[tokio::test]
    async fn read_only_gate_allows_catalog_discovery_and_load_without_authorizing_mutation() {
        use meerkat_core::ops::ToolAccessPolicy;
        use meerkat_core::{
            DynamicToolComposite, ExecutionPolicyGatedDispatcher, ToolExecutionPolicy,
            ToolMutationClass,
        };
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct DeclaredSessionDispatcher {
            catalog: ExactCatalogDispatcher,
            calls: AtomicUsize,
        }

        #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
        #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
        impl AgentToolDispatcher for DeclaredSessionDispatcher {
            fn tools(&self) -> Arc<[Arc<ToolDef>]> {
                self.catalog.tools()
            }

            fn tool_catalog_capabilities(&self) -> ToolCatalogCapabilities {
                self.catalog.tool_catalog_capabilities()
            }

            fn tool_catalog(&self) -> Arc<[ToolCatalogEntry]> {
                self.catalog.tool_catalog()
            }

            fn tool_mutation_class(&self, name: &str) -> ToolMutationClass {
                match name {
                    "workspace_read" => ToolMutationClass::ReadOnly,
                    "workspace_apply" => ToolMutationClass::Mutating,
                    _ => ToolMutationClass::Unknown,
                }
            }

            async fn dispatch(
                &self,
                call: ToolCallView<'_>,
            ) -> Result<ToolDispatchOutcome, ToolError> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                Ok(ToolDispatchOutcome::sync_result(ToolResult::new(
                    call.id.to_string(),
                    "session operation executed".to_string(),
                    false,
                )))
            }
        }

        let tools: Arc<[Arc<ToolDef>]> = vec![
            session_tool("workspace_read", "Read a workspace"),
            session_tool("workspace_apply", "Change a workspace"),
        ]
        .into();
        let session = Arc::new(DeclaredSessionDispatcher {
            catalog: ExactCatalogDispatcher {
                tools: Arc::clone(&tools),
                catalog: tools
                    .iter()
                    .map(|tool| {
                        ToolCatalogEntry::session_deferred(
                            Arc::clone(tool),
                            true,
                            callback_provenance("test"),
                        )
                    })
                    .collect::<Vec<_>>()
                    .into(),
                pending_sources: Arc::from([]),
                may_require_control_plane: false,
            },
            calls: AtomicUsize::new(0),
        });
        let scope = generated_visibility_scope(
            Arc::clone(&tools),
            tools.iter().map(|tool| tool.name.clone()).collect(),
        );
        let visibility_provider = Arc::new(CatalogControlVisibilityProvider::new());
        visibility_provider.set_scope(scope.clone());
        let control = Arc::new(CatalogControlDispatcher::new(
            session.clone(),
            visibility_provider,
        ));
        for name in ["workspace_read", "workspace_apply", "unknown"] {
            assert_eq!(
                control.tool_mutation_class(name),
                ToolMutationClass::Unknown
            );
        }
        let composed: Arc<dyn AgentToolDispatcher> =
            Arc::new(DynamicToolComposite::new(vec![control, session.clone()]));
        let gated = ExecutionPolicyGatedDispatcher::new(
            composed,
            ToolExecutionPolicy::resolve(ToolAccessPolicy::ReadOnly).unwrap(),
        );

        let search = gated
            .dispatch(search_call(SEARCH_TOOL_NAME, json!({"query": "workspace"})))
            .await
            .expect("read-only agents must be able to discover deferred tools");
        assert!(!search.result.is_error);
        assert!(search.session_effects.is_empty());
        let found: SearchResponse = serde_json::from_str(&search.result.text_content()).unwrap();
        assert!(found.catalog_exact);
        assert_eq!(found.total_matches, 2);
        assert!(found.results.iter().all(|tool| !tool.currently_callable));
        assert!(scope.visible_tool_names().unwrap().is_empty());

        let loaded = gated
            .dispatch_with_context(
                search_call(
                    LOAD_TOOL_NAME,
                    json!({"names": ["workspace_read", "workspace_apply"]}),
                ),
                &meerkat_core::ToolDispatchContext::default(),
            )
            .await
            .expect("catalog load is a permitted session visibility operation");
        assert!(!loaded.result.is_error);
        let response: LoadResponse = serde_json::from_str(&loaded.result.text_content()).unwrap();
        assert!(response.catalog_exact);
        assert_eq!(
            response.accepted_names.into_iter().collect::<BTreeSet<_>>(),
            BTreeSet::from(["workspace_read".to_string(), "workspace_apply".to_string()])
        );
        assert!(response.noop_names.is_empty());
        let [SessionEffect::RequestDeferredTools { authorities }] =
            loaded.session_effects.as_slice()
        else {
            panic!("expected exactly one deferred-load effect");
        };
        assert_eq!(authorities.len(), 2);
        scope
            .add_requested_deferred_authorities(authorities)
            .unwrap();
        assert!(scope.visible_tool_names().unwrap().is_empty());
        scope.apply_staged(Arc::clone(&tools)).unwrap();
        assert_eq!(
            scope.visible_tool_names().unwrap(),
            tools.iter().map(|tool| tool.name.clone()).collect()
        );

        gated
            .dispatch(search_call("workspace_read", json!({})))
            .await
            .expect("the loaded read remains executable");
        let error = gated
            .dispatch(search_call("workspace_apply", json!({})))
            .await
            .expect_err("loading a mutating tool must not authorize its execution");
        assert_eq!(error, ToolError::access_denied("workspace_apply"));
        assert_eq!(session.calls.load(Ordering::SeqCst), 1);

        session
            .dispatch(search_call("workspace_apply", json!({})))
            .await
            .expect("the ungated fixture can execute the same mutating call");
        assert_eq!(session.calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn search_only_returns_deferred_eligible_session_entries() {
        let deferred = session_tool("deferred_mcp_tool", "Deferred MCP tool");
        let inline = session_tool("inline_tool", "Inline callback tool");
        let dispatcher = Arc::new(ExactCatalogDispatcher {
            tools: vec![Arc::clone(&deferred), Arc::clone(&inline)].into(),
            catalog: vec![
                ToolCatalogEntry::session_deferred(
                    Arc::clone(&deferred),
                    true,
                    callback_provenance("test"),
                ),
                ToolCatalogEntry::session_inline(Arc::clone(&inline), true),
            ]
            .into(),
            pending_sources: Arc::from([]),
            may_require_control_plane: false,
        });
        let visibility_provider = Arc::new(CatalogControlVisibilityProvider::new());
        let scope = generated_visibility_scope(
            vec![Arc::clone(&deferred), Arc::clone(&inline)].into(),
            ["deferred_mcp_tool".into()].into_iter().collect(),
        );
        visibility_provider.set_scope(scope);
        let control = CatalogControlDispatcher::new(dispatcher, visibility_provider);

        let outcome = control
            .dispatch(search_call(SEARCH_TOOL_NAME, json!({ "query": "mcp" })))
            .await
            .unwrap();
        let response: SearchResponse =
            serde_json::from_str(&outcome.result.text_content()).unwrap();

        assert!(response.catalog_exact);
        assert_eq!(response.total_matches, 1);
        assert_eq!(response.results[0].name, "deferred_mcp_tool");
        assert!(!response.results[0].currently_callable);
    }

    #[tokio::test]
    async fn app_only_tools_cannot_be_searched_or_loaded_into_model_catalog() {
        let model = session_tool("model_tool", "Visible model tool");
        let app = Arc::new(
            session_tool("app_secret", "Private app operation")
                .as_ref()
                .clone()
                .with_audience(meerkat_core::ToolAudience::App),
        );
        let tools: Arc<[Arc<ToolDef>]> = vec![Arc::clone(&model), Arc::clone(&app)].into();
        let dispatcher = Arc::new(ExactCatalogDispatcher {
            tools: Arc::clone(&tools),
            catalog: tools
                .iter()
                .map(|tool| {
                    ToolCatalogEntry::session_deferred(
                        Arc::clone(tool),
                        true,
                        callback_provenance("test"),
                    )
                })
                .collect::<Vec<_>>()
                .into(),
            pending_sources: Arc::from([]),
            may_require_control_plane: false,
        });
        let visibility = Arc::new(CatalogControlVisibilityProvider::new());
        let scope = generated_visibility_scope(
            tools,
            ["model_tool".into(), "app_secret".into()]
                .into_iter()
                .collect(),
        );
        visibility.set_scope(scope.clone());
        let control = CatalogControlDispatcher::new(dispatcher, visibility);
        let searched = control
            .dispatch(search_call(SEARCH_TOOL_NAME, json!({})))
            .await
            .unwrap();
        let searched: SearchResponse =
            serde_json::from_str(&searched.result.text_content()).unwrap();
        assert_eq!(
            searched
                .results
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            vec!["model_tool"]
        );
        let loaded = control
            .dispatch(search_call(
                LOAD_TOOL_NAME,
                json!({"names": ["app_secret"]}),
            ))
            .await
            .unwrap();
        assert!(loaded.session_effects.is_empty());
        let loaded: LoadResponse = serde_json::from_str(&loaded.result.text_content()).unwrap();
        assert_eq!(
            loaded.resolutions[0].rejected_reason,
            Some(ToolCatalogLoadRejectedReason::UnknownKey)
        );
        assert!(scope.visible_tool_names().unwrap().is_empty());
        assert_eq!(
            scope.app_visible_tool_names().unwrap(),
            vec![meerkat_core::ToolName::from("app_secret")]
        );
    }

    #[test]
    fn should_enable_requires_exact_catalog_with_deferred_session_entries() {
        let inline = session_tool("inline_tool", "Inline callback tool");
        let exact_inline_only = ExactCatalogDispatcher {
            tools: vec![Arc::clone(&inline)].into(),
            catalog: vec![ToolCatalogEntry::session_inline(Arc::clone(&inline), true)].into(),
            pending_sources: Arc::from([]),
            may_require_control_plane: false,
        };
        assert!(
            !CatalogControlDispatcher::should_enable_for(&exact_inline_only),
            "inline-only exact catalogs should not expose control-plane tools"
        );

        let deferred = session_tool(
            "deferred_mcp_tool",
            "Deferred MCP tool with enough surface area to justify catalog mode.",
        );
        let deferred_two = session_tool(
            "deferred_mcp_tool_two",
            &over_catalog_budget(
                "Another deferred MCP tool so the exact catalog crosses the byte budget.",
            ),
        );
        let exact_with_deferred = ExactCatalogDispatcher {
            tools: vec![Arc::clone(&deferred), Arc::clone(&deferred_two)].into(),
            catalog: vec![
                ToolCatalogEntry::session_deferred(
                    Arc::clone(&deferred),
                    true,
                    callback_provenance("test"),
                ),
                ToolCatalogEntry::session_deferred(
                    Arc::clone(&deferred_two),
                    true,
                    callback_provenance("test"),
                ),
            ]
            .into(),
            pending_sources: Arc::from([]),
            may_require_control_plane: false,
        };
        assert!(
            CatalogControlDispatcher::should_enable_for(&exact_with_deferred),
            "exact catalogs with deferred session entries should enable the control plane"
        );
    }

    #[test]
    fn should_enable_when_pending_sources_are_warming_without_catalog_entries_yet() {
        let pending_only = ExactCatalogDispatcher {
            tools: Arc::from([]),
            catalog: Arc::from([]),
            pending_sources: Arc::from(["pending-mcp".to_string()]),
            may_require_control_plane: false,
        };
        assert!(
            CatalogControlDispatcher::should_enable_for(&pending_only),
            "pending exact sources should keep the catalog control plane available"
        );
    }

    #[test]
    fn should_compose_for_dynamic_exact_dispatchers_even_before_threshold() {
        let deferred = session_tool("secret_lookup", "Deferred secret lookup");
        let below_threshold_dynamic = ExactCatalogDispatcher {
            tools: vec![Arc::clone(&deferred)].into(),
            catalog: vec![ToolCatalogEntry::session_deferred(
                Arc::clone(&deferred),
                true,
                callback_provenance("test"),
            )]
            .into(),
            pending_sources: Arc::from([]),
            may_require_control_plane: true,
        };

        assert!(
            CatalogControlDispatcher::should_compose_for(&below_threshold_dynamic),
            "dynamic exact dispatchers should pre-compose the control plane before adaptive mode flips"
        );
        assert!(
            !CatalogControlDispatcher::should_enable_for(&below_threshold_dynamic),
            "a single deferred tool should still stay inline until the adaptive threshold is crossed"
        );
    }

    #[test]
    fn should_compose_for_is_idempotent_when_control_plane_is_already_present() {
        let deferred = session_tool("deferred_mcp_tool", "Deferred MCP tool");
        let deferred_two = session_tool(
            "deferred_mcp_tool_two",
            &over_catalog_budget("Second deferred MCP tool"),
        );
        let search = Arc::new(search_tool_def());
        let load = Arc::new(load_tool_def());
        let already_composed = ExactCatalogDispatcher {
            tools: vec![
                Arc::clone(&deferred),
                Arc::clone(&deferred_two),
                Arc::clone(&search),
                Arc::clone(&load),
            ]
            .into(),
            catalog: vec![
                ToolCatalogEntry::session_deferred(
                    Arc::clone(&deferred),
                    true,
                    callback_provenance("test"),
                ),
                ToolCatalogEntry::session_deferred(
                    Arc::clone(&deferred_two),
                    true,
                    callback_provenance("test"),
                ),
                ToolCatalogEntry::control_inline(search, true),
                ToolCatalogEntry::control_inline(load, true),
            ]
            .into(),
            pending_sources: Arc::from([]),
            may_require_control_plane: false,
        };

        assert!(
            CatalogControlDispatcher::should_enable_for(&already_composed),
            "already-composed deferred catalogs should still project control tools"
        );
        assert!(
            !CatalogControlDispatcher::should_compose_for(&already_composed),
            "factory auto-hoisting must not add a second catalog control plane"
        );
    }

    #[tokio::test]
    async fn load_noops_for_already_requested_names_and_rejects_unknown_or_inline_entries() {
        let deferred = session_tool("deferred_mcp_tool", "Deferred MCP tool");
        let inline = session_tool("inline_tool", "Inline callback tool");
        let dispatcher = Arc::new(ExactCatalogDispatcher {
            tools: vec![Arc::clone(&deferred), Arc::clone(&inline)].into(),
            catalog: vec![
                ToolCatalogEntry::session_deferred(
                    Arc::clone(&deferred),
                    false,
                    callback_provenance("test"),
                ),
                ToolCatalogEntry::session_inline(Arc::clone(&inline), true),
            ]
            .into(),
            pending_sources: Arc::from([]),
            may_require_control_plane: false,
        });
        let visibility_provider = Arc::new(CatalogControlVisibilityProvider::new());
        let scope = generated_visibility_scope(
            vec![Arc::clone(&deferred), Arc::clone(&inline)].into(),
            ["deferred_mcp_tool".into()].into_iter().collect(),
        );
        scope
            .set_visibility_state(SessionToolVisibilityState {
                staged_requested_deferred_names: ["deferred_mcp_tool".into()].into_iter().collect(),
                staged_revision: 1,
                requested_witnesses: [(
                    "deferred_mcp_tool".into(),
                    ToolVisibilityWitness {
                        last_seen_provenance: deferred.provenance.clone(),
                    },
                )]
                .into_iter()
                .collect(),
                ..Default::default()
            })
            .unwrap();
        visibility_provider.set_scope(scope);

        let control = CatalogControlDispatcher::new(dispatcher, visibility_provider);
        let outcome = control
            .dispatch(search_call(
                LOAD_TOOL_NAME,
                json!({ "names": ["deferred_mcp_tool", "inline_tool", "missing_tool"] }),
            ))
            .await
            .unwrap();
        let response: LoadResponse = serde_json::from_str(&outcome.result.text_content()).unwrap();

        assert!(response.catalog_exact);
        assert!(response.accepted_names.is_empty());
        assert_eq!(response.noop_names, vec!["deferred_mcp_tool".to_string()]);
        assert_eq!(
            response.resolutions,
            vec![
                ToolCatalogLoadResolution {
                    name: "deferred_mcp_tool".into(),
                    accepted: true,
                    accepted_noop: true,
                    rejected_reason: None,
                },
                ToolCatalogLoadResolution {
                    name: "inline_tool".into(),
                    accepted: false,
                    accepted_noop: false,
                    rejected_reason: Some(ToolCatalogLoadRejectedReason::NotDeferredEligible),
                },
                ToolCatalogLoadResolution {
                    name: "missing_tool".into(),
                    accepted: false,
                    accepted_noop: false,
                    rejected_reason: Some(ToolCatalogLoadRejectedReason::UnknownKey),
                },
            ]
        );
        assert!(outcome.session_effects.is_empty());
    }

    /// The load reports, as typed data beside its result, every name the
    /// catalog accepted: a newly requested one, and one requested or loaded
    /// before, which carries no effect. A call that accepts nothing reports
    /// no load. The owner of the boundary after the call acts on this, so a
    /// repeated load is acted on too.
    #[tokio::test]
    async fn load_reports_the_accepted_names_as_a_typed_deferred_load() {
        let first = session_tool("deferred_first", "First deferred tool");
        let second = session_tool("deferred_second", "Second deferred tool");
        let dispatcher = Arc::new(ExactCatalogDispatcher {
            tools: vec![Arc::clone(&first), Arc::clone(&second)].into(),
            catalog: vec![
                ToolCatalogEntry::session_deferred(
                    Arc::clone(&first),
                    true,
                    callback_provenance("test"),
                ),
                ToolCatalogEntry::session_deferred(
                    Arc::clone(&second),
                    true,
                    callback_provenance("test"),
                ),
            ]
            .into(),
            pending_sources: Arc::from([]),
            may_require_control_plane: false,
        });
        let visibility_provider = Arc::new(CatalogControlVisibilityProvider::new());
        let scope = generated_visibility_scope(
            vec![Arc::clone(&first), Arc::clone(&second)].into(),
            ["deferred_first".into(), "deferred_second".into()]
                .into_iter()
                .collect(),
        );
        visibility_provider.set_scope(scope.clone());
        let control = CatalogControlDispatcher::new(dispatcher, visibility_provider);
        let load = |names: serde_json::Value| {
            control.dispatch(search_call(LOAD_TOOL_NAME, json!({ "names": names })))
        };

        let requested = load(json!(["deferred_first"])).await.unwrap();
        let reported = requested.deferred_tool_load().expect("a typed load");
        assert_eq!(
            reported.stage(),
            meerkat_core::DeferredToolLoadStage::Requested
        );
        assert_eq!(reported.names(), ["deferred_first"]);
        let [SessionEffect::RequestDeferredTools { authorities }] =
            requested.session_effects.as_slice()
        else {
            panic!("expected one request effect: {requested:?}");
        };

        // The boundary after an external call: request, then commit.
        scope
            .add_requested_deferred_authorities(authorities)
            .unwrap();
        scope.commit_requested_deferred_tools().unwrap();
        assert!(
            scope
                .visible_tool_names()
                .unwrap()
                .contains("deferred_first")
        );

        let repeated = load(json!(["deferred_first", "missing_tool"]))
            .await
            .unwrap();
        assert!(
            repeated.session_effects.is_empty(),
            "a loaded name requests nothing again"
        );
        let reported = repeated
            .deferred_tool_load()
            .expect("a repeated load is still reported");
        assert_eq!(reported.names(), ["deferred_first"]);

        let rejected = load(json!(["missing_tool"])).await.unwrap();
        assert_eq!(
            rejected.deferred_tool_load(),
            None,
            "a call that accepts no name reports no load"
        );
    }

    /// The load tool's description holds on every surface. In an ordinary
    /// turn and in a live bridge operation an accepted tool is in the next
    /// model call. In a live conversation it is callable once the call
    /// returns, unless the host could not update the conversation's tool
    /// list, which the result's `availability` field then says. It no longer
    /// promises a "next boundary", which a live conversation never reaches.
    #[test]
    fn load_description_is_true_for_ordinary_turns_and_live_conversations() {
        let description = load_tool_def().description;
        assert!(
            description.contains("can be called from your next step"),
            "{description}"
        );
        assert!(description.contains("`availability`"), "{description}");
        assert!(description.contains("`tool_availability`"), "{description}");
        assert!(!description.contains("boundary"), "{description}");
    }

    /// The built-in skill an agent reads about the catalog tools says what
    /// the description says, not the old "next admitted tool boundary" rule.
    #[test]
    fn the_builtin_skill_describes_a_load_as_the_description_does() {
        let skill = include_str!("../skills/builtin-utilities-workflow/SKILL.md");
        let about_load = skill
            .split("- Use `tool_catalog_search` and `tool_catalog_load`")
            .nth(1)
            .and_then(|rest| rest.split("\n\n").next())
            .expect("the skill's bullet about the catalog tools");
        assert!(
            about_load.contains("callable from your next step"),
            "{about_load}"
        );
        assert!(about_load.contains("`availability`"), "{about_load}");
        assert!(!about_load.contains("boundary"), "{about_load}");
    }

    #[tokio::test]
    async fn load_repairs_already_requested_names_missing_witnesses() {
        let deferred = session_tool("deferred_mcp_tool", "Deferred MCP tool");
        let dispatcher = Arc::new(ExactCatalogDispatcher {
            tools: vec![Arc::clone(&deferred)].into(),
            catalog: vec![ToolCatalogEntry::session_deferred(
                Arc::clone(&deferred),
                true,
                callback_provenance("test"),
            )]
            .into(),
            pending_sources: Arc::from([]),
            may_require_control_plane: false,
        });
        let visibility_provider = Arc::new(CatalogControlVisibilityProvider::new());
        visibility_provider.set_visibility_state_for_test(SessionToolVisibilityState {
            staged_requested_deferred_names: ["deferred_mcp_tool".into()].into_iter().collect(),
            ..Default::default()
        });

        let control = CatalogControlDispatcher::new(dispatcher, visibility_provider);
        let outcome = control
            .dispatch(search_call(
                LOAD_TOOL_NAME,
                json!({ "names": ["deferred_mcp_tool"] }),
            ))
            .await
            .unwrap();
        let response: LoadResponse = serde_json::from_str(&outcome.result.text_content()).unwrap();

        assert!(response.catalog_exact);
        assert_eq!(
            response.accepted_names,
            vec!["deferred_mcp_tool".to_string()]
        );
        assert!(response.noop_names.is_empty());
        assert_eq!(
            response.resolutions,
            vec![ToolCatalogLoadResolution {
                name: "deferred_mcp_tool".into(),
                accepted: true,
                accepted_noop: false,
                rejected_reason: None,
            }]
        );
        assert_eq!(outcome.session_effects.len(), 1);
        let SessionEffect::RequestDeferredTools { authorities } = &outcome.session_effects[0]
        else {
            panic!("expected RequestDeferredTools session effect");
        };
        assert_eq!(
            authorities.as_slice(),
            &[DeferredToolLoadAuthority::new(
                "deferred_mcp_tool",
                ToolVisibilityWitness {
                    last_seen_provenance: deferred.provenance.clone(),
                }
            )]
        );
    }

    #[tokio::test]
    async fn load_effect_carries_deferred_authority_values_not_name_keyed_witnesses() {
        let deferred = session_tool("deferred_mcp_tool", "Deferred MCP tool");
        let dispatcher = Arc::new(ExactCatalogDispatcher {
            tools: vec![Arc::clone(&deferred)].into(),
            catalog: vec![ToolCatalogEntry::session_deferred(
                Arc::clone(&deferred),
                true,
                callback_provenance("test"),
            )]
            .into(),
            pending_sources: Arc::from([]),
            may_require_control_plane: false,
        });
        let visibility_provider = Arc::new(CatalogControlVisibilityProvider::new());
        visibility_provider.set_scope(generated_visibility_scope(
            vec![Arc::clone(&deferred)].into(),
            ["deferred_mcp_tool".into()].into_iter().collect(),
        ));

        let control = CatalogControlDispatcher::new(dispatcher, visibility_provider);
        let outcome = control
            .dispatch(search_call(
                LOAD_TOOL_NAME,
                json!({ "names": ["deferred_mcp_tool"] }),
            ))
            .await
            .unwrap();

        assert_eq!(outcome.session_effects.len(), 1);
        let effect = serde_json::to_value(&outcome.session_effects[0]).unwrap();
        assert_eq!(effect["effect_type"], "request_deferred_tools");
        assert!(
            effect["authorities"].is_array(),
            "deferred load authority should serialize as typed authority values, not a name-keyed witness map: {effect}"
        );
        assert_eq!(effect["authorities"][0]["name"], "deferred_mcp_tool");
        assert_eq!(
            effect["authorities"][0]["witness"]["last_seen_provenance"]["kind"],
            "callback"
        );
        assert_eq!(
            effect["authorities"][0]["witness"]["last_seen_provenance"]["source_id"],
            "test"
        );
    }

    /// Gate (#201): with NO `ToolScope` bound, the deferred-load control plane
    /// fails closed — it refuses every requested name and emits NO
    /// `RequestDeferredTools` effect, rather than accepting loads under default
    /// visibility. (Fails-old: the prior `unwrap_or_default()` fallback admitted
    /// the load and emitted the effect.)
    #[tokio::test]
    async fn load_without_scope_refuses_and_emits_no_effect() {
        let deferred = session_tool("deferred_mcp_tool", "Deferred MCP tool");
        let dispatcher = Arc::new(ExactCatalogDispatcher {
            tools: vec![Arc::clone(&deferred)].into(),
            catalog: vec![ToolCatalogEntry::session_deferred(
                Arc::clone(&deferred),
                true,
                callback_provenance("test"),
            )]
            .into(),
            pending_sources: Arc::from([]),
            may_require_control_plane: false,
        });
        // No scope and no test-override is set on the provider: there is no
        // visibility authority to consult.
        let visibility_provider = Arc::new(CatalogControlVisibilityProvider::new());
        let control = CatalogControlDispatcher::new(dispatcher, visibility_provider);

        let outcome = control
            .dispatch(search_call(
                LOAD_TOOL_NAME,
                json!({ "names": ["deferred_mcp_tool"] }),
            ))
            .await
            .unwrap();
        let response: LoadResponse = serde_json::from_str(&outcome.result.text_content()).unwrap();

        assert!(
            response.accepted_names.is_empty(),
            "no name may be accepted without a bound ToolScope"
        );
        assert_eq!(
            response.resolutions,
            vec![ToolCatalogLoadResolution {
                name: "deferred_mcp_tool".into(),
                accepted: false,
                accepted_noop: false,
                rejected_reason: Some(ToolCatalogLoadRejectedReason::TemporarilyUnavailable),
            }]
        );
        assert!(
            outcome.session_effects.is_empty(),
            "no RequestDeferredTools effect may be emitted without a bound ToolScope"
        );
    }

    /// Without a bound `ToolScope` the search surface must fail closed too:
    /// with no visibility truth it cannot tell which entries the session may
    /// find, so it names none. The answer holds no row, no name and no
    /// description, and says at the result level that the catalog is
    /// temporarily unavailable. It never reports `Deferred` on the word of a
    /// locally composed default filter: the control plane is not a fallback
    /// visibility authority.
    #[tokio::test]
    async fn search_without_scope_names_nothing_and_reports_temporarily_unavailable() {
        let deferred = session_tool("deferred_mcp_tool", "Deferred MCP tool");
        let dispatcher = Arc::new(ExactCatalogDispatcher {
            tools: vec![Arc::clone(&deferred)].into(),
            catalog: vec![ToolCatalogEntry::session_deferred(
                Arc::clone(&deferred),
                true,
                callback_provenance("test"),
            )]
            .into(),
            pending_sources: Arc::from([]),
            may_require_control_plane: false,
        });
        let visibility_provider = Arc::new(CatalogControlVisibilityProvider::new());
        let control = CatalogControlDispatcher::new(dispatcher, visibility_provider);

        for args in [json!({ "query": "deferred" }), json!({})] {
            let outcome = control
                .dispatch(search_call(SEARCH_TOOL_NAME, args))
                .await
                .unwrap();
            let text = outcome.result.text_content();
            assert_eq!(
                text,
                r#"{"catalog_exact":true,"total_matches":0,"results":[],"empty_result_status":"temporarily_unavailable"}"#,
                "no bound scope means no visibility truth: fail closed, and name nothing"
            );
            let response: SearchResponse = serde_json::from_str(&text).unwrap();
            assert_eq!(
                response.empty_result_status,
                Some(SearchVisibilityStatus::TemporarilyUnavailable)
            );
        }
    }

    /// Gate (#201): with a `ToolScope` bound, behavior is unchanged — a valid
    /// deferred load is accepted and the `RequestDeferredTools` effect is
    /// emitted as before.
    #[tokio::test]
    async fn load_with_scope_still_accepts_and_emits_effect() {
        let deferred = session_tool("deferred_mcp_tool", "Deferred MCP tool");
        let dispatcher = Arc::new(ExactCatalogDispatcher {
            tools: vec![Arc::clone(&deferred)].into(),
            catalog: vec![ToolCatalogEntry::session_deferred(
                Arc::clone(&deferred),
                true,
                callback_provenance("test"),
            )]
            .into(),
            pending_sources: Arc::from([]),
            may_require_control_plane: false,
        });
        let visibility_provider = Arc::new(CatalogControlVisibilityProvider::new());
        visibility_provider.set_scope(generated_visibility_scope(
            vec![Arc::clone(&deferred)].into(),
            ["deferred_mcp_tool".into()].into_iter().collect(),
        ));
        let control = CatalogControlDispatcher::new(dispatcher, visibility_provider);

        let outcome = control
            .dispatch(search_call(
                LOAD_TOOL_NAME,
                json!({ "names": ["deferred_mcp_tool"] }),
            ))
            .await
            .unwrap();
        let response: LoadResponse = serde_json::from_str(&outcome.result.text_content()).unwrap();

        assert_eq!(
            response.accepted_names,
            vec!["deferred_mcp_tool".to_string()]
        );
        assert_eq!(outcome.session_effects.len(), 1);
        assert!(matches!(
            &outcome.session_effects[0],
            SessionEffect::RequestDeferredTools { .. }
        ));
    }

    #[tokio::test]
    async fn load_repairs_already_requested_names_missing_provenance_authority() {
        let deferred = session_tool("deferred_mcp_tool", "Deferred MCP tool");
        let dispatcher = Arc::new(ExactCatalogDispatcher {
            tools: vec![Arc::clone(&deferred)].into(),
            catalog: vec![ToolCatalogEntry::session_deferred(
                Arc::clone(&deferred),
                true,
                callback_provenance("test"),
            )]
            .into(),
            pending_sources: Arc::from([]),
            may_require_control_plane: false,
        });
        let visibility_provider = Arc::new(CatalogControlVisibilityProvider::new());
        visibility_provider.set_visibility_state_for_test(SessionToolVisibilityState {
            staged_requested_deferred_names: ["deferred_mcp_tool".into()].into_iter().collect(),
            requested_witnesses: [(
                "deferred_mcp_tool".into(),
                ToolVisibilityWitness {
                    last_seen_provenance: None,
                },
            )]
            .into_iter()
            .collect(),
            ..Default::default()
        });

        let control = CatalogControlDispatcher::new(dispatcher, visibility_provider);
        let outcome = control
            .dispatch(search_call(
                LOAD_TOOL_NAME,
                json!({ "names": ["deferred_mcp_tool"] }),
            ))
            .await
            .unwrap();
        let response: LoadResponse = serde_json::from_str(&outcome.result.text_content()).unwrap();

        assert!(response.catalog_exact);
        assert_eq!(
            response.accepted_names,
            vec!["deferred_mcp_tool".to_string()],
            "a staged name with only a stable-owner string must be repaired with provenance authority"
        );
        assert!(response.noop_names.is_empty());
        assert_eq!(outcome.session_effects.len(), 1);
        let SessionEffect::RequestDeferredTools { authorities } = &outcome.session_effects[0]
        else {
            panic!("expected RequestDeferredTools session effect");
        };
        assert_eq!(
            authorities.as_slice(),
            &[DeferredToolLoadAuthority::new(
                "deferred_mcp_tool",
                ToolVisibilityWitness {
                    last_seen_provenance: deferred.provenance.clone(),
                }
            )]
        );
    }

    #[tokio::test]
    async fn load_rejects_deferred_entry_missing_provenance_authority() {
        let deferred = session_tool_without_provenance("deferred_mcp_tool", "Deferred MCP tool");
        let dispatcher = Arc::new(ExactCatalogDispatcher {
            tools: vec![Arc::clone(&deferred)].into(),
            catalog: vec![ToolCatalogEntry::session_deferred(
                Arc::clone(&deferred),
                true,
                callback_provenance("test"),
            )]
            .into(),
            pending_sources: Arc::from([]),
            may_require_control_plane: false,
        });
        let control = CatalogControlDispatcher::new(
            dispatcher,
            Arc::new(CatalogControlVisibilityProvider::new()),
        );

        let outcome = control
            .dispatch(search_call(
                LOAD_TOOL_NAME,
                json!({ "names": ["deferred_mcp_tool"] }),
            ))
            .await
            .unwrap();
        let response: LoadResponse = serde_json::from_str(&outcome.result.text_content()).unwrap();

        assert!(response.accepted_names.is_empty());
        assert!(response.noop_names.is_empty());
        assert_eq!(
            response.resolutions,
            vec![ToolCatalogLoadResolution {
                name: "deferred_mcp_tool".into(),
                accepted: false,
                accepted_noop: false,
                rejected_reason: Some(ToolCatalogLoadRejectedReason::TemporarilyUnavailable),
            }]
        );
        assert!(
            outcome.session_effects.is_empty(),
            "missing provenance authority must be rejected before RequestDeferredTools"
        );
    }

    #[tokio::test]
    async fn load_rejects_deferred_entry_with_mismatched_owner_and_provenance() {
        let deferred = session_tool("deferred_mcp_tool", "Deferred MCP tool");
        let dispatcher = Arc::new(ExactCatalogDispatcher {
            tools: vec![Arc::clone(&deferred)].into(),
            catalog: vec![ToolCatalogEntry::session_deferred(
                Arc::clone(&deferred),
                true,
                callback_provenance("other"),
            )]
            .into(),
            pending_sources: Arc::from([]),
            may_require_control_plane: false,
        });
        let control = CatalogControlDispatcher::new(
            dispatcher,
            Arc::new(CatalogControlVisibilityProvider::new()),
        );

        let outcome = control
            .dispatch(search_call(
                LOAD_TOOL_NAME,
                json!({ "names": ["deferred_mcp_tool"] }),
            ))
            .await
            .unwrap();
        let response: LoadResponse = serde_json::from_str(&outcome.result.text_content()).unwrap();

        assert!(response.accepted_names.is_empty());
        assert!(response.noop_names.is_empty());
        assert_eq!(
            response.resolutions,
            vec![ToolCatalogLoadResolution {
                name: "deferred_mcp_tool".into(),
                accepted: false,
                accepted_noop: false,
                rejected_reason: Some(ToolCatalogLoadRejectedReason::TemporarilyUnavailable),
            }]
        );
        assert!(
            outcome.session_effects.is_empty(),
            "mismatched deferred owner/provenance authority must not be staged"
        );
    }

    #[tokio::test]
    async fn load_rejects_temporarily_unavailable_deferred_names() {
        let deferred = session_tool("deferred_mcp_tool", "Deferred MCP tool");
        let dispatcher = Arc::new(ExactCatalogDispatcher {
            tools: Arc::from([]),
            catalog: vec![ToolCatalogEntry::session_deferred(
                Arc::clone(&deferred),
                false,
                callback_provenance("test"),
            )]
            .into(),
            pending_sources: Arc::from([]),
            may_require_control_plane: false,
        });
        let control = CatalogControlDispatcher::new(
            dispatcher,
            Arc::new(CatalogControlVisibilityProvider::new()),
        );

        let outcome = control
            .dispatch(search_call(
                LOAD_TOOL_NAME,
                json!({ "names": ["deferred_mcp_tool"] }),
            ))
            .await
            .unwrap();
        let response: LoadResponse = serde_json::from_str(&outcome.result.text_content()).unwrap();

        assert!(response.accepted_names.is_empty());
        assert!(response.noop_names.is_empty());
        assert_eq!(
            response.resolutions,
            vec![ToolCatalogLoadResolution {
                name: "deferred_mcp_tool".into(),
                accepted: false,
                accepted_noop: false,
                rejected_reason: Some(ToolCatalogLoadRejectedReason::TemporarilyUnavailable),
            }]
        );
        assert!(outcome.session_effects.is_empty());
    }

    #[tokio::test]
    async fn load_rejects_deferred_names_hidden_by_staged_filters() {
        let deferred = session_tool("deferred_mcp_tool", "Deferred MCP tool");
        let dispatcher = Arc::new(ExactCatalogDispatcher {
            tools: vec![Arc::clone(&deferred)].into(),
            catalog: vec![ToolCatalogEntry::session_deferred(
                Arc::clone(&deferred),
                true,
                callback_provenance("test"),
            )]
            .into(),
            pending_sources: Arc::from([]),
            may_require_control_plane: false,
        });
        let visibility_provider = Arc::new(CatalogControlVisibilityProvider::new());
        let scope = generated_visibility_scope(
            vec![Arc::clone(&deferred)].into(),
            ["deferred_mcp_tool".into()].into_iter().collect(),
        );
        scope
            .set_visibility_state(SessionToolVisibilityState {
                staged_filter: ToolFilter::Deny(
                    ["deferred_mcp_tool".to_string()].into_iter().collect(),
                ),
                staged_revision: 1,
                filter_witnesses: [(
                    "deferred_mcp_tool".into(),
                    ToolVisibilityWitness {
                        last_seen_provenance: deferred.provenance.clone(),
                    },
                )]
                .into_iter()
                .collect(),
                ..Default::default()
            })
            .unwrap();
        visibility_provider.set_scope(scope);

        let control = CatalogControlDispatcher::new(dispatcher, visibility_provider);
        let outcome = control
            .dispatch(search_call(
                LOAD_TOOL_NAME,
                json!({ "names": ["deferred_mcp_tool"] }),
            ))
            .await
            .unwrap();
        let response: LoadResponse = serde_json::from_str(&outcome.result.text_content()).unwrap();

        assert!(response.accepted_names.is_empty());
        assert!(response.noop_names.is_empty());
        assert_eq!(
            response.resolutions,
            vec![ToolCatalogLoadResolution {
                name: "deferred_mcp_tool".into(),
                accepted: false,
                accepted_noop: false,
                rejected_reason: Some(ToolCatalogLoadRejectedReason::UnknownKey),
            }],
            "a tool the session's filters do not allow answers as a name the catalog does not hold"
        );
        assert!(
            outcome.session_effects.is_empty(),
            "filter-hidden deferred tools should not emit a staged load effect"
        );
    }

    #[tokio::test]
    async fn search_reports_pending_sources_when_catalog_is_still_warming() {
        let dispatcher = Arc::new(ExactCatalogDispatcher {
            tools: Arc::from([]),
            catalog: Arc::from([]),
            pending_sources: Arc::from(["pending-mcp".to_string()]),
            may_require_control_plane: false,
        });
        // A session's control plane always has its scope bound; the pending
        // status below is the answer of a readable scope with nothing to find.
        let visibility_provider = Arc::new(CatalogControlVisibilityProvider::new());
        visibility_provider.set_scope(generated_visibility_scope(Arc::from([]), HashSet::new()));
        let control = CatalogControlDispatcher::new(dispatcher, visibility_provider);

        let outcome = control
            .dispatch(search_call(SEARCH_TOOL_NAME, json!({ "query": "secret" })))
            .await
            .unwrap();
        let response: SearchResponse =
            serde_json::from_str(&outcome.result.text_content()).unwrap();

        assert!(response.catalog_exact);
        assert_eq!(response.total_matches, 0);
        assert_eq!(response.pending_sources, vec!["pending-mcp".to_string()]);
        assert_eq!(
            response.empty_result_status,
            Some(SearchVisibilityStatus::PendingSource)
        );
    }

    #[tokio::test]
    async fn search_reports_loaded_deferred_and_temporarily_unavailable_states_and_omits_a_filtered_tool()
     {
        let loaded = session_tool(
            "loaded_secret_lookup",
            "Deferred secret lookup that is already loaded and visible.",
        );
        let deferred = session_tool(
            "deferred_secret_lookup",
            "Deferred secret lookup that still needs tool_catalog_load.",
        );
        let blocked = session_tool(
            "blocked_secret_lookup",
            "Deferred secret lookup that is currently blocked by a session filter.",
        );
        let unavailable = session_tool(
            "offline_secret_lookup",
            "Deferred secret lookup whose source is temporarily unavailable.",
        );
        let dispatcher = Arc::new(ExactCatalogDispatcher {
            tools: vec![
                Arc::clone(&loaded),
                Arc::clone(&deferred),
                Arc::clone(&blocked),
            ]
            .into(),
            catalog: vec![
                ToolCatalogEntry::session_deferred(
                    Arc::clone(&loaded),
                    true,
                    callback_provenance("test"),
                ),
                ToolCatalogEntry::session_deferred(
                    Arc::clone(&deferred),
                    true,
                    callback_provenance("test"),
                ),
                ToolCatalogEntry::session_deferred(
                    Arc::clone(&blocked),
                    true,
                    callback_provenance("test"),
                ),
                ToolCatalogEntry::session_deferred(
                    Arc::clone(&unavailable),
                    false,
                    callback_provenance("test"),
                ),
            ]
            .into(),
            pending_sources: Arc::from([]),
            may_require_control_plane: false,
        });
        let visibility_provider = Arc::new(CatalogControlVisibilityProvider::new());
        let scope = generated_visibility_scope(
            vec![
                Arc::clone(&loaded),
                Arc::clone(&deferred),
                Arc::clone(&blocked),
            ]
            .into(),
            [
                "loaded_secret_lookup".into(),
                "deferred_secret_lookup".into(),
                "blocked_secret_lookup".into(),
                "offline_secret_lookup".into(),
            ]
            .into_iter()
            .collect(),
        );
        scope
            .set_visibility_state(SessionToolVisibilityState {
                active_requested_deferred_names: ["loaded_secret_lookup".into()]
                    .into_iter()
                    .collect(),
                staged_requested_deferred_names: ["loaded_secret_lookup".into()]
                    .into_iter()
                    .collect(),
                requested_witnesses: [(
                    "loaded_secret_lookup".into(),
                    meerkat_core::ToolVisibilityWitness {
                        last_seen_provenance: loaded.provenance.clone(),
                    },
                )]
                .into_iter()
                .collect(),
                staged_filter: ToolFilter::Deny(
                    ["blocked_secret_lookup".to_string()].into_iter().collect(),
                ),
                active_filter: ToolFilter::Deny(
                    ["blocked_secret_lookup".to_string()].into_iter().collect(),
                ),
                filter_witnesses: [(
                    "blocked_secret_lookup".into(),
                    ToolVisibilityWitness {
                        last_seen_provenance: blocked.provenance.clone(),
                    },
                )]
                .into_iter()
                .collect(),
                ..Default::default()
            })
            .unwrap();
        visibility_provider.set_scope(scope);

        let control = CatalogControlDispatcher::new(dispatcher, visibility_provider);
        let outcome = control
            .dispatch(search_call(SEARCH_TOOL_NAME, json!({ "query": "secret" })))
            .await
            .unwrap();
        let response: SearchResponse =
            serde_json::from_str(&outcome.result.text_content()).unwrap();

        assert_eq!(
            response.total_matches, 3,
            "the total counts only what the session may find"
        );
        let statuses = response
            .results
            .into_iter()
            .map(|item| (item.name, item.visibility_status))
            .collect::<BTreeMap<_, _>>();

        assert_eq!(
            statuses.get("loaded_secret_lookup"),
            Some(&SearchVisibilityStatus::Loaded)
        );
        assert_eq!(
            statuses.get("deferred_secret_lookup"),
            Some(&SearchVisibilityStatus::Deferred)
        );
        assert_eq!(
            statuses.get("blocked_secret_lookup"),
            None,
            "a tool the session's filters do not allow is not a search result"
        );
        assert_eq!(
            statuses.get("offline_secret_lookup"),
            Some(&SearchVisibilityStatus::TemporarilyUnavailable)
        );
    }

    // A tool the session's filters do not allow is absent from the deferred
    // catalog: no search returns or counts it and its load answers as a name
    // the catalog does not hold. The rows below hold that for every filter
    // layer the scope composes, and that the tool is back once the filter
    // allows it, on the same control plane.

    const HIDDEN_TOOL: &str = "ledger_export";
    const HIDDEN_DESCRIPTION: &str = "Ledger export the session filters do not allow.";
    const OPEN_TOOLS: [&str; 2] = ["report_lookup", "report_archive"];

    fn open_tools() -> Vec<Arc<ToolDef>> {
        vec![
            session_tool(OPEN_TOOLS[0], "Report lookup behind the deferred catalog."),
            session_tool(OPEN_TOOLS[1], "Report archive behind the deferred catalog."),
        ]
    }

    fn open_and_hidden_tools() -> Vec<Arc<ToolDef>> {
        let mut tools = open_tools();
        tools.push(session_tool(HIDDEN_TOOL, HIDDEN_DESCRIPTION));
        tools
    }

    fn names(names: &[&str]) -> meerkat_core::types::ToolNameSet {
        names.iter().map(|name| (*name).to_string()).collect()
    }

    fn test_witness() -> ToolVisibilityWitness {
        ToolVisibilityWitness {
            last_seen_provenance: Some(callback_provenance("test")),
        }
    }

    /// A control plane over callable deferred session tools, bound to a
    /// generated visibility scope it shares with the caller.
    fn deferred_control(tools: Vec<Arc<ToolDef>>) -> (CatalogControlDispatcher, ToolScope) {
        deferred_control_for_model(tools, None, &ToolFilter::All)
    }

    fn deferred_control_for_model(
        tools: Vec<Arc<ToolDef>>,
        model_profile: Option<&meerkat_core::model_profile::ModelProfile>,
        capability_base_filter: &ToolFilter,
    ) -> (CatalogControlDispatcher, ToolScope) {
        let tools: Arc<[Arc<ToolDef>]> = tools.into();
        let dispatcher = Arc::new(ExactCatalogDispatcher {
            tools: Arc::clone(&tools),
            catalog: tools
                .iter()
                .map(|tool| {
                    ToolCatalogEntry::session_deferred(
                        Arc::clone(tool),
                        true,
                        callback_provenance("test"),
                    )
                })
                .collect::<Vec<_>>()
                .into(),
            pending_sources: Arc::from([]),
            may_require_control_plane: false,
        });
        let scope = generated_visibility_scope_for_model(
            Arc::clone(&tools),
            tools.iter().map(|tool| tool.name.clone()).collect(),
            model_profile,
            capability_base_filter,
        );
        let visibility_provider = Arc::new(CatalogControlVisibilityProvider::new());
        visibility_provider.set_scope(scope.clone());
        (
            CatalogControlDispatcher::new(dispatcher, visibility_provider),
            scope,
        )
    }

    /// The search answer exactly as the model reads it.
    async fn search_text(control: &CatalogControlDispatcher, args: serde_json::Value) -> String {
        let outcome = control
            .dispatch(search_call(SEARCH_TOOL_NAME, args))
            .await
            .unwrap();
        assert!(!outcome.result.is_error);
        assert!(outcome.session_effects.is_empty());
        outcome.result.text_content()
    }

    /// The load answer exactly as the model reads it, and its effects.
    async fn load_text(
        control: &CatalogControlDispatcher,
        names: &[&str],
    ) -> (String, Vec<SessionEffect>) {
        let outcome = control
            .dispatch(search_call(LOAD_TOOL_NAME, json!({ "names": names })))
            .await
            .unwrap();
        assert!(!outcome.result.is_error);
        (outcome.result.text_content(), outcome.session_effects)
    }

    /// The whole contract for the hidden name: no search finds or counts it,
    /// and its load answers byte for byte what the same load answers in a
    /// session whose catalog never held that name.
    async fn assert_hidden_tool_is_absent(control: &CatalogControlDispatcher, case: &str) {
        assert_tool_is_absent(
            control,
            HIDDEN_TOOL,
            HIDDEN_DESCRIPTION,
            &["ledger", HIDDEN_TOOL, "session filters do not allow"],
            case,
        )
        .await;
    }

    /// The same contract for any hidden tool beside the two open ones:
    /// `only_hidden_queries` match that tool alone.
    async fn assert_tool_is_absent(
        control: &CatalogControlDispatcher,
        hidden_name: &str,
        hidden_description: &str,
        only_hidden_queries: &[&str],
        case: &str,
    ) {
        let all_text = search_text(control, json!({})).await;
        let all: SearchResponse = serde_json::from_str(&all_text).unwrap();
        assert_eq!(
            all.results
                .iter()
                .map(|item| item.name.as_str())
                .collect::<Vec<_>>(),
            OPEN_TOOLS.to_vec(),
            "{case}: search returns only what the session's filters allow"
        );
        assert_eq!(
            all.total_matches, 2,
            "{case}: the total does not count the hidden entry"
        );
        assert!(
            !all_text.contains(hidden_name) && !all_text.contains(hidden_description),
            "{case}: neither the name nor the description reaches the answer: {all_text}"
        );

        // A limit below the count still reports the count of what the session
        // may find, so the "more results" figure does not include the entry.
        let limited: SearchResponse =
            serde_json::from_str(&search_text(control, json!({ "limit": 1 })).await).unwrap();
        assert_eq!(limited.results.len(), 1, "{case}");
        assert_eq!(
            limited.total_matches, 2,
            "{case}: the total behind a limit does not count the hidden entry"
        );

        // A query only the hidden entry matches, by name or by description,
        // answers exactly what a query nothing matches answers.
        let nothing = search_text(control, json!({ "query": "no such tool anywhere" })).await;
        assert_eq!(
            nothing, r#"{"catalog_exact":true,"total_matches":0,"results":[]}"#,
            "{case}"
        );
        for query in only_hidden_queries {
            assert_eq!(
                search_text(control, json!({ "query": query })).await,
                nothing,
                "{case}: a query for the hidden entry ({query}) answers as a query nothing matches"
            );
        }

        // The same load in a session whose catalog never held the name.
        let (never_held, _) = deferred_control(open_tools());
        let (unknown_text, unknown_effects) = load_text(&never_held, &[hidden_name]).await;
        assert_eq!(
            unknown_text,
            format!(
                r#"{{"catalog_exact":true,"accepted_names":[],"noop_names":[],"resolutions":[{{"name":"{hidden_name}","accepted":false,"rejected_reason":"unknown_key"}}]}}"#
            ),
            "{case}: precondition: the answer for a name the catalog does not hold"
        );
        assert!(unknown_effects.is_empty());
        let (hidden_text, hidden_effects) = load_text(control, &[hidden_name]).await;
        assert_eq!(
            hidden_text, unknown_text,
            "{case}: a load of the hidden tool answers byte for byte as an unknown name"
        );
        assert!(
            hidden_effects.is_empty(),
            "{case}: a load of the hidden tool stages nothing"
        );

        // Beside other names its resolution differs from an unknown name's
        // only by the name asked for.
        let (mixed_text, _) =
            load_text(control, &[OPEN_TOOLS[0], hidden_name, "no_such_tool"]).await;
        let mixed: LoadResponse = serde_json::from_str(&mixed_text).unwrap();
        assert_eq!(
            mixed.accepted_names,
            vec![OPEN_TOOLS[0].to_string()],
            "{case}"
        );
        assert!(mixed.noop_names.is_empty(), "{case}");
        assert_eq!(
            mixed.resolutions[1..],
            [
                ToolCatalogLoadResolution {
                    name: hidden_name.into(),
                    accepted: false,
                    accepted_noop: false,
                    rejected_reason: Some(ToolCatalogLoadRejectedReason::UnknownKey),
                },
                ToolCatalogLoadResolution {
                    name: "no_such_tool".into(),
                    accepted: false,
                    accepted_noop: false,
                    rejected_reason: Some(ToolCatalogLoadRejectedReason::UnknownKey),
                },
            ],
            "{case}"
        );
    }

    /// The hidden tool is a deferred entry again: search finds and counts
    /// it, and its load is accepted with the load effect.
    async fn assert_hidden_tool_is_findable_and_loadable(
        control: &CatalogControlDispatcher,
        case: &str,
    ) {
        let all: SearchResponse =
            serde_json::from_str(&search_text(control, json!({})).await).unwrap();
        assert_eq!(all.total_matches, 3, "{case}");
        let found: SearchResponse =
            serde_json::from_str(&search_text(control, json!({ "query": "ledger" })).await)
                .unwrap();
        assert_eq!(found.total_matches, 1, "{case}");
        assert_eq!(
            found.results,
            vec![SearchResultItem {
                name: HIDDEN_TOOL.into(),
                description: HIDDEN_DESCRIPTION.into(),
                currently_callable: false,
                visibility_status: SearchVisibilityStatus::Deferred,
            }],
            "{case}: the tool is findable once the filter allows it"
        );
        let (text, effects) = load_text(control, &[HIDDEN_TOOL]).await;
        let loaded: LoadResponse = serde_json::from_str(&text).unwrap();
        assert_eq!(
            loaded.accepted_names,
            vec![HIDDEN_TOOL.to_string()],
            "{case}: the tool is loadable once the filter allows it"
        );
        assert!(
            matches!(
                effects.as_slice(),
                [SessionEffect::RequestDeferredTools { .. }]
            ),
            "{case}"
        );
    }

    /// A deny list over a deferrable tool: the staged filter hides it from
    /// the moment it is staged, before any boundary applies it, and lifting
    /// the filter brings the tool back on the same control plane.
    #[tokio::test]
    async fn a_deny_list_hides_the_tool_from_search_and_load_until_it_is_lifted() {
        let (control, scope) = deferred_control(open_and_hidden_tools());
        assert_hidden_tool_is_findable_and_loadable(&control, "before the deny list").await;

        scope
            .handle()
            .stage_external_filter(ToolFilter::Deny(names(&[HIDDEN_TOOL])))
            .unwrap();
        let staged = scope.visibility_state().unwrap();
        assert_eq!(staged.active_filter, ToolFilter::All);
        assert_eq!(
            staged.staged_filter,
            ToolFilter::Deny(names(&[HIDDEN_TOOL]))
        );
        assert_hidden_tool_is_absent(&control, "a staged deny list, before its boundary").await;

        scope.apply_staged(open_and_hidden_tools().into()).unwrap();
        assert_hidden_tool_is_absent(&control, "an applied deny list").await;

        scope
            .handle()
            .stage_external_filter(ToolFilter::All)
            .unwrap();
        assert_hidden_tool_is_findable_and_loadable(&control, "the deny list lifted").await;
    }

    /// A tool the model already holds keeps its `loaded` row while a deny is
    /// only staged; the boundary that applies the deny removes the tool from
    /// the model's list and from the catalog together.
    #[tokio::test]
    async fn a_staged_deny_of_a_loaded_tool_reaches_search_at_its_boundary() {
        let tools = open_and_hidden_tools();
        let (control, scope) = deferred_control(tools.clone());
        scope
            .add_requested_deferred_authorities(&[DeferredToolLoadAuthority::new(
                HIDDEN_TOOL,
                test_witness(),
            )])
            .unwrap();
        scope.apply_staged(tools.clone().into()).unwrap();
        assert!(scope.visible_tool_names().unwrap().contains(HIDDEN_TOOL));

        scope
            .handle()
            .stage_external_filter(ToolFilter::Deny(names(&[HIDDEN_TOOL])))
            .unwrap();
        let found: SearchResponse =
            serde_json::from_str(&search_text(&control, json!({ "query": "ledger" })).await)
                .unwrap();
        assert_eq!(found.total_matches, 1);
        assert_eq!(
            found.results[0].visibility_status,
            SearchVisibilityStatus::Loaded,
            "the model still holds the tool until the boundary applies the deny"
        );
        let (text, effects) = load_text(&control, &[HIDDEN_TOOL]).await;
        let loaded: LoadResponse = serde_json::from_str(&text).unwrap();
        assert_eq!(loaded.noop_names, vec![HIDDEN_TOOL.to_string()]);
        assert!(effects.is_empty());

        scope.apply_staged(tools.into()).unwrap();
        assert!(!scope.visible_tool_names().unwrap().contains(HIDDEN_TOOL));
        assert_hidden_tool_is_absent(&control, "the deny applied at its boundary").await;
    }

    /// A launch policy that makes the tool unreachable hides it the same way.
    #[tokio::test]
    async fn a_launch_policy_deny_hides_the_tool_from_search_and_load() {
        let (control, scope) = deferred_control(open_and_hidden_tools());
        let mut state = scope.visibility_state().unwrap();
        state.policy_base_filter = ToolFilter::Deny(names(&[HIDDEN_TOOL]));
        scope.set_visibility_state(state).unwrap();
        assert_hidden_tool_is_absent(&control, "a launch policy deny").await;
    }

    /// A tool the session's model cannot use is absent the same way. The
    /// capability filter is the model's: it hides the image viewer from a
    /// model that cannot read an image in a tool result.
    #[tokio::test]
    async fn a_capability_filtered_tool_is_absent_from_search_and_load() {
        const VIEWER_DESCRIPTION: &str = "Image viewer the model cannot use.";
        let model_profile: meerkat_core::model_profile::ModelProfile =
            serde_json::from_value(json!({
                "provider": "anthropic",
                "model_family": "test-family",
                "supports_temperature": true,
                "supports_thinking": false,
                "supports_reasoning": false,
                "inline_video": false,
                "vision": false,
                "image_input": false,
                "image_tool_results": false,
                "realtime": false,
                "supports_web_search": false,
                "supports_mid_conversation_system_messages": false,
                "image_generation": false,
                "params_schema": {},
            }))
            .expect("a model profile without image tool results");
        let capability_base_filter =
            meerkat_core::capability_base_filter_for_image_tool_results(false);
        let mut tools = open_tools();
        tools.push(session_tool(
            meerkat_core::VIEW_IMAGE_TOOL_NAME,
            VIEWER_DESCRIPTION,
        ));
        let (control, scope) =
            deferred_control_for_model(tools, Some(&model_profile), &capability_base_filter);
        assert_eq!(
            scope.visibility_state().unwrap().capability_base_filter,
            ToolFilter::Deny(names(&[meerkat_core::VIEW_IMAGE_TOOL_NAME])),
            "precondition: the scope holds the model's capability filter"
        );
        assert_tool_is_absent(
            &control,
            meerkat_core::VIEW_IMAGE_TOOL_NAME,
            VIEWER_DESCRIPTION,
            &[meerkat_core::VIEW_IMAGE_TOOL_NAME, "image viewer"],
            "a capability filter",
        )
        .await;
    }

    /// A closed grant gate hides the tool; once a live grant opens the name
    /// it is findable and loadable on the same control plane.
    #[tokio::test]
    async fn a_closed_grant_gate_hides_the_tool_until_a_grant_opens_it() {
        let (control, scope) = deferred_control(open_and_hidden_tools());
        scope
            .replace_tool_grant_gate(meerkat_core::tool_grant::InstalledToolGrantGate::names(
                BTreeSet::from([HIDDEN_TOOL.into()]),
            ))
            .unwrap();
        assert_hidden_tool_is_absent(&control, "a closed grant gate").await;

        scope
            .apply_tool_grant_open_set(BTreeSet::from([HIDDEN_TOOL.into()]))
            .unwrap();
        assert_hidden_tool_is_findable_and_loadable(&control, "the grant gate opened").await;
    }

    /// An inherited ceiling counts a name only once it is witnessed against
    /// the session's own current tool. Until then the name is absent; once
    /// its source is witnessed it is findable and loadable, with no rebuild.
    #[tokio::test]
    async fn an_unwitnessed_ceiling_name_is_absent_until_its_source_is_witnessed() {
        let (control, scope) = deferred_control(open_and_hidden_tools());
        let mut state = scope.visibility_state().unwrap();
        state.inherited_base_filter =
            ToolFilter::Allow(names(&[OPEN_TOOLS[0], OPEN_TOOLS[1], HIDDEN_TOOL]));
        state.filter_witnesses = [
            (OPEN_TOOLS[0].into(), test_witness()),
            (OPEN_TOOLS[1].into(), test_witness()),
            (
                HIDDEN_TOOL.into(),
                ToolVisibilityWitness {
                    last_seen_provenance: Some(callback_provenance("not-yet-this-source")),
                },
            ),
        ]
        .into_iter()
        .collect();
        scope.set_visibility_state(state.clone()).unwrap();
        assert_hidden_tool_is_absent(&control, "a ceiling name not yet witnessed").await;

        state
            .filter_witnesses
            .insert(HIDDEN_TOOL.into(), test_witness());
        scope.set_visibility_state(state).unwrap();
        assert_hidden_tool_is_findable_and_loadable(&control, "the ceiling name witnessed").await;
    }

    /// A tool that is never deferred and that the session's filters do not
    /// allow answers as an unknown name too: its load does not confirm that
    /// the name exists.
    #[tokio::test]
    async fn a_filtered_inline_tool_load_answers_as_an_unknown_name() {
        const INLINE: &str = "inline_admin";
        let inline = session_tool(INLINE, "Inline administration tool.");
        let control_over = |with_inline: bool, denied: bool| {
            let mut tools = open_tools();
            let mut catalog = tools
                .iter()
                .map(|tool| {
                    ToolCatalogEntry::session_deferred(
                        Arc::clone(tool),
                        true,
                        callback_provenance("test"),
                    )
                })
                .collect::<Vec<_>>();
            if with_inline {
                tools.push(Arc::clone(&inline));
                catalog.push(ToolCatalogEntry::session_inline(Arc::clone(&inline), true));
            }
            let tools: Arc<[Arc<ToolDef>]> = tools.into();
            let scope = generated_visibility_scope(
                Arc::clone(&tools),
                OPEN_TOOLS.iter().map(|name| (*name).into()).collect(),
            );
            if denied {
                scope
                    .handle()
                    .stage_external_filter(ToolFilter::Deny(names(&[INLINE])))
                    .unwrap();
                scope.apply_staged(Arc::clone(&tools)).unwrap();
            }
            let visibility_provider = Arc::new(CatalogControlVisibilityProvider::new());
            visibility_provider.set_scope(scope.clone());
            (
                CatalogControlDispatcher::new(
                    Arc::new(ExactCatalogDispatcher {
                        tools,
                        catalog: catalog.into(),
                        pending_sources: Arc::from([]),
                        may_require_control_plane: false,
                    }),
                    visibility_provider,
                ),
                scope,
            )
        };

        // Control: an inline tool the model holds is named as not deferrable.
        let (allowed_control, allowed_scope) = control_over(true, false);
        let (allowed_text, _) = load_text(&allowed_control, &[INLINE]).await;
        let allowed: LoadResponse = serde_json::from_str(&allowed_text).unwrap();
        assert_eq!(
            allowed.resolutions[0].rejected_reason,
            Some(ToolCatalogLoadRejectedReason::NotDeferredEligible)
        );

        let (unknown_text, _) = load_text(&control_over(false, false).0, &[INLINE]).await;
        let (denied_text, effects) = load_text(&control_over(true, true).0, &[INLINE]).await;
        assert_eq!(
            denied_text, unknown_text,
            "a load of a filtered inline tool answers byte for byte as an unknown name"
        );
        assert!(effects.is_empty());

        // The same tool in a turn that blocks it: unknown for that turn, and
        // named as not deferrable again in the next turn.
        allowed_scope
            .handle()
            .set_turn_overlay(None, [INLINE.into()].into_iter().collect())
            .unwrap();
        let (turn_text, effects) = load_text(&allowed_control, &[INLINE]).await;
        assert_eq!(
            turn_text, unknown_text,
            "a load of an inline tool this turn blocks answers byte for byte as an unknown name"
        );
        assert!(effects.is_empty());
        allowed_scope.handle().clear_turn_overlay().unwrap();
        let (next_turn_text, _) = load_text(&allowed_control, &[INLINE]).await;
        assert_eq!(next_turn_text, allowed_text);
    }

    /// A turn's own overlay is a filter the model's tool list honours for
    /// that turn, so the catalog honours it too: a tool the turn blocks, by
    /// name or by an allow list that leaves it out, is absent for that turn,
    /// and the next turn, without the overlay, finds and loads it again.
    #[tokio::test]
    async fn a_turn_overlay_hides_the_tool_for_that_turn_only() {
        let (control, scope) = deferred_control(open_and_hidden_tools());

        scope
            .handle()
            .set_turn_overlay(None, [HIDDEN_TOOL.into()].into_iter().collect())
            .unwrap();
        assert_hidden_tool_is_absent(&control, "a turn that blocks the tool").await;
        scope.handle().clear_turn_overlay().unwrap();
        assert_hidden_tool_is_findable_and_loadable(&control, "the next turn, without the overlay")
            .await;

        scope
            .handle()
            .set_turn_overlay(
                Some(OPEN_TOOLS.iter().map(|name| (*name).into()).collect()),
                HashSet::new(),
            )
            .unwrap();
        assert_hidden_tool_is_absent(&control, "a turn that allows only the other tools").await;
        scope.handle().clear_turn_overlay().unwrap();
        assert_hidden_tool_is_findable_and_loadable(&control, "the turn after the allow list")
            .await;
    }

    /// The answer of a search that cannot tell which entries the session may
    /// find: no row, no count, one status.
    const UNREADABLE_SEARCH: &str = r#"{"catalog_exact":true,"total_matches":0,"results":[],"empty_result_status":"temporarily_unavailable"}"#;

    fn refused_as_temporarily_unavailable(name: &str) -> ToolCatalogLoadResolution {
        ToolCatalogLoadResolution {
            name: name.to_string(),
            accepted: false,
            accepted_noop: false,
            rejected_reason: Some(ToolCatalogLoadRejectedReason::TemporarilyUnavailable),
        }
    }

    /// A bound scope that cannot say which names the model holds is not a
    /// scope whose model holds none. Every search answers the same, whatever
    /// its query matches, and every load is refused: with the filter
    /// predicate failing too (the scope's own state is unreadable), and with
    /// the predicate still answering.
    #[tokio::test]
    async fn an_unreadable_set_of_held_names_answers_every_search_the_same_and_refuses_every_load()
    {
        for filters_unreadable in [true, false] {
            let (control, _scope) = deferred_control(open_and_hidden_tools());
            let provider = Arc::clone(&control.visibility_provider);
            provider.set_visible_names_unreadable_for_test(true);
            provider.set_filters_unreadable_for_test(filters_unreadable);

            for args in [
                json!({}),
                json!({ "limit": 1 }),
                json!({ "query": "ledger" }),
                json!({ "query": "report" }),
                json!({ "query": "a query no entry matches" }),
            ] {
                assert_eq!(
                    search_text(&control, args.clone()).await,
                    UNREADABLE_SEARCH,
                    "filters unreadable: {filters_unreadable}; search {args}"
                );
            }

            let asked = [OPEN_TOOLS[0], HIDDEN_TOOL, "no_such_tool"];
            let (text, effects) = load_text(&control, &asked).await;
            let response: LoadResponse = serde_json::from_str(&text).unwrap();
            assert!(response.accepted_names.is_empty(), "{text}");
            assert!(response.noop_names.is_empty(), "{text}");
            assert_eq!(
                response.resolutions,
                asked.map(refused_as_temporarily_unavailable).to_vec(),
                "filters unreadable: {filters_unreadable}"
            );
            assert!(effects.is_empty());

            provider.set_visible_names_unreadable_for_test(false);
            provider.set_filters_unreadable_for_test(false);
            assert_hidden_tool_is_findable_and_loadable(&control, "the scope readable again").await;
        }
    }

    /// The filter predicate stops answering while the names the model holds
    /// are still readable. A load keeps the no-op for a tool the model holds
    /// and gives one answer for every other name, whether the catalog holds
    /// it, a filter hides it, or nothing provides it. A search whose query
    /// reaches an entry the model does not hold names nothing.
    #[tokio::test]
    async fn unreadable_filters_refuse_the_load_of_every_name_the_model_does_not_hold() {
        let tools = open_and_hidden_tools();
        let (control, scope) = deferred_control(tools.clone());
        scope
            .add_requested_deferred_authorities(&[DeferredToolLoadAuthority::new(
                OPEN_TOOLS[0],
                test_witness(),
            )])
            .unwrap();
        scope.apply_staged(tools.into()).unwrap();
        assert!(scope.visible_tool_names().unwrap().contains(OPEN_TOOLS[0]));
        scope
            .handle()
            .stage_external_filter(ToolFilter::Deny(names(&[HIDDEN_TOOL])))
            .unwrap();
        control
            .visibility_provider
            .set_filters_unreadable_for_test(true);

        let (text, effects) = load_text(
            &control,
            &[OPEN_TOOLS[0], OPEN_TOOLS[1], HIDDEN_TOOL, "no_such_tool"],
        )
        .await;
        let response: LoadResponse = serde_json::from_str(&text).unwrap();
        assert!(response.accepted_names.is_empty(), "{text}");
        assert_eq!(response.noop_names, vec![OPEN_TOOLS[0].to_string()]);
        assert_eq!(
            response.resolutions,
            vec![
                ToolCatalogLoadResolution {
                    name: OPEN_TOOLS[0].to_string(),
                    accepted: true,
                    accepted_noop: true,
                    rejected_reason: None,
                },
                refused_as_temporarily_unavailable(OPEN_TOOLS[1]),
                refused_as_temporarily_unavailable(HIDDEN_TOOL),
                refused_as_temporarily_unavailable("no_such_tool"),
            ],
            "one answer for every name the model does not hold: {text}"
        );
        assert!(effects.is_empty());

        for args in [
            json!({}),
            json!({ "query": "ledger" }),
            json!({ "query": "archive" }),
        ] {
            assert_eq!(
                search_text(&control, args.clone()).await,
                UNREADABLE_SEARCH,
                "search {args}"
            );
        }
    }
}
