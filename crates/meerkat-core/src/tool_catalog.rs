use crate::tool_execution::ToolExecutionContract;
use crate::types::{ToolDef, ToolName, ToolProvenance, ToolSourceKind};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;

/// Canonical names of the catalog control plane, never MCP exposure aliases.
pub const TOOL_CATALOG_SEARCH_NAME: &str = "tool_catalog_search";
pub const TOOL_CATALOG_LOAD_NAME: &str = "tool_catalog_load";

pub const DEFERRED_CATALOG_TOOL_COUNT_THRESHOLD: usize = 2;
pub const DEFERRED_CATALOG_SCHEMA_VOLUME_THRESHOLD: usize = 160;

/// Which projection plane a catalog entry belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolPlaneClass {
    Session,
    Control,
}

/// Whether a session should keep deferred tools inline or expose a deferred catalog.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ToolCatalogMode {
    #[default]
    Inline,
    Deferred,
}

/// Whether a catalog entry may be deferred behind the control plane.
///
/// Deferred eligibility carries the typed provenance owner directly. The
/// string witness key is a read-only projection obtained on demand via
/// [`stable_owner_key_from_provenance`], never stored beside the owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolCatalogDeferredEligibility {
    InlineOnly,
    DeferredEligible { provenance: ToolProvenance },
}

/// A stock tool family a host may let wait behind the deferred catalog
/// ([`StockToolDeferralPolicy`]).
///
/// Only a family whose tools were checked for what must stay in every
/// request can be named, and the comms family is the only one so far. More
/// may be added later: code outside this crate that matches on the enum
/// needs a wildcard arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum StockToolSourceKind {
    /// The comms tools (provenance kind [`ToolSourceKind::Comms`]).
    Comms,
}

impl StockToolSourceKind {
    /// The provenance kind this family's tools carry.
    #[must_use]
    pub const fn source_kind(self) -> ToolSourceKind {
        match self {
            Self::Comms => ToolSourceKind::Comms,
        }
    }
}

/// Which stock tool families a host lets wait behind the deferred catalog.
///
/// Host build configuration, constant for the process: a host sets it once
/// where it composes its sessions (the agent factory's
/// `stock_tool_deferral`). The default names no family, and then nothing
/// about a session's catalog, requests or tool binding digests changes.
///
/// A named family's tools become deferred-eligible in the session's composed
/// catalog ([`crate::StockDeferralDispatcher`] states the exact rule and
/// what stays in every request). Whether the session then defers is decided
/// as for any deferred-eligible tool
/// ([`select_catalog_mode_from_snapshot`]): a session whose catalog is not
/// exact defers nothing, and one whose catalog is exact defers from
/// [`DEFERRED_CATALOG_TOOL_COUNT_THRESHOLD`] deferred-eligible tools or
/// [`DEFERRED_CATALOG_SCHEMA_VOLUME_THRESHOLD`] bytes of their definitions.
///
/// In practice opting in changes the first model request of a session with
/// an exact catalog that mounts the comms tools. Three of them may wait,
/// which reaches the tool count on its own, so such a session starts in the
/// deferred mode: its first request carries the two catalog control tools in
/// their place, and the model loads a tool before its first call of it (two
/// more model requests for that tool, once).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StockToolDeferralPolicy {
    deferrable: std::collections::BTreeSet<StockToolSourceKind>,
}

impl StockToolDeferralPolicy {
    /// The policy that names no family (the default).
    #[must_use]
    pub fn none() -> Self {
        Self::default()
    }

    /// This policy with `kind` also allowed to wait behind the catalog.
    #[must_use]
    pub fn deferring(mut self, kind: StockToolSourceKind) -> Self {
        self.deferrable.insert(kind);
        self
    }

    /// Whether `kind`'s tools may wait behind the catalog.
    #[must_use]
    pub fn allows(&self, kind: StockToolSourceKind) -> bool {
        self.deferrable.contains(&kind)
    }

    /// Whether the policy names no family.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.deferrable.is_empty()
    }

    /// Whether a tool of provenance kind `kind` belongs to a named family.
    pub(crate) fn allows_source_kind(&self, kind: &ToolSourceKind) -> bool {
        self.deferrable
            .iter()
            .any(|allowed| allowed.source_kind() == *kind)
    }
}

/// Precedence-resolved catalog entry for one canonical tool name.
///
/// Entries represent the canonical winner for a tool identity even when that
/// tool is not currently callable. Policy-hidden names and collision losers are
/// omitted from the catalog entirely.
#[derive(Debug, Clone)]
pub struct ToolCatalogEntry {
    pub tool: Arc<ToolDef>,
    pub plane: ToolPlaneClass,
    pub callability: ToolCallability,
    pub deferred_eligibility: ToolCatalogDeferredEligibility,
    pub execution: ToolExecutionContract,
}

impl ToolCatalogEntry {
    pub fn session_inline(tool: Arc<ToolDef>, currently_callable: bool) -> Self {
        Self::session_inline_with_callability(tool, ToolCallability::from_bool(currently_callable))
    }

    pub fn session_inline_with_callability(
        tool: Arc<ToolDef>,
        callability: ToolCallability,
    ) -> Self {
        Self {
            tool,
            plane: ToolPlaneClass::Session,
            callability,
            deferred_eligibility: ToolCatalogDeferredEligibility::InlineOnly,
            execution: ToolExecutionContract::default(),
        }
    }

    pub fn control_inline(tool: Arc<ToolDef>, currently_callable: bool) -> Self {
        Self::control_inline_with_callability(tool, ToolCallability::from_bool(currently_callable))
    }

    pub fn control_inline_with_callability(
        tool: Arc<ToolDef>,
        callability: ToolCallability,
    ) -> Self {
        Self {
            tool,
            plane: ToolPlaneClass::Control,
            callability,
            deferred_eligibility: ToolCatalogDeferredEligibility::InlineOnly,
            execution: ToolExecutionContract::default(),
        }
    }

    pub fn session_deferred(
        tool: Arc<ToolDef>,
        currently_callable: bool,
        provenance: ToolProvenance,
    ) -> Self {
        Self::session_deferred_with_callability(
            tool,
            ToolCallability::from_bool(currently_callable),
            provenance,
        )
    }

    pub fn session_deferred_with_callability(
        tool: Arc<ToolDef>,
        callability: ToolCallability,
        provenance: ToolProvenance,
    ) -> Self {
        Self {
            tool,
            plane: ToolPlaneClass::Session,
            callability,
            deferred_eligibility: ToolCatalogDeferredEligibility::DeferredEligible { provenance },
            execution: ToolExecutionContract::default(),
        }
    }

    #[must_use]
    pub fn with_execution_contract(mut self, execution: ToolExecutionContract) -> Self {
        self.execution = execution;
        self
    }

    pub fn currently_callable(&self) -> bool {
        self.callability.is_callable()
    }
}

/// Typed reason that a catalog-owned tool cannot be called right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolUnavailableReason {
    NotCurrentlyCallable,
    NoPeersConfigured,
    RuntimeCommandAuthorityUnavailable,
    /// No dispatcher in this ownership chain implements the resolved
    /// Streaming or Detached execution mode.
    ExecutionModeOwnerUnavailable,
    /// The live routing owner no longer matches the owner captured during
    /// execution-plan resolution.
    ExecutionOwnerChanged,
    TemporarilyUnavailable,
    /// The tool is served over a mob's callback route, and no connected
    /// client holds that route: no connection claimed it in this process,
    /// or the connection that held it closed.
    CallbackRouteUnclaimed,
}

impl std::fmt::Display for ToolUnavailableReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ToolUnavailableReason::NotCurrentlyCallable => {
                f.write_str("tool is not currently callable")
            }
            ToolUnavailableReason::NoPeersConfigured => f.write_str("no peers configured"),
            ToolUnavailableReason::RuntimeCommandAuthorityUnavailable => {
                f.write_str("runtime command authority unavailable")
            }
            ToolUnavailableReason::ExecutionModeOwnerUnavailable => {
                f.write_str("resolved execution mode owner unavailable")
            }
            ToolUnavailableReason::ExecutionOwnerChanged => {
                f.write_str("tool execution owner changed after plan resolution")
            }
            ToolUnavailableReason::TemporarilyUnavailable => {
                f.write_str("tool is temporarily unavailable")
            }
            ToolUnavailableReason::CallbackRouteUnclaimed => {
                f.write_str("no connected client holds the callback route that serves it")
            }
        }
    }
}

/// Catalog-owned callability for a tool identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "status", content = "reason")]
pub enum ToolCallability {
    Callable,
    Unavailable(ToolUnavailableReason),
}

impl ToolCallability {
    pub fn callable() -> Self {
        Self::Callable
    }

    pub fn unavailable(reason: ToolUnavailableReason) -> Self {
        Self::Unavailable(reason)
    }

    pub fn from_bool(currently_callable: bool) -> Self {
        if currently_callable {
            Self::Callable
        } else {
            Self::Unavailable(ToolUnavailableReason::NotCurrentlyCallable)
        }
    }

    pub fn is_callable(self) -> bool {
        matches!(self, Self::Callable)
    }

    pub fn unavailable_reason(self) -> Option<ToolUnavailableReason> {
        match self {
            Self::Callable => None,
            Self::Unavailable(reason) => Some(reason),
        }
    }
}

fn stable_tool_source_kind_key(kind: &ToolSourceKind) -> &'static str {
    match kind {
        ToolSourceKind::Builtin => "builtin",
        ToolSourceKind::Shell => "shell",
        ToolSourceKind::Comms => "comms",
        ToolSourceKind::Memory => "memory",
        ToolSourceKind::Schedule => "schedule",
        ToolSourceKind::WorkGraph => "workgraph",
        ToolSourceKind::Mob => "mob",
        ToolSourceKind::Callback => "callback",
        ToolSourceKind::Mcp => "mcp",
        ToolSourceKind::RustBundle => "rust_bundle",
    }
}

/// Stable witness key for a tool provenance owner.
pub fn stable_owner_key_from_provenance(provenance: &ToolProvenance) -> String {
    format!(
        "{}:{}",
        stable_tool_source_kind_key(&provenance.kind),
        provenance.source_id
    )
}

/// Stable witness key for a concrete tool definition.
pub fn stable_owner_key_for_tool(tool: &ToolDef) -> Option<String> {
    tool.provenance
        .as_ref()
        .map(stable_owner_key_from_provenance)
}

/// Dispatcher-level catalog support flags.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ToolCatalogCapabilities {
    /// True only when `tool_catalog()` is an exact precedence-resolved registry
    /// for this dispatcher.
    pub exact_catalog: bool,
    /// True when the dispatcher can dynamically require the deferred catalog
    /// control plane in the future, even if the current adaptive snapshot is
    /// still inline.
    pub may_require_catalog_control_plane: bool,
}

/// Count deferred-eligible session entries in a catalog snapshot.
pub fn deferred_session_entry_count(catalog: &[ToolCatalogEntry]) -> usize {
    catalog
        .iter()
        .filter(|entry| entry.tool.audience.allows_model())
        .filter(|entry| entry.plane == ToolPlaneClass::Session)
        .filter(|entry| {
            matches!(
                entry.deferred_eligibility,
                ToolCatalogDeferredEligibility::DeferredEligible { .. }
            )
        })
        .count()
}

/// Approximate schema/instruction footprint for deferred-eligible session entries.
pub fn deferred_session_schema_volume(catalog: &[ToolCatalogEntry]) -> usize {
    catalog
        .iter()
        .filter(|entry| entry.tool.audience.allows_model())
        .filter(|entry| entry.plane == ToolPlaneClass::Session)
        .filter(|entry| {
            matches!(
                entry.deferred_eligibility,
                ToolCatalogDeferredEligibility::DeferredEligible { .. }
            )
        })
        .map(|entry| {
            entry.tool.name.len()
                + entry.tool.description.len()
                + entry.tool.input_schema.to_string().len()
        })
        .sum()
}

/// Select inline vs deferred-catalog mode from the current exact catalog snapshot.
pub fn select_catalog_mode_from_snapshot(
    exact_catalog: bool,
    catalog: &[ToolCatalogEntry],
    pending_sources: &[String],
) -> ToolCatalogMode {
    if !exact_catalog {
        return ToolCatalogMode::Inline;
    }

    if !pending_sources.is_empty() {
        return ToolCatalogMode::Deferred;
    }

    let deferred_count = deferred_session_entry_count(catalog);
    if deferred_count == 0 {
        return ToolCatalogMode::Inline;
    }

    if deferred_count >= DEFERRED_CATALOG_TOOL_COUNT_THRESHOLD
        || deferred_session_schema_volume(catalog) >= DEFERRED_CATALOG_SCHEMA_VOLUME_THRESHOLD
    {
        ToolCatalogMode::Deferred
    } else {
        ToolCatalogMode::Inline
    }
}

/// How a session's next model request shows one catalog entry to the model
/// ([`ToolCatalogView`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ToolCatalogVisibility {
    /// The request carries the tool: the model sees it and can call it.
    Visible,
    /// The tool is deferred-eligible and hidden behind the deferred catalog:
    /// the model finds it with `tool_catalog_search` and asks for it with
    /// `tool_catalog_load`, and sees it from the boundary after an accepted
    /// load.
    Deferred,
    /// The model does not see the tool: the session's tool filters hide it,
    /// its audience is not the model, or it is not currently callable.
    Hidden,
}

/// One catalog entry of a [`ToolCatalogView`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCatalogViewEntry {
    pub name: ToolName,
    pub plane: ToolPlaneClass,
    /// Whether the entry may be deferred, with its typed source owner when
    /// it may.
    pub deferred_eligibility: ToolCatalogDeferredEligibility,
    pub callability: ToolCallability,
    pub visibility: ToolCatalogVisibility,
}

/// A live session's tool catalog as its next model request would project
/// it, read without changing anything: the catalog mode and how each catalog
/// entry shows to the model. It follows the rules each model-call boundary
/// applies, from the session's current catalog and its active visibility
/// state. A staged change (an accepted `tool_catalog_load`, a staged filter)
/// applies at the next boundary and is reported as pending until then.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCatalogView {
    /// `Deferred` when the next request hides deferred-eligible tools behind
    /// the catalog control plane: the exact catalog selects the deferred
    /// mode and a control plane exists or may be composed. `Inline`
    /// otherwise.
    pub mode: ToolCatalogMode,
    /// The dispatcher reports an exact catalog; without one the mode is
    /// always inline.
    pub exact_catalog: bool,
    /// Sources still connecting, which keep an exact catalog in the deferred
    /// mode while present.
    pub pending_sources: Vec<String>,
    /// A visibility change is staged and applies at the next boundary; until
    /// then the entries report the active state.
    pub staged_changes_pending: bool,
    /// Every catalog entry, in catalog order.
    pub entries: Vec<ToolCatalogViewEntry>,
}

/// The names one model-call boundary projects from the dispatcher's exact
/// catalog, and whether deferral applies at all: control-plane entries are
/// always visible, and session-plane deferred-eligible entries are deferred
/// only when the catalog selects the deferred mode and a control plane exists
/// or may be composed.
#[derive(Debug, Default)]
pub(crate) struct CatalogBoundaryProjection {
    pub(crate) defers: bool,
    pub(crate) control_tool_names: HashSet<ToolName>,
    pub(crate) deferred_tool_names: HashSet<ToolName>,
}

/// The projection of `catalog` (present only for an exact catalog) under the
/// `selected_mode` its snapshot selects.
pub(crate) fn catalog_boundary_projection(
    capabilities: ToolCatalogCapabilities,
    catalog: Option<&[ToolCatalogEntry]>,
    selected_mode: ToolCatalogMode,
) -> CatalogBoundaryProjection {
    let (true, Some(catalog)) = (capabilities.exact_catalog, catalog) else {
        return CatalogBoundaryProjection::default();
    };
    let control_tool_names = catalog
        .iter()
        .filter(|entry| entry.plane == ToolPlaneClass::Control)
        .map(|entry| entry.tool.name.clone())
        .collect::<HashSet<_>>();
    let defers = (!control_tool_names.is_empty() || capabilities.may_require_catalog_control_plane)
        && matches!(selected_mode, ToolCatalogMode::Deferred);
    let deferred_tool_names = if defers {
        catalog
            .iter()
            .filter(|entry| entry.plane == ToolPlaneClass::Session)
            .filter(|entry| {
                matches!(
                    entry.deferred_eligibility,
                    ToolCatalogDeferredEligibility::DeferredEligible { .. }
                )
            })
            .map(|entry| entry.tool.name.clone())
            .collect()
    } else {
        HashSet::new()
    };
    CatalogBoundaryProjection {
        defers,
        control_tool_names,
        deferred_tool_names,
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// Canonical rejection reasons for deferred tool loads.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolCatalogLoadRejectedReason {
    UnknownKey,
    NotDeferredEligible,
    AlreadyRequested,
    NotFilterable,
    TemporarilyUnavailable,
}

/// Structured result for one requested catalog name.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct ToolCatalogLoadResolution {
    pub name: String,
    pub accepted: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub accepted_noop: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rejected_reason: Option<ToolCatalogLoadRejectedReason>,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::{
        DetachedToolExecutionPolicy, IdempotencyScope, RestartClass, RunnerIdentity,
        ToolExecutionContract, ToolExecutionMode,
    };
    use std::collections::BTreeSet;
    use std::time::Duration;

    #[test]
    fn deferred_eligibility_owner_is_typed_provenance_with_projected_key() {
        let provenance = ToolProvenance {
            kind: ToolSourceKind::Callback,
            source_id: "owner-a".into(),
            artifact: None,
        };
        let tool = Arc::new(ToolDef::new(
            "deferred",
            "deferred tool",
            serde_json::json!({ "type": "object" }),
        ));
        let entry = ToolCatalogEntry::session_deferred(tool, true, provenance.clone());

        // The eligibility tag owns the typed provenance; the string key is a
        // read-only projection derived on demand, never a stored fact.
        let ToolCatalogDeferredEligibility::DeferredEligible { provenance: stored } =
            entry.deferred_eligibility
        else {
            panic!("session_deferred entry must be deferred eligible");
        };
        assert_eq!(stored, provenance);
        assert_eq!(
            stable_owner_key_from_provenance(&stored),
            "callback:owner-a"
        );
    }

    #[test]
    fn app_only_catalog_entries_do_not_enable_model_discovery() {
        let provenance = ToolProvenance {
            kind: ToolSourceKind::Mcp,
            source_id: "app-server".into(),
            artifact: None,
        };
        let entries = (0..3)
            .map(|index| {
                ToolCatalogEntry::session_deferred(
                    Arc::new(
                        ToolDef::new(
                            format!("app_{index}"),
                            "x".repeat(200),
                            serde_json::json!({}),
                        )
                        .with_audience(crate::ToolAudience::App),
                    ),
                    true,
                    provenance.clone(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(deferred_session_entry_count(&entries), 0);
        assert_eq!(deferred_session_schema_volume(&entries), 0);
        assert_eq!(
            select_catalog_mode_from_snapshot(true, &entries, &[]),
            ToolCatalogMode::Inline
        );
    }

    #[test]
    fn every_catalog_constructor_declares_fast_execution_by_default() {
        let tool = Arc::new(ToolDef::new(
            "fast",
            "fast tool",
            serde_json::json!({ "type": "object" }),
        ));

        for entry in [
            ToolCatalogEntry::session_inline(Arc::clone(&tool), true),
            ToolCatalogEntry::control_inline(Arc::clone(&tool), true),
            ToolCatalogEntry::session_deferred(
                tool,
                true,
                ToolProvenance {
                    kind: ToolSourceKind::Callback,
                    source_id: "owner".into(),
                    artifact: None,
                },
            ),
        ] {
            assert_eq!(entry.execution, ToolExecutionContract::default());
        }
    }

    #[test]
    fn catalog_entry_preserves_non_default_execution_contract() {
        let detached = DetachedToolExecutionPolicy::new(
            RunnerIdentity::new("example.security_scan", "v1").unwrap(),
            RestartClass::NonResumable,
            IdempotencyScope::InteractionAndArguments,
            Duration::from_secs(10),
        )
        .unwrap();
        let contract = ToolExecutionContract::new(
            BTreeSet::from([ToolExecutionMode::Detached]),
            ToolExecutionMode::Detached,
            None,
            Some(detached),
        )
        .unwrap();
        let entry = ToolCatalogEntry::session_inline(
            Arc::new(ToolDef::new(
                "security_scan",
                "scan",
                serde_json::json!({ "type": "object" }),
            )),
            true,
        )
        .with_execution_contract(contract.clone());

        assert_eq!(entry.execution, contract);
    }
}
