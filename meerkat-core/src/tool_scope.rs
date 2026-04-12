//! Tool visibility scope and external filter staging.

use crate::session::{SessionToolVisibilityState, ToolVisibilityWitness};
use crate::tool_catalog::stable_owner_key_for_tool;
use crate::types::ToolDef;
use std::collections::BTreeSet;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

/// Visibility filter for tools.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ToolFilter {
    /// All tools are visible.
    #[default]
    All,
    /// Only listed tools are visible.
    Allow(HashSet<String>),
    /// Listed tools are hidden.
    Deny(HashSet<String>),
}

impl ToolFilter {
    fn names(&self) -> Option<&HashSet<String>> {
        match self {
            Self::All => None,
            Self::Allow(names) | Self::Deny(names) => Some(names),
        }
    }
}

/// Session metadata key storing the persisted external tool filter.
pub const EXTERNAL_TOOL_FILTER_METADATA_KEY: &str = "tool_scope_external_filter";

/// Session metadata key storing the inherited tool filter from a parent agent.
///
/// When a child session is created from a parent mob, the parent's visible tool
/// set is captured as a base filter. On session rebuild, this key is recovered
/// and applied as the `ToolScope` base filter.
pub const INHERITED_TOOL_FILTER_METADATA_KEY: &str = "tool_scope_inherited_filter";

/// Monotonic revision for staged external visibility updates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct ToolScopeRevision(pub u64);

/// Diagnostic snapshot of the current live tool-scope state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolScopeSnapshot {
    pub known_base_names: Vec<String>,
    pub visible_names: Vec<String>,
    pub base_filter: ToolFilter,
    pub active_external_filter: ToolFilter,
    pub active_turn_allow: Option<Vec<String>>,
    pub active_turn_deny: Vec<String>,
    pub active_revision: ToolScopeRevision,
    pub staged_external_filter: ToolFilter,
    pub staged_revision: ToolScopeRevision,
}

/// Global phase of the live external tool-surface machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalToolSurfaceGlobalPhase {
    Operating,
    Shutdown,
}

/// Base lifecycle state for one external tool surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalToolSurfaceBaseState {
    Absent,
    Active,
    Removing,
    Removed,
}

/// Pending async operation for one external tool surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalToolSurfacePendingOp {
    None,
    Add,
    Reload,
}

/// Staged-but-not-yet-applied operation for one external tool surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalToolSurfaceStagedOp {
    None,
    Add,
    Remove,
    Reload,
}

/// Last emitted lifecycle delta operation for one external tool surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalToolSurfaceDeltaOperation {
    None,
    Add,
    Remove,
    Reload,
}

/// Last emitted lifecycle delta phase for one external tool surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalToolSurfaceDeltaPhase {
    None,
    Pending,
    Applied,
    Draining,
    Failed,
    Forced,
}

/// Diagnostic snapshot of one external tool surface entry.
///
/// This is an observational surface over the canonical external-tool authority.
/// It does not create a second semantic owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalToolSurfaceEntrySnapshot {
    pub surface_id: String,
    pub visible: bool,
    pub base_state: ExternalToolSurfaceBaseState,
    pub has_removal_timing: bool,
    pub pending_op: ExternalToolSurfacePendingOp,
    pub staged_op: ExternalToolSurfaceStagedOp,
    pub staged_intent_sequence: u64,
    pub pending_task_sequence: u64,
    pub pending_lineage_sequence: u64,
    pub inflight_call_count: u64,
    pub last_delta_operation: ExternalToolSurfaceDeltaOperation,
    pub last_delta_phase: ExternalToolSurfaceDeltaPhase,
}

/// Diagnostic snapshot of the live external tool-surface machine state.
///
/// This keeps external tool mutation lineage and publication alignment visible
/// for MeerkatMachine mapping work without changing who owns the underlying
/// lifecycle truth.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalToolSurfaceSnapshot {
    pub phase: ExternalToolSurfaceGlobalPhase,
    pub snapshot_epoch: u64,
    pub snapshot_aligned_epoch: u64,
    pub entries: Vec<ExternalToolSurfaceEntrySnapshot>,
}

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum ToolScopeStageError {
    #[error("Unknown tool(s) in filter: {names:?}")]
    UnknownTools { names: Vec<String> },
    #[error("Tool scope state lock poisoned")]
    LockPoisoned,
}

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum ToolScopeApplyError {
    #[error("Tool scope state lock poisoned")]
    LockPoisoned,
    #[error("Injected boundary failure for testing")]
    InjectedFailure,
}

#[derive(Debug, Clone)]
struct ToolScopeState {
    base_tools: Arc<[Arc<ToolDef>]>,
    known_base_names: HashSet<String>,
    control_tool_names: HashSet<String>,
    deferred_tool_names: HashSet<String>,
    durable_state: SessionToolVisibilityState,
    active_turn_allow: Option<HashSet<String>>,
    active_turn_deny: HashSet<String>,
}

/// Composed filter representation using most-restrictive semantics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposedToolFilter {
    allow: Option<HashSet<String>>,
    deny: HashSet<String>,
}

impl ComposedToolFilter {
    pub fn allows(&self, name: &str) -> bool {
        let allowed = self.allow.as_ref().is_none_or(|set| set.contains(name));
        allowed && !self.deny.contains(name)
    }
}

/// Runtime tool scope used to determine provider-visible tools.
#[derive(Debug, Clone)]
pub struct ToolScope {
    state: Arc<RwLock<ToolScopeState>>,
    next_revision: Arc<AtomicU64>,
    fail_next_boundary_apply: Arc<AtomicBool>,
}

#[derive(Debug, Clone)]
pub struct ToolScopeBoundaryResult {
    pub previous_base_names: HashSet<String>,
    pub current_base_names: HashSet<String>,
    pub previous_visible_names: Vec<String>,
    pub visible_names: Vec<String>,
    pub previous_active_revision: ToolScopeRevision,
    pub applied_revision: ToolScopeRevision,
    pub tools: Arc<[Arc<ToolDef>]>,
}

impl ToolScopeBoundaryResult {
    pub fn base_changed(&self) -> bool {
        self.previous_base_names != self.current_base_names
    }

    pub fn visible_changed(&self) -> bool {
        self.previous_visible_names != self.visible_names
    }

    pub fn changed(&self) -> bool {
        self.base_changed() || self.visible_changed()
    }
}

impl ToolScope {
    /// Build a scope with no base restriction.
    pub fn new(base_tools: Arc<[Arc<ToolDef>]>) -> Self {
        Self::new_with_projection_names(base_tools, HashSet::new(), HashSet::new())
    }

    /// Build a scope with an explicit set of control-plane tool names.
    pub fn new_with_control_tool_names(
        base_tools: Arc<[Arc<ToolDef>]>,
        control_tool_names: HashSet<String>,
    ) -> Self {
        Self::new_with_projection_names(base_tools, control_tool_names, HashSet::new())
    }

    /// Build a scope with explicit control-plane and deferred-session names.
    pub fn new_with_projection_names(
        base_tools: Arc<[Arc<ToolDef>]>,
        control_tool_names: HashSet<String>,
        deferred_tool_names: HashSet<String>,
    ) -> Self {
        let known_base_names: HashSet<String> =
            base_tools.iter().map(|tool| tool.name.clone()).collect();

        Self {
            state: Arc::new(RwLock::new(ToolScopeState {
                base_tools,
                known_base_names,
                control_tool_names,
                deferred_tool_names,
                durable_state: SessionToolVisibilityState::default(),
                active_turn_allow: None,
                active_turn_deny: HashSet::new(),
            })),
            next_revision: Arc::new(AtomicU64::new(0)),
            fail_next_boundary_apply: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Returns the currently visible tools using base + active external filter composition.
    pub fn visible_tools(&self) -> Arc<[Arc<ToolDef>]> {
        match self.visible_tools_result() {
            Ok(tools) => tools,
            Err(_) => self
                .state
                .read()
                .map(|state| Arc::clone(&state.base_tools))
                .unwrap_or_else(|_| Vec::<Arc<ToolDef>>::new().into()),
        }
    }

    /// Returns current visible tools, or an explicit error for boundary fail-safe handling.
    pub fn visible_tools_result(&self) -> Result<Arc<[Arc<ToolDef>]>, ToolScopeApplyError> {
        let state = self
            .state
            .read()
            .map_err(|_| ToolScopeApplyError::LockPoisoned)?;

        let composed = Self::compose_state_filters(&state);

        Ok(state
            .base_tools
            .iter()
            .filter(|tool| {
                state.control_tool_names.contains(tool.name.as_str())
                    || (Self::is_requested_session_tool_visible(&state, tool.as_ref())
                        && composed.allows(tool.name.as_str()))
            })
            .map(Arc::clone)
            .collect::<Vec<_>>()
            .into())
    }

    /// Return a handle for thread-safe staged external updates.
    pub fn handle(&self) -> ToolScopeHandle {
        ToolScopeHandle {
            state: Arc::clone(&self.state),
            next_revision: Arc::clone(&self.next_revision),
        }
    }

    /// Snapshot the current live scope state for diagnostics.
    pub fn snapshot(&self) -> Option<ToolScopeSnapshot> {
        let state = self.state.read().ok()?;
        Some(ToolScopeSnapshot {
            known_base_names: sorted_names(&state.known_base_names),
            visible_names: Self::visible_names_for_state(&state),
            base_filter: state.durable_state.inherited_base_filter.clone(),
            active_external_filter: state.durable_state.active_filter.clone(),
            active_turn_allow: state.active_turn_allow.as_ref().map(sorted_names),
            active_turn_deny: sorted_names(&state.active_turn_deny),
            active_revision: ToolScopeRevision(state.durable_state.active_revision),
            staged_external_filter: state.durable_state.staged_filter.clone(),
            staged_revision: ToolScopeRevision(state.durable_state.staged_revision),
        })
    }

    /// Atomically apply staged state at CallingLlm boundary.
    ///
    /// Sequence:
    /// 1) Refresh base from dispatcher snapshot.
    /// 2) Prune active/pending filters against base deltas.
    /// 3) Apply staged external filter revision.
    /// 4) Compute visible tools.
    pub fn apply_staged(
        &self,
        new_base_tools: Arc<[Arc<ToolDef>]>,
    ) -> Result<ToolScopeBoundaryResult, ToolScopeApplyError> {
        let (control_tool_names, deferred_tool_names) = self
            .state
            .read()
            .map(|state| {
                (
                    state.control_tool_names.clone(),
                    state.deferred_tool_names.clone(),
                )
            })
            .map_err(|_| ToolScopeApplyError::LockPoisoned)?;
        self.apply_staged_projection(new_base_tools, control_tool_names, deferred_tool_names)
    }

    /// Atomically apply staged state and refresh the live projection names.
    pub fn apply_staged_projection(
        &self,
        new_base_tools: Arc<[Arc<ToolDef>]>,
        control_tool_names: HashSet<String>,
        deferred_tool_names: HashSet<String>,
    ) -> Result<ToolScopeBoundaryResult, ToolScopeApplyError> {
        if self
            .fail_next_boundary_apply
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            return Err(ToolScopeApplyError::InjectedFailure);
        }

        let mut state = self
            .state
            .write()
            .map_err(|_| ToolScopeApplyError::LockPoisoned)?;

        let previous_base_names = state.known_base_names.clone();
        let previous_visible_names = Self::visible_names_for_state(&state);
        let previous_active_revision = ToolScopeRevision(state.durable_state.active_revision);

        state.base_tools = new_base_tools;
        state.control_tool_names = control_tool_names;
        state.deferred_tool_names = deferred_tool_names;
        state.known_base_names = state
            .base_tools
            .iter()
            .map(|tool| tool.name.clone())
            .collect::<HashSet<_>>();

        let known_base_names = state.known_base_names.clone();
        if let Some(allow) = state.active_turn_allow.as_mut() {
            allow.retain(|name| known_base_names.contains(name));
        }
        state
            .active_turn_deny
            .retain(|name| known_base_names.contains(name));

        state.durable_state.active_filter = state.durable_state.staged_filter.clone();
        state.durable_state.active_requested_deferred_names =
            state.durable_state.staged_requested_deferred_names.clone();
        state.durable_state.active_revision = state.durable_state.staged_revision;

        let tools = Self::visible_tools_for_state(&state);
        let visible_names = tools
            .iter()
            .map(|tool| tool.name.clone())
            .collect::<Vec<_>>();

        Ok(ToolScopeBoundaryResult {
            previous_base_names,
            current_base_names: state.known_base_names.clone(),
            previous_visible_names,
            visible_names,
            previous_active_revision,
            applied_revision: ToolScopeRevision(state.durable_state.active_revision),
            tools,
        })
    }

    /// Compose filters with most-restrictive semantics.
    pub fn compose(filters: &[ToolFilter]) -> ComposedToolFilter {
        let mut allow: Option<HashSet<String>> = None;
        let mut deny: HashSet<String> = HashSet::new();

        for filter in filters {
            match filter {
                ToolFilter::All => {}
                ToolFilter::Allow(names) => {
                    allow = Some(match allow {
                        Some(existing) => Self::allow_intersection(&existing, names),
                        None => names.clone(),
                    });
                }
                ToolFilter::Deny(names) => {
                    deny = Self::deny_union(&deny, names);
                }
            }
        }

        ComposedToolFilter { allow, deny }
    }

    /// Helper: intersection for allow-list composition.
    fn allow_intersection(left: &HashSet<String>, right: &HashSet<String>) -> HashSet<String> {
        left.intersection(right).cloned().collect()
    }

    /// Helper: union for deny-list composition.
    fn deny_union(left: &HashSet<String>, right: &HashSet<String>) -> HashSet<String> {
        left.union(right).cloned().collect()
    }

    fn visible_names_for_state(state: &ToolScopeState) -> Vec<String> {
        let tools = Self::visible_tools_for_state(state);
        tools.iter().map(|tool| tool.name.clone()).collect()
    }

    fn visible_tools_for_state(state: &ToolScopeState) -> Arc<[Arc<ToolDef>]> {
        let composed = Self::compose_state_filters(state);

        state
            .base_tools
            .iter()
            .filter(|tool| {
                state.control_tool_names.contains(tool.name.as_str())
                    || (Self::is_requested_session_tool_visible(state, tool.as_ref())
                        && composed.allows(tool.name.as_str()))
            })
            .map(Arc::clone)
            .collect::<Vec<_>>()
            .into()
    }

    fn compose_state_filters(state: &ToolScopeState) -> ComposedToolFilter {
        let mut filters = vec![
            Self::effective_filter_for_current_projection(
                state,
                &state.durable_state.inherited_base_filter,
            ),
            Self::effective_filter_for_current_projection(
                state,
                &state.durable_state.active_filter,
            ),
        ];
        if let Some(allow) = &state.active_turn_allow {
            filters.push(ToolFilter::Allow(allow.clone()));
        }
        if !state.active_turn_deny.is_empty() {
            filters.push(ToolFilter::Deny(state.active_turn_deny.clone()));
        }
        Self::compose(&filters)
    }

    fn current_projection_tool<'a>(state: &'a ToolScopeState, name: &str) -> Option<&'a ToolDef> {
        state
            .base_tools
            .iter()
            .find(|tool| tool.name == name)
            .map(Arc::as_ref)
    }

    fn witness_matches_tool(witness: Option<&ToolVisibilityWitness>, tool: &ToolDef) -> bool {
        let Some(witness) = witness else {
            return true;
        };
        if let Some(expected_owner) = witness.stable_owner_key.as_deref()
            && stable_owner_key_for_tool(tool).as_deref() != Some(expected_owner)
        {
            return false;
        }
        if let Some(expected_provenance) = witness.last_seen_provenance.as_ref()
            && tool.provenance.as_ref() != Some(expected_provenance)
        {
            return false;
        }
        true
    }

    fn filter_name_applies(state: &ToolScopeState, name: &str) -> bool {
        Self::current_projection_tool(state, name).is_none_or(|tool| {
            Self::witness_matches_tool(state.durable_state.filter_witnesses.get(name), tool)
        })
    }

    fn effective_filter_for_current_projection(
        state: &ToolScopeState,
        filter: &ToolFilter,
    ) -> ToolFilter {
        match filter {
            ToolFilter::All => ToolFilter::All,
            ToolFilter::Allow(names) => ToolFilter::Allow(
                names
                    .iter()
                    .filter(|name| Self::filter_name_applies(state, name))
                    .cloned()
                    .collect(),
            ),
            ToolFilter::Deny(names) => ToolFilter::Deny(
                names
                    .iter()
                    .filter(|name| Self::filter_name_applies(state, name))
                    .cloned()
                    .collect(),
            ),
        }
    }

    fn is_requested_session_tool_visible(state: &ToolScopeState, tool: &ToolDef) -> bool {
        if !state.deferred_tool_names.contains(tool.name.as_str()) {
            return true;
        }
        state
            .durable_state
            .active_requested_deferred_names
            .contains(tool.name.as_str())
            && Self::witness_matches_tool(
                state
                    .durable_state
                    .requested_witnesses
                    .get(tool.name.as_str()),
                tool,
            )
    }

    /// Set the base filter for this scope.
    ///
    /// The base filter is the most fundamental restriction layer — it is
    /// composed with external and turn-level filters using most-restrictive
    /// semantics. This is used for inherited tool visibility from a parent
    /// agent's scope.
    pub fn set_base_filter(&self, filter: ToolFilter) -> Result<(), ToolScopeApplyError> {
        let mut state = self
            .state
            .write()
            .map_err(|_| ToolScopeApplyError::LockPoisoned)?;
        record_filter_witnesses(&mut state, &filter);
        state.durable_state.inherited_base_filter = filter;
        Ok(())
    }

    /// Replace the durable tool visibility state carried by this projection bridge.
    pub fn set_visibility_state(
        &self,
        visibility_state: SessionToolVisibilityState,
    ) -> Result<(), ToolScopeApplyError> {
        let mut state = self
            .state
            .write()
            .map_err(|_| ToolScopeApplyError::LockPoisoned)?;
        let next_revision = visibility_state
            .active_revision
            .max(visibility_state.staged_revision);
        state.durable_state = visibility_state;
        self.next_revision.store(next_revision, Ordering::SeqCst);
        Ok(())
    }

    /// Snapshot the current durable visibility state.
    pub fn visibility_state(&self) -> Result<SessionToolVisibilityState, ToolScopeApplyError> {
        self.state
            .read()
            .map(|state| state.durable_state.clone())
            .map_err(|_| ToolScopeApplyError::LockPoisoned)
    }

    /// Return the names currently visible to the session plane.
    pub fn visible_tool_names(&self) -> Result<BTreeSet<String>, ToolScopeApplyError> {
        self.visible_tools_result().map(|tools| {
            tools
                .iter()
                .map(|tool| tool.name.clone())
                .collect::<BTreeSet<_>>()
        })
    }

    /// Return whether the staged durable session filters would allow a session-plane tool name
    /// to become visible after the next boundary.
    pub fn staged_session_filters_allow_name(
        &self,
        name: &str,
    ) -> Result<bool, ToolScopeApplyError> {
        self.state
            .read()
            .map(|state| {
                Self::compose(&[
                    Self::effective_filter_for_current_projection(
                        &state,
                        &state.durable_state.inherited_base_filter,
                    ),
                    Self::effective_filter_for_current_projection(
                        &state,
                        &state.durable_state.staged_filter,
                    ),
                ])
                .allows(name)
            })
            .map_err(|_| ToolScopeApplyError::LockPoisoned)
    }

    /// Return the current base tool snapshot.
    pub fn base_tools_snapshot(&self) -> Result<Arc<[Arc<ToolDef>]>, ToolScopeApplyError> {
        self.state
            .read()
            .map(|state| Arc::clone(&state.base_tools))
            .map_err(|_| ToolScopeApplyError::LockPoisoned)
    }

    /// Return the current base tool names.
    pub fn base_tool_names(&self) -> Result<BTreeSet<String>, ToolScopeApplyError> {
        self.state
            .read()
            .map(|state| {
                state
                    .known_base_names
                    .iter()
                    .cloned()
                    .collect::<BTreeSet<_>>()
            })
            .map_err(|_| ToolScopeApplyError::LockPoisoned)
    }

    /// Return the currently configured active and staged revisions.
    pub fn revisions(&self) -> Result<(ToolScopeRevision, ToolScopeRevision), ToolScopeApplyError> {
        self.state
            .read()
            .map(|state| {
                (
                    ToolScopeRevision(state.durable_state.active_revision),
                    ToolScopeRevision(state.durable_state.staged_revision),
                )
            })
            .map_err(|_| ToolScopeApplyError::LockPoisoned)
    }

    /// Return any requested deferred names that are not currently present in the base snapshot.
    pub fn missing_requested_names(&self) -> Result<BTreeSet<String>, ToolScopeApplyError> {
        self.state
            .read()
            .map(|state| {
                state
                    .durable_state
                    .active_requested_deferred_names
                    .iter()
                    .filter(|name| !state.known_base_names.contains(name.as_str()))
                    .cloned()
                    .collect::<BTreeSet<_>>()
            })
            .map_err(|_| ToolScopeApplyError::LockPoisoned)
    }

    /// Return any durable filter names that are not currently present in the base snapshot.
    pub fn missing_filter_names(&self) -> Result<BTreeSet<String>, ToolScopeApplyError> {
        self.state
            .read()
            .map(|state| {
                durable_filter_names(&state.durable_state)
                    .into_iter()
                    .filter(|name| !state.known_base_names.contains(name.as_str()))
                    .collect::<BTreeSet<_>>()
            })
            .map_err(|_| ToolScopeApplyError::LockPoisoned)
    }

    /// Record durable requested deferred names for the next boundary.
    pub fn stage_requested_deferred_names(
        &self,
        names: BTreeSet<String>,
    ) -> Result<ToolScopeRevision, ToolScopeStageError> {
        let mut state = self
            .state
            .write()
            .map_err(|_| ToolScopeStageError::LockPoisoned)?;
        let revision = ToolScopeRevision(self.next_revision.fetch_add(1, Ordering::SeqCst) + 1);
        state.durable_state.staged_requested_deferred_names = names;
        state.durable_state.staged_revision = revision.0;
        Ok(revision)
    }

    /// Add durable requested deferred names for the next boundary.
    pub fn add_requested_deferred_names(
        &self,
        names: &BTreeSet<String>,
        witnesses: &std::collections::BTreeMap<String, ToolVisibilityWitness>,
    ) -> Result<ToolScopeRevision, ToolScopeStageError> {
        let mut state = self
            .state
            .write()
            .map_err(|_| ToolScopeStageError::LockPoisoned)?;
        let revision = ToolScopeRevision(self.next_revision.fetch_add(1, Ordering::SeqCst) + 1);
        state
            .durable_state
            .staged_requested_deferred_names
            .extend(names.iter().cloned());
        state.durable_state.requested_witnesses.extend(
            witnesses
                .iter()
                .map(|(name, witness)| (name.clone(), witness.clone())),
        );
        state.durable_state.staged_revision = revision.0;
        Ok(revision)
    }

    #[cfg(test)]
    pub(crate) fn inject_boundary_failure_once_for_test(&self) {
        self.fail_next_boundary_apply.store(true, Ordering::SeqCst);
    }
}

/// Thread-safe handle for staging external scope updates.
#[derive(Debug, Clone)]
pub struct ToolScopeHandle {
    state: Arc<RwLock<ToolScopeState>>,
    next_revision: Arc<AtomicU64>,
}

impl ToolScopeHandle {
    /// Stage an external filter update and return its monotonic revision.
    pub fn stage_external_filter(
        &self,
        filter: ToolFilter,
    ) -> Result<ToolScopeRevision, ToolScopeStageError> {
        let mut state = self
            .state
            .write()
            .map_err(|_| ToolScopeStageError::LockPoisoned)?;

        let mut known_names = state.known_base_names.clone();
        for control_name in &state.control_tool_names {
            known_names.remove(control_name);
        }
        known_names.extend(durable_filter_names(&state.durable_state));
        validate_filter(&filter, &known_names)?;

        let revision = ToolScopeRevision(self.next_revision.fetch_add(1, Ordering::SeqCst) + 1);
        record_filter_witnesses(&mut state, &filter);
        state.durable_state.staged_filter = filter;
        state.durable_state.staged_revision = revision.0;
        Ok(revision)
    }

    pub(crate) fn staged_revision(&self) -> Result<ToolScopeRevision, ToolScopeStageError> {
        self.state
            .read()
            .map(|state| ToolScopeRevision(state.durable_state.staged_revision))
            .map_err(|_| ToolScopeStageError::LockPoisoned)
    }

    /// Set or clear an ephemeral per-turn overlay.
    pub fn set_turn_overlay(
        &self,
        allow: Option<HashSet<String>>,
        deny: HashSet<String>,
    ) -> Result<(), ToolScopeStageError> {
        let mut state = self
            .state
            .write()
            .map_err(|_| ToolScopeStageError::LockPoisoned)?;

        if let Some(allow_set) = &allow {
            validate_filter(
                &ToolFilter::Allow(allow_set.clone()),
                &state.known_base_names,
            )?;
        }
        if !deny.is_empty() {
            validate_filter(&ToolFilter::Deny(deny.clone()), &state.known_base_names)?;
        }

        state.active_turn_allow = allow;
        state.active_turn_deny = deny;
        Ok(())
    }

    /// Clear ephemeral per-turn overlay.
    pub fn clear_turn_overlay(&self) {
        if let Ok(mut state) = self.state.write() {
            state.active_turn_allow = None;
            state.active_turn_deny.clear();
        }
    }
}

fn validate_filter(
    filter: &ToolFilter,
    known_base_names: &HashSet<String>,
) -> Result<(), ToolScopeStageError> {
    let Some(names) = filter.names() else {
        return Ok(());
    };

    let mut unknown: Vec<String> = names
        .iter()
        .filter(|name| !known_base_names.contains(*name))
        .cloned()
        .collect();

    if unknown.is_empty() {
        return Ok(());
    }

    unknown.sort_unstable();
    unknown.dedup();
    Err(ToolScopeStageError::UnknownTools { names: unknown })
}

fn durable_filter_names(state: &SessionToolVisibilityState) -> HashSet<String> {
    let mut names = HashSet::new();
    for filter in [
        &state.inherited_base_filter,
        &state.active_filter,
        &state.staged_filter,
    ] {
        if let Some(filter_names) = filter.names() {
            names.extend(filter_names.iter().cloned());
        }
    }
    names
}

fn record_filter_witnesses(state: &mut ToolScopeState, filter: &ToolFilter) {
    let Some(filter_names) = filter.names() else {
        return;
    };

    for name in filter_names {
        if let Some(tool) = state.base_tools.iter().find(|tool| tool.name == *name) {
            state.durable_state.filter_witnesses.insert(
                name.clone(),
                ToolVisibilityWitness {
                    stable_owner_key: stable_owner_key_for_tool(tool),
                    last_seen_provenance: tool.provenance.clone(),
                },
            );
        }
    }
}

fn sorted_names(names: &HashSet<String>) -> Vec<String> {
    let mut values = names.iter().cloned().collect::<Vec<_>>();
    values.sort_unstable();
    values
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::ToolScopeRevision;
    use super::{ToolFilter, ToolScope, ToolScopeStageError};
    use crate::types::{ToolDef, ToolProvenance, ToolSourceKind};
    use std::collections::HashSet;
    use std::sync::Arc;

    fn set(names: &[&str]) -> HashSet<String> {
        names.iter().map(|name| (*name).to_string()).collect()
    }

    fn tools(names: &[&str]) -> Arc<[Arc<ToolDef>]> {
        names
            .iter()
            .map(|name| {
                Arc::new(ToolDef {
                    name: (*name).to_string(),
                    description: format!("{name} tool"),
                    input_schema: serde_json::json!({ "type": "object" }),
                    provenance: None,
                })
            })
            .collect::<Vec<_>>()
            .into()
    }

    fn tool_with_provenance(name: &str, source_id: &str) -> Arc<ToolDef> {
        Arc::new(ToolDef {
            name: name.to_string(),
            description: format!("{name} tool"),
            input_schema: serde_json::json!({ "type": "object" }),
            provenance: Some(ToolProvenance {
                kind: ToolSourceKind::Callback,
                source_id: source_id.to_string(),
            }),
        })
    }

    #[test]
    fn stage_revision_is_monotonic() {
        let scope = ToolScope::new(tools(&["a", "b", "c"]));
        let handle = scope.handle();

        let first = handle
            .stage_external_filter(ToolFilter::Deny(set(&["a"])))
            .unwrap();
        let second = handle
            .stage_external_filter(ToolFilter::Allow(set(&["b", "c"])))
            .unwrap();

        assert!(second > first);
        assert_eq!(handle.staged_revision().unwrap(), second);
    }

    #[test]
    fn stage_rejects_unknown_tools() {
        let scope = ToolScope::new(tools(&["known"]));
        let handle = scope.handle();

        let err = handle
            .stage_external_filter(ToolFilter::Allow(set(&["known", "missing"])))
            .unwrap_err();

        assert_eq!(
            err,
            ToolScopeStageError::UnknownTools {
                names: vec!["missing".to_string()],
            }
        );
    }

    #[test]
    fn control_tools_remain_visible_and_unfilterable() {
        let scope = ToolScope::new_with_control_tool_names(
            tools(&["visible", "tool_catalog_search"]),
            set(&["tool_catalog_search"]),
        );
        let handle = scope.handle();

        let err = handle
            .stage_external_filter(ToolFilter::Deny(set(&["tool_catalog_search"])))
            .unwrap_err();
        assert_eq!(
            err,
            ToolScopeStageError::UnknownTools {
                names: vec!["tool_catalog_search".to_string()],
            }
        );

        handle
            .stage_external_filter(ToolFilter::Deny(set(&["visible"])))
            .unwrap();
        let applied = scope
            .apply_staged(tools(&["visible", "tool_catalog_search"]))
            .unwrap();

        assert_eq!(
            applied.visible_names,
            vec!["tool_catalog_search".to_string()],
            "control tools should remain visible even when the session plane is filtered out"
        );
    }

    #[test]
    fn deferred_tools_stay_hidden_until_requested_boundary_applies() {
        let scope = ToolScope::new_with_projection_names(
            tools(&["visible", "deferred"]),
            HashSet::new(),
            set(&["deferred"]),
        );

        assert_eq!(
            scope.visible_tool_names().unwrap(),
            ["visible".to_string()].into_iter().collect(),
            "deferred tools should be hidden until requested"
        );

        scope
            .add_requested_deferred_names(
                &["deferred".to_string()].into_iter().collect(),
                &std::collections::BTreeMap::new(),
            )
            .unwrap();

        assert_eq!(
            scope.visible_tool_names().unwrap(),
            ["visible".to_string()].into_iter().collect(),
            "staged requests should not publish deferred tools before the next boundary"
        );

        let applied = scope.apply_staged(tools(&["visible", "deferred"])).unwrap();
        assert_eq!(
            applied.visible_names,
            vec!["visible".to_string(), "deferred".to_string()],
            "the next boundary should promote requested deferred tools into the visible set"
        );
    }

    #[test]
    fn late_deferred_names_stay_hidden_until_requested_after_projection_refresh() {
        let scope = ToolScope::new(tools(&["visible"]));

        let late_deferred = tools(&["visible", "late_deferred"]);
        let applied = scope
            .apply_staged_projection(late_deferred, HashSet::new(), set(&["late_deferred"]))
            .expect("projection refresh should succeed");

        assert_eq!(
            applied.visible_names,
            vec!["visible".to_string()],
            "late deferred additions should stay hidden until explicitly requested"
        );
    }

    #[test]
    fn snapshot_reflects_active_and_staged_scope_state() {
        let scope = ToolScope::new(tools(&["a", "b", "c"]));
        let handle = scope.handle();

        handle
            .stage_external_filter(ToolFilter::Deny(set(&["a"])))
            .unwrap();
        handle
            .set_turn_overlay(Some(set(&["b", "c"])), set(&["c"]))
            .unwrap();

        let snapshot = scope.snapshot().expect("snapshot should be available");

        assert_eq!(
            snapshot.known_base_names,
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
        assert_eq!(snapshot.visible_names, vec!["b".to_string()]);
        assert_eq!(snapshot.base_filter, ToolFilter::All);
        assert_eq!(snapshot.active_external_filter, ToolFilter::All);
        assert_eq!(
            snapshot.active_turn_allow,
            Some(vec!["b".to_string(), "c".to_string()])
        );
        assert_eq!(snapshot.active_turn_deny, vec!["c".to_string()]);
        assert_eq!(snapshot.active_revision, ToolScopeRevision(0));
        assert_eq!(snapshot.staged_revision, ToolScopeRevision(1));
        assert_eq!(
            snapshot.staged_external_filter,
            ToolFilter::Deny(set(&["a"]))
        );
    }

    #[test]
    fn filter_algebra_is_most_restrictive() {
        let allow_a_b = ToolFilter::Allow(set(&["a", "b"]));
        let allow_b_c = ToolFilter::Allow(set(&["b", "c"]));
        let deny_c = ToolFilter::Deny(set(&["c"]));
        let deny_b = ToolFilter::Deny(set(&["b"]));

        // Allow lists intersect.
        let composed_allow = ToolScope::compose(&[allow_a_b.clone(), allow_b_c]);
        assert!(composed_allow.allows("b"));
        assert!(!composed_allow.allows("a"));
        assert!(!composed_allow.allows("c"));

        // Deny lists union.
        let composed_deny = ToolScope::compose(&[deny_c, deny_b.clone()]);
        assert!(!composed_deny.allows("b"));
        assert!(!composed_deny.allows("c"));
        assert!(composed_deny.allows("a"));

        // Deny wins after allow.
        let composed_precedence = ToolScope::compose(&[allow_a_b, deny_b]);
        assert!(composed_precedence.allows("a"));
        assert!(!composed_precedence.allows("b"));
    }

    #[test]
    fn staged_update_is_boundary_only_until_apply_staged() {
        let scope = ToolScope::new(tools(&["visible", "secret"]));
        let handle = scope.handle();

        assert_eq!(
            scope
                .visible_tools()
                .iter()
                .map(|t| t.name.clone())
                .collect::<Vec<_>>(),
            vec!["visible".to_string(), "secret".to_string()]
        );

        handle
            .stage_external_filter(ToolFilter::Deny(set(&["secret"])))
            .unwrap();

        // Still unchanged until boundary apply.
        assert_eq!(
            scope
                .visible_tools()
                .iter()
                .map(|t| t.name.clone())
                .collect::<Vec<_>>(),
            vec!["visible".to_string(), "secret".to_string()]
        );

        let applied = scope
            .apply_staged(tools(&["visible", "secret"]))
            .expect("boundary apply should succeed");
        assert!(applied.visible_changed());
        assert_eq!(applied.visible_names, vec!["visible".to_string()]);
        assert_eq!(
            scope
                .visible_tools()
                .iter()
                .map(|t| t.name.clone())
                .collect::<Vec<_>>(),
            vec!["visible".to_string()]
        );
    }

    #[test]
    fn structural_base_change_preserves_dormant_filter_names() {
        let scope = ToolScope::new(tools(&["a", "b", "c"]));
        let handle = scope.handle();

        handle
            .stage_external_filter(ToolFilter::Deny(set(&["c"])))
            .unwrap();
        scope
            .apply_staged(tools(&["a", "b", "c"]))
            .expect("initial apply should succeed");
        assert_eq!(
            scope
                .visible_tools()
                .iter()
                .map(|t| t.name.clone())
                .collect::<Vec<_>>(),
            vec!["a".to_string(), "b".to_string()]
        );

        // Pending filter still references `c`, and the durable state should
        // preserve that dormant intent even after the base snapshot shrinks.
        handle
            .stage_external_filter(ToolFilter::Allow(set(&["b", "c"])))
            .unwrap();

        let applied = scope
            .apply_staged(tools(&["a", "b"]))
            .expect("boundary apply after structural delta should succeed");

        assert!(applied.base_changed());
        assert_eq!(applied.visible_names, vec!["b".to_string()]);
        assert_eq!(
            scope
                .visible_tools()
                .iter()
                .map(|t| t.name.clone())
                .collect::<Vec<_>>(),
            vec!["b".to_string()]
        );
        let visibility_state = scope.visibility_state().expect("visibility state");
        assert_eq!(
            visibility_state.active_filter,
            ToolFilter::Allow(set(&["b", "c"])),
            "missing names should remain in durable active filter state"
        );
        assert_eq!(
            visibility_state.staged_filter,
            ToolFilter::Allow(set(&["b", "c"])),
            "missing names should remain in durable staged filter state"
        );
    }

    #[test]
    fn requested_witness_mismatch_prevents_rebinding_a_dormant_deferred_name() {
        let requested = tool_with_provenance("deferred", "owner-a");
        let rebound = tool_with_provenance("deferred", "owner-b");
        let scope = ToolScope::new_with_projection_names(
            vec![Arc::clone(&requested)].into(),
            HashSet::new(),
            set(&["deferred"]),
        );

        scope
            .add_requested_deferred_names(
                &["deferred".to_string()].into_iter().collect(),
                &[(
                    "deferred".to_string(),
                    crate::ToolVisibilityWitness {
                        stable_owner_key: Some("callback:owner-a".to_string()),
                        last_seen_provenance: requested.provenance.clone(),
                    },
                )]
                .into_iter()
                .collect(),
            )
            .unwrap();
        scope
            .apply_staged_projection(
                vec![Arc::clone(&requested)].into(),
                HashSet::new(),
                set(&["deferred"]),
            )
            .unwrap();
        assert_eq!(
            scope.visible_tool_names().unwrap(),
            ["deferred".to_string()].into_iter().collect(),
            "the matching owner should remain visible"
        );

        scope
            .apply_staged_projection(
                vec![Arc::clone(&rebound)].into(),
                HashSet::new(),
                set(&["deferred"]),
            )
            .unwrap();
        assert!(
            scope.visible_tool_names().unwrap().is_empty(),
            "a different owner must not inherit prior deferred visibility intent"
        );
    }

    #[test]
    fn filter_witness_mismatch_prevents_rebinding_a_dormant_filter_name() {
        let original = tool_with_provenance("a", "owner-a");
        let rebound = tool_with_provenance("a", "owner-b");
        let visible = tool_with_provenance("b", "owner-b");
        let scope = ToolScope::new(vec![Arc::clone(&original), Arc::clone(&visible)].into());
        let handle = scope.handle();

        handle
            .stage_external_filter(ToolFilter::Deny(set(&["a"])))
            .unwrap();
        scope
            .apply_staged(vec![Arc::clone(&original), Arc::clone(&visible)].into())
            .unwrap();
        assert_eq!(
            scope.visible_tool_names().unwrap(),
            ["b".to_string()].into_iter().collect(),
            "the original owner should be hidden by the deny filter"
        );

        scope
            .apply_staged(vec![Arc::clone(&rebound), Arc::clone(&visible)].into())
            .unwrap();
        assert_eq!(
            scope.visible_tool_names().unwrap(),
            ["a".to_string(), "b".to_string()].into_iter().collect(),
            "a different owner must not inherit the dormant filter intent"
        );
    }

    #[test]
    fn turn_overlay_is_ephemeral_and_most_restrictive() {
        let scope = ToolScope::new(tools(&["a", "b", "c"]));
        let handle = scope.handle();

        handle
            .stage_external_filter(ToolFilter::Allow(set(&["a", "b"])))
            .unwrap();
        scope
            .apply_staged(tools(&["a", "b", "c"]))
            .expect("initial apply should succeed");

        assert_eq!(
            scope
                .visible_tools()
                .iter()
                .map(|t| t.name.clone())
                .collect::<Vec<_>>(),
            vec!["a".to_string(), "b".to_string()]
        );

        handle
            .set_turn_overlay(Some(set(&["b", "c"])), set(&["b"]))
            .unwrap();
        assert_eq!(
            scope
                .visible_tools()
                .iter()
                .map(|t| t.name.clone())
                .collect::<Vec<_>>(),
            Vec::<String>::new(),
            "external allow(a,b) + turn allow(b,c) + turn deny(b) should be empty"
        );

        handle.clear_turn_overlay();
        assert_eq!(
            scope
                .visible_tools()
                .iter()
                .map(|t| t.name.clone())
                .collect::<Vec<_>>(),
            vec!["a".to_string(), "b".to_string()]
        );
    }

    #[test]
    fn set_base_filter_restricts_visible_tools() {
        let scope = ToolScope::new(tools(&["a", "b", "c"]));

        // All visible initially
        assert_eq!(
            scope
                .visible_tools()
                .iter()
                .map(|t| t.name.clone())
                .collect::<Vec<_>>(),
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );

        // Set base filter to allow only a and b
        scope
            .set_base_filter(ToolFilter::Allow(set(&["a", "b"])))
            .unwrap();

        assert_eq!(
            scope
                .visible_tools()
                .iter()
                .map(|t| t.name.clone())
                .collect::<Vec<_>>(),
            vec!["a".to_string(), "b".to_string()]
        );
    }

    #[test]
    fn set_base_filter_composes_with_external_filter() {
        let scope = ToolScope::new(tools(&["a", "b", "c"]));
        let handle = scope.handle();

        // Base restricts to a and b
        scope
            .set_base_filter(ToolFilter::Allow(set(&["a", "b"])))
            .unwrap();

        // External further restricts to b and c
        handle
            .stage_external_filter(ToolFilter::Allow(set(&["b", "c"])))
            .unwrap();
        scope.apply_staged(tools(&["a", "b", "c"])).unwrap();

        // Most-restrictive: intersection = b only
        assert_eq!(
            scope
                .visible_tools()
                .iter()
                .map(|t| t.name.clone())
                .collect::<Vec<_>>(),
            vec!["b".to_string()]
        );
    }

    #[test]
    fn inherited_metadata_key_is_distinct_from_external() {
        assert_ne!(
            super::INHERITED_TOOL_FILTER_METADATA_KEY,
            super::EXTERNAL_TOOL_FILTER_METADATA_KEY
        );
    }
}
