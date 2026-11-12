use crate::tool_execution::ToolExecutionContract;
use crate::types::{ToolDef, ToolName, ToolProvenance, ToolSourceKind};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;

/// Canonical names of the catalog control plane, never MCP exposure aliases.
pub const TOOL_CATALOG_SEARCH_NAME: &str = "tool_catalog_search";
pub const TOOL_CATALOG_LOAD_NAME: &str = "tool_catalog_load";

/// The deferred catalog's byte budget: an exact catalog defers once the name,
/// description and schema text of its deferred-eligible tools reach this
/// many bytes ([`deferred_session_schema_volume`]).
///
/// The mode adds about 1.2 KB to every request (the two control tools, a
/// system prompt line and a notice) and two model requests to a tool's first
/// use. At 8,192 bytes a request shrinks by at least 7 KB, which repays one
/// search-and-load pair within a handful of requests; below it the tools
/// stay inline.
pub const DEFERRED_CATALOG_SCHEMA_VOLUME_THRESHOLD: usize = 8_192;

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

/// How a tool source needs the deferred catalog's control tools
/// (`tool_catalog_search`, `tool_catalog_load`).
///
/// `may_require_catalog_control_plane` says whether a session mounts them
/// for this source. This says whether a mounted control plane is in every
/// model request or only when it is useful. The variants are ordered: a
/// composition takes the strongest need of its sources.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum ToolCatalogControlPlane {
    /// The source does not ask for the control tools. A session still mounts
    /// them when it has a deferrable tool or a connecting source.
    #[default]
    Never,
    /// The source can get deferrable tools later (a server added to a live
    /// session, tools registered after the session started): the session
    /// mounts the control tools and shows them only while a deferrable tool
    /// waits behind the catalog or a source is connecting.
    OnDemand,
    /// The source needs the control tools mounted and in every model request
    /// whatever the catalog mode: its hosts tell the model to search and load
    /// by name, or expect them before the first tool arrives.
    Always,
}

/// Dispatcher-level catalog support flags.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ToolCatalogCapabilities {
    /// True only when `tool_catalog()` is an exact precedence-resolved registry
    /// for this dispatcher.
    pub exact_catalog: bool,
    /// True when this source may need the deferred catalog's control tools
    /// (`tool_catalog_search`, `tool_catalog_load`), now or later: a session
    /// mounts them for it even while its catalog is inline. Whether the
    /// mounted control tools are in every model request or only when they
    /// are useful is the dispatcher's
    /// [`tool_catalog_control_plane`](crate::AgentToolDispatcher::tool_catalog_control_plane),
    /// which answers [`ToolCatalogControlPlane::Always`] for a source that
    /// sets this flag and says nothing more. A source that leaves it false
    /// still gets the control tools mounted when the session has a
    /// deferrable tool; they are then in a request only while a tool waits
    /// behind the catalog or a source is connecting.
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
///
/// An inexact catalog is always inline. An exact one is deferred while a
/// source is still connecting (so the model can search what arrives), or
/// when its deferred-eligible definitions reach the byte budget
/// ([`DEFERRED_CATALOG_SCHEMA_VOLUME_THRESHOLD`]). The number of tools alone
/// never starts the mode: a few small tools cost more to defer than to send.
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

    if deferred_session_schema_volume(catalog) >= DEFERRED_CATALOG_SCHEMA_VOLUME_THRESHOLD {
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
    /// its audience is not the model, it is not currently callable, or it is
    /// a catalog control tool with nothing to search or load (no tool waits
    /// behind the catalog, no source is connecting and no source asks for
    /// the control tools always).
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
/// catalog, and whether deferral applies at all: session-plane
/// deferred-eligible entries are deferred only when the catalog selects the
/// deferred mode and a control plane exists or may be composed. The
/// control-plane entries show to the model always when a source declares
/// that it needs them or a source is still connecting; otherwise they show
/// only while a deferred tool waits behind the catalog.
#[derive(Debug, Default)]
pub(crate) struct CatalogBoundaryProjection {
    pub(crate) defers: bool,
    pub(crate) control_tool_names: HashSet<ToolName>,
    pub(crate) control_tools_always_visible: bool,
    pub(crate) deferred_tool_names: HashSet<ToolName>,
}

/// The projection of `catalog` (present only for an exact catalog) under the
/// `selected_mode` its snapshot selects, with the sources still connecting.
pub(crate) fn catalog_boundary_projection(
    capabilities: ToolCatalogCapabilities,
    control_plane: ToolCatalogControlPlane,
    catalog: Option<&[ToolCatalogEntry]>,
    pending_sources: &[String],
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
    let control_tools_always_visible =
        control_plane == ToolCatalogControlPlane::Always || !pending_sources.is_empty();
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
        control_tools_always_visible,
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

    fn callback_provenance() -> ToolProvenance {
        ToolProvenance {
            kind: ToolSourceKind::Callback,
            source_id: "callback".into(),
            artifact: None,
        }
    }

    fn deferred_entry(
        name: &str,
        description: &str,
        schema: serde_json::Value,
    ) -> ToolCatalogEntry {
        ToolCatalogEntry::session_deferred(
            Arc::new(ToolDef::new(name, description, schema)),
            true,
            callback_provenance(),
        )
    }

    /// A deferred-eligible tool whose name, description and schema text add
    /// up to exactly `volume` bytes.
    fn entry_of_volume(name: &str, volume: usize) -> ToolCatalogEntry {
        let schema = serde_json::json!({ "type": "object" });
        let fixed = name.len() + schema.to_string().len();
        assert!(volume >= fixed, "{volume} is below the fixed part {fixed}");
        deferred_entry(name, &"d".repeat(volume - fixed), schema)
    }

    /// A small callback tool: one described string parameter.
    fn small_tool(name: &str) -> ToolCatalogEntry {
        deferred_entry(
            name,
            "Look up one value by key.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "key": { "type": "string", "description": "The key to look up" }
                },
                "required": ["key"]
            }),
        )
    }

    /// A medium tool: three described parameters, two of them required.
    fn medium_tool(index: usize) -> ToolCatalogEntry {
        deferred_entry(
            &format!("host_report_{index:02}"),
            &format!(
                "Build report {index} for one account and period. Returns the \
                 report rows as JSON, with totals, and the period's notes."
            ),
            serde_json::json!({
                "type": "object",
                "properties": {
                    "account": {
                        "type": "string",
                        "description": "Account the report covers."
                    },
                    "period": {
                        "type": "string",
                        "description": "Reporting period, as YYYY-MM."
                    },
                    "include_notes": {
                        "type": "boolean",
                        "description": "Whether the period's notes are included."
                    }
                },
                "required": ["account", "period"]
            }),
        )
    }

    fn medium_tools(count: usize) -> Vec<ToolCatalogEntry> {
        (0..count).map(medium_tool).collect()
    }

    #[test]
    fn an_inexact_catalog_is_always_inline() {
        assert_eq!(
            select_catalog_mode_from_snapshot(false, &medium_tools(40), &[]),
            ToolCatalogMode::Inline
        );
    }

    /// One tool at the old single-tool line (160 bytes of name, description
    /// and schema) costs more to defer than it saves.
    #[test]
    fn one_tool_of_160_bytes_stays_inline() {
        let catalog = [entry_of_volume("lookup_value", 160)];
        assert_eq!(deferred_session_schema_volume(&catalog), 160);
        assert_eq!(
            select_catalog_mode_from_snapshot(true, &catalog, &[]),
            ToolCatalogMode::Inline
        );
    }

    /// Two small tools: a count alone never starts the deferred mode.
    #[test]
    fn two_small_tools_stay_inline() {
        let catalog = [small_tool("lookup_value"), small_tool("lookup_owner")];
        assert_eq!(deferred_session_schema_volume(&catalog), 294);
        assert_eq!(
            select_catalog_mode_from_snapshot(true, &catalog, &[]),
            ToolCatalogMode::Inline
        );
    }

    #[test]
    fn ten_medium_tools_stay_inline() {
        let catalog = medium_tools(10);
        assert_eq!(deferred_session_schema_volume(&catalog), 4_260);
        assert_eq!(
            select_catalog_mode_from_snapshot(true, &catalog, &[]),
            ToolCatalogMode::Inline
        );
    }

    #[test]
    fn twenty_medium_tools_defer() {
        let catalog = medium_tools(20);
        assert_eq!(deferred_session_schema_volume(&catalog), 8_530);
        assert_eq!(
            select_catalog_mode_from_snapshot(true, &catalog, &[]),
            ToolCatalogMode::Deferred
        );
    }

    /// The budget is the line, whatever the number of tools: one byte under
    /// it stays inline, and the budget itself defers.
    #[test]
    fn the_volume_budget_is_the_line() {
        assert_eq!(DEFERRED_CATALOG_SCHEMA_VOLUME_THRESHOLD, 8_192);
        for tools in [1, 2, 7] {
            let share = DEFERRED_CATALOG_SCHEMA_VOLUME_THRESHOLD / tools;
            let rest = DEFERRED_CATALOG_SCHEMA_VOLUME_THRESHOLD - share * (tools - 1);
            let at_the_line = (0..tools)
                .map(|index| {
                    entry_of_volume(
                        &format!("tool_{index}"),
                        if index == 0 { rest } else { share },
                    )
                })
                .collect::<Vec<_>>();
            assert_eq!(
                deferred_session_schema_volume(&at_the_line),
                DEFERRED_CATALOG_SCHEMA_VOLUME_THRESHOLD
            );
            assert_eq!(
                select_catalog_mode_from_snapshot(true, &at_the_line, &[]),
                ToolCatalogMode::Deferred,
                "{tools} tools at the budget defer"
            );

            let mut under = at_the_line;
            under[0] = entry_of_volume("tool_0", rest - 1);
            assert_eq!(
                deferred_session_schema_volume(&under),
                DEFERRED_CATALOG_SCHEMA_VOLUME_THRESHOLD - 1
            );
            assert_eq!(
                select_catalog_mode_from_snapshot(true, &under, &[]),
                ToolCatalogMode::Inline,
                "{tools} tools one byte under the budget stay inline"
            );
        }
    }

    /// A source that is still connecting keeps an exact catalog deferred,
    /// whatever it holds so far, so the model can search what arrives.
    #[test]
    fn a_connecting_source_defers_whatever_the_volume() {
        let pending = ["docs-server".to_string()];
        for catalog in [
            Vec::new(),
            vec![small_tool("lookup_value")],
            medium_tools(40),
        ] {
            assert_eq!(
                select_catalog_mode_from_snapshot(true, &catalog, &pending),
                ToolCatalogMode::Deferred
            );
        }
        assert_eq!(
            select_catalog_mode_from_snapshot(false, &medium_tools(40), &pending),
            ToolCatalogMode::Inline,
            "an inexact catalog stays inline while a source connects"
        );
    }

    /// Inline-only entries never count toward the budget.
    #[test]
    fn inline_only_tools_do_not_count() {
        let catalog = (0..40)
            .map(|index| {
                ToolCatalogEntry::session_inline(
                    Arc::new(ToolDef::new(
                        format!("inline_{index}"),
                        "d".repeat(1_000),
                        serde_json::json!({ "type": "object" }),
                    )),
                    true,
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(deferred_session_schema_volume(&catalog), 0);
        assert_eq!(
            select_catalog_mode_from_snapshot(true, &catalog, &[]),
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
