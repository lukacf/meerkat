//! Tool visibility scope and external filter staging.

use crate::session::{
    DeferredToolLoadAuthority, SessionToolVisibilityState, ToolVisibilityWitness,
    WitnessedToolFilter,
};
use crate::tool_catalog::stable_owner_key_for_tool;
use crate::types::{ToolDef, ToolNameSet};
use std::collections::BTreeSet;
use std::collections::HashSet;
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

/// Visibility filter for tools.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ToolFilter {
    /// All tools are visible.
    #[default]
    All,
    /// Only listed tools are visible.
    Allow(ToolNameSet),
    /// Listed tools are hidden.
    Deny(ToolNameSet),
}

impl ToolFilter {
    fn names(&self) -> Option<&ToolNameSet> {
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
    pub capability_base_filter: ToolFilter,
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

/// Closed cause set for external tool surface failures and call rejections.
///
/// These codes are the stable external projection for MCP/router callers. Keep
/// `as_str` and serde in snake_case because older consumers already observe
/// these string codes at the surface boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalToolSurfaceFailureCause {
    PendingFailed,
    SurfaceDraining,
    SurfaceUnavailable,
}

impl ExternalToolSurfaceFailureCause {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PendingFailed => "pending_failed",
            Self::SurfaceDraining => "surface_draining",
            Self::SurfaceUnavailable => "surface_unavailable",
        }
    }
}

impl fmt::Display for ExternalToolSurfaceFailureCause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
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
    #[error("Missing tool visibility witness(es) for deferred tool(s): {names:?}")]
    MissingWitnesses { names: Vec<String> },
    #[error("Missing tool visibility witness(es) for filter tool(s): {names:?}")]
    MissingFilterWitnesses { names: Vec<String> },
    #[error("Invalid tool visibility witness(es) for deferred tool(s): {names:?}")]
    InvalidWitnesses { names: Vec<String> },
    #[error("Invalid tool visibility witness(es) for filter tool(s): {names:?}")]
    InvalidFilterWitnesses { names: Vec<String> },
    #[error("Tool scope state lock poisoned")]
    LockPoisoned,
    #[error("Tool visibility owner error: {message}")]
    Owner { message: String },
}

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum ToolScopeApplyError {
    #[error("Tool scope state lock poisoned")]
    LockPoisoned,
    #[error("Injected boundary failure for testing")]
    InjectedFailure,
    #[error("Tool visibility owner error: {message}")]
    Owner { message: String },
}

/// Canonical owner for durable tool-visibility state.
///
/// `ToolScope` reads committed/staged visibility from this trait and routes all
/// durable visibility mutations through it, so the projection bridge does not
/// become a competing owner.
pub trait ToolVisibilityOwner: Send + Sync {
    fn visibility_state(&self) -> Result<SessionToolVisibilityState, ToolScopeApplyError>;

    fn replace_visibility_state(
        &self,
        visibility_state: SessionToolVisibilityState,
    ) -> Result<(), ToolScopeApplyError>;

    fn stage_persistent_filter(
        &self,
        filter: ToolFilter,
        witnesses: std::collections::BTreeMap<String, ToolVisibilityWitness>,
    ) -> Result<ToolScopeRevision, ToolScopeStageError>;

    fn stage_requested_deferred_names(
        &self,
        names: BTreeSet<String>,
    ) -> Result<ToolScopeRevision, ToolScopeStageError>;

    fn request_deferred_tools(
        &self,
        authorities: Vec<DeferredToolLoadAuthority>,
    ) -> Result<ToolScopeRevision, ToolScopeStageError>;

    fn replace_deferred_tool_authority_catalog(
        &self,
        _catalog: std::collections::BTreeMap<String, ToolVisibilityWitness>,
    ) {
    }

    fn replace_filter_tool_authority_catalog(
        &self,
        _catalog: std::collections::BTreeMap<String, ToolVisibilityWitness>,
    ) {
    }

    fn requires_filter_witnesses(&self) -> bool {
        false
    }

    fn boundary_applied(&self) -> Result<SessionToolVisibilityState, ToolScopeApplyError>;
}

/// Local in-process owner used when no runtime-backed MeerkatMachine owner is supplied.
#[derive(Debug, Clone, Default)]
pub struct LocalToolVisibilityOwner {
    state: Arc<RwLock<SessionToolVisibilityState>>,
    next_revision: Arc<AtomicU64>,
    deferred_authority_catalog:
        Arc<RwLock<std::collections::BTreeMap<String, ToolVisibilityWitness>>>,
}

impl LocalToolVisibilityOwner {
    pub fn new() -> Self {
        Self {
            state: Arc::new(RwLock::new(SessionToolVisibilityState::default())),
            next_revision: Arc::new(AtomicU64::new(0)),
            deferred_authority_catalog: Arc::new(RwLock::new(std::collections::BTreeMap::new())),
        }
    }
}

impl ToolVisibilityOwner for LocalToolVisibilityOwner {
    fn visibility_state(&self) -> Result<SessionToolVisibilityState, ToolScopeApplyError> {
        self.state
            .read()
            .map(|state| state.clone())
            .map_err(|_| ToolScopeApplyError::LockPoisoned)
    }

    fn replace_visibility_state(
        &self,
        mut visibility_state: SessionToolVisibilityState,
    ) -> Result<(), ToolScopeApplyError> {
        let deferred_names = deferred_authority_names_for_visibility_state(&visibility_state);
        if !deferred_names.is_empty() {
            let canonical_authorities = {
                let catalog = self
                    .deferred_authority_catalog
                    .read()
                    .map_err(|_| ToolScopeApplyError::LockPoisoned)?;
                canonical_deferred_authorities_for_names(
                    &deferred_names,
                    &visibility_state.requested_witnesses,
                    &catalog,
                )
                .map_err(|err| ToolScopeApplyError::Owner {
                    message: format!("invalid deferred visibility authority: {err}"),
                })?
            };
            visibility_state
                .requested_witnesses
                .extend(canonical_authorities);
        }
        let mut state = self
            .state
            .write()
            .map_err(|_| ToolScopeApplyError::LockPoisoned)?;
        let next_revision = visibility_state
            .active_revision
            .max(visibility_state.staged_revision);
        *state = visibility_state;
        self.next_revision.store(next_revision, Ordering::SeqCst);
        Ok(())
    }

    fn stage_persistent_filter(
        &self,
        filter: ToolFilter,
        witnesses: std::collections::BTreeMap<String, ToolVisibilityWitness>,
    ) -> Result<ToolScopeRevision, ToolScopeStageError> {
        let mut state = self
            .state
            .write()
            .map_err(|_| ToolScopeStageError::LockPoisoned)?;
        let revision = ToolScopeRevision(self.next_revision.fetch_add(1, Ordering::SeqCst) + 1);
        state.staged_filter = filter;
        state.filter_witnesses.extend(witnesses);
        state.staged_revision = revision.0;
        Ok(revision)
    }

    fn stage_requested_deferred_names(
        &self,
        names: BTreeSet<String>,
    ) -> Result<ToolScopeRevision, ToolScopeStageError> {
        if !names.is_empty() {
            return Err(ToolScopeStageError::MissingWitnesses {
                names: names.into_iter().collect(),
            });
        }
        let mut state = self
            .state
            .write()
            .map_err(|_| ToolScopeStageError::LockPoisoned)?;
        let revision = ToolScopeRevision(self.next_revision.fetch_add(1, Ordering::SeqCst) + 1);
        state.staged_requested_deferred_names = names;
        state.staged_revision = revision.0;
        Ok(revision)
    }

    fn request_deferred_tools(
        &self,
        authorities: Vec<DeferredToolLoadAuthority>,
    ) -> Result<ToolScopeRevision, ToolScopeStageError> {
        let mut state = self
            .state
            .write()
            .map_err(|_| ToolScopeStageError::LockPoisoned)?;
        let authorities = deferred_load_authority_map(&authorities)?;
        let names = authorities.keys().cloned().collect::<BTreeSet<_>>();
        let extended = state
            .staged_requested_deferred_names
            .union(&names)
            .cloned()
            .collect();
        let mut combined_witnesses = state.requested_witnesses.clone();
        combined_witnesses.extend(authorities);
        let canonical_authorities = {
            let catalog = self
                .deferred_authority_catalog
                .read()
                .map_err(|_| ToolScopeStageError::LockPoisoned)?;
            canonical_deferred_authorities_for_names(&extended, &combined_witnesses, &catalog)?
        };
        combined_witnesses.extend(canonical_authorities);
        let revision = ToolScopeRevision(self.next_revision.fetch_add(1, Ordering::SeqCst) + 1);
        state.staged_requested_deferred_names = extended;
        state.requested_witnesses = combined_witnesses;
        state.staged_revision = revision.0;
        Ok(revision)
    }

    fn replace_deferred_tool_authority_catalog(
        &self,
        catalog: std::collections::BTreeMap<String, ToolVisibilityWitness>,
    ) {
        if let Ok(mut guard) = self.deferred_authority_catalog.write() {
            *guard = catalog;
        }
    }

    fn boundary_applied(&self) -> Result<SessionToolVisibilityState, ToolScopeApplyError> {
        let mut state = self
            .state
            .write()
            .map_err(|_| ToolScopeApplyError::LockPoisoned)?;
        state.active_filter = state.staged_filter.clone();
        state.active_requested_deferred_names = state.staged_requested_deferred_names.clone();
        state.active_revision = state.staged_revision;
        Ok(state.clone())
    }
}

#[derive(Debug, Clone)]
struct ToolScopeState {
    base_tools: Arc<[Arc<ToolDef>]>,
    known_base_names: ToolNameSet,
    control_tool_names: ToolNameSet,
    deferred_tool_names: ToolNameSet,
    active_turn_allow: Option<ToolNameSet>,
    active_turn_deny: ToolNameSet,
}

/// Composed filter representation using most-restrictive semantics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposedToolFilter {
    allow: Option<ToolNameSet>,
    deny: ToolNameSet,
}

impl ComposedToolFilter {
    pub fn allows(&self, name: &str) -> bool {
        let allowed = self.allow.as_ref().is_none_or(|set| set.contains(name));
        allowed && !self.deny.contains(name)
    }
}

/// Runtime tool scope used to determine provider-visible tools.
#[derive(Clone)]
pub struct ToolScope {
    state: Arc<RwLock<ToolScopeState>>,
    visibility_owner: Arc<dyn ToolVisibilityOwner>,
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
        Self::new_with_visibility_owner(
            base_tools,
            control_tool_names,
            deferred_tool_names,
            Arc::new(LocalToolVisibilityOwner::new()),
        )
    }

    /// Build a scope with an explicit durable visibility owner.
    pub fn new_with_visibility_owner(
        base_tools: Arc<[Arc<ToolDef>]>,
        control_tool_names: HashSet<String>,
        deferred_tool_names: HashSet<String>,
        visibility_owner: Arc<dyn ToolVisibilityOwner>,
    ) -> Self {
        let deferred_tool_names: ToolNameSet = deferred_tool_names.into_iter().collect();
        visibility_owner.replace_deferred_tool_authority_catalog(
            deferred_authority_catalog_for_base_tools(&base_tools, &deferred_tool_names),
        );
        visibility_owner.replace_filter_tool_authority_catalog(
            filter_authority_catalog_for_base_tools(&base_tools),
        );
        let known_base_names: ToolNameSet = base_tools
            .iter()
            .map(|tool| tool.name.to_string())
            .collect();

        Self {
            state: Arc::new(RwLock::new(ToolScopeState {
                base_tools,
                known_base_names,
                control_tool_names: control_tool_names.into_iter().collect(),
                deferred_tool_names,
                active_turn_allow: None,
                active_turn_deny: ToolNameSet::new(),
            })),
            visibility_owner,
            fail_next_boundary_apply: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Returns the currently visible tools using base + active external filter composition.
    pub fn visible_tools(&self) -> Arc<[Arc<ToolDef>]> {
        match self.visible_tools_result() {
            Ok(tools) => tools,
            Err(_) => Vec::<Arc<ToolDef>>::new().into(),
        }
    }

    /// Returns current visible tools, or an explicit error for boundary fail-safe handling.
    pub fn visible_tools_result(&self) -> Result<Arc<[Arc<ToolDef>]>, ToolScopeApplyError> {
        let state = self
            .state
            .read()
            .map_err(|_| ToolScopeApplyError::LockPoisoned)?;
        let visibility_state = self.visibility_owner.visibility_state()?;
        let require_filter_witnesses = self.visibility_owner.requires_filter_witnesses();

        let composed =
            Self::compose_state_filters(&state, &visibility_state, require_filter_witnesses);

        Ok(state
            .base_tools
            .iter()
            .filter(|tool| {
                state.control_tool_names.contains(tool.name.as_str())
                    || (Self::is_requested_session_tool_visible(
                        &state,
                        &visibility_state,
                        tool.as_ref(),
                    ) && composed.allows(tool.name.as_str()))
            })
            .map(Arc::clone)
            .collect::<Vec<_>>()
            .into())
    }

    /// Return a handle for thread-safe staged external updates.
    pub fn handle(&self) -> ToolScopeHandle {
        ToolScopeHandle {
            state: Arc::clone(&self.state),
            visibility_owner: Arc::clone(&self.visibility_owner),
        }
    }

    /// Snapshot the current live scope state for diagnostics.
    pub fn snapshot(&self) -> Option<ToolScopeSnapshot> {
        let state = self.state.read().ok()?;
        let visibility_state = self.visibility_owner.visibility_state().ok()?;
        let require_filter_witnesses = self.visibility_owner.requires_filter_witnesses();
        Some(ToolScopeSnapshot {
            known_base_names: sorted_names(&state.known_base_names),
            visible_names: Self::visible_names_for_state(
                &state,
                &visibility_state,
                require_filter_witnesses,
            ),
            capability_base_filter: visibility_state.capability_base_filter.clone(),
            base_filter: visibility_state.inherited_base_filter.clone(),
            active_external_filter: visibility_state.active_filter.clone(),
            active_turn_allow: state.active_turn_allow.as_ref().map(sorted_names),
            active_turn_deny: sorted_names(&state.active_turn_deny),
            active_revision: ToolScopeRevision(visibility_state.active_revision),
            staged_external_filter: visibility_state.staged_filter.clone(),
            staged_revision: ToolScopeRevision(visibility_state.staged_revision),
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
        let previous_visibility_state = self.visibility_owner.visibility_state()?;
        let visibility_state = self.promote_staged_visibility()?;
        self.apply_staged_projection_with_previous(
            new_base_tools,
            control_tool_names,
            deferred_tool_names,
            &previous_visibility_state,
            &visibility_state,
        )
    }

    /// Promote the durable staged visibility state to the active boundary state.
    pub fn promote_staged_visibility(
        &self,
    ) -> Result<SessionToolVisibilityState, ToolScopeApplyError> {
        self.visibility_owner.boundary_applied()
    }

    /// Atomically apply refreshed projection inputs against a supplied boundary state.
    pub fn apply_staged_projection(
        &self,
        new_base_tools: Arc<[Arc<ToolDef>]>,
        control_tool_names: HashSet<String>,
        deferred_tool_names: HashSet<String>,
        visibility_state: &SessionToolVisibilityState,
    ) -> Result<ToolScopeBoundaryResult, ToolScopeApplyError> {
        let previous_visibility_state = self.visibility_owner.visibility_state()?;
        self.apply_staged_projection_with_previous(
            new_base_tools,
            control_tool_names.into_iter().collect(),
            deferred_tool_names.into_iter().collect(),
            &previous_visibility_state,
            visibility_state,
        )
    }

    pub(crate) fn apply_staged_projection_with_previous(
        &self,
        new_base_tools: Arc<[Arc<ToolDef>]>,
        control_tool_names: ToolNameSet,
        deferred_tool_names: ToolNameSet,
        previous_visibility_state: &SessionToolVisibilityState,
        visibility_state: &SessionToolVisibilityState,
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
        let require_filter_witnesses = self.visibility_owner.requires_filter_witnesses();

        let previous_base_names = state.known_base_names.clone();
        let previous_visible_names = Self::visible_names_for_state(
            &state,
            previous_visibility_state,
            require_filter_witnesses,
        );
        let previous_active_revision = ToolScopeRevision(previous_visibility_state.active_revision);

        state.base_tools = new_base_tools;
        state.control_tool_names = control_tool_names;
        state.deferred_tool_names = deferred_tool_names;
        state.known_base_names = state
            .base_tools
            .iter()
            .map(|tool| tool.name.to_string())
            .collect::<ToolNameSet>();

        let known_base_names = state.known_base_names.clone();
        if let Some(allow) = state.active_turn_allow.as_mut() {
            allow.retain(|name| known_base_names.contains(name.as_str()));
        }
        state
            .active_turn_deny
            .retain(|name| known_base_names.contains(name.as_str()));

        let tools =
            Self::visible_tools_for_state(&state, visibility_state, require_filter_witnesses);
        let visible_names = tools
            .iter()
            .map(|tool| tool.name.to_string())
            .collect::<Vec<_>>();
        self.visibility_owner
            .replace_deferred_tool_authority_catalog(deferred_authority_catalog_for_base_tools(
                &state.base_tools,
                &state.deferred_tool_names,
            ));
        self.visibility_owner.replace_filter_tool_authority_catalog(
            filter_authority_catalog_for_base_tools(&state.base_tools),
        );

        Ok(ToolScopeBoundaryResult {
            previous_base_names: previous_base_names.to_string_set(),
            current_base_names: state.known_base_names.to_string_set(),
            previous_visible_names,
            visible_names,
            previous_active_revision,
            applied_revision: ToolScopeRevision(visibility_state.active_revision),
            tools,
        })
    }

    /// Compose filters with most-restrictive semantics.
    pub fn compose(filters: &[ToolFilter]) -> ComposedToolFilter {
        let mut allow: Option<ToolNameSet> = None;
        let mut deny = ToolNameSet::new();

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
    fn allow_intersection(left: &ToolNameSet, right: &ToolNameSet) -> ToolNameSet {
        left.iter()
            .filter(|name| right.contains(name.as_str()))
            .cloned()
            .collect()
    }

    /// Helper: union for deny-list composition.
    fn deny_union(left: &ToolNameSet, right: &ToolNameSet) -> ToolNameSet {
        let mut union = left.clone();
        union.extend(right.iter().cloned());
        union
    }

    fn visible_names_for_state(
        state: &ToolScopeState,
        visibility_state: &SessionToolVisibilityState,
        require_filter_witnesses: bool,
    ) -> Vec<String> {
        let tools =
            Self::visible_tools_for_state(state, visibility_state, require_filter_witnesses);
        tools.iter().map(|tool| tool.name.to_string()).collect()
    }

    fn visible_tools_for_state(
        state: &ToolScopeState,
        visibility_state: &SessionToolVisibilityState,
        require_filter_witnesses: bool,
    ) -> Arc<[Arc<ToolDef>]> {
        let composed =
            Self::compose_state_filters(state, visibility_state, require_filter_witnesses);

        state
            .base_tools
            .iter()
            .filter(|tool| {
                state.control_tool_names.contains(tool.name.as_str())
                    || (Self::is_requested_session_tool_visible(
                        state,
                        visibility_state,
                        tool.as_ref(),
                    ) && composed.allows(tool.name.as_str()))
            })
            .map(Arc::clone)
            .collect::<Vec<_>>()
            .into()
    }

    fn compose_state_filters(
        state: &ToolScopeState,
        visibility_state: &SessionToolVisibilityState,
        require_filter_witnesses: bool,
    ) -> ComposedToolFilter {
        let mut filters = vec![
            Self::effective_filter_for_current_projection(
                state,
                visibility_state,
                &visibility_state.capability_base_filter,
                false,
            ),
            Self::effective_inherited_filter_for_current_projection(
                state,
                visibility_state,
                &visibility_state.inherited_base_filter,
            ),
            Self::effective_filter_for_current_projection(
                state,
                visibility_state,
                &visibility_state.active_filter,
                require_filter_witnesses,
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

    fn requested_witness_matches_tool(
        witness: Option<&ToolVisibilityWitness>,
        tool: &ToolDef,
    ) -> bool {
        witness.is_some_and(|witness| {
            witness.has_provenance_identity_witness()
                && Self::witness_matches_tool(Some(witness), tool)
        })
    }

    fn filter_name_applies(
        state: &ToolScopeState,
        visibility_state: &SessionToolVisibilityState,
        name: &str,
        require_filter_witnesses: bool,
    ) -> bool {
        let witness = visibility_state.filter_witnesses.get(name);
        Self::current_projection_tool(state, name).is_none_or(|tool| match witness {
            Some(witness) => {
                witness.has_identity_witness() && Self::witness_matches_tool(Some(witness), tool)
            }
            None => !require_filter_witnesses,
        })
    }

    fn inherited_filter_name_applies(
        state: &ToolScopeState,
        visibility_state: &SessionToolVisibilityState,
        name: &str,
    ) -> bool {
        Self::current_projection_tool(state, name).is_none_or(|tool| {
            match visibility_state.filter_witnesses.get(name) {
                Some(witness) => {
                    witness.has_identity_witness()
                        && Self::witness_matches_tool(Some(witness), tool)
                }
                None => true,
            }
        })
    }

    fn effective_filter_for_current_projection(
        state: &ToolScopeState,
        visibility_state: &SessionToolVisibilityState,
        filter: &ToolFilter,
        require_filter_witnesses: bool,
    ) -> ToolFilter {
        match filter {
            ToolFilter::All => ToolFilter::All,
            ToolFilter::Allow(names) => ToolFilter::Allow(
                names
                    .iter()
                    .filter(|name| {
                        Self::filter_name_applies(
                            state,
                            visibility_state,
                            name.as_str(),
                            require_filter_witnesses,
                        )
                    })
                    .cloned()
                    .collect(),
            ),
            ToolFilter::Deny(names) => ToolFilter::Deny(
                names
                    .iter()
                    .filter(|name| {
                        Self::filter_name_applies(
                            state,
                            visibility_state,
                            name.as_str(),
                            require_filter_witnesses,
                        )
                    })
                    .cloned()
                    .collect(),
            ),
        }
    }

    fn effective_inherited_filter_for_current_projection(
        state: &ToolScopeState,
        visibility_state: &SessionToolVisibilityState,
        filter: &ToolFilter,
    ) -> ToolFilter {
        match filter {
            ToolFilter::All => ToolFilter::All,
            ToolFilter::Allow(names) => ToolFilter::Allow(
                names
                    .iter()
                    .filter(|name| {
                        Self::inherited_filter_name_applies(state, visibility_state, name.as_str())
                    })
                    .cloned()
                    .collect(),
            ),
            ToolFilter::Deny(names) => ToolFilter::Deny(
                names
                    .iter()
                    .filter(|name| {
                        Self::inherited_filter_name_applies(state, visibility_state, name.as_str())
                    })
                    .cloned()
                    .collect(),
            ),
        }
    }

    fn is_requested_session_tool_visible(
        state: &ToolScopeState,
        visibility_state: &SessionToolVisibilityState,
        tool: &ToolDef,
    ) -> bool {
        if !state.deferred_tool_names.contains(tool.name.as_str()) {
            return true;
        }
        visibility_state
            .active_requested_deferred_names
            .contains(tool.name.as_str())
            && Self::requested_witness_matches_tool(
                visibility_state.requested_witnesses.get(tool.name.as_str()),
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
        let state = self
            .state
            .read()
            .map_err(|_| ToolScopeApplyError::LockPoisoned)?;
        let mut visibility_state = self.visibility_owner.visibility_state()?;
        extend_filter_witnesses(
            &state.base_tools,
            &mut visibility_state.filter_witnesses,
            &filter,
        );
        visibility_state.inherited_base_filter = filter;
        self.visibility_owner
            .replace_visibility_state(visibility_state)
    }

    /// Replace the durable tool visibility state carried by this projection bridge.
    pub fn set_visibility_state(
        &self,
        visibility_state: SessionToolVisibilityState,
    ) -> Result<(), ToolScopeApplyError> {
        self.visibility_owner
            .replace_visibility_state(visibility_state)
    }

    /// Snapshot the current durable visibility state.
    pub fn visibility_state(&self) -> Result<SessionToolVisibilityState, ToolScopeApplyError> {
        self.visibility_owner.visibility_state()
    }

    /// Return the names currently visible to the session plane.
    pub fn visible_tool_names(&self) -> Result<BTreeSet<String>, ToolScopeApplyError> {
        self.visible_tools_result().map(|tools| {
            tools
                .iter()
                .map(|tool| tool.name.to_string())
                .collect::<BTreeSet<_>>()
        })
    }

    /// Return whether the staged durable session filters would allow a session-plane tool name
    /// to become visible after the next boundary.
    pub fn staged_session_filters_allow_name(
        &self,
        name: &str,
    ) -> Result<bool, ToolScopeApplyError> {
        let state = self
            .state
            .read()
            .map_err(|_| ToolScopeApplyError::LockPoisoned)?;
        let visibility_state = self.visibility_owner.visibility_state()?;
        let require_filter_witnesses = self.visibility_owner.requires_filter_witnesses();
        Ok(Self::compose(&[
            Self::effective_filter_for_current_projection(
                &state,
                &visibility_state,
                &visibility_state.capability_base_filter,
                false,
            ),
            Self::effective_inherited_filter_for_current_projection(
                &state,
                &visibility_state,
                &visibility_state.inherited_base_filter,
            ),
            Self::effective_filter_for_current_projection(
                &state,
                &visibility_state,
                &visibility_state.staged_filter,
                require_filter_witnesses,
            ),
        ])
        .allows(name))
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
                    .map(|name| name.as_str().to_string())
                    .collect::<BTreeSet<_>>()
            })
            .map_err(|_| ToolScopeApplyError::LockPoisoned)
    }

    /// Force the live projection closed after a boundary apply failure.
    ///
    /// This updates the projection state itself, not only the caller's local
    /// provider tool list, so later dispatch prechecks cannot accidentally
    /// observe the previous full tool set before the next healthy boundary.
    pub fn fail_closed_projection(&self) -> Result<Arc<[Arc<ToolDef>]>, ToolScopeApplyError> {
        let mut state = self
            .state
            .write()
            .map_err(|_| ToolScopeApplyError::LockPoisoned)?;
        state.base_tools = Arc::<[Arc<ToolDef>]>::from([]);
        state.known_base_names.clear();
        state.control_tool_names.clear();
        state.deferred_tool_names.clear();
        state.active_turn_allow = Some(ToolNameSet::new());
        state.active_turn_deny.clear();
        Ok(Arc::<[Arc<ToolDef>]>::from([]))
    }

    /// Return the currently configured active and staged revisions.
    pub fn revisions(&self) -> Result<(ToolScopeRevision, ToolScopeRevision), ToolScopeApplyError> {
        let visibility_state = self.visibility_owner.visibility_state()?;
        Ok((
            ToolScopeRevision(visibility_state.active_revision),
            ToolScopeRevision(visibility_state.staged_revision),
        ))
    }

    /// Return any requested deferred names that are not currently present in the base snapshot.
    pub fn missing_requested_names(&self) -> Result<BTreeSet<String>, ToolScopeApplyError> {
        let state = self
            .state
            .read()
            .map_err(|_| ToolScopeApplyError::LockPoisoned)?;
        let visibility_state = self.visibility_owner.visibility_state()?;
        Ok(visibility_state
            .active_requested_deferred_names
            .iter()
            .filter(|name| !state.known_base_names.contains(name.as_str()))
            .cloned()
            .collect::<BTreeSet<_>>())
    }

    /// Return any durable filter names that are not currently present in the base snapshot.
    pub fn missing_filter_names(&self) -> Result<BTreeSet<String>, ToolScopeApplyError> {
        let state = self
            .state
            .read()
            .map_err(|_| ToolScopeApplyError::LockPoisoned)?;
        let visibility_state = self.visibility_owner.visibility_state()?;
        Ok(durable_filter_names(&visibility_state)
            .into_iter()
            .filter(|name| !state.known_base_names.contains(name.as_str()))
            .map(crate::types::ToolName::into_string)
            .collect::<BTreeSet<_>>())
    }

    /// Record durable requested deferred names for the next boundary.
    pub fn stage_requested_deferred_names(
        &self,
        names: BTreeSet<String>,
    ) -> Result<ToolScopeRevision, ToolScopeStageError> {
        self.visibility_owner.stage_requested_deferred_names(names)
    }

    /// Add durable requested deferred authorities for the next boundary.
    pub fn add_requested_deferred_authorities(
        &self,
        authorities: &[DeferredToolLoadAuthority],
    ) -> Result<ToolScopeRevision, ToolScopeStageError> {
        let state = self
            .state
            .read()
            .map_err(|_| ToolScopeStageError::LockPoisoned)?;
        let visibility_state =
            self.visibility_owner
                .visibility_state()
                .map_err(|err| ToolScopeStageError::Owner {
                    message: err.to_string(),
                })?;
        let authorities_by_name = deferred_load_authority_map(authorities)?;
        let names = authorities_by_name.keys().cloned().collect::<BTreeSet<_>>();
        let staged_requested_deferred_names = visibility_state.staged_requested_deferred_names;
        let mut combined_witnesses = visibility_state.requested_witnesses;
        let extended = staged_requested_deferred_names
            .union(&names)
            .cloned()
            .collect::<BTreeSet<_>>();
        combined_witnesses.extend(authorities_by_name);
        let catalog = deferred_authority_catalog_for_base_tools(
            &state.base_tools,
            &state.deferred_tool_names,
        );
        let canonical_authorities =
            canonical_deferred_authorities_for_names(&extended, &combined_witnesses, &catalog)?;
        let requested_canonical_authorities = canonical_authorities
            .into_iter()
            .filter(|(name, _)| names.contains(name.as_str()))
            .map(|(name, witness)| DeferredToolLoadAuthority::new(name, witness))
            .collect();
        drop(state);

        self.visibility_owner
            .request_deferred_tools(requested_canonical_authorities)
    }

    #[cfg(test)]
    pub(crate) fn inject_boundary_failure_once_for_test(&self) {
        self.fail_next_boundary_apply.store(true, Ordering::SeqCst);
    }
}

/// Thread-safe handle for staging external scope updates.
#[derive(Clone)]
pub struct ToolScopeHandle {
    state: Arc<RwLock<ToolScopeState>>,
    visibility_owner: Arc<dyn ToolVisibilityOwner>,
}

impl std::fmt::Debug for ToolScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolScope")
            .field("state", &"<projection>")
            .field("visibility_owner", &"<dyn ToolVisibilityOwner>")
            .finish()
    }
}

impl std::fmt::Debug for ToolScopeHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolScopeHandle")
            .field("state", &"<projection>")
            .field("visibility_owner", &"<dyn ToolVisibilityOwner>")
            .finish()
    }
}

impl ToolScopeHandle {
    /// Stage an external filter update and return its monotonic revision.
    pub fn stage_external_filter(
        &self,
        filter: ToolFilter,
    ) -> Result<ToolScopeRevision, ToolScopeStageError> {
        let state = self
            .state
            .read()
            .map_err(|_| ToolScopeStageError::LockPoisoned)?;
        let visibility_state =
            self.visibility_owner
                .visibility_state()
                .map_err(|err| ToolScopeStageError::Owner {
                    message: err.to_string(),
                })?;

        let mut known_names = state.known_base_names.clone();
        for control_name in &state.control_tool_names {
            known_names.remove(control_name.as_str());
        }
        known_names.extend(durable_filter_names(&visibility_state));
        validate_filter(&filter, &known_names)?;

        self.visibility_owner.stage_persistent_filter(
            filter.clone(),
            filter_witnesses_for_base_tools_or_existing(
                &state.base_tools,
                &visibility_state.filter_witnesses,
                &filter,
            ),
        )
    }

    pub(crate) fn staged_revision(&self) -> Result<ToolScopeRevision, ToolScopeStageError> {
        self.visibility_owner
            .visibility_state()
            .map(|state| ToolScopeRevision(state.staged_revision))
            .map_err(|err| ToolScopeStageError::Owner {
                message: err.to_string(),
            })
    }

    /// Set or clear an ephemeral per-turn overlay.
    pub fn set_turn_overlay(
        &self,
        allow: Option<HashSet<String>>,
        deny: HashSet<String>,
    ) -> Result<(), ToolScopeStageError> {
        let allow: Option<ToolNameSet> = allow.map(|names| names.into_iter().collect());
        let deny: ToolNameSet = deny.into_iter().collect();
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
    known_base_names: &ToolNameSet,
) -> Result<(), ToolScopeStageError> {
    let Some(names) = filter.names() else {
        return Ok(());
    };

    let mut unknown: Vec<String> = names
        .iter()
        .filter(|name| !known_base_names.contains(name.as_str()))
        .map(|name| name.as_str().to_string())
        .collect();

    if unknown.is_empty() {
        return Ok(());
    }

    unknown.sort_unstable();
    unknown.dedup();
    Err(ToolScopeStageError::UnknownTools { names: unknown })
}

pub fn witnessed_tool_filter_for_defs(
    filter: ToolFilter,
    tool_defs: &[ToolDef],
) -> WitnessedToolFilter {
    let witnesses = filter_witnesses_for_tool_defs(tool_defs, &filter);
    WitnessedToolFilter::new(filter, witnesses)
}

pub fn filter_witnesses_for_tool_defs(
    tool_defs: &[ToolDef],
    filter: &ToolFilter,
) -> std::collections::BTreeMap<String, ToolVisibilityWitness> {
    let Some(filter_names) = filter.names() else {
        return Default::default();
    };

    let mut witnesses = std::collections::BTreeMap::new();
    for name in filter_names {
        if let Some(tool) = tool_defs.iter().find(|tool| tool.name == name.as_str()) {
            let witness = filter_witness_for_tool(tool);
            if witness.has_identity_witness() {
                witnesses.insert(name.as_str().to_string(), witness);
            }
        }
    }
    witnesses
}

pub fn validate_inherited_filter_witnesses(
    filter: &ToolFilter,
    witnesses: &std::collections::BTreeMap<String, ToolVisibilityWitness>,
) -> Result<(), ToolScopeStageError> {
    let Some(filter_names) = filter.names() else {
        return Ok(());
    };

    let mut missing = filter_names
        .iter()
        .filter(|name| {
            witnesses
                .get(name.as_str())
                .is_none_or(|witness| !witness.has_identity_witness())
        })
        .map(|name| name.as_str().to_string())
        .collect::<Vec<_>>();

    if missing.is_empty() {
        return Ok(());
    }

    missing.sort_unstable();
    missing.dedup();
    Err(ToolScopeStageError::MissingFilterWitnesses { names: missing })
}

pub fn validate_filter_witnesses_match_catalog(
    filter: &ToolFilter,
    witnesses: &std::collections::BTreeMap<String, ToolVisibilityWitness>,
    catalog: &std::collections::BTreeMap<String, ToolVisibilityWitness>,
) -> Result<(), ToolScopeStageError> {
    validate_inherited_filter_witnesses(filter, witnesses)?;
    let Some(filter_names) = filter.names() else {
        return Ok(());
    };

    let mut invalid = filter_names
        .iter()
        .filter(|name| {
            let Some(expected) = catalog.get(name.as_str()) else {
                return false;
            };
            witnesses
                .get(name.as_str())
                .is_some_and(|witness| !filter_witness_matches_catalog(witness, expected))
        })
        .map(|name| name.as_str().to_string())
        .collect::<Vec<_>>();

    if invalid.is_empty() {
        return Ok(());
    }

    invalid.sort_unstable();
    invalid.dedup();
    Err(ToolScopeStageError::InvalidFilterWitnesses { names: invalid })
}

pub fn validate_witnessed_filter_authority(
    filter: &ToolFilter,
    witnesses: &std::collections::BTreeMap<String, ToolVisibilityWitness>,
) -> Result<(), ToolScopeStageError> {
    validate_inherited_filter_witnesses(filter, witnesses)?;
    let Some(filter_names) = filter.names() else {
        if witnesses.is_empty() {
            return Ok(());
        }
        let invalid = witnesses.keys().cloned().collect::<Vec<_>>();
        return Err(ToolScopeStageError::InvalidFilterWitnesses { names: invalid });
    };

    let mut invalid = witnesses
        .keys()
        .filter(|name| !filter_names.contains(name.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    if invalid.is_empty() {
        return Ok(());
    }

    invalid.sort_unstable();
    invalid.dedup();
    Err(ToolScopeStageError::InvalidFilterWitnesses { names: invalid })
}

fn filter_witness_matches_catalog(
    witness: &ToolVisibilityWitness,
    expected: &ToolVisibilityWitness,
) -> bool {
    if let Some(owner) = witness.stable_owner_key.as_deref()
        && expected.stable_owner_key.as_deref() != Some(owner)
    {
        return false;
    }
    if let Some(provenance) = witness.last_seen_provenance.as_ref()
        && expected.last_seen_provenance.as_ref() != Some(provenance)
    {
        return false;
    }
    witness.has_identity_witness()
}

fn deferred_authority_catalog_for_base_tools(
    base_tools: &[Arc<ToolDef>],
    deferred_tool_names: &ToolNameSet,
) -> std::collections::BTreeMap<String, ToolVisibilityWitness> {
    base_tools
        .iter()
        .filter(|tool| deferred_tool_names.contains(tool.name.as_str()))
        .filter_map(|tool| {
            let provenance = tool.provenance.as_ref()?;
            Some((
                tool.name.to_string(),
                ToolVisibilityWitness {
                    stable_owner_key: stable_owner_key_for_tool(tool),
                    last_seen_provenance: Some(provenance.clone()),
                },
            ))
        })
        .collect()
}

fn filter_authority_catalog_for_base_tools(
    base_tools: &[Arc<ToolDef>],
) -> std::collections::BTreeMap<String, ToolVisibilityWitness> {
    base_tools
        .iter()
        .filter_map(|tool| {
            let witness = filter_witness_for_tool(tool);
            witness
                .has_identity_witness()
                .then(|| (tool.name.to_string(), witness))
        })
        .collect()
}

fn deferred_authority_names_for_visibility_state(
    visibility_state: &SessionToolVisibilityState,
) -> BTreeSet<String> {
    visibility_state
        .active_requested_deferred_names
        .union(&visibility_state.staged_requested_deferred_names)
        .cloned()
        .collect()
}

fn deferred_load_authority_map(
    authorities: &[DeferredToolLoadAuthority],
) -> Result<std::collections::BTreeMap<String, ToolVisibilityWitness>, ToolScopeStageError> {
    let mut by_name = std::collections::BTreeMap::new();
    let mut invalid = Vec::new();

    for authority in authorities {
        match by_name.insert(authority.name.clone(), authority.witness.clone()) {
            Some(existing) if existing != authority.witness => invalid.push(authority.name.clone()),
            _ => {}
        }
    }

    if invalid.is_empty() {
        return Ok(by_name);
    }

    invalid.sort_unstable();
    invalid.dedup();
    Err(ToolScopeStageError::InvalidWitnesses { names: invalid })
}

pub(crate) fn validate_deferred_authorities_for_names(
    names: &BTreeSet<String>,
    witnesses: &std::collections::BTreeMap<String, ToolVisibilityWitness>,
    authority_catalog: &std::collections::BTreeMap<String, ToolVisibilityWitness>,
) -> Result<(), ToolScopeStageError> {
    let missing = missing_visibility_witness_names(names, witnesses);
    if !missing.is_empty() {
        return Err(ToolScopeStageError::MissingWitnesses { names: missing });
    }

    let mut invalid = names
        .iter()
        .filter(|name| {
            let witness = witnesses.get(name.as_str());
            let expected = authority_catalog.get(name.as_str());
            !matches!(
                (witness, expected),
                (Some(witness), Some(expected))
                    if witness.stable_owner_key == expected.stable_owner_key
                        && witness.last_seen_provenance == expected.last_seen_provenance
            )
        })
        .cloned()
        .collect::<Vec<_>>();

    if invalid.is_empty() {
        return Ok(());
    }

    invalid.sort_unstable();
    invalid.dedup();
    Err(ToolScopeStageError::InvalidWitnesses { names: invalid })
}

fn canonical_deferred_authorities_for_names(
    names: &BTreeSet<String>,
    witnesses: &std::collections::BTreeMap<String, ToolVisibilityWitness>,
    authority_catalog: &std::collections::BTreeMap<String, ToolVisibilityWitness>,
) -> Result<std::collections::BTreeMap<String, ToolVisibilityWitness>, ToolScopeStageError> {
    validate_deferred_authorities_for_names(names, witnesses, authority_catalog)?;
    let mut authorities = std::collections::BTreeMap::new();
    for name in names {
        let Some(witness) = authority_catalog.get(name.as_str()) else {
            return Err(ToolScopeStageError::InvalidWitnesses {
                names: vec![name.clone()],
            });
        };
        authorities.insert(name.clone(), witness.clone());
    }
    Ok(authorities)
}

fn durable_filter_names(state: &SessionToolVisibilityState) -> ToolNameSet {
    let mut names = ToolNameSet::new();
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

fn filter_witnesses_for_base_tools(
    base_tools: &Arc<[Arc<ToolDef>]>,
    filter: &ToolFilter,
) -> std::collections::BTreeMap<String, ToolVisibilityWitness> {
    let mut witnesses = std::collections::BTreeMap::new();
    extend_filter_witnesses(base_tools, &mut witnesses, filter);
    witnesses
}

fn filter_witnesses_for_base_tools_or_existing(
    base_tools: &Arc<[Arc<ToolDef>]>,
    existing: &std::collections::BTreeMap<String, ToolVisibilityWitness>,
    filter: &ToolFilter,
) -> std::collections::BTreeMap<String, ToolVisibilityWitness> {
    let mut witnesses = filter_witnesses_for_base_tools(base_tools, filter);
    let Some(filter_names) = filter.names() else {
        return witnesses;
    };

    for name in filter_names {
        if witnesses.contains_key(name.as_str()) {
            continue;
        }
        if let Some(witness) = existing
            .get(name.as_str())
            .filter(|witness| witness.has_identity_witness())
        {
            witnesses.insert(name.as_str().to_string(), witness.clone());
        }
    }
    witnesses
}

fn extend_filter_witnesses(
    base_tools: &Arc<[Arc<ToolDef>]>,
    witnesses: &mut std::collections::BTreeMap<String, ToolVisibilityWitness>,
    filter: &ToolFilter,
) {
    let Some(filter_names) = filter.names() else {
        return;
    };

    for name in filter_names {
        if let Some(tool) = base_tools.iter().find(|tool| tool.name == name.as_str()) {
            let witness = filter_witness_for_tool(tool);
            if witness.has_identity_witness() {
                witnesses.insert(name.as_str().to_string(), witness);
            }
        }
    }
}

fn filter_witness_for_tool(tool: &ToolDef) -> ToolVisibilityWitness {
    ToolVisibilityWitness {
        stable_owner_key: stable_owner_key_for_tool(tool),
        last_seen_provenance: tool.provenance.clone(),
    }
}

fn missing_visibility_witness_names(
    names: &BTreeSet<String>,
    witnesses: &std::collections::BTreeMap<String, ToolVisibilityWitness>,
) -> Vec<String> {
    names
        .iter()
        .filter(|name| {
            witnesses
                .get(name.as_str())
                .is_none_or(|witness| !witness.has_provenance_identity_witness())
        })
        .cloned()
        .collect()
}

fn sorted_names(names: &ToolNameSet) -> Vec<String> {
    let mut values = names
        .iter()
        .map(|name| name.as_str().to_string())
        .collect::<Vec<_>>();
    values.sort_unstable();
    values
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::ToolScopeRevision;
    use super::{
        ToolFilter, ToolScope, ToolScopeApplyError, ToolScopeStageError, ToolVisibilityOwner,
    };
    use crate::session::{
        DeferredToolLoadAuthority, SessionToolVisibilityState, ToolVisibilityWitness,
    };
    use crate::types::{ToolDef, ToolNameSet, ToolProvenance, ToolSourceKind};
    use std::collections::{BTreeMap, BTreeSet, HashSet};
    use std::sync::Arc;

    fn set(names: &[&str]) -> ToolNameSet {
        names.iter().map(|name| (*name).to_string()).collect()
    }

    fn raw_set(names: &[&str]) -> HashSet<String> {
        names.iter().map(|name| (*name).to_string()).collect()
    }

    fn tools(names: &[&str]) -> Arc<[Arc<ToolDef>]> {
        names
            .iter()
            .map(|name| {
                Arc::new(ToolDef {
                    name: (*name).into(),
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
            name: name.into(),
            description: format!("{name} tool"),
            input_schema: serde_json::json!({ "type": "object" }),
            provenance: Some(ToolProvenance {
                kind: ToolSourceKind::Callback,
                source_id: source_id.into(),
            }),
        })
    }

    struct VisibilityReadFailingOwner;

    impl ToolVisibilityOwner for VisibilityReadFailingOwner {
        fn visibility_state(&self) -> Result<SessionToolVisibilityState, ToolScopeApplyError> {
            Err(ToolScopeApplyError::Owner {
                message: "visibility read fixture failed".to_string(),
            })
        }

        fn replace_visibility_state(
            &self,
            _visibility_state: SessionToolVisibilityState,
        ) -> Result<(), ToolScopeApplyError> {
            Ok(())
        }

        fn stage_persistent_filter(
            &self,
            _filter: ToolFilter,
            _witnesses: BTreeMap<String, ToolVisibilityWitness>,
        ) -> Result<ToolScopeRevision, ToolScopeStageError> {
            Err(ToolScopeStageError::Owner {
                message: "visibility read fixture failed".to_string(),
            })
        }

        fn stage_requested_deferred_names(
            &self,
            _names: BTreeSet<String>,
        ) -> Result<ToolScopeRevision, ToolScopeStageError> {
            Err(ToolScopeStageError::Owner {
                message: "visibility read fixture failed".to_string(),
            })
        }

        fn request_deferred_tools(
            &self,
            _authorities: Vec<DeferredToolLoadAuthority>,
        ) -> Result<ToolScopeRevision, ToolScopeStageError> {
            Err(ToolScopeStageError::Owner {
                message: "visibility read fixture failed".to_string(),
            })
        }

        fn boundary_applied(&self) -> Result<SessionToolVisibilityState, ToolScopeApplyError> {
            Err(ToolScopeApplyError::Owner {
                message: "visibility read fixture failed".to_string(),
            })
        }
    }

    #[test]
    fn tool_filter_typed_names_keep_legacy_string_wire_shape() -> Result<(), String> {
        let filter = ToolFilter::Allow(set(&["read_file", "shell"]));
        let value = serde_json::to_value(&filter).unwrap();
        let names = value["Allow"]
            .as_array()
            .expect("tool filter names remain string array-shaped");
        assert_eq!(names.len(), 2);
        assert!(names.contains(&serde_json::json!("read_file")));
        assert!(names.contains(&serde_json::json!("shell")));

        let parsed: ToolFilter = serde_json::from_value(value).unwrap();
        match parsed {
            ToolFilter::Allow(names) => {
                assert!(names.contains("read_file"));
                assert!(names.contains("shell"));
            }
            other => return Err(format!("expected allow filter, got {other:?}")),
        }
        Ok(())
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
    fn owner_visibility_read_failure_fails_closed_without_local_defaults() {
        let scope = ToolScope::new_with_visibility_owner(
            tools(&["visible", "deferred"]),
            HashSet::new(),
            raw_set(&["deferred"]),
            Arc::new(VisibilityReadFailingOwner),
        );

        let err = scope
            .visible_tools_result()
            .expect_err("owner read failures must stay explicit");
        assert!(
            err.to_string().contains("visibility read fixture failed"),
            "unexpected visibility error: {err}"
        );
        assert!(
            scope.visible_tools().is_empty(),
            "fallible visible_tools facade must close the projected tool set"
        );
        assert!(
            scope.visible_tool_names().is_err(),
            "dispatch prechecks must not synthesize names from local defaults"
        );
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
            raw_set(&["tool_catalog_search"]),
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
        let visible = tools(&["visible"])[0].clone();
        let deferred = tool_with_provenance("deferred", "owner-a");
        let scope = ToolScope::new_with_projection_names(
            vec![Arc::clone(&visible), Arc::clone(&deferred)].into(),
            HashSet::new(),
            raw_set(&["deferred"]),
        );

        assert_eq!(
            scope.visible_tool_names().unwrap(),
            ["visible".to_string()].into_iter().collect(),
            "deferred tools should be hidden until requested"
        );

        scope
            .add_requested_deferred_authorities(&[crate::DeferredToolLoadAuthority::new(
                "deferred",
                crate::ToolVisibilityWitness {
                    stable_owner_key: Some("callback:owner-a".to_string()),
                    last_seen_provenance: deferred.provenance.clone(),
                },
            )])
            .unwrap();

        assert_eq!(
            scope.visible_tool_names().unwrap(),
            ["visible".to_string()].into_iter().collect(),
            "staged requests should not publish deferred tools before the next boundary"
        );

        let applied = scope
            .apply_staged(vec![Arc::clone(&visible), Arc::clone(&deferred)].into())
            .unwrap();
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
        let visibility_state = scope.visibility_state().unwrap();
        let applied = scope
            .apply_staged_projection(
                late_deferred,
                HashSet::new(),
                raw_set(&["late_deferred"]),
                &visibility_state,
            )
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
            .set_turn_overlay(Some(raw_set(&["b", "c"])), raw_set(&["c"]))
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
                .map(|t| t.name.to_string())
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
                .map(|t| t.name.to_string())
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
                .map(|t| t.name.to_string())
                .collect::<Vec<_>>(),
            vec!["visible".to_string()]
        );
    }

    #[test]
    fn boundary_projection_uses_pre_promotion_visibility_for_change_detection() {
        let scope = ToolScope::new(tools(&["visible", "secret"]));
        let handle = scope.handle();

        handle
            .stage_external_filter(ToolFilter::Deny(set(&["secret"])))
            .unwrap();

        let previous_visibility_state = scope.visibility_state().unwrap();
        let promoted_visibility_state = scope.promote_staged_visibility().unwrap();
        let applied = scope
            .apply_staged_projection_with_previous(
                tools(&["visible", "secret"]),
                ToolNameSet::new(),
                ToolNameSet::new(),
                &previous_visibility_state,
                &promoted_visibility_state,
            )
            .expect("projection refresh should detect the promoted visibility change");

        assert!(applied.visible_changed());
        assert_eq!(applied.previous_visible_names, vec!["visible", "secret"]);
        assert_eq!(applied.visible_names, vec!["visible"]);
        assert_eq!(applied.previous_active_revision, ToolScopeRevision(0));
        assert_eq!(applied.applied_revision, ToolScopeRevision(1));
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
                .map(|t| t.name.to_string())
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
                .map(|t| t.name.to_string())
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
            raw_set(&["deferred"]),
        );

        scope
            .add_requested_deferred_authorities(&[crate::DeferredToolLoadAuthority::new(
                "deferred",
                crate::ToolVisibilityWitness {
                    stable_owner_key: Some("callback:owner-a".to_string()),
                    last_seen_provenance: requested.provenance.clone(),
                },
            )])
            .unwrap();
        let promoted = scope.promote_staged_visibility().unwrap();
        scope
            .apply_staged_projection(
                vec![Arc::clone(&requested)].into(),
                HashSet::new(),
                raw_set(&["deferred"]),
                &promoted,
            )
            .unwrap();
        assert_eq!(
            scope.visible_tool_names().unwrap(),
            ["deferred".to_string()].into_iter().collect(),
            "the matching owner should remain visible"
        );

        let current = scope.visibility_state().unwrap();
        scope
            .apply_staged_projection(
                vec![Arc::clone(&rebound)].into(),
                HashSet::new(),
                raw_set(&["deferred"]),
                &current,
            )
            .unwrap();
        assert!(
            scope.visible_tool_names().unwrap().is_empty(),
            "a different owner must not inherit prior deferred visibility intent"
        );
    }

    #[test]
    fn local_visibility_replace_rejects_empty_deferred_authority() {
        let requested = tool_with_provenance("deferred", "owner-a");
        let scope = ToolScope::new_with_projection_names(
            vec![Arc::clone(&requested)].into(),
            HashSet::new(),
            raw_set(&["deferred"]),
        );

        let err = scope
            .set_visibility_state(crate::SessionToolVisibilityState {
                active_requested_deferred_names: ["deferred".to_string()].into_iter().collect(),
                staged_requested_deferred_names: ["deferred".to_string()].into_iter().collect(),
                requested_witnesses: [(
                    "deferred".to_string(),
                    crate::ToolVisibilityWitness::default(),
                )]
                .into_iter()
                .collect(),
                ..Default::default()
            })
            .expect_err("local replacement must reject empty deferred-tool authority");

        assert!(
            err.to_string().contains("deferred"),
            "rejection should name the missing deferred authority: {err}"
        );
        assert!(
            scope
                .visibility_state()
                .unwrap()
                .active_requested_deferred_names
                .is_empty(),
            "failed local replacement must not install active deferred names"
        );
    }

    #[test]
    fn local_visibility_replace_rejects_mismatched_deferred_authority() {
        let requested = tool_with_provenance("deferred", "owner-a");
        let scope = ToolScope::new_with_projection_names(
            vec![Arc::clone(&requested)].into(),
            HashSet::new(),
            raw_set(&["deferred"]),
        );

        let err = scope
            .set_visibility_state(crate::SessionToolVisibilityState {
                active_requested_deferred_names: ["deferred".to_string()].into_iter().collect(),
                staged_requested_deferred_names: ["deferred".to_string()].into_iter().collect(),
                requested_witnesses: [(
                    "deferred".to_string(),
                    crate::ToolVisibilityWitness {
                        stable_owner_key: Some("callback:owner-b".to_string()),
                        last_seen_provenance: Some(ToolProvenance {
                            kind: ToolSourceKind::Callback,
                            source_id: "owner-b".into(),
                        }),
                    },
                )]
                .into_iter()
                .collect(),
                ..Default::default()
            })
            .expect_err("local replacement must reject forged deferred-tool authority");

        assert!(
            err.to_string().contains("deferred"),
            "rejection should name the mismatched deferred authority: {err}"
        );
        assert!(
            scope
                .visibility_state()
                .unwrap()
                .staged_requested_deferred_names
                .is_empty(),
            "failed local replacement must not install staged deferred names"
        );
    }

    #[test]
    fn name_only_deferred_staging_rejects_without_witness_authority() {
        let requested = tool_with_provenance("deferred", "owner-a");
        let scope = ToolScope::new_with_projection_names(
            vec![Arc::clone(&requested)].into(),
            HashSet::new(),
            raw_set(&["deferred"]),
        );

        let err = scope
            .stage_requested_deferred_names(["deferred".to_string()].into_iter().collect())
            .expect_err("name-only deferred staging must not become authority");

        assert_eq!(
            err,
            ToolScopeStageError::MissingWitnesses {
                names: vec!["deferred".to_string()],
            }
        );
        assert!(
            scope
                .visibility_state()
                .unwrap()
                .staged_requested_deferred_names
                .is_empty(),
            "failed name-only staging must not stage deferred names"
        );
    }

    #[test]
    fn requested_deferred_authorities_require_provenance_witnesses() {
        let requested = tool_with_provenance("deferred", "owner-a");
        let scope = ToolScope::new_with_projection_names(
            vec![Arc::clone(&requested)].into(),
            HashSet::new(),
            raw_set(&["deferred"]),
        );

        let err = scope
            .add_requested_deferred_authorities(&[crate::DeferredToolLoadAuthority::new(
                "deferred",
                crate::ToolVisibilityWitness {
                    stable_owner_key: Some("callback:owner-a".to_string()),
                    last_seen_provenance: None,
                },
            )])
            .expect_err("requesting a deferred tool without provenance authority must fail");

        assert!(
            err.to_string().contains("deferred"),
            "missing-witness error should name the requested tool: {err}"
        );
        assert!(
            scope
                .visibility_state()
                .unwrap()
                .staged_requested_deferred_names
                .is_empty(),
            "failed witness validation must not stage deferred names"
        );
    }

    #[test]
    fn requested_deferred_names_reject_empty_witnesses() {
        let requested = tool_with_provenance("deferred", "owner-a");
        let scope = ToolScope::new_with_projection_names(
            vec![Arc::clone(&requested)].into(),
            HashSet::new(),
            raw_set(&["deferred"]),
        );

        let err = scope
            .add_requested_deferred_authorities(&[crate::DeferredToolLoadAuthority::new(
                "deferred",
                crate::ToolVisibilityWitness::default(),
            )])
            .expect_err("empty deferred-tool witnesses should fail");

        assert!(
            err.to_string().contains("deferred"),
            "missing-witness error should name the requested tool: {err}"
        );
        assert!(
            scope
                .visibility_state()
                .unwrap()
                .staged_requested_deferred_names
                .is_empty(),
            "failed empty-witness validation must not stage deferred names"
        );
    }

    #[test]
    fn requested_deferred_authorities_reject_mismatched_visible_catalog() {
        let requested = tool_with_provenance("deferred", "owner-a");
        let scope = ToolScope::new_with_projection_names(
            vec![Arc::clone(&requested)].into(),
            HashSet::new(),
            raw_set(&["deferred"]),
        );

        let err = scope
            .add_requested_deferred_authorities(&[crate::DeferredToolLoadAuthority::new(
                "deferred",
                crate::ToolVisibilityWitness {
                    stable_owner_key: Some("callback:owner-b".to_string()),
                    last_seen_provenance: Some(ToolProvenance {
                        kind: ToolSourceKind::Callback,
                        source_id: "owner-b".into(),
                    }),
                },
            )])
            .expect_err("mismatched deferred-tool authority should fail");

        assert!(
            err.to_string().contains("deferred"),
            "mismatch error should name the requested tool: {err}"
        );
        assert!(
            scope
                .visibility_state()
                .unwrap()
                .staged_requested_deferred_names
                .is_empty(),
            "failed mismatch validation must not stage deferred names"
        );
    }

    #[test]
    fn requested_deferred_authorities_reject_conflicting_duplicate_authority_values() {
        let requested = tool_with_provenance("deferred", "owner-a");
        let scope = ToolScope::new_with_projection_names(
            vec![Arc::clone(&requested)].into(),
            HashSet::new(),
            raw_set(&["deferred"]),
        );

        let err = scope
            .add_requested_deferred_authorities(&[
                crate::DeferredToolLoadAuthority::new(
                    "deferred",
                    crate::ToolVisibilityWitness {
                        stable_owner_key: Some("callback:owner-a".to_string()),
                        last_seen_provenance: requested.provenance.clone(),
                    },
                ),
                crate::DeferredToolLoadAuthority::new(
                    "deferred",
                    crate::ToolVisibilityWitness {
                        stable_owner_key: Some("callback:forged".to_string()),
                        last_seen_provenance: Some(ToolProvenance {
                            kind: ToolSourceKind::Callback,
                            source_id: "forged".into(),
                        }),
                    },
                ),
            ])
            .expect_err("conflicting authority values for one deferred route must fail");

        assert!(
            err.to_string().contains("deferred"),
            "duplicate authority rejection should name the conflicted tool: {err}"
        );
        assert!(
            scope
                .visibility_state()
                .unwrap()
                .staged_requested_deferred_names
                .is_empty(),
            "failed duplicate-authority validation must not stage deferred names"
        );
    }

    #[test]
    fn requested_deferred_names_reuse_existing_witnesses_for_extended_sets() {
        let requested_a = tool_with_provenance("deferred_a", "owner-a");
        let requested_b = tool_with_provenance("deferred_b", "owner-b");
        let scope = ToolScope::new_with_projection_names(
            vec![Arc::clone(&requested_a), Arc::clone(&requested_b)].into(),
            HashSet::new(),
            raw_set(&["deferred_a", "deferred_b"]),
        );

        scope
            .add_requested_deferred_authorities(&[crate::DeferredToolLoadAuthority::new(
                "deferred_a",
                crate::ToolVisibilityWitness {
                    stable_owner_key: Some("callback:owner-a".to_string()),
                    last_seen_provenance: requested_a.provenance.clone(),
                },
            )])
            .expect("initial deferred request should stage witness");

        scope
            .add_requested_deferred_authorities(&[crate::DeferredToolLoadAuthority::new(
                "deferred_b",
                crate::ToolVisibilityWitness {
                    stable_owner_key: Some("callback:owner-b".to_string()),
                    last_seen_provenance: requested_b.provenance.clone(),
                },
            )])
            .expect("extended deferred request should reuse already staged witnesses");

        let state = scope.visibility_state().unwrap();
        assert_eq!(
            state.staged_requested_deferred_names,
            ["deferred_a".to_string(), "deferred_b".to_string()]
                .into_iter()
                .collect()
        );
        assert!(state.requested_witnesses.contains_key("deferred_a"));
        assert!(state.requested_witnesses.contains_key("deferred_b"));
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
    fn empty_inherited_witness_does_not_become_authority() {
        let original = tool_with_provenance("a", "owner-a");
        let rebound = tool_with_provenance("a", "owner-b");
        let scope = ToolScope::new(vec![Arc::clone(&original)].into());

        scope
            .set_visibility_state(crate::SessionToolVisibilityState {
                inherited_base_filter: ToolFilter::Allow(set(&["a"])),
                filter_witnesses: [("a".to_string(), crate::ToolVisibilityWitness::default())]
                    .into_iter()
                    .collect(),
                ..Default::default()
            })
            .unwrap();

        assert!(
            scope.visible_tool_names().unwrap().is_empty(),
            "empty inherited witness must fail closed even while the original name is present"
        );

        let current = scope.visibility_state().unwrap();
        scope
            .apply_staged_projection(
                vec![Arc::clone(&rebound)].into(),
                HashSet::new(),
                HashSet::new(),
                &current,
            )
            .unwrap();
        assert!(
            scope.visible_tool_names().unwrap().is_empty(),
            "empty inherited witness must not rebind to a same-name replacement"
        );
    }

    #[test]
    fn inherited_filter_witness_mismatch_prevents_rebinding_a_dormant_name() {
        let original = tool_with_provenance("a", "owner-a");
        let rebound = tool_with_provenance("a", "owner-b");
        let scope = ToolScope::new(vec![Arc::clone(&original)].into());

        scope
            .set_visibility_state(crate::SessionToolVisibilityState {
                inherited_base_filter: ToolFilter::Allow(set(&["a"])),
                filter_witnesses: [(
                    "a".to_string(),
                    crate::ToolVisibilityWitness {
                        stable_owner_key: Some("callback:owner-a".to_string()),
                        last_seen_provenance: original.provenance.clone(),
                    },
                )]
                .into_iter()
                .collect(),
                ..Default::default()
            })
            .unwrap();

        assert_eq!(
            scope.visible_tool_names().unwrap(),
            ["a".to_string()].into_iter().collect(),
            "matching inherited witness should keep the original tool visible"
        );

        let current = scope.visibility_state().unwrap();
        scope
            .apply_staged_projection(
                vec![Arc::clone(&rebound)].into(),
                HashSet::new(),
                HashSet::new(),
                &current,
            )
            .unwrap();
        assert!(
            scope.visible_tool_names().unwrap().is_empty(),
            "a different owner must not inherit prior inherited-base visibility intent"
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
                .map(|t| t.name.to_string())
                .collect::<Vec<_>>(),
            vec!["a".to_string(), "b".to_string()]
        );

        handle
            .set_turn_overlay(Some(raw_set(&["b", "c"])), raw_set(&["b"]))
            .unwrap();
        assert_eq!(
            scope
                .visible_tools()
                .iter()
                .map(|t| t.name.to_string())
                .collect::<Vec<_>>(),
            Vec::<String>::new(),
            "external allow(a,b) + turn allow(b,c) + turn deny(b) should be empty"
        );

        handle.clear_turn_overlay();
        assert_eq!(
            scope
                .visible_tools()
                .iter()
                .map(|t| t.name.to_string())
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
                .map(|t| t.name.to_string())
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
                .map(|t| t.name.to_string())
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
                .map(|t| t.name.to_string())
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
