//! Tool visibility scope and external filter staging.

use crate::session::{
    AuthorizedSessionToolVisibilityState, DeferredToolLoadAuthority, SessionToolVisibilityState,
    ToolVisibilityWitness, WitnessedToolFilter,
};
use crate::types::{ToolDef, ToolName, ToolNameSet};
use std::collections::BTreeSet;
use std::collections::HashSet;
use std::fmt;
use std::sync::atomic::AtomicBool;
#[cfg(test)]
use std::sync::atomic::Ordering;
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
    pub known_base_names: Vec<ToolName>,
    pub visible_names: Vec<ToolName>,
    pub capability_base_filter: ToolFilter,
    pub base_filter: ToolFilter,
    pub active_external_filter: ToolFilter,
    pub active_turn_allow: Option<Vec<ToolName>>,
    pub active_turn_deny: Vec<ToolName>,
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
    UnknownTools { names: Vec<ToolName> },
    #[error("Missing tool visibility witness(es) for deferred tool(s): {names:?}")]
    MissingWitnesses { names: Vec<ToolName> },
    #[error("Missing tool visibility witness(es) for filter tool(s): {names:?}")]
    MissingFilterWitnesses { names: Vec<ToolName> },
    #[error("Invalid tool visibility witness(es) for deferred tool(s): {names:?}")]
    InvalidWitnesses { names: Vec<ToolName> },
    #[error("Invalid tool visibility witness(es) for filter tool(s): {names:?}")]
    InvalidFilterWitnesses { names: Vec<ToolName> },
    #[error("Tool scope state lock poisoned")]
    LockPoisoned,
    #[error("Tool visibility owner error: {message}")]
    Owner { message: String },
    #[error("failed to persist durable tool visibility projection: {message}")]
    DurableProjectionPersist { message: String },
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

/// Mechanical bridge for durable tool-visibility state.
///
/// Generated visibility authority is carried separately by
/// [`GeneratedToolVisibilityOwner`]. Raw trait objects are not sufficient to
/// install semantic visibility authority on a [`ToolScope`].
pub trait ToolVisibilityOwner: Send + Sync {
    fn visibility_state(&self) -> Result<SessionToolVisibilityState, ToolScopeApplyError>;

    fn replace_visibility_state(
        &self,
        visibility_state: SessionToolVisibilityState,
    ) -> Result<(), ToolScopeApplyError>;

    fn stage_persistent_filter(
        &self,
        filter: ToolFilter,
        witnesses: std::collections::BTreeMap<ToolName, ToolVisibilityWitness>,
    ) -> Result<ToolScopeRevision, ToolScopeStageError>;

    fn stage_requested_deferred_names(
        &self,
        names: BTreeSet<ToolName>,
    ) -> Result<ToolScopeRevision, ToolScopeStageError>;

    fn request_deferred_tools(
        &self,
        authorities: Vec<DeferredToolLoadAuthority>,
    ) -> Result<ToolScopeRevision, ToolScopeStageError>;

    fn replace_deferred_tool_authority_catalog(
        &self,
        _catalog: std::collections::BTreeMap<ToolName, ToolVisibilityWitness>,
    ) -> Result<(), ToolScopeApplyError> {
        Ok(())
    }

    fn replace_filter_tool_authority_catalog(
        &self,
        _catalog: std::collections::BTreeMap<ToolName, ToolVisibilityWitness>,
    ) -> Result<(), ToolScopeApplyError> {
        Ok(())
    }

    fn set_turn_overlay(
        &self,
        _allow: Option<BTreeSet<ToolName>>,
        _deny: BTreeSet<ToolName>,
    ) -> Result<ToolScopeTurnOverlay, ToolScopeStageError> {
        Err(ToolScopeStageError::Owner {
            message: "generated MeerkatMachine turn tool overlay authority is required".to_string(),
        })
    }

    fn clear_turn_overlay(&self) -> Result<ToolScopeTurnOverlay, ToolScopeStageError> {
        Err(ToolScopeStageError::Owner {
            message: "generated MeerkatMachine turn tool overlay authority is required".to_string(),
        })
    }

    fn requires_filter_witnesses(&self) -> bool {
        false
    }

    fn boundary_applied(&self) -> Result<SessionToolVisibilityState, ToolScopeApplyError>;
}

/// Generated-authority handoff for durable tool-visibility state.
///
/// This is the only owner shape accepted by `ToolScope`/`AgentBuilder` for
/// semantic visibility changes and durable persistence. Production instances
/// are minted by the MeerkatMachine runtime authority; tests may wrap fixtures
/// that deliberately exercise generated-authority failure paths.
#[derive(Clone)]
pub struct GeneratedToolVisibilityOwner {
    owner: Arc<dyn ToolVisibilityOwner>,
}

impl GeneratedToolVisibilityOwner {
    #[doc(hidden)]
    #[cfg_attr(not(meerkat_internal_generated_authority_bridge), allow(dead_code))]
    pub(crate) fn from_generated_authority(owner: Arc<dyn ToolVisibilityOwner>) -> Self {
        Self { owner }
    }

    pub(crate) fn owner(&self) -> Arc<dyn ToolVisibilityOwner> {
        Arc::clone(&self.owner)
    }

    pub fn visibility_state(&self) -> Result<SessionToolVisibilityState, ToolScopeApplyError> {
        self.owner.visibility_state()
    }

    pub fn replace_visibility_state(
        &self,
        visibility_state: SessionToolVisibilityState,
    ) -> Result<(), ToolScopeApplyError> {
        self.owner.replace_visibility_state(visibility_state)
    }

    pub fn stage_persistent_filter(
        &self,
        filter: ToolFilter,
        witnesses: std::collections::BTreeMap<ToolName, ToolVisibilityWitness>,
    ) -> Result<ToolScopeRevision, ToolScopeStageError> {
        self.owner.stage_persistent_filter(filter, witnesses)
    }

    pub fn stage_requested_deferred_names(
        &self,
        names: BTreeSet<ToolName>,
    ) -> Result<ToolScopeRevision, ToolScopeStageError> {
        self.owner.stage_requested_deferred_names(names)
    }

    pub fn request_deferred_tools(
        &self,
        authorities: Vec<DeferredToolLoadAuthority>,
    ) -> Result<ToolScopeRevision, ToolScopeStageError> {
        self.owner.request_deferred_tools(authorities)
    }

    pub fn replace_deferred_tool_authority_catalog(
        &self,
        catalog: std::collections::BTreeMap<ToolName, ToolVisibilityWitness>,
    ) -> Result<(), ToolScopeApplyError> {
        self.owner.replace_deferred_tool_authority_catalog(catalog)
    }

    pub fn replace_filter_tool_authority_catalog(
        &self,
        catalog: std::collections::BTreeMap<ToolName, ToolVisibilityWitness>,
    ) -> Result<(), ToolScopeApplyError> {
        self.owner.replace_filter_tool_authority_catalog(catalog)
    }

    pub fn set_turn_overlay(
        &self,
        allow: Option<BTreeSet<ToolName>>,
        deny: BTreeSet<ToolName>,
    ) -> Result<ToolScopeTurnOverlay, ToolScopeStageError> {
        self.owner.set_turn_overlay(allow, deny)
    }

    pub fn clear_turn_overlay(&self) -> Result<ToolScopeTurnOverlay, ToolScopeStageError> {
        self.owner.clear_turn_overlay()
    }

    pub fn requires_filter_witnesses(&self) -> bool {
        self.owner.requires_filter_witnesses()
    }

    pub fn boundary_applied(&self) -> Result<SessionToolVisibilityState, ToolScopeApplyError> {
        self.owner.boundary_applied()
    }
}

#[cfg(test)]
pub(crate) fn generated_test_tool_visibility_owner_from(
    owner: Arc<dyn ToolVisibilityOwner>,
) -> GeneratedToolVisibilityOwner {
    GeneratedToolVisibilityOwner::from_generated_authority(owner)
}

impl std::fmt::Debug for GeneratedToolVisibilityOwner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GeneratedToolVisibilityOwner")
            .field("owner", &"<dyn ToolVisibilityOwner>")
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolVisibilityAuthorityKind {
    GeneratedMachine,
    LocalReadOnlyProjection,
}

fn generated_visibility_authority_required_message(context: &str) -> String {
    format!("generated MeerkatMachine tool visibility authority is required for {context}")
}

/// Read-only local projection used when no runtime-backed MeerkatMachine owner is supplied.
///
/// This owner exists so standalone callers can inspect the default visibility
/// projection, but it is deliberately not a behavior authority: semantic
/// visibility changes must be routed through a generated machine owner.
#[derive(Debug, Clone, Default)]
pub struct LocalToolVisibilityOwner {
    state: Arc<RwLock<SessionToolVisibilityState>>,
}

impl LocalToolVisibilityOwner {
    pub fn new() -> Self {
        Self {
            state: Arc::new(RwLock::new(SessionToolVisibilityState::default())),
        }
    }
}

fn local_visibility_owner_read_only_message() -> String {
    "local tool visibility owner is read-only; generated MeerkatMachine visibility authority is required for semantic visibility changes"
        .to_string()
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
        visibility_state: SessionToolVisibilityState,
    ) -> Result<(), ToolScopeApplyError> {
        let state = self
            .state
            .read()
            .map_err(|_| ToolScopeApplyError::LockPoisoned)?;
        if *state == visibility_state {
            Ok(())
        } else {
            Err(ToolScopeApplyError::Owner {
                message: local_visibility_owner_read_only_message(),
            })
        }
    }

    fn stage_persistent_filter(
        &self,
        _filter: ToolFilter,
        _witnesses: std::collections::BTreeMap<ToolName, ToolVisibilityWitness>,
    ) -> Result<ToolScopeRevision, ToolScopeStageError> {
        Err(ToolScopeStageError::Owner {
            message: local_visibility_owner_read_only_message(),
        })
    }

    fn stage_requested_deferred_names(
        &self,
        names: BTreeSet<ToolName>,
    ) -> Result<ToolScopeRevision, ToolScopeStageError> {
        if names.is_empty() {
            let state = self
                .state
                .read()
                .map_err(|_| ToolScopeStageError::LockPoisoned)?;
            Ok(ToolScopeRevision(state.staged_revision))
        } else {
            Err(ToolScopeStageError::Owner {
                message: local_visibility_owner_read_only_message(),
            })
        }
    }

    fn request_deferred_tools(
        &self,
        _authorities: Vec<DeferredToolLoadAuthority>,
    ) -> Result<ToolScopeRevision, ToolScopeStageError> {
        Err(ToolScopeStageError::Owner {
            message: local_visibility_owner_read_only_message(),
        })
    }

    fn boundary_applied(&self) -> Result<SessionToolVisibilityState, ToolScopeApplyError> {
        let state = self
            .state
            .read()
            .map_err(|_| ToolScopeApplyError::LockPoisoned)?;
        if state.active_filter != state.staged_filter
            || state.active_requested_deferred_names != state.staged_requested_deferred_names
            || state.active_revision != state.staged_revision
        {
            return Err(ToolScopeApplyError::Owner {
                message: local_visibility_owner_read_only_message(),
            });
        }
        Ok(state.clone())
    }
}

fn default_tool_visibility_owner() -> Arc<dyn ToolVisibilityOwner> {
    Arc::new(LocalToolVisibilityOwner::new())
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod generated_visibility_test_owner {
    use std::collections::BTreeMap;

    use super::*;
    use meerkat_machine_kernels::generated::meerkat;
    use meerkat_machine_kernels::test_oracle::{
        GeneratedMachineKernel, KernelInput, KernelSignal, KernelState, KernelValue,
        TransitionRefusal,
    };
    use meerkat_machine_schema::identity::{
        EnumTypeId, EnumVariantId, FieldId, InputVariantId, NamedTypeId, SignalVariantId,
    };

    fn field_id(slug: &str) -> FieldId {
        FieldId::parse(slug).expect("generated MeerkatMachine field id")
    }

    fn input_id(slug: &str) -> InputVariantId {
        InputVariantId::parse(slug).expect("generated MeerkatMachine input id")
    }

    fn signal_id(slug: &str) -> SignalVariantId {
        SignalVariantId::parse(slug).expect("generated MeerkatMachine signal id")
    }

    fn named_type_id(slug: &str) -> NamedTypeId {
        NamedTypeId::parse(slug).expect("generated MeerkatMachine named type id")
    }

    fn enum_type_id(slug: &str) -> EnumTypeId {
        EnumTypeId::parse(slug).expect("generated MeerkatMachine enum type id")
    }

    fn enum_variant_id(slug: &str) -> EnumVariantId {
        EnumVariantId::parse(slug).expect("generated MeerkatMachine enum variant id")
    }

    fn named_string(type_name: &str, value: String) -> KernelValue {
        KernelValue::Named {
            type_name: named_type_id(type_name),
            value: Box::new(KernelValue::String(value)),
        }
    }

    fn enum_value(enum_name: &str, variant: &str) -> KernelValue {
        KernelValue::NamedVariant {
            enum_name: enum_type_id(enum_name),
            variant: enum_variant_id(variant),
        }
    }

    fn input(
        variant: &str,
        fields: impl IntoIterator<Item = (&'static str, KernelValue)>,
    ) -> KernelInput {
        KernelInput {
            variant: input_id(variant),
            fields: fields
                .into_iter()
                .map(|(name, value)| (field_id(name), value))
                .collect(),
        }
    }

    fn named_payload<'a>(value: &'a KernelValue, expected_type: &str) -> Option<&'a KernelValue> {
        match value {
            KernelValue::Named { type_name, value } if type_name.as_str() == expected_type => {
                Some(value.as_ref())
            }
            _ => None,
        }
    }

    fn state_field<'a>(state: &'a KernelState, name: &str) -> Option<&'a KernelValue> {
        state.fields.get(&field_id(name))
    }

    fn owner_projection_error(reason: impl Into<String>) -> ToolScopeApplyError {
        ToolScopeApplyError::Owner {
            message: format!(
                "generated MeerkatMachine visibility authority projection failed: {}",
                reason.into()
            ),
        }
    }

    fn map_projection_for_stage(err: ToolScopeApplyError) -> ToolScopeStageError {
        ToolScopeStageError::Owner {
            message: err.to_string(),
        }
    }

    fn required_state_field<'a>(
        state: &'a KernelState,
        name: &str,
    ) -> Result<&'a KernelValue, ToolScopeApplyError> {
        state_field(state, name)
            .ok_or_else(|| owner_projection_error(format!("missing required state field `{name}`")))
    }

    fn state_u64(state: &KernelState, name: &str) -> Result<u64, ToolScopeApplyError> {
        match required_state_field(state, name)? {
            KernelValue::U64(value) => Ok(*value),
            _ => Err(owner_projection_error(format!(
                "field `{name}` was not u64"
            ))),
        }
    }

    fn state_bool(state: &KernelState, name: &str) -> Result<bool, ToolScopeApplyError> {
        match required_state_field(state, name)? {
            KernelValue::Bool(value) => Ok(*value),
            _ => Err(owner_projection_error(format!(
                "field `{name}` was not bool"
            ))),
        }
    }

    fn state_string_set(
        state: &KernelState,
        name: &str,
    ) -> Result<BTreeSet<ToolName>, ToolScopeApplyError> {
        match required_state_field(state, name)? {
            KernelValue::Set(values) => values
                .iter()
                .map(|value| match value {
                    KernelValue::String(value) => Ok(ToolName::from(value.clone())),
                    _ => Err(owner_projection_error(format!(
                        "field `{name}` contained a non-string set member"
                    ))),
                })
                .collect(),
            _ => Err(owner_projection_error(format!(
                "field `{name}` was not a string set"
            ))),
        }
    }

    fn kernel_string_set(values: &BTreeSet<ToolName>) -> KernelValue {
        KernelValue::Set(
            values
                .iter()
                .map(|name| KernelValue::String(name.as_str().to_owned()))
                .collect(),
        )
    }

    fn tool_filter_value(filter: &ToolFilter) -> KernelValue {
        match filter {
            ToolFilter::All => enum_value("ToolFilter", "All"),
            ToolFilter::Allow(names) | ToolFilter::Deny(names) => {
                let tag = match filter {
                    ToolFilter::Allow(_) => "Allow",
                    ToolFilter::Deny(_) => "Deny",
                    ToolFilter::All => unreachable!(),
                };
                KernelValue::Named {
                    type_name: named_type_id("ToolFilter"),
                    value: Box::new(KernelValue::Map(BTreeMap::from([
                        (
                            KernelValue::String("tag".to_string()),
                            KernelValue::String(tag.to_string()),
                        ),
                        (
                            KernelValue::String("names".to_string()),
                            KernelValue::Set(
                                names
                                    .iter()
                                    .map(|name| KernelValue::String(name.as_str().to_string()))
                                    .collect(),
                            ),
                        ),
                    ]))),
                }
            }
        }
    }

    fn tool_filter_from_value(value: &KernelValue) -> Result<ToolFilter, ToolScopeApplyError> {
        let Some(payload) = named_payload(value, "ToolFilter") else {
            return match value {
                KernelValue::NamedVariant { enum_name, variant }
                    if enum_name.as_str() == "ToolFilter" && variant.as_str() == "All" =>
                {
                    Ok(ToolFilter::All)
                }
                _ => Err(owner_projection_error(
                    "tool filter was not generated ToolFilter authority value",
                )),
            };
        };
        match payload {
            KernelValue::String(tag) if tag == "All" => Ok(ToolFilter::All),
            KernelValue::Map(fields) => {
                let tag = fields
                    .get(&KernelValue::String("tag".to_string()))
                    .and_then(|value| match value {
                        KernelValue::String(value) => Some(value.as_str()),
                        _ => None,
                    })
                    .ok_or_else(|| {
                        owner_projection_error("ToolFilter map missing string `tag` field")
                    })?;
                let names_value = fields
                    .get(&KernelValue::String("names".to_string()))
                    .ok_or_else(|| {
                        owner_projection_error("ToolFilter map missing `names` field")
                    })?;
                let KernelValue::Set(values) = names_value else {
                    return Err(owner_projection_error("ToolFilter `names` was not a set"));
                };
                let names = values
                    .iter()
                    .map(|value| match value {
                        KernelValue::String(value) => Ok(value.clone()),
                        _ => Err(owner_projection_error(
                            "ToolFilter `names` contained non-string member",
                        )),
                    })
                    .collect::<Result<ToolNameSet, _>>()?;
                match tag {
                    "Allow" => Ok(ToolFilter::Allow(names)),
                    "Deny" => Ok(ToolFilter::Deny(names)),
                    "All" if names.is_empty() => Ok(ToolFilter::All),
                    _ => Err(owner_projection_error(format!(
                        "unknown generated ToolFilter tag `{tag}`"
                    ))),
                }
            }
            _ => Err(owner_projection_error(
                "ToolFilter payload was neither tag string nor field map",
            )),
        }
    }

    fn tool_source_kind_variant(kind: crate::types::ToolSourceKind) -> &'static str {
        match kind {
            crate::types::ToolSourceKind::Builtin => "Builtin",
            crate::types::ToolSourceKind::Shell => "Shell",
            crate::types::ToolSourceKind::Comms => "Comms",
            crate::types::ToolSourceKind::Memory => "Memory",
            crate::types::ToolSourceKind::Schedule => "Schedule",
            crate::types::ToolSourceKind::WorkGraph => "WorkGraph",
            crate::types::ToolSourceKind::Mob => "Mob",
            crate::types::ToolSourceKind::Callback => "Callback",
            crate::types::ToolSourceKind::Mcp => "Mcp",
            crate::types::ToolSourceKind::RustBundle => "RustBundle",
        }
    }

    fn tool_source_kind_from_variant(variant: &str) -> Option<crate::types::ToolSourceKind> {
        Some(match variant {
            "Builtin" => crate::types::ToolSourceKind::Builtin,
            "Shell" => crate::types::ToolSourceKind::Shell,
            "Comms" => crate::types::ToolSourceKind::Comms,
            "Memory" => crate::types::ToolSourceKind::Memory,
            "Schedule" => crate::types::ToolSourceKind::Schedule,
            "WorkGraph" => crate::types::ToolSourceKind::WorkGraph,
            "Mob" => crate::types::ToolSourceKind::Mob,
            "Callback" => crate::types::ToolSourceKind::Callback,
            "Mcp" => crate::types::ToolSourceKind::Mcp,
            "RustBundle" => crate::types::ToolSourceKind::RustBundle,
            _ => return None,
        })
    }

    fn tool_provenance_value(provenance: &crate::types::ToolProvenance) -> KernelValue {
        KernelValue::Map(BTreeMap::from([
            (
                KernelValue::String("kind".to_string()),
                enum_value(
                    "ToolSourceKind",
                    tool_source_kind_variant(provenance.kind.clone()),
                ),
            ),
            (
                KernelValue::String("source_id".to_string()),
                KernelValue::String(provenance.source_id.to_string()),
            ),
        ]))
    }

    fn tool_provenance_from_value(
        value: &KernelValue,
    ) -> Result<crate::types::ToolProvenance, ToolScopeApplyError> {
        let payload = named_payload(value, "ToolProvenance").unwrap_or(value);
        let KernelValue::Map(fields) = payload else {
            return Err(owner_projection_error(
                "ToolProvenance payload was not generated map",
            ));
        };
        let kind = fields
            .get(&KernelValue::String("kind".to_string()))
            .and_then(|value| match value {
                KernelValue::NamedVariant { variant, .. } => {
                    tool_source_kind_from_variant(variant.as_str())
                }
                KernelValue::Named { value, .. } => match value.as_ref() {
                    KernelValue::String(value) => tool_source_kind_from_variant(value),
                    _ => None,
                },
                KernelValue::String(value) => tool_source_kind_from_variant(value),
                _ => None,
            })
            .ok_or_else(|| owner_projection_error("ToolProvenance missing valid `kind`"))?;
        let source_id = fields
            .get(&KernelValue::String("source_id".to_string()))
            .and_then(|value| match value {
                KernelValue::String(value) => Some(value.clone()),
                KernelValue::Named { .. } => match named_payload(value, "ToolSourceId") {
                    Some(KernelValue::String(value)) => Some(value.clone()),
                    _ => None,
                },
                _ => None,
            })
            .ok_or_else(|| owner_projection_error("ToolProvenance missing valid `source_id`"))?;
        Ok(crate::types::ToolProvenance {
            kind,
            source_id: source_id.into(),
        })
    }

    fn witness_value(witness: &ToolVisibilityWitness) -> KernelValue {
        let mut fields = BTreeMap::new();
        if let Some(provenance) = witness.last_seen_provenance.as_ref() {
            fields.insert(
                KernelValue::String("last_seen_provenance".to_string()),
                tool_provenance_value(provenance),
            );
        }
        KernelValue::Named {
            type_name: named_type_id("ToolVisibilityWitness"),
            value: Box::new(KernelValue::Map(fields)),
        }
    }

    fn witness_from_value(
        value: &KernelValue,
    ) -> Result<ToolVisibilityWitness, ToolScopeApplyError> {
        let payload = named_payload(value, "ToolVisibilityWitness").unwrap_or(value);
        let KernelValue::Map(fields) = payload else {
            return Err(owner_projection_error(
                "ToolVisibilityWitness payload was not generated map",
            ));
        };
        let last_seen_provenance =
            match fields.get(&KernelValue::String("last_seen_provenance".to_string())) {
                Some(value) => Some(tool_provenance_from_value(value)?),
                None => None,
            };
        Ok(ToolVisibilityWitness {
            last_seen_provenance,
        })
    }

    fn witness_map_value(witnesses: &BTreeMap<ToolName, ToolVisibilityWitness>) -> KernelValue {
        KernelValue::Map(
            witnesses
                .iter()
                .map(|(name, witness)| {
                    (
                        KernelValue::String(name.as_str().to_owned()),
                        witness_value(witness),
                    )
                })
                .collect(),
        )
    }

    fn witness_map_from_value(
        value: &KernelValue,
    ) -> Result<BTreeMap<ToolName, ToolVisibilityWitness>, ToolScopeApplyError> {
        match value {
            KernelValue::Map(entries) => entries
                .iter()
                .map(|(key, value)| match key {
                    KernelValue::String(name) => witness_from_value(value)
                        .map(|witness| (ToolName::from(name.clone()), witness)),
                    _ => Err(owner_projection_error(
                        "witness map contained non-string key",
                    )),
                })
                .collect(),
            _ => Err(owner_projection_error(
                "witness map was not a generated map",
            )),
        }
    }

    fn state_witness_map(
        state: &KernelState,
        name: &str,
    ) -> Result<BTreeMap<ToolName, ToolVisibilityWitness>, ToolScopeApplyError> {
        witness_map_from_value(required_state_field(state, name)?)
    }

    fn authority_witnesses_for_names(
        names: &BTreeSet<ToolName>,
        witnesses: &BTreeMap<ToolName, ToolVisibilityWitness>,
    ) -> Result<BTreeMap<ToolName, ToolVisibilityWitness>, ToolScopeApplyError> {
        names
            .iter()
            .map(|name| {
                witnesses
                    .get(name)
                    .cloned()
                    .map(|witness| (name.clone(), witness))
                    .ok_or_else(|| {
                        owner_projection_error(format!(
                            "missing visibility witness for deferred tool `{name}`"
                        ))
                    })
            })
            .collect()
    }

    fn map_apply_refusal(err: TransitionRefusal, context: &str) -> ToolScopeApplyError {
        ToolScopeApplyError::Owner {
            message: format!(
                "generated MeerkatMachine visibility authority rejected {context}: {err}"
            ),
        }
    }

    fn map_stage_refusal(err: TransitionRefusal, context: &str) -> ToolScopeStageError {
        ToolScopeStageError::Owner {
            message: format!(
                "generated MeerkatMachine visibility authority rejected {context}: {err}"
            ),
        }
    }

    /// Test-only visibility owner backed by the generated MeerkatMachine kernel.
    #[derive(Debug)]
    pub(crate) struct GeneratedTestToolVisibilityOwner {
        kernel: GeneratedMachineKernel,
        state: RwLock<KernelState>,
    }

    impl GeneratedTestToolVisibilityOwner {
        pub(crate) fn new() -> Self {
            let kernel = GeneratedMachineKernel::new(meerkat::schema());
            let mut state = kernel
                .initial_state()
                .expect("generated MeerkatMachine initial state");
            state = kernel
                .transition_signal(
                    &state,
                    &KernelSignal {
                        variant: signal_id("Initialize"),
                        fields: BTreeMap::new(),
                    },
                )
                .expect("generated MeerkatMachine initialize")
                .next_state;
            state = kernel
                .transition(
                    &state,
                    &input(
                        "RegisterSession",
                        [(
                            "session_id",
                            named_string("SessionId", "tool-scope-test-session".to_string()),
                        )],
                    ),
                )
                .expect("generated MeerkatMachine register session")
                .next_state;
            Self {
                kernel,
                state: RwLock::new(state),
            }
        }

        fn apply_for_apply(
            &self,
            input: KernelInput,
            context: &str,
        ) -> Result<SessionToolVisibilityState, ToolScopeApplyError> {
            let mut state = self
                .state
                .write()
                .map_err(|_| ToolScopeApplyError::LockPoisoned)?;
            let outcome = self
                .kernel
                .transition(&state, &input)
                .map_err(|err| map_apply_refusal(err, context))?;
            *state = outcome.next_state;
            visibility_state_from_kernel(&state)
        }

        fn apply_for_stage(
            &self,
            input: KernelInput,
            context: &str,
        ) -> Result<ToolScopeRevision, ToolScopeStageError> {
            let mut state = self
                .state
                .write()
                .map_err(|_| ToolScopeStageError::LockPoisoned)?;
            let outcome = self
                .kernel
                .transition(&state, &input)
                .map_err(|err| map_stage_refusal(err, context))?;
            *state = outcome.next_state;
            state_u64(&state, "staged_visibility_revision")
                .map(ToolScopeRevision)
                .map_err(map_projection_for_stage)
        }
    }

    fn visibility_state_from_kernel(
        state: &KernelState,
    ) -> Result<SessionToolVisibilityState, ToolScopeApplyError> {
        Ok(SessionToolVisibilityState {
            capability_base_filter: tool_filter_from_value(required_state_field(
                state,
                "current_session_capability_base_filter",
            )?)?,
            inherited_base_filter: tool_filter_from_value(required_state_field(
                state,
                "inherited_base_filter",
            )?)?,
            active_filter: tool_filter_from_value(required_state_field(state, "active_filter")?)?,
            staged_filter: tool_filter_from_value(required_state_field(state, "staged_filter")?)?,
            active_requested_deferred_names: state_string_set(state, "active_deferred_names")?,
            staged_requested_deferred_names: state_string_set(state, "staged_deferred_names")?,
            active_revision: state_u64(state, "active_visibility_revision")?,
            staged_revision: state_u64(state, "staged_visibility_revision")?,
            requested_witnesses: state_witness_map(state, "requested_visibility_witnesses")?,
            filter_witnesses: state_witness_map(state, "filter_visibility_witnesses")?,
        })
    }

    impl ToolVisibilityOwner for GeneratedTestToolVisibilityOwner {
        fn visibility_state(&self) -> Result<SessionToolVisibilityState, ToolScopeApplyError> {
            self.state
                .read()
                .map_err(|_| ToolScopeApplyError::LockPoisoned)
                .and_then(|state| visibility_state_from_kernel(&state))
        }

        fn replace_visibility_state(
            &self,
            visibility_state: SessionToolVisibilityState,
        ) -> Result<(), ToolScopeApplyError> {
            let active_deferred_authorities = authority_witnesses_for_names(
                &visibility_state.active_requested_deferred_names,
                &visibility_state.requested_witnesses,
            )?;
            let staged_deferred_authorities = authority_witnesses_for_names(
                &visibility_state.staged_requested_deferred_names,
                &visibility_state.requested_witnesses,
            )?;
            self.apply_for_apply(
                input(
                    "ReplaceVisibilityState",
                    [
                        (
                            "capability_base_filter",
                            tool_filter_value(&visibility_state.capability_base_filter),
                        ),
                        (
                            "inherited_base_filter",
                            tool_filter_value(&visibility_state.inherited_base_filter),
                        ),
                        (
                            "active_filter",
                            tool_filter_value(&visibility_state.active_filter),
                        ),
                        (
                            "staged_filter",
                            tool_filter_value(&visibility_state.staged_filter),
                        ),
                        (
                            "active_revision",
                            KernelValue::U64(visibility_state.active_revision),
                        ),
                        (
                            "staged_revision",
                            KernelValue::U64(visibility_state.staged_revision),
                        ),
                        (
                            "active_deferred_names",
                            kernel_string_set(&visibility_state.active_requested_deferred_names),
                        ),
                        (
                            "staged_deferred_names",
                            kernel_string_set(&visibility_state.staged_requested_deferred_names),
                        ),
                        (
                            "requested_witnesses",
                            witness_map_value(&visibility_state.requested_witnesses),
                        ),
                        (
                            "filter_witnesses",
                            witness_map_value(&visibility_state.filter_witnesses),
                        ),
                        (
                            "active_deferred_authorities",
                            witness_map_value(&active_deferred_authorities),
                        ),
                        (
                            "staged_deferred_authorities",
                            witness_map_value(&staged_deferred_authorities),
                        ),
                    ],
                ),
                "ReplaceVisibilityState",
            )?;
            Ok(())
        }

        fn stage_persistent_filter(
            &self,
            filter: ToolFilter,
            witnesses: BTreeMap<ToolName, ToolVisibilityWitness>,
        ) -> Result<ToolScopeRevision, ToolScopeStageError> {
            let mut filter_witnesses = self
                .visibility_state()
                .map_err(|err| ToolScopeStageError::Owner {
                    message: err.to_string(),
                })?
                .filter_witnesses;
            filter_witnesses.extend(witnesses);
            self.apply_for_stage(
                input(
                    "StageVisibilityFilter",
                    [
                        ("filter", tool_filter_value(&filter)),
                        ("witnesses", witness_map_value(&filter_witnesses)),
                    ],
                ),
                "StageVisibilityFilter",
            )
        }

        fn stage_requested_deferred_names(
            &self,
            names: BTreeSet<ToolName>,
        ) -> Result<ToolScopeRevision, ToolScopeStageError> {
            if !names.is_empty() {
                return Err(ToolScopeStageError::MissingWitnesses {
                    names: names.into_iter().collect(),
                });
            }
            self.apply_for_stage(
                input("StageDeferredNames", [("names", kernel_string_set(&names))]),
                "StageDeferredNames",
            )
        }

        fn request_deferred_tools(
            &self,
            authorities: Vec<DeferredToolLoadAuthority>,
        ) -> Result<ToolScopeRevision, ToolScopeStageError> {
            let authorities = deferred_load_authority_map(&authorities)?;
            if authorities.is_empty() {
                return Err(ToolScopeStageError::Owner {
                    message: "deferred tool request requires at least one authority".to_string(),
                });
            }
            let state = self
                .state
                .read()
                .map_err(|_| ToolScopeStageError::LockPoisoned)?;
            let mut target_authorities = state_witness_map(&state, "staged_deferred_authorities")
                .map_err(map_projection_for_stage)?;
            drop(state);
            target_authorities.extend(authorities);
            self.apply_for_stage(
                input(
                    "RequestDeferredTools",
                    [("authorities", witness_map_value(&target_authorities))],
                ),
                "RequestDeferredTools",
            )
        }

        fn replace_deferred_tool_authority_catalog(
            &self,
            catalog: BTreeMap<ToolName, ToolVisibilityWitness>,
        ) -> Result<(), ToolScopeApplyError> {
            self.apply_for_apply(
                input(
                    "ReplaceDeferredToolAuthorityCatalog",
                    [("catalog", witness_map_value(&catalog))],
                ),
                "ReplaceDeferredToolAuthorityCatalog",
            )?;
            Ok(())
        }

        fn replace_filter_tool_authority_catalog(
            &self,
            catalog: BTreeMap<ToolName, ToolVisibilityWitness>,
        ) -> Result<(), ToolScopeApplyError> {
            self.apply_for_apply(
                input(
                    "ReplaceFilterToolAuthorityCatalog",
                    [("catalog", witness_map_value(&catalog))],
                ),
                "ReplaceFilterToolAuthorityCatalog",
            )?;
            Ok(())
        }

        fn set_turn_overlay(
            &self,
            allow: Option<BTreeSet<ToolName>>,
            deny: BTreeSet<ToolName>,
        ) -> Result<ToolScopeTurnOverlay, ToolScopeStageError> {
            let allow_active = allow.is_some();
            let allow_names = allow.unwrap_or_default();
            self.apply_for_stage(
                input(
                    "SetTurnToolOverlay",
                    [
                        ("allow_active", KernelValue::Bool(allow_active)),
                        ("allow_names", kernel_string_set(&allow_names)),
                        ("deny_names", kernel_string_set(&deny)),
                    ],
                ),
                "SetTurnToolOverlay",
            )?;
            let state = self
                .state
                .read()
                .map_err(|_| ToolScopeStageError::LockPoisoned)?;
            let allow = if state_bool(&state, "turn_tool_overlay_allow_active")
                .map_err(map_projection_for_stage)?
            {
                Some(
                    state_string_set(&state, "turn_tool_overlay_allow_names")
                        .map_err(map_projection_for_stage)?,
                )
            } else {
                None
            };
            let deny = state_string_set(&state, "turn_tool_overlay_deny_names")
                .map_err(map_projection_for_stage)?;
            Ok(ToolScopeTurnOverlay::from_string_sets(allow, deny))
        }

        fn clear_turn_overlay(&self) -> Result<ToolScopeTurnOverlay, ToolScopeStageError> {
            self.apply_for_stage(input("ClearTurnToolOverlay", []), "ClearTurnToolOverlay")?;
            Ok(ToolScopeTurnOverlay::cleared())
        }

        fn requires_filter_witnesses(&self) -> bool {
            true
        }

        fn boundary_applied(&self) -> Result<SessionToolVisibilityState, ToolScopeApplyError> {
            let (staged_filter, staged_revision, staged_authorities) = {
                let state = self
                    .state
                    .read()
                    .map_err(|_| ToolScopeApplyError::LockPoisoned)?;
                (
                    tool_filter_from_value(required_state_field(&state, "staged_filter")?)?,
                    state_u64(&state, "staged_visibility_revision")?,
                    state_witness_map(&state, "staged_deferred_authorities")?,
                )
            };
            self.apply_for_apply(
                input(
                    "CommitVisibilityFilter",
                    [
                        ("filter", tool_filter_value(&staged_filter)),
                        ("revision", KernelValue::U64(staged_revision)),
                    ],
                ),
                "CommitVisibilityFilter",
            )?;
            self.apply_for_apply(
                input(
                    "CommitDeferredNames",
                    [("authorities", witness_map_value(&staged_authorities))],
                ),
                "CommitDeferredNames",
            )
        }
    }
}

#[cfg(test)]
pub(crate) use generated_visibility_test_owner::GeneratedTestToolVisibilityOwner;

#[derive(Debug, Clone)]
struct ToolScopeState {
    base_tools: Arc<[Arc<ToolDef>]>,
    known_base_names: ToolNameSet,
    control_tool_names: ToolNameSet,
    deferred_tool_names: ToolNameSet,
    active_turn_allow: Option<ToolNameSet>,
    active_turn_deny: ToolNameSet,
}

/// Projection payload accepted by the generated turn-overlay authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolScopeTurnOverlay {
    pub allow: Option<ToolNameSet>,
    pub deny: ToolNameSet,
}

impl ToolScopeTurnOverlay {
    pub fn from_string_sets(allow: Option<BTreeSet<ToolName>>, deny: BTreeSet<ToolName>) -> Self {
        Self {
            allow: allow.map(|names| names.into_iter().collect()),
            deny: deny.into_iter().collect(),
        }
    }

    pub fn cleared() -> Self {
        Self {
            allow: None,
            deny: ToolNameSet::new(),
        }
    }
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
    visibility_authority: ToolVisibilityAuthorityKind,
    fail_next_boundary_apply: Arc<AtomicBool>,
}

#[derive(Debug, Clone)]
pub struct ToolScopeBoundaryResult {
    pub previous_base_names: HashSet<ToolName>,
    pub current_base_names: HashSet<ToolName>,
    pub previous_visible_names: Vec<ToolName>,
    pub visible_names: Vec<ToolName>,
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
        control_tool_names: HashSet<ToolName>,
    ) -> Self {
        Self::new_with_projection_names(base_tools, control_tool_names, HashSet::new())
    }

    /// Build a scope with explicit control-plane and deferred-session names.
    pub fn new_with_projection_names(
        base_tools: Arc<[Arc<ToolDef>]>,
        control_tool_names: HashSet<ToolName>,
        deferred_tool_names: HashSet<ToolName>,
    ) -> Self {
        Self::build_with_visibility_owner(
            base_tools,
            control_tool_names,
            deferred_tool_names.into_iter().collect(),
            default_tool_visibility_owner(),
            ToolVisibilityAuthorityKind::LocalReadOnlyProjection,
        )
    }

    /// Build a scope with an explicit generated durable visibility owner.
    pub fn new_with_visibility_owner(
        base_tools: Arc<[Arc<ToolDef>]>,
        control_tool_names: HashSet<ToolName>,
        deferred_tool_names: HashSet<ToolName>,
        visibility_owner: GeneratedToolVisibilityOwner,
    ) -> Result<Self, ToolScopeApplyError> {
        let deferred_tool_names: ToolNameSet = deferred_tool_names.into_iter().collect();
        let visibility_owner = visibility_owner.owner();
        visibility_owner.replace_deferred_tool_authority_catalog(
            deferred_authority_catalog_for_base_tools(&base_tools, &deferred_tool_names),
        )?;
        visibility_owner.replace_filter_tool_authority_catalog(
            filter_authority_catalog_for_base_tools(&base_tools),
        )?;

        Ok(Self::build_with_visibility_owner(
            base_tools,
            control_tool_names,
            deferred_tool_names,
            visibility_owner,
            ToolVisibilityAuthorityKind::GeneratedMachine,
        ))
    }

    fn build_with_visibility_owner(
        base_tools: Arc<[Arc<ToolDef>]>,
        control_tool_names: HashSet<ToolName>,
        deferred_tool_names: ToolNameSet,
        visibility_owner: Arc<dyn ToolVisibilityOwner>,
        visibility_authority: ToolVisibilityAuthorityKind,
    ) -> Self {
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
            visibility_authority,
            fail_next_boundary_apply: Arc::new(AtomicBool::new(false)),
        }
    }

    fn ensure_generated_authority_for_apply(
        &self,
        context: &'static str,
    ) -> Result<(), ToolScopeApplyError> {
        if self.visibility_authority == ToolVisibilityAuthorityKind::GeneratedMachine {
            Ok(())
        } else {
            Err(ToolScopeApplyError::Owner {
                message: generated_visibility_authority_required_message(context),
            })
        }
    }

    /// Whether this scope owns a durable (generated-MeerkatMachine) tool
    /// visibility projection that must be persisted on boundary apply.
    ///
    /// Standalone builds use a read-only local projection
    /// ([`ToolVisibilityAuthorityKind::LocalReadOnlyProjection`]) with no
    /// durable session-metadata projection to write, so publishing the
    /// committed visible set there is a legitimate no-op rather than a fault.
    pub(crate) fn owns_durable_visibility_projection(&self) -> bool {
        self.visibility_authority == ToolVisibilityAuthorityKind::GeneratedMachine
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
            visibility_authority: self.visibility_authority,
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
        control_tool_names: HashSet<ToolName>,
        deferred_tool_names: HashSet<ToolName>,
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

        let mut deferred_authority_catalog = std::collections::BTreeMap::new();
        extend_deferred_authority_catalog_from_visibility_state(
            &mut deferred_authority_catalog,
            visibility_state,
        );
        for (name, witness) in
            deferred_authority_catalog_for_base_tools(&new_base_tools, &deferred_tool_names)
        {
            deferred_authority_catalog.entry(name).or_insert(witness);
        }
        self.visibility_owner
            .replace_deferred_tool_authority_catalog(deferred_authority_catalog)?;

        let mut filter_authority_catalog = filter_authority_catalog_for_base_tools(&new_base_tools);
        extend_filter_authority_catalog_from_visibility_state(
            &mut filter_authority_catalog,
            visibility_state,
        );
        self.visibility_owner
            .replace_filter_tool_authority_catalog(filter_authority_catalog)?;

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

        let tools =
            Self::visible_tools_for_state(&state, visibility_state, require_filter_witnesses);
        let visible_names = tools
            .iter()
            .map(|tool| tool.name.clone())
            .collect::<Vec<_>>();

        Ok(ToolScopeBoundaryResult {
            previous_base_names: previous_base_names.into_inner(),
            current_base_names: state.known_base_names.clone().into_inner(),
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
    ) -> Vec<ToolName> {
        let tools =
            Self::visible_tools_for_state(state, visibility_state, require_filter_witnesses);
        tools.iter().map(|tool| tool.name.clone()).collect()
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
            witness.has_identity_witness() && Self::witness_matches_tool(Some(witness), tool)
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
        self.ensure_generated_authority_for_apply("setting inherited base filter")?;
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
        self.ensure_generated_authority_for_apply("replacing durable visibility state")?;
        let (base_tools, deferred_tool_names, current_visibility_state) = {
            let state = self
                .state
                .read()
                .map_err(|_| ToolScopeApplyError::LockPoisoned)?;
            (
                Arc::clone(&state.base_tools),
                state.deferred_tool_names.clone(),
                self.visibility_owner.visibility_state()?,
            )
        };
        let mut deferred_authority_catalog =
            deferred_authority_catalog_for_base_tools(&base_tools, &deferred_tool_names);
        extend_deferred_authority_catalog_from_visibility_state(
            &mut deferred_authority_catalog,
            &current_visibility_state,
        );
        extend_deferred_authority_catalog_from_visibility_state(
            &mut deferred_authority_catalog,
            &visibility_state,
        );
        self.visibility_owner
            .replace_deferred_tool_authority_catalog(deferred_authority_catalog)?;

        let mut filter_authority_catalog = filter_authority_catalog_for_base_tools(&base_tools);
        extend_filter_authority_catalog_from_visibility_state(
            &mut filter_authority_catalog,
            &current_visibility_state,
        );
        extend_filter_authority_catalog_from_visibility_state(
            &mut filter_authority_catalog,
            &visibility_state,
        );
        self.visibility_owner
            .replace_filter_tool_authority_catalog(filter_authority_catalog)?;
        self.visibility_owner
            .replace_visibility_state(visibility_state)
    }

    /// Snapshot the current durable visibility state.
    pub fn visibility_state(&self) -> Result<SessionToolVisibilityState, ToolScopeApplyError> {
        self.visibility_owner.visibility_state()
    }

    /// Snapshot the generated-authority-approved durable visibility projection.
    pub fn authorized_visibility_state(
        &self,
    ) -> Result<AuthorizedSessionToolVisibilityState, ToolScopeApplyError> {
        self.ensure_generated_authority_for_apply("authorizing durable visibility state")?;
        self.visibility_owner
            .visibility_state()
            .map(AuthorizedSessionToolVisibilityState::from_generated_authority)
    }

    /// Return the names currently visible to the session plane.
    pub fn visible_tool_names(&self) -> Result<BTreeSet<ToolName>, ToolScopeApplyError> {
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
    pub fn base_tool_names(&self) -> Result<BTreeSet<ToolName>, ToolScopeApplyError> {
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
    pub fn missing_requested_names(&self) -> Result<BTreeSet<ToolName>, ToolScopeApplyError> {
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
    pub fn missing_filter_names(&self) -> Result<BTreeSet<ToolName>, ToolScopeApplyError> {
        let state = self
            .state
            .read()
            .map_err(|_| ToolScopeApplyError::LockPoisoned)?;
        let visibility_state = self.visibility_owner.visibility_state()?;
        Ok(durable_filter_names(&visibility_state)
            .into_iter()
            .filter(|name| !state.known_base_names.contains(name.as_str()))
            .collect::<BTreeSet<_>>())
    }

    /// Record durable requested deferred names for the next boundary.
    pub fn stage_requested_deferred_names(
        &self,
        names: BTreeSet<ToolName>,
    ) -> Result<ToolScopeRevision, ToolScopeStageError> {
        self.visibility_owner.stage_requested_deferred_names(names)
    }

    /// Add durable requested deferred authorities for the next boundary.
    pub fn add_requested_deferred_authorities(
        &self,
        authorities: &[DeferredToolLoadAuthority],
    ) -> Result<ToolScopeRevision, ToolScopeStageError> {
        if self.visibility_authority != ToolVisibilityAuthorityKind::GeneratedMachine {
            return Err(ToolScopeStageError::Owner {
                message: generated_visibility_authority_required_message(
                    "requesting deferred tools",
                ),
            });
        }
        self.visibility_owner
            .request_deferred_tools(authorities.to_vec())
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
    visibility_authority: ToolVisibilityAuthorityKind,
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
    fn ensure_generated_authority_for_stage(
        &self,
        context: &'static str,
    ) -> Result<(), ToolScopeStageError> {
        if self.visibility_authority == ToolVisibilityAuthorityKind::GeneratedMachine {
            Ok(())
        } else {
            Err(ToolScopeStageError::Owner {
                message: generated_visibility_authority_required_message(context),
            })
        }
    }

    /// Stage an external filter update and return its monotonic revision.
    pub fn stage_external_filter(
        &self,
        filter: ToolFilter,
    ) -> Result<ToolScopeRevision, ToolScopeStageError> {
        self.ensure_generated_authority_for_stage("staging external tool visibility")?;
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
        allow: Option<HashSet<ToolName>>,
        deny: HashSet<ToolName>,
    ) -> Result<(), ToolScopeStageError> {
        self.ensure_generated_authority_for_stage("setting turn tool overlay")?;
        let allow: Option<ToolNameSet> = allow.map(|names| names.into_iter().collect());
        let deny: ToolNameSet = deny.into_iter().collect();
        let accepted = self.visibility_owner.set_turn_overlay(
            allow.as_ref().map(tool_name_set_to_btree),
            tool_name_set_to_btree(&deny),
        )?;
        let mut state = self
            .state
            .write()
            .map_err(|_| ToolScopeStageError::LockPoisoned)?;

        state.active_turn_allow = accepted.allow;
        state.active_turn_deny = accepted.deny;
        Ok(())
    }

    /// Clear ephemeral per-turn overlay.
    pub fn clear_turn_overlay(&self) -> Result<(), ToolScopeStageError> {
        self.ensure_generated_authority_for_stage("clearing turn tool overlay")?;
        let accepted = self.visibility_owner.clear_turn_overlay()?;
        let mut state = self
            .state
            .write()
            .map_err(|_| ToolScopeStageError::LockPoisoned)?;
        state.active_turn_allow = accepted.allow;
        state.active_turn_deny = accepted.deny;
        Ok(())
    }
}

fn tool_name_set_to_btree(names: &ToolNameSet) -> BTreeSet<ToolName> {
    names.iter().cloned().collect()
}

fn validate_filter(
    filter: &ToolFilter,
    known_base_names: &ToolNameSet,
) -> Result<(), ToolScopeStageError> {
    let Some(names) = filter.names() else {
        return Ok(());
    };

    let mut unknown: Vec<ToolName> = names
        .iter()
        .filter(|name| !known_base_names.contains(name.as_str()))
        .cloned()
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
) -> std::collections::BTreeMap<ToolName, ToolVisibilityWitness> {
    let Some(filter_names) = filter.names() else {
        return Default::default();
    };

    let mut witnesses = std::collections::BTreeMap::new();
    for name in filter_names {
        if let Some(tool) = tool_defs.iter().find(|tool| tool.name == name.as_str()) {
            let witness = filter_witness_for_tool(tool);
            if witness.has_identity_witness() {
                witnesses.insert(name.clone(), witness);
            }
        }
    }
    witnesses
}

pub fn validate_inherited_filter_witnesses(
    filter: &ToolFilter,
    witnesses: &std::collections::BTreeMap<ToolName, ToolVisibilityWitness>,
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
        .cloned()
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
    witnesses: &std::collections::BTreeMap<ToolName, ToolVisibilityWitness>,
    catalog: &std::collections::BTreeMap<ToolName, ToolVisibilityWitness>,
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
        .cloned()
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
    witnesses: &std::collections::BTreeMap<ToolName, ToolVisibilityWitness>,
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
) -> std::collections::BTreeMap<ToolName, ToolVisibilityWitness> {
    base_tools
        .iter()
        .filter(|tool| deferred_tool_names.contains(tool.name.as_str()))
        .filter_map(|tool| {
            let provenance = tool.provenance.as_ref()?;
            Some((
                tool.name.clone(),
                ToolVisibilityWitness {
                    last_seen_provenance: Some(provenance.clone()),
                },
            ))
        })
        .collect()
}

fn filter_authority_catalog_for_base_tools(
    base_tools: &[Arc<ToolDef>],
) -> std::collections::BTreeMap<ToolName, ToolVisibilityWitness> {
    base_tools
        .iter()
        .filter_map(|tool| {
            let witness = filter_witness_for_tool(tool);
            witness
                .has_identity_witness()
                .then(|| (tool.name.clone(), witness))
        })
        .collect()
}

fn extend_deferred_authority_catalog_from_visibility_state(
    catalog: &mut std::collections::BTreeMap<ToolName, ToolVisibilityWitness>,
    visibility_state: &SessionToolVisibilityState,
) {
    for name in visibility_state
        .active_requested_deferred_names
        .iter()
        .chain(visibility_state.staged_requested_deferred_names.iter())
    {
        if catalog.contains_key(name.as_str()) {
            continue;
        }
        if let Some(witness) = visibility_state
            .requested_witnesses
            .get(name.as_str())
            .filter(|witness| witness.has_identity_witness())
        {
            catalog.insert(name.clone(), witness.clone());
        }
    }
}

fn extend_filter_authority_catalog_from_visibility_state(
    catalog: &mut std::collections::BTreeMap<ToolName, ToolVisibilityWitness>,
    visibility_state: &SessionToolVisibilityState,
) {
    for witness_names in [
        visibility_state.inherited_base_filter.names(),
        visibility_state.active_filter.names(),
        visibility_state.staged_filter.names(),
    ]
    .into_iter()
    .flatten()
    {
        for name in witness_names {
            if catalog.contains_key(name.as_str()) {
                continue;
            }
            if let Some(witness) = visibility_state
                .filter_witnesses
                .get(name.as_str())
                .filter(|witness| witness.has_identity_witness())
            {
                catalog.insert(name.clone(), witness.clone());
            }
        }
    }
}

#[cfg(test)]
fn deferred_load_authority_map(
    authorities: &[DeferredToolLoadAuthority],
) -> Result<std::collections::BTreeMap<ToolName, ToolVisibilityWitness>, ToolScopeStageError> {
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
) -> std::collections::BTreeMap<ToolName, ToolVisibilityWitness> {
    let mut witnesses = std::collections::BTreeMap::new();
    extend_filter_witnesses(base_tools, &mut witnesses, filter);
    witnesses
}

fn filter_witnesses_for_base_tools_or_existing(
    base_tools: &Arc<[Arc<ToolDef>]>,
    existing: &std::collections::BTreeMap<ToolName, ToolVisibilityWitness>,
    filter: &ToolFilter,
) -> std::collections::BTreeMap<ToolName, ToolVisibilityWitness> {
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
            witnesses.insert(name.clone(), witness.clone());
        }
    }
    witnesses
}

fn extend_filter_witnesses(
    base_tools: &Arc<[Arc<ToolDef>]>,
    witnesses: &mut std::collections::BTreeMap<ToolName, ToolVisibilityWitness>,
    filter: &ToolFilter,
) {
    let Some(filter_names) = filter.names() else {
        return;
    };

    for name in filter_names {
        if let Some(tool) = base_tools.iter().find(|tool| tool.name == name.as_str()) {
            let witness = filter_witness_for_tool(tool);
            if witness.has_identity_witness() {
                witnesses.insert(name.clone(), witness);
            }
        }
    }
}

fn filter_witness_for_tool(tool: &ToolDef) -> ToolVisibilityWitness {
    ToolVisibilityWitness {
        last_seen_provenance: tool.provenance.clone(),
    }
}

fn sorted_names(names: &ToolNameSet) -> Vec<ToolName> {
    let mut values = names.iter().cloned().collect::<Vec<_>>();
    values.sort_unstable();
    values
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::ToolScopeRevision;
    use super::{
        GeneratedToolVisibilityOwner, LocalToolVisibilityOwner, ToolFilter, ToolScope,
        ToolScopeApplyError, ToolScopeStageError, ToolVisibilityOwner,
    };
    use crate::session::{
        DeferredToolLoadAuthority, SessionToolVisibilityState, ToolVisibilityWitness,
    };
    use crate::types::{ToolDef, ToolName, ToolNameSet, ToolProvenance, ToolSourceKind};
    use std::collections::{BTreeMap, BTreeSet, HashSet};
    use std::sync::Arc;

    fn set(names: &[&str]) -> ToolNameSet {
        names.iter().map(|name| (*name).to_string()).collect()
    }

    fn raw_set(names: &[&str]) -> HashSet<ToolName> {
        names.iter().map(|name| ToolName::from(*name)).collect()
    }

    fn tools(names: &[&str]) -> Arc<[Arc<ToolDef>]> {
        names
            .iter()
            .map(|name| {
                Arc::new(ToolDef {
                    name: (*name).into(),
                    description: format!("{name} tool"),
                    input_schema: serde_json::json!({ "type": "object" }),
                    provenance: Some(ToolProvenance {
                        kind: ToolSourceKind::Callback,
                        source_id: (*name).into(),
                    }),
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

    fn generated_visibility_owner_from<T>(owner: Arc<T>) -> GeneratedToolVisibilityOwner
    where
        T: ToolVisibilityOwner + 'static,
    {
        let owner: Arc<dyn ToolVisibilityOwner> = owner;
        GeneratedToolVisibilityOwner::from_generated_authority(owner)
    }

    fn generated_visibility_owner() -> GeneratedToolVisibilityOwner {
        generated_visibility_owner_from(Arc::new(super::GeneratedTestToolVisibilityOwner::new()))
    }

    fn scope_with_generated_visibility(base_tools: Arc<[Arc<ToolDef>]>) -> ToolScope {
        ToolScope::new_with_visibility_owner(
            base_tools,
            HashSet::new(),
            HashSet::new(),
            generated_visibility_owner(),
        )
        .expect("generated test visibility owner should accept initial catalog authority")
    }

    fn scope_with_generated_projection_names(
        base_tools: Arc<[Arc<ToolDef>]>,
        deferred_tool_names: HashSet<ToolName>,
    ) -> ToolScope {
        ToolScope::new_with_visibility_owner(
            base_tools,
            HashSet::new(),
            deferred_tool_names,
            generated_visibility_owner(),
        )
        .expect("generated test visibility owner should accept projection authority")
    }

    fn scope_with_generated_control_tool_names(
        base_tools: Arc<[Arc<ToolDef>]>,
        control_tool_names: HashSet<ToolName>,
    ) -> ToolScope {
        ToolScope::new_with_visibility_owner(
            base_tools,
            control_tool_names,
            HashSet::new(),
            generated_visibility_owner(),
        )
        .expect("generated test visibility owner should accept control authority")
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
            _witnesses: BTreeMap<ToolName, ToolVisibilityWitness>,
        ) -> Result<ToolScopeRevision, ToolScopeStageError> {
            Err(ToolScopeStageError::Owner {
                message: "visibility read fixture failed".to_string(),
            })
        }

        fn stage_requested_deferred_names(
            &self,
            _names: BTreeSet<ToolName>,
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
    fn local_visibility_owner_is_read_only() {
        let owner = LocalToolVisibilityOwner::new();
        assert_eq!(
            owner.boundary_applied().unwrap(),
            SessionToolVisibilityState::default()
        );
        let staged =
            owner.stage_persistent_filter(ToolFilter::Deny(set(&["secret"])), BTreeMap::new());
        assert!(matches!(staged, Err(ToolScopeStageError::Owner { .. })));

        let replaced = owner.replace_visibility_state(SessionToolVisibilityState {
            active_filter: ToolFilter::Deny(set(&["secret"])),
            staged_filter: ToolFilter::Deny(set(&["secret"])),
            ..Default::default()
        });
        assert!(matches!(replaced, Err(ToolScopeApplyError::Owner { .. })));
    }

    #[test]
    fn local_tool_scope_cannot_authorize_or_stage_visibility() {
        let scope = ToolScope::new(tools(&["visible", "secret"]));
        let authorized = scope.authorized_visibility_state();
        assert!(matches!(authorized, Err(ToolScopeApplyError::Owner { .. })));

        let staged = scope
            .handle()
            .stage_external_filter(ToolFilter::Deny(set(&["secret"])));
        assert!(matches!(staged, Err(ToolScopeStageError::Owner { .. })));
    }

    #[test]
    fn stage_revision_is_monotonic() {
        let scope = scope_with_generated_visibility(tools(&["a", "b", "c"]));
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
            generated_visibility_owner_from(Arc::new(VisibilityReadFailingOwner)),
        )
        .expect("read-failing fixture should still accept initial catalog authority");

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
        let scope = scope_with_generated_visibility(tools(&["known"]));
        let handle = scope.handle();

        let err = handle
            .stage_external_filter(ToolFilter::Allow(set(&["known", "missing"])))
            .unwrap_err();

        assert_eq!(
            err,
            ToolScopeStageError::UnknownTools {
                names: vec!["missing".into()],
            }
        );
    }

    #[test]
    fn control_tools_remain_visible_and_unfilterable() {
        let scope = scope_with_generated_control_tool_names(
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
                names: vec!["tool_catalog_search".into()],
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
        let scope = scope_with_generated_projection_names(
            vec![Arc::clone(&visible), Arc::clone(&deferred)].into(),
            raw_set(&["deferred"]),
        );

        assert_eq!(
            scope.visible_tool_names().unwrap(),
            ["visible".into()].into_iter().collect(),
            "deferred tools should be hidden until requested"
        );

        scope
            .add_requested_deferred_authorities(&[crate::DeferredToolLoadAuthority::new(
                "deferred",
                crate::ToolVisibilityWitness {
                    last_seen_provenance: deferred.provenance.clone(),
                },
            )])
            .unwrap();

        assert_eq!(
            scope.visible_tool_names().unwrap(),
            ["visible".into()].into_iter().collect(),
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
        let scope = scope_with_generated_visibility(tools(&["a", "b", "c"]));
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
            Some(vec!["b".into(), "c".into()])
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
        let scope = scope_with_generated_visibility(tools(&["visible", "secret"]));
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
        let scope = scope_with_generated_visibility(tools(&["visible", "secret"]));
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
        let scope = scope_with_generated_visibility(tools(&["a", "b", "c"]));
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
        let scope = scope_with_generated_projection_names(
            vec![Arc::clone(&requested)].into(),
            raw_set(&["deferred"]),
        );

        scope
            .add_requested_deferred_authorities(&[crate::DeferredToolLoadAuthority::new(
                "deferred",
                crate::ToolVisibilityWitness {
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
            ["deferred".into()].into_iter().collect(),
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
        let scope = scope_with_generated_projection_names(
            vec![Arc::clone(&requested)].into(),
            raw_set(&["deferred"]),
        );

        let err = scope
            .set_visibility_state(crate::SessionToolVisibilityState {
                active_requested_deferred_names: ["deferred".into()].into_iter().collect(),
                staged_requested_deferred_names: ["deferred".into()].into_iter().collect(),
                requested_witnesses: [("deferred".into(), crate::ToolVisibilityWitness::default())]
                    .into_iter()
                    .collect(),
                ..Default::default()
            })
            .expect_err("local replacement must reject empty deferred-tool authority");

        assert!(
            err.to_string().contains("ReplaceVisibilityState"),
            "generated visibility authority should reject the missing deferred authority: {err}"
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
        let scope = scope_with_generated_projection_names(
            vec![Arc::clone(&requested)].into(),
            raw_set(&["deferred"]),
        );

        let err = scope
            .set_visibility_state(crate::SessionToolVisibilityState {
                active_requested_deferred_names: ["deferred".into()].into_iter().collect(),
                staged_requested_deferred_names: ["deferred".into()].into_iter().collect(),
                requested_witnesses: [(
                    "deferred".into(),
                    crate::ToolVisibilityWitness {
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
            err.to_string().contains("ReplaceVisibilityState"),
            "generated visibility authority should reject the mismatched deferred authority: {err}"
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
        let scope = scope_with_generated_projection_names(
            vec![Arc::clone(&requested)].into(),
            raw_set(&["deferred"]),
        );

        let err = scope
            .stage_requested_deferred_names(["deferred".into()].into_iter().collect())
            .expect_err("name-only deferred staging must not become authority");

        assert_eq!(
            err,
            ToolScopeStageError::MissingWitnesses {
                names: vec!["deferred".into()],
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
    fn requested_deferred_names_reject_empty_witnesses() {
        let requested = tool_with_provenance("deferred", "owner-a");
        let scope = scope_with_generated_projection_names(
            vec![Arc::clone(&requested)].into(),
            raw_set(&["deferred"]),
        );

        let err = scope
            .add_requested_deferred_authorities(&[crate::DeferredToolLoadAuthority::new(
                "deferred",
                crate::ToolVisibilityWitness::default(),
            )])
            .expect_err("empty deferred-tool witnesses should fail");

        assert!(
            err.to_string().contains("RequestDeferredTools"),
            "generated visibility authority should reject empty deferred authority: {err}"
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
        let scope = scope_with_generated_projection_names(
            vec![Arc::clone(&requested)].into(),
            raw_set(&["deferred"]),
        );

        let err = scope
            .add_requested_deferred_authorities(&[crate::DeferredToolLoadAuthority::new(
                "deferred",
                crate::ToolVisibilityWitness {
                    last_seen_provenance: Some(ToolProvenance {
                        kind: ToolSourceKind::Callback,
                        source_id: "owner-b".into(),
                    }),
                },
            )])
            .expect_err("mismatched deferred-tool authority should fail");

        assert!(
            err.to_string().contains("RequestDeferredTools"),
            "generated visibility authority should reject mismatched deferred authority: {err}"
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
        let scope = scope_with_generated_projection_names(
            vec![Arc::clone(&requested)].into(),
            raw_set(&["deferred"]),
        );

        let err = scope
            .add_requested_deferred_authorities(&[
                crate::DeferredToolLoadAuthority::new(
                    "deferred",
                    crate::ToolVisibilityWitness {
                        last_seen_provenance: requested.provenance.clone(),
                    },
                ),
                crate::DeferredToolLoadAuthority::new(
                    "deferred",
                    crate::ToolVisibilityWitness {
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
        let scope = scope_with_generated_projection_names(
            vec![Arc::clone(&requested_a), Arc::clone(&requested_b)].into(),
            raw_set(&["deferred_a", "deferred_b"]),
        );

        scope
            .add_requested_deferred_authorities(&[crate::DeferredToolLoadAuthority::new(
                "deferred_a",
                crate::ToolVisibilityWitness {
                    last_seen_provenance: requested_a.provenance.clone(),
                },
            )])
            .expect("initial deferred request should stage witness");

        scope
            .add_requested_deferred_authorities(&[crate::DeferredToolLoadAuthority::new(
                "deferred_b",
                crate::ToolVisibilityWitness {
                    last_seen_provenance: requested_b.provenance.clone(),
                },
            )])
            .expect("extended deferred request should reuse already staged witnesses");

        let state = scope.visibility_state().unwrap();
        assert_eq!(
            state.staged_requested_deferred_names,
            ["deferred_a".into(), "deferred_b".into()]
                .into_iter()
                .collect::<std::collections::BTreeSet<ToolName>>()
        );
        assert!(state.requested_witnesses.contains_key("deferred_a"));
        assert!(state.requested_witnesses.contains_key("deferred_b"));
    }

    #[test]
    fn filter_witness_mismatch_prevents_rebinding_a_dormant_filter_name() {
        let original = tool_with_provenance("a", "owner-a");
        let rebound = tool_with_provenance("a", "owner-b");
        let visible = tool_with_provenance("b", "owner-b");
        let scope = scope_with_generated_visibility(
            vec![Arc::clone(&original), Arc::clone(&visible)].into(),
        );
        let handle = scope.handle();

        handle
            .stage_external_filter(ToolFilter::Deny(set(&["a"])))
            .unwrap();
        scope
            .apply_staged(vec![Arc::clone(&original), Arc::clone(&visible)].into())
            .unwrap();
        assert_eq!(
            scope.visible_tool_names().unwrap(),
            ["b".into()].into_iter().collect(),
            "the original owner should be hidden by the deny filter"
        );

        scope
            .apply_staged(vec![Arc::clone(&rebound), Arc::clone(&visible)].into())
            .unwrap();
        assert_eq!(
            scope.visible_tool_names().unwrap(),
            ["a".into(), "b".into()].into_iter().collect(),
            "a different owner must not inherit the dormant filter intent"
        );
    }

    #[test]
    fn empty_inherited_witness_does_not_become_authority() {
        let original = tool_with_provenance("a", "owner-a");
        let scope = scope_with_generated_visibility(vec![Arc::clone(&original)].into());

        let err = scope
            .set_visibility_state(crate::SessionToolVisibilityState {
                inherited_base_filter: ToolFilter::Allow(set(&["a"])),
                filter_witnesses: [("a".into(), crate::ToolVisibilityWitness::default())]
                    .into_iter()
                    .collect(),
                ..Default::default()
            })
            .expect_err("generated visibility authority must reject empty inherited witnesses");

        assert!(
            err.to_string().contains("ReplaceVisibilityState"),
            "generated visibility authority should reject empty inherited witness: {err}"
        );
        assert!(
            matches!(
                scope.visibility_state().unwrap().inherited_base_filter,
                ToolFilter::All
            ),
            "failed inherited witness replacement must not install local visibility facts"
        );
    }

    #[test]
    fn inherited_filter_witness_mismatch_prevents_rebinding_a_dormant_name() {
        let original = tool_with_provenance("a", "owner-a");
        let rebound = tool_with_provenance("a", "owner-b");
        let scope = scope_with_generated_visibility(vec![Arc::clone(&original)].into());

        scope
            .set_visibility_state(crate::SessionToolVisibilityState {
                inherited_base_filter: ToolFilter::Allow(set(&["a"])),
                filter_witnesses: [(
                    "a".into(),
                    crate::ToolVisibilityWitness {
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
            ["a".into()].into_iter().collect(),
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
        let scope = scope_with_generated_visibility(tools(&["a", "b", "c"]));
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

        handle.clear_turn_overlay().unwrap();
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
        let scope = scope_with_generated_visibility(tools(&["a", "b", "c"]));

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
        let scope = scope_with_generated_visibility(tools(&["a", "b", "c"]));
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
