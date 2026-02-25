//! Tool visibility scope and external filter staging.

use crate::types::ToolDef;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

/// Visibility filter for tools.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ToolFilter {
    /// All tools are visible.
    All,
    /// Only listed tools are visible.
    Allow(HashSet<String>),
    /// Listed tools are hidden.
    Deny(HashSet<String>),
}

impl Default for ToolFilter {
    fn default() -> Self {
        Self::All
    }
}

impl ToolFilter {
    fn names(&self) -> Option<&HashSet<String>> {
        match self {
            Self::All => None,
            Self::Allow(names) | Self::Deny(names) => Some(names),
        }
    }

    fn prune_to_known(&mut self, known: &HashSet<String>) {
        match self {
            Self::All => {}
            Self::Allow(names) | Self::Deny(names) => names.retain(|name| known.contains(name)),
        }
    }
}

/// Session metadata key storing the persisted external tool filter.
pub const EXTERNAL_TOOL_FILTER_METADATA_KEY: &str = "tool_scope_external_filter";

/// Monotonic revision for staged external visibility updates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct ToolScopeRevision(pub u64);

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum ToolScopeStageError {
    #[error("Unknown tool(s) in filter: {names:?}")]
    UnknownTools { names: Vec<String> },
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
    base_filter: ToolFilter,
    active_external_filter: ToolFilter,
    active_turn_allow: Option<HashSet<String>>,
    active_turn_deny: HashSet<String>,
    active_revision: ToolScopeRevision,
    staged_external_filter: ToolFilter,
    staged_revision: ToolScopeRevision,
}

/// Composed filter representation using most-restrictive semantics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposedToolFilter {
    allow: Option<HashSet<String>>,
    deny: HashSet<String>,
}

impl ComposedToolFilter {
    fn allows(&self, name: &str) -> bool {
        let allowed = self.allow.as_ref().map_or(true, |set| set.contains(name));
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
        let known_base_names: HashSet<String> =
            base_tools.iter().map(|tool| tool.name.clone()).collect();

        Self {
            state: Arc::new(RwLock::new(ToolScopeState {
                base_tools,
                known_base_names,
                base_filter: ToolFilter::All,
                active_external_filter: ToolFilter::All,
                active_turn_allow: None,
                active_turn_deny: HashSet::new(),
                active_revision: ToolScopeRevision(0),
                staged_external_filter: ToolFilter::All,
                staged_revision: ToolScopeRevision(0),
            })),
            next_revision: Arc::new(AtomicU64::new(0)),
            fail_next_boundary_apply: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Returns the currently visible tools using base + active external filter composition.
    pub fn visible_tools(&self) -> Arc<[Arc<ToolDef>]> {
        self.visible_tools_result()
            .expect("tool scope lock poisoned while reading visibility")
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
            .filter(|tool| composed.allows(tool.name.as_str()))
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
        let previous_active_revision = state.active_revision;

        state.base_tools = new_base_tools;
        state.known_base_names = state
            .base_tools
            .iter()
            .map(|tool| tool.name.clone())
            .collect::<HashSet<_>>();

        let known_base_names = state.known_base_names.clone();
        state.active_external_filter.prune_to_known(&known_base_names);
        state.staged_external_filter.prune_to_known(&known_base_names);
        if let Some(allow) = state.active_turn_allow.as_mut() {
            allow.retain(|name| known_base_names.contains(name));
        }
        state
            .active_turn_deny
            .retain(|name| known_base_names.contains(name));

        state.active_external_filter = state.staged_external_filter.clone();
        state.active_revision = state.staged_revision;

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
            applied_revision: state.active_revision,
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
            .filter(|tool| composed.allows(tool.name.as_str()))
            .map(Arc::clone)
            .collect::<Vec<_>>()
            .into()
    }

    fn compose_state_filters(state: &ToolScopeState) -> ComposedToolFilter {
        let mut filters = vec![state.base_filter.clone(), state.active_external_filter.clone()];
        if let Some(allow) = &state.active_turn_allow {
            filters.push(ToolFilter::Allow(allow.clone()));
        }
        if !state.active_turn_deny.is_empty() {
            filters.push(ToolFilter::Deny(state.active_turn_deny.clone()));
        }
        Self::compose(&filters)
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
            .expect("tool scope lock poisoned while staging filter");

        validate_filter(&filter, &state.known_base_names)?;

        let revision = ToolScopeRevision(self.next_revision.fetch_add(1, Ordering::SeqCst) + 1);
        state.staged_external_filter = filter;
        state.staged_revision = revision;
        Ok(revision)
    }

    pub(crate) fn staged_revision(&self) -> ToolScopeRevision {
        self.state
            .read()
            .expect("tool scope lock poisoned while reading revision")
            .staged_revision
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
            .expect("tool scope lock poisoned while setting turn overlay");

        if let Some(allow_set) = &allow {
            validate_filter(&ToolFilter::Allow(allow_set.clone()), &state.known_base_names)?;
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
        let mut state = self
            .state
            .write()
            .expect("tool scope lock poisoned while clearing turn overlay");
        state.active_turn_allow = None;
        state.active_turn_deny.clear();
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::{ToolFilter, ToolScope, ToolScopeStageError};
    use crate::types::ToolDef;
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
                })
            })
            .collect::<Vec<_>>()
            .into()
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
        assert_eq!(handle.staged_revision(), second);
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
    fn filter_algebra_is_most_restrictive() {
        let allow_a_b = ToolFilter::Allow(set(&["a", "b"]));
        let allow_b_c = ToolFilter::Allow(set(&["b", "c"]));
        let deny_c = ToolFilter::Deny(set(&["c"]));
        let deny_b = ToolFilter::Deny(set(&["b"]));

        // Allow lists intersect.
        let composed_allow = ToolScope::compose(&[allow_a_b.clone(), allow_b_c.clone()]);
        assert!(composed_allow.allows("b"));
        assert!(!composed_allow.allows("a"));
        assert!(!composed_allow.allows("c"));

        // Deny lists union.
        let composed_deny = ToolScope::compose(&[deny_c.clone(), deny_b.clone()]);
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
    fn structural_base_change_prunes_active_and_pending_filters() {
        let scope = ToolScope::new(tools(&["a", "b", "c"]));
        let handle = scope.handle();

        handle
            .stage_external_filter(ToolFilter::Deny(set(&["c"])))
            .unwrap();
        scope.apply_staged(tools(&["a", "b", "c"]))
            .expect("initial apply should succeed");
        assert_eq!(
            scope
                .visible_tools()
                .iter()
                .map(|t| t.name.clone())
                .collect::<Vec<_>>(),
            vec!["a".to_string(), "b".to_string()]
        );

        // Pending filter still references `c`, but should be pruned on base refresh.
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
}
