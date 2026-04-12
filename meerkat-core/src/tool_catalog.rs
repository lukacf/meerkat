use crate::types::{ToolDef, ToolProvenance, ToolSourceKind};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolCatalogDeferredEligibility {
    InlineOnly,
    DeferredEligible { stable_owner_key: String },
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
    pub currently_callable: bool,
    pub deferred_eligibility: ToolCatalogDeferredEligibility,
}

impl ToolCatalogEntry {
    pub fn session_inline(tool: Arc<ToolDef>, currently_callable: bool) -> Self {
        Self {
            tool,
            plane: ToolPlaneClass::Session,
            currently_callable,
            deferred_eligibility: ToolCatalogDeferredEligibility::InlineOnly,
        }
    }

    pub fn control_inline(tool: Arc<ToolDef>, currently_callable: bool) -> Self {
        Self {
            tool,
            plane: ToolPlaneClass::Control,
            currently_callable,
            deferred_eligibility: ToolCatalogDeferredEligibility::InlineOnly,
        }
    }

    pub fn session_deferred(
        tool: Arc<ToolDef>,
        currently_callable: bool,
        stable_owner_key: String,
    ) -> Self {
        Self {
            tool,
            plane: ToolPlaneClass::Session,
            currently_callable,
            deferred_eligibility: ToolCatalogDeferredEligibility::DeferredEligible {
                stable_owner_key,
            },
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
        ToolSourceKind::Mob => "mob",
        ToolSourceKind::MobTasks => "mob_tasks",
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
