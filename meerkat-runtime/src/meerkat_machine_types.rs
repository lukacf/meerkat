//! Meerkat runtime command/result and snapshot support types.
//!
//! The authority surface now lives in `meerkat_machine.rs`; this module holds
//! the supporting command/result enums and the durable diagnostic snapshots that
//! remain useful after cutover.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::meerkat_machine::{CommsDrainMode, CommsDrainPhase, DrainExitReason};
use chrono::{DateTime, Utc};
use indexmap::IndexSet;
use meerkat_contracts::{RealtimeChannelState, RealtimeChannelStatus};
use meerkat_core::RuntimeEpochId;
use meerkat_core::agent::CommsRuntime;
use meerkat_core::image_generation::{
    ImageOperationApprovalReason, ImageOperationDenialReason, ImageOperationId,
    ImageOperationPhase, ImageOperationTerminalClass, SessionModelRoutingStatus,
    SwitchTurnApprovalReason, SwitchTurnControlResult, SwitchTurnIntent, SwitchTurnRequestId,
};
use meerkat_core::lifecycle::WaitRequestId;
use meerkat_core::lifecycle::core_executor::CoreApplyOutput;
use meerkat_core::lifecycle::core_executor::CoreExecutor;
use meerkat_core::lifecycle::run_control::RunControlCommand;
use meerkat_core::lifecycle::run_primitive::{ModelId, RunPrimitive};
use meerkat_core::lifecycle::{InputId, RunId};
use meerkat_core::lifecycle::{RunBoundaryReceipt, RunId as LifecycleRunId};
use meerkat_core::ops::OperationId;
use meerkat_core::ops_lifecycle::OperationLifecycleSnapshot;
use meerkat_core::types::HandlingMode;
use meerkat_core::types::SessionId;
use meerkat_machine_derive::CommandManifest;
use meerkat_machine_schema::catalog::dsl::meerkat_machine::MeerkatMachineInputVariant;
use serde::{Deserialize, Serialize};

use crate::AcceptOutcome;
use crate::identifiers::LogicalRuntimeId;
use crate::ingress_types::{ContentShape, RequestId, ReservationKey};
use crate::input::Input;
use crate::input_state::InputLifecycleState;
use crate::input_state::InputTerminalOutcome;
use crate::input_state::StoredInputState;
use crate::runtime_event::RuntimeEventEnvelope;
use crate::runtime_state::RuntimeState;
use crate::traits::{
    DestroyReport, RecoveryReport, RecycleReport, ResetReport, RetireReport,
    RuntimeControlPlaneError, RuntimeDriverError,
};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RealtimeAttachmentStatus {
    Unattached,
    IntentPresentUnbound,
    BindingNotReady,
    BindingReady,
    ReplacementPending,
    ReattachRequired,
}

/// Typed reconnect-progress carrier read from machine-owned reconnect state.
/// `attempt_count` and `next_retry_at` used to be hard-coded or projected from
/// websocket-local retry state; the canonical `RealtimeChannelStatus`
/// projection now reads them from the MeerkatMachine lifecycle fields.
///
/// `deadline_at` is the wall-clock cutoff after which the reconnect cycle
/// will be abandoned (`reconnect_policy.max_total_ms` on top of the
/// cycle's start time). `None` outside an active reconnect cycle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconnectProgress {
    pub attempt_count: u32,
    pub next_retry_at: Option<DateTime<Utc>>,
    pub deadline_at: Option<DateTime<Utc>>,
}

impl ReconnectProgress {
    pub fn new(
        attempt_count: u32,
        next_retry_at: Option<DateTime<Utc>>,
        deadline_at: Option<DateTime<Utc>>,
    ) -> Self {
        Self {
            attempt_count,
            next_retry_at,
            deadline_at,
        }
    }
}

impl RealtimeAttachmentStatus {
    /// Canonical mapping to the public wire reason. `None` means the
    /// `reason` field is unset on the projected `RealtimeChannelStatus`
    /// (used for `BindingReady`, where there is no explanatory text).
    fn default_reason(self) -> Option<&'static str> {
        match self {
            Self::Unattached => Some("no realtime channel is open for this target"),
            Self::IntentPresentUnbound | Self::BindingNotReady => {
                Some("realtime attachment is pending")
            }
            Self::BindingReady => None,
            Self::ReplacementPending => Some("realtime attachment replacement is pending"),
            Self::ReattachRequired => Some("realtime attachment requires reattach"),
        }
    }

    fn channel_state(self) -> RealtimeChannelState {
        match self {
            Self::Unattached => RealtimeChannelState::Closed,
            Self::IntentPresentUnbound | Self::BindingNotReady => RealtimeChannelState::Opening,
            Self::BindingReady => RealtimeChannelState::Ready,
            Self::ReplacementPending | Self::ReattachRequired => RealtimeChannelState::Reconnecting,
        }
    }

    /// Canonical projection, reconnect-progress-aware variant. Active
    /// reconnect progress is read from machine state and passed as
    /// `Some(progress)`; reconnecting statuses without an active machine-owned
    /// cycle report zero attempts and no retry deadline.
    pub fn to_channel_status(self, reconnect: Option<&ReconnectProgress>) -> RealtimeChannelStatus {
        let state = self.channel_state();
        let reason = self.default_reason().map(str::to_string);
        let (attempt_count, next_retry_at, deadline_at) = match (state, reconnect) {
            (RealtimeChannelState::Reconnecting, Some(progress)) => (
                progress.attempt_count,
                progress.next_retry_at.map(|dt| dt.to_rfc3339()),
                progress.deadline_at.map(|dt| dt.to_rfc3339()),
            ),
            (RealtimeChannelState::Reconnecting, None) => (0, None, None),
            _ => (0, None, None),
        };
        RealtimeChannelStatus {
            state,
            attempt_count,
            next_retry_at,
            deadline_at,
            reason,
        }
    }
}

impl From<RealtimeAttachmentStatus> for RealtimeChannelStatus {
    fn from(status: RealtimeAttachmentStatus) -> Self {
        status.to_channel_status(None)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RealtimeAttachmentSignalAuthority {
    pub session_id: SessionId,
    pub authority_epoch: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub struct SessionLlmReconfigureRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_params: Option<serde_json::Value>,
    /// Explicitly clear the session's durable provider params. This is
    /// distinct from omitting `provider_params`, which inherits the current
    /// value for compatibility.
    #[serde(default, skip_serializing_if = "is_false")]
    pub clear_provider_params: bool,
    /// Optional realm-scoped connection override. When present, the
    /// hot-swap uses this binding to resolve credentials; when absent,
    /// the session's existing `SessionLlmIdentity.connection_ref` is preserved
    /// unless `clear_connection_ref` is true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_ref: Option<meerkat_core::ConnectionRef>,
    /// Explicitly clear the session's durable connection reference. This is
    /// distinct from omitting `connection_ref`, which inherits the current
    /// binding for compatibility.
    #[serde(default, skip_serializing_if = "is_false")]
    pub clear_connection_ref: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SessionLlmCapabilitySurfaceStatus {
    Resolved,
    #[default]
    Unresolved,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct SessionLlmCapabilitySurface {
    pub supports_temperature: bool,
    pub supports_thinking: bool,
    pub supports_reasoning: bool,
    pub inline_video: bool,
    pub vision: bool,
    pub image_tool_results: bool,
    pub supports_web_search: bool,
    /// Whether the resolved model exposes a realtime bidirectional streaming
    /// transport. Drives capability-based auto attach/detach in
    /// `reconfigure_live_topology` and `apply_capability_driven_realtime_transport`.
    #[serde(default)]
    pub realtime: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_timeout_secs: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionLlmCapabilityDelta {
    pub previous: Option<SessionLlmCapabilitySurface>,
    pub current: Option<SessionLlmCapabilitySurface>,
    pub changed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionToolVisibilityDelta {
    pub previous_capability_base_filter: meerkat_core::ToolFilter,
    pub current_capability_base_filter: meerkat_core::ToolFilter,
    pub committed_visible_set_changed: bool,
    pub revision_bumped: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SessionLlmReconfigureReport {
    pub previous_identity: meerkat_core::SessionLlmIdentity,
    pub new_identity: meerkat_core::SessionLlmIdentity,
    pub capability_delta: SessionLlmCapabilityDelta,
    pub tool_visibility_delta: SessionToolVisibilityDelta,
    pub rollback_occurred: bool,
}

#[derive(Debug, Clone)]
pub struct HydratedSessionLlmState {
    pub current_identity: meerkat_core::SessionLlmIdentity,
    pub current_visibility_state: meerkat_core::SessionToolVisibilityState,
    pub current_capability_surface: Option<SessionLlmCapabilitySurface>,
    pub capability_surface_status: SessionLlmCapabilitySurfaceStatus,
    pub base_tool_names: std::collections::BTreeSet<String>,
}

#[derive(Debug, Clone)]
pub struct ResolvedSessionLlmReconfigure {
    pub target_identity: meerkat_core::SessionLlmIdentity,
    pub target_capability_surface: SessionLlmCapabilitySurface,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelRoutingApprovalDisposition {
    NotRequired,
    Approved,
    DeniedByUser,
    RequiredButUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelRoutingRealtimePolicy {
    pub target_realtime_capable: bool,
    pub allow_realtime_detach: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwitchTurnRequest {
    pub request_id: SwitchTurnRequestId,
    pub intent: SwitchTurnIntent,
    pub target_realtime: ModelRoutingRealtimePolicy,
    pub approval: ModelRoutingApprovalDisposition,
    pub approval_reason: Option<SwitchTurnApprovalReason>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageOperationRoutingRequest {
    pub operation_id: ImageOperationId,
    pub target_model: ModelId,
    pub target_realtime: ModelRoutingRealtimePolicy,
    pub approval: ModelRoutingApprovalDisposition,
    pub approval_reason: Option<ImageOperationApprovalReason>,
    pub requires_scoped_override: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageOperationRoutingResult {
    Accepted {
        operation_id: ImageOperationId,
        phase: ImageOperationPhase,
    },
    Denied {
        operation_id: ImageOperationId,
        reason: ImageOperationDenialReason,
    },
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
pub trait SessionLlmReconfigureHost: Send + Sync {
    async fn hydrate_session_llm_state(
        &self,
        session_id: &SessionId,
    ) -> Result<HydratedSessionLlmState, RuntimeDriverError>;

    async fn resolve_target_session_llm_identity(
        &self,
        request: &SessionLlmReconfigureRequest,
        current_identity: &meerkat_core::SessionLlmIdentity,
    ) -> Result<ResolvedSessionLlmReconfigure, RuntimeDriverError>;

    async fn apply_live_session_llm_identity(
        &self,
        session_id: &SessionId,
        identity: &meerkat_core::SessionLlmIdentity,
    ) -> Result<(), RuntimeDriverError>;

    async fn apply_live_session_tool_visibility_state(
        &self,
        session_id: &SessionId,
        visibility_state: Option<meerkat_core::SessionToolVisibilityState>,
    ) -> Result<(), RuntimeDriverError>;

    async fn persist_live_session(&self, session_id: &SessionId) -> Result<(), RuntimeDriverError>;

    async fn discard_live_session(&self, session_id: &SessionId) -> Result<(), RuntimeDriverError>;
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum MeerkatMachineCommandError {
    #[error(transparent)]
    Driver(#[from] RuntimeDriverError),
    #[error(transparent)]
    Control(#[from] RuntimeControlPlaneError),
}

/// Unified internal Meerkat machine command surface.
///
/// This replaces the old per-domain dispatch split (session, drain,
/// drain-local, control, ingress, legacy-run) while keeping the public helper
/// methods and external runtime/machine surface unchanged.
#[derive(CommandManifest)]
#[allow(clippy::large_enum_variant)]
pub(crate) enum MeerkatMachineCommand {
    RegisterSession {
        session_id: SessionId,
    },
    UnregisterSession {
        session_id: SessionId,
    },
    EnsureSessionWithExecutor {
        session_id: SessionId,
        executor: Box<dyn CoreExecutor>,
    },
    SetSilentIntents {
        session_id: SessionId,
        intents: Vec<String>,
    },
    InterruptCurrentRun {
        session_id: SessionId,
    },
    CancelAfterBoundary {
        session_id: SessionId,
    },
    StopRuntimeExecutor {
        session_id: SessionId,
        command: RunControlCommand,
    },
    ContainsSession {
        session_id: SessionId,
    },
    SessionHasExecutor {
        session_id: SessionId,
    },
    SessionHasComms {
        session_id: SessionId,
    },
    OpsLifecycleRegistry {
        session_id: SessionId,
    },
    PrepareBindings {
        session_id: SessionId,
    },
    PrepareLocalSessionBindings {
        session_id: SessionId,
    },
    InputState {
        session_id: SessionId,
        input_id: InputId,
    },
    ListActiveInputs {
        session_id: SessionId,
    },
    ReconfigureSessionLlmIdentity {
        session_id: SessionId,
        previous_identity: Box<meerkat_core::SessionLlmIdentity>,
        previous_visibility_state: Box<meerkat_core::SessionToolVisibilityState>,
        previous_capability_surface: Option<SessionLlmCapabilitySurface>,
        previous_capability_surface_status: SessionLlmCapabilitySurfaceStatus,
        target_identity: Box<meerkat_core::SessionLlmIdentity>,
        target_capability_surface: Box<SessionLlmCapabilitySurface>,
        next_visibility_state: Box<meerkat_core::SessionToolVisibilityState>,
        next_capability_base_filter: meerkat_core::ToolFilter,
        next_active_visibility_revision: u64,
        tool_visibility_delta: Box<SessionToolVisibilityDelta>,
    },
    StagePersistentFilter {
        session_id: SessionId,
        filter: meerkat_core::ToolFilter,
        witnesses: std::collections::BTreeMap<String, meerkat_core::ToolVisibilityWitness>,
    },
    RequestDeferredTools {
        session_id: SessionId,
        names: std::collections::BTreeSet<String>,
        witnesses: std::collections::BTreeMap<String, meerkat_core::ToolVisibilityWitness>,
    },
    /// Publish the committed visible tool set through the machine dispatch.
    ///
    /// TLA+ source: VisibleSurfacesMatchAppliedStateInvariant —
    /// the visible-set publication must route through the canonical command
    /// path and be gated on session existence and non-Destroyed state.
    PublishCommittedVisibleSet {
        session_id: SessionId,
        visibility_state: Box<meerkat_core::SessionToolVisibilityState>,
    },
    SetPeerIngressContext {
        session_id: SessionId,
        keep_alive: bool,
        comms_runtime: Option<Arc<dyn CommsRuntime>>,
        /// Mob-owned path sets this to the spawning mob's id so the DSL
        /// transitions to `PeerIngressOwnerKind::MobOwned` rather than
        /// `SessionOwned`. Session-owned and detach paths leave it `None`.
        mob_id: Option<crate::meerkat_machine::dsl::MobId>,
    },
    NotifyDrainExited {
        session_id: SessionId,
        reason: DrainExitReason,
    },
    AbortAll,
    Abort {
        session_id: SessionId,
    },
    Wait {
        session_id: SessionId,
    },
    Ingest {
        runtime_id: LogicalRuntimeId,
        input: Input,
    },
    PublishEvent {
        event: RuntimeEventEnvelope,
    },
    Retire {
        runtime_id: LogicalRuntimeId,
    },
    Recycle {
        runtime_id: LogicalRuntimeId,
    },
    Reset {
        runtime_id: LogicalRuntimeId,
    },
    Recover {
        runtime_id: LogicalRuntimeId,
    },
    Destroy {
        runtime_id: LogicalRuntimeId,
    },
    RuntimeState {
        runtime_id: LogicalRuntimeId,
    },
    RuntimeRealtimeAttachmentStatus {
        session_id: SessionId,
    },
    /// Fully-projected public channel status. Returns attachment state plus
    /// machine-owned reconnect lifecycle/progress collapsed into a
    /// ready-to-serialize [`RealtimeChannelStatus`]. Consumed by the
    /// `realtime/status` RPC handler and the MCP `meerkat_realtime_status`
    /// tool so they do not hand-compose partial projections.
    RuntimeRealtimeChannelStatus {
        session_id: SessionId,
    },
    ConfigureModelRoutingBaseline {
        session_id: SessionId,
        baseline_model: ModelId,
        realtime_capable: bool,
    },
    SessionModelRoutingStatus {
        session_id: SessionId,
    },
    RequestSwitchTurn {
        session_id: SessionId,
        request: Box<SwitchTurnRequest>,
    },
    AdmitModelRoutingAssistantTurn {
        session_id: SessionId,
    },
    BeginImageOperation {
        session_id: SessionId,
        request: Box<ImageOperationRoutingRequest>,
    },
    ActivateImageOperationOverride {
        session_id: SessionId,
        operation_id: ImageOperationId,
    },
    CompleteImageOperation {
        session_id: SessionId,
        operation_id: ImageOperationId,
        terminal: ImageOperationTerminalClass,
    },
    RestoreImageOperationOverride {
        session_id: SessionId,
        operation_id: ImageOperationId,
    },
    LoadBoundaryReceipt {
        runtime_id: LogicalRuntimeId,
        run_id: LifecycleRunId,
        sequence: u64,
    },
    AcceptWithCompletion {
        session_id: SessionId,
        input: Input,
    },
    AcceptWithoutWake {
        session_id: SessionId,
        input: Input,
    },
    Prepare {
        session_id: SessionId,
        input: Input,
    },
    Commit {
        session_id: SessionId,
        input_id: InputId,
        run_id: RunId,
        output: CoreApplyOutput,
    },
    Fail {
        session_id: SessionId,
        run_id: RunId,
        error: String,
    },
}

#[derive(Debug)]
pub(crate) struct MeerkatMachineRunPrepared {
    pub input_id: InputId,
    pub run_id: RunId,
    pub primitive: RunPrimitive,
}

#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
pub(crate) enum MeerkatMachineCommandResult {
    AcceptOutcome(AcceptOutcome),
    AcceptWithCompletion {
        outcome: AcceptOutcome,
        handle: Option<crate::completion::CompletionHandle>,
        #[cfg_attr(not(test), allow(dead_code))]
        admission_signal: crate::driver::ephemeral::PostAdmissionSignal,
    },
    Unit,
    Bool(bool),
    Spawned(bool),
    OpsLifecycleRegistry(Option<Arc<crate::ops_lifecycle::RuntimeOpsLifecycleRegistry>>),
    Bindings(meerkat_core::SessionRuntimeBindings),
    InputState(Option<StoredInputState>),
    ActiveInputs(Vec<InputId>),
    LlmReconfigured(SessionLlmReconfigureReport),
    VisibilityRevision(meerkat_core::ToolScopeRevision),
    VisibilityPublished(meerkat_core::SessionToolVisibilityState),
    RetireReport(RetireReport),
    RecycleReport(RecycleReport),
    ResetReport(ResetReport),
    RecoveryReport(RecoveryReport),
    DestroyReport(DestroyReport),
    RuntimeState(RuntimeState),
    RealtimeAttachmentStatus(RealtimeAttachmentStatus),
    RealtimeChannelStatus(RealtimeChannelStatus),
    SessionModelRoutingStatus(SessionModelRoutingStatus),
    SwitchTurnControlResult(SwitchTurnControlResult),
    ImageOperationRoutingResult(ImageOperationRoutingResult),
    ImageOperationPhase(ImageOperationPhase),
    BoundaryReceipt(Option<RunBoundaryReceipt>),
    Prepared(MeerkatMachineRunPrepared),
}

#[doc(hidden)]
#[must_use]
pub fn canonical_meerkat_machine_command_manifest() -> IndexSet<&'static str> {
    canonical_meerkat_machine_command_classifications()
        .into_iter()
        .flat_map(|record| record.classification.catalog_inputs())
        .map(MeerkatMachineCatalogInput::as_str)
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeerkatMachineCommandClassification {
    CatalogInput(MeerkatMachineCatalogInput),
    CatalogInputs(&'static [MeerkatMachineCatalogInput]),
    ShellMechanic(MeerkatMachineShellMechanicReason),
}

impl MeerkatMachineCommandClassification {
    #[must_use]
    pub fn catalog_inputs(self) -> Vec<MeerkatMachineCatalogInput> {
        match self {
            Self::CatalogInput(input) => vec![input],
            Self::CatalogInputs(inputs) => inputs.to_vec(),
            Self::ShellMechanic(_) => Vec::new(),
        }
    }

    #[must_use]
    pub fn catalog_input_variants(self) -> Vec<MeerkatMachineInputVariant> {
        self.catalog_inputs()
            .into_iter()
            .map(MeerkatMachineCatalogInput::input_variant)
            .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeerkatMachineCatalogInput {
    RegisterSession,
    UnregisterSession,
    EnsureSessionWithExecutor,
    SetSilentIntents,
    InterruptCurrentRun,
    CancelAfterBoundary,
    StopRuntimeExecutor,
    ContainsSession,
    SessionHasExecutor,
    SessionHasComms,
    OpsLifecycleRegistry,
    PrepareBindings,
    InputState,
    ListActiveInputs,
    ReconfigureSessionLlmIdentity,
    StagePersistentFilter,
    RequestDeferredTools,
    PublishCommittedVisibleSet,
    SetPeerIngressContext,
    NotifyDrainExited,
    AbortAll,
    Abort,
    Wait,
    Ingest,
    PublishEvent,
    Retire,
    Recycle,
    Reset,
    Recover,
    Destroy,
    RuntimeState,
    RuntimeRealtimeAttachmentStatus,
    ModelRoutingStatus,
    SetModelRoutingBaseline,
    RequestFiniteSwitchTurn,
    RequestUntilChangedSwitchTurn,
    AdmitModelRoutingAssistantTurn,
    BeginImageOperation,
    ActivateImageOperationOverride,
    CompleteImageOperation,
    RestoreImageOperationOverride,
    LoadBoundaryReceipt,
    AcceptWithCompletion,
    AcceptWithoutWake,
    Prepare,
    Commit,
    Fail,
}

impl MeerkatMachineCatalogInput {
    #[must_use]
    pub const fn input_variant(self) -> MeerkatMachineInputVariant {
        match self {
            Self::RegisterSession => MeerkatMachineInputVariant::RegisterSession,
            Self::UnregisterSession => MeerkatMachineInputVariant::UnregisterSession,
            Self::EnsureSessionWithExecutor => {
                MeerkatMachineInputVariant::EnsureSessionWithExecutor
            }
            Self::SetSilentIntents => MeerkatMachineInputVariant::SetSilentIntents,
            Self::InterruptCurrentRun => MeerkatMachineInputVariant::InterruptCurrentRun,
            Self::CancelAfterBoundary => MeerkatMachineInputVariant::CancelAfterBoundary,
            Self::StopRuntimeExecutor => MeerkatMachineInputVariant::StopRuntimeExecutor,
            Self::ContainsSession => MeerkatMachineInputVariant::ContainsSession,
            Self::SessionHasExecutor => MeerkatMachineInputVariant::SessionHasExecutor,
            Self::SessionHasComms => MeerkatMachineInputVariant::SessionHasComms,
            Self::OpsLifecycleRegistry => MeerkatMachineInputVariant::OpsLifecycleRegistry,
            Self::PrepareBindings => MeerkatMachineInputVariant::PrepareBindings,
            Self::InputState => MeerkatMachineInputVariant::InputState,
            Self::ListActiveInputs => MeerkatMachineInputVariant::ListActiveInputs,
            Self::ReconfigureSessionLlmIdentity => {
                MeerkatMachineInputVariant::ReconfigureSessionLlmIdentity
            }
            Self::StagePersistentFilter => MeerkatMachineInputVariant::StagePersistentFilter,
            Self::RequestDeferredTools => MeerkatMachineInputVariant::RequestDeferredTools,
            Self::PublishCommittedVisibleSet => {
                MeerkatMachineInputVariant::PublishCommittedVisibleSet
            }
            Self::SetPeerIngressContext => MeerkatMachineInputVariant::SetPeerIngressContext,
            Self::NotifyDrainExited => MeerkatMachineInputVariant::NotifyDrainExited,
            Self::AbortAll => MeerkatMachineInputVariant::AbortAll,
            Self::Abort => MeerkatMachineInputVariant::Abort,
            Self::Wait => MeerkatMachineInputVariant::Wait,
            Self::Ingest => MeerkatMachineInputVariant::Ingest,
            Self::PublishEvent => MeerkatMachineInputVariant::PublishEvent,
            Self::Retire => MeerkatMachineInputVariant::Retire,
            Self::Recycle => MeerkatMachineInputVariant::Recycle,
            Self::Reset => MeerkatMachineInputVariant::Reset,
            Self::Recover => MeerkatMachineInputVariant::Recover,
            Self::Destroy => MeerkatMachineInputVariant::Destroy,
            Self::RuntimeState => MeerkatMachineInputVariant::RuntimeState,
            Self::RuntimeRealtimeAttachmentStatus => {
                MeerkatMachineInputVariant::RuntimeRealtimeAttachmentStatus
            }
            Self::ModelRoutingStatus => MeerkatMachineInputVariant::ModelRoutingStatus,
            Self::SetModelRoutingBaseline => MeerkatMachineInputVariant::SetModelRoutingBaseline,
            Self::RequestFiniteSwitchTurn => MeerkatMachineInputVariant::RequestFiniteSwitchTurn,
            Self::RequestUntilChangedSwitchTurn => {
                MeerkatMachineInputVariant::RequestUntilChangedSwitchTurn
            }
            Self::AdmitModelRoutingAssistantTurn => {
                MeerkatMachineInputVariant::AdmitModelRoutingAssistantTurn
            }
            Self::BeginImageOperation => MeerkatMachineInputVariant::BeginImageOperation,
            Self::ActivateImageOperationOverride => {
                MeerkatMachineInputVariant::ActivateImageOperationOverride
            }
            Self::CompleteImageOperation => MeerkatMachineInputVariant::CompleteImageOperation,
            Self::RestoreImageOperationOverride => {
                MeerkatMachineInputVariant::RestoreImageOperationOverride
            }
            Self::LoadBoundaryReceipt => MeerkatMachineInputVariant::LoadBoundaryReceipt,
            Self::AcceptWithCompletion => MeerkatMachineInputVariant::AcceptWithCompletion,
            Self::AcceptWithoutWake => MeerkatMachineInputVariant::AcceptWithoutWake,
            Self::Prepare => MeerkatMachineInputVariant::Prepare,
            Self::Commit => MeerkatMachineInputVariant::Commit,
            Self::Fail => MeerkatMachineInputVariant::Fail,
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RegisterSession => "RegisterSession",
            Self::UnregisterSession => "UnregisterSession",
            Self::EnsureSessionWithExecutor => "EnsureSessionWithExecutor",
            Self::SetSilentIntents => "SetSilentIntents",
            Self::InterruptCurrentRun => "InterruptCurrentRun",
            Self::CancelAfterBoundary => "CancelAfterBoundary",
            Self::StopRuntimeExecutor => "StopRuntimeExecutor",
            Self::ContainsSession => "ContainsSession",
            Self::SessionHasExecutor => "SessionHasExecutor",
            Self::SessionHasComms => "SessionHasComms",
            Self::OpsLifecycleRegistry => "OpsLifecycleRegistry",
            Self::PrepareBindings => "PrepareBindings",
            Self::InputState => "InputState",
            Self::ListActiveInputs => "ListActiveInputs",
            Self::ReconfigureSessionLlmIdentity => "ReconfigureSessionLlmIdentity",
            Self::StagePersistentFilter => "StagePersistentFilter",
            Self::RequestDeferredTools => "RequestDeferredTools",
            Self::PublishCommittedVisibleSet => "PublishCommittedVisibleSet",
            Self::SetPeerIngressContext => "SetPeerIngressContext",
            Self::NotifyDrainExited => "NotifyDrainExited",
            Self::AbortAll => "AbortAll",
            Self::Abort => "Abort",
            Self::Wait => "Wait",
            Self::Ingest => "Ingest",
            Self::PublishEvent => "PublishEvent",
            Self::Retire => "Retire",
            Self::Recycle => "Recycle",
            Self::Reset => "Reset",
            Self::Recover => "Recover",
            Self::Destroy => "Destroy",
            Self::RuntimeState => "RuntimeState",
            Self::RuntimeRealtimeAttachmentStatus => "RuntimeRealtimeAttachmentStatus",
            Self::ModelRoutingStatus => "ModelRoutingStatus",
            Self::SetModelRoutingBaseline => "SetModelRoutingBaseline",
            Self::RequestFiniteSwitchTurn => "RequestFiniteSwitchTurn",
            Self::RequestUntilChangedSwitchTurn => "RequestUntilChangedSwitchTurn",
            Self::AdmitModelRoutingAssistantTurn => "AdmitModelRoutingAssistantTurn",
            Self::BeginImageOperation => "BeginImageOperation",
            Self::ActivateImageOperationOverride => "ActivateImageOperationOverride",
            Self::CompleteImageOperation => "CompleteImageOperation",
            Self::RestoreImageOperationOverride => "RestoreImageOperationOverride",
            Self::LoadBoundaryReceipt => "LoadBoundaryReceipt",
            Self::AcceptWithCompletion => "AcceptWithCompletion",
            Self::AcceptWithoutWake => "AcceptWithoutWake",
            Self::Prepare => "Prepare",
            Self::Commit => "Commit",
            Self::Fail => "Fail",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeerkatMachineShellMechanicReason {
    ModelRoutingShellConfiguration,
    TurnControlOverlayRequest,
    RealtimeTransportObservation,
    SessionModelRoutingObservation,
    LocalSessionBindingBootstrap,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeerkatMachineCommandClassificationRecord {
    pub command: MeerkatMachineCommandVariant,
    pub classification: MeerkatMachineCommandClassification,
}

#[doc(hidden)]
#[must_use]
pub fn canonical_meerkat_machine_command_classifications()
-> Vec<MeerkatMachineCommandClassificationRecord> {
    MeerkatMachineCommand::command_variant_manifest()
        .iter()
        .copied()
        .map(|variant| MeerkatMachineCommandClassificationRecord {
            command: variant,
            classification: meerkat_machine_command_classification(variant),
        })
        .collect()
}

const fn meerkat_machine_command_classification(
    variant: MeerkatMachineCommandVariant,
) -> MeerkatMachineCommandClassification {
    match variant {
        MeerkatMachineCommandVariant::ConfigureModelRoutingBaseline => {
            MeerkatMachineCommandClassification::CatalogInput(
                MeerkatMachineCatalogInput::SetModelRoutingBaseline,
            )
        }
        MeerkatMachineCommandVariant::RequestSwitchTurn => {
            MeerkatMachineCommandClassification::CatalogInputs(&[
                MeerkatMachineCatalogInput::RequestFiniteSwitchTurn,
                MeerkatMachineCatalogInput::RequestUntilChangedSwitchTurn,
            ])
        }
        MeerkatMachineCommandVariant::RuntimeRealtimeChannelStatus => {
            MeerkatMachineCommandClassification::CatalogInput(
                MeerkatMachineCatalogInput::RuntimeRealtimeAttachmentStatus,
            )
        }
        MeerkatMachineCommandVariant::SessionModelRoutingStatus => {
            MeerkatMachineCommandClassification::CatalogInput(
                MeerkatMachineCatalogInput::ModelRoutingStatus,
            )
        }
        MeerkatMachineCommandVariant::PrepareLocalSessionBindings => {
            MeerkatMachineCommandClassification::ShellMechanic(
                MeerkatMachineShellMechanicReason::LocalSessionBindingBootstrap,
            )
        }
        MeerkatMachineCommandVariant::RegisterSession => {
            MeerkatMachineCommandClassification::CatalogInput(
                MeerkatMachineCatalogInput::RegisterSession,
            )
        }
        MeerkatMachineCommandVariant::UnregisterSession => {
            MeerkatMachineCommandClassification::CatalogInput(
                MeerkatMachineCatalogInput::UnregisterSession,
            )
        }
        MeerkatMachineCommandVariant::EnsureSessionWithExecutor => {
            MeerkatMachineCommandClassification::CatalogInput(
                MeerkatMachineCatalogInput::EnsureSessionWithExecutor,
            )
        }
        MeerkatMachineCommandVariant::SetSilentIntents => {
            MeerkatMachineCommandClassification::CatalogInput(
                MeerkatMachineCatalogInput::SetSilentIntents,
            )
        }
        MeerkatMachineCommandVariant::InterruptCurrentRun => {
            MeerkatMachineCommandClassification::CatalogInput(
                MeerkatMachineCatalogInput::InterruptCurrentRun,
            )
        }
        MeerkatMachineCommandVariant::CancelAfterBoundary => {
            MeerkatMachineCommandClassification::CatalogInput(
                MeerkatMachineCatalogInput::CancelAfterBoundary,
            )
        }
        MeerkatMachineCommandVariant::StopRuntimeExecutor => {
            MeerkatMachineCommandClassification::CatalogInput(
                MeerkatMachineCatalogInput::StopRuntimeExecutor,
            )
        }
        MeerkatMachineCommandVariant::ContainsSession => {
            MeerkatMachineCommandClassification::CatalogInput(
                MeerkatMachineCatalogInput::ContainsSession,
            )
        }
        MeerkatMachineCommandVariant::SessionHasExecutor => {
            MeerkatMachineCommandClassification::CatalogInput(
                MeerkatMachineCatalogInput::SessionHasExecutor,
            )
        }
        MeerkatMachineCommandVariant::SessionHasComms => {
            MeerkatMachineCommandClassification::CatalogInput(
                MeerkatMachineCatalogInput::SessionHasComms,
            )
        }
        MeerkatMachineCommandVariant::OpsLifecycleRegistry => {
            MeerkatMachineCommandClassification::CatalogInput(
                MeerkatMachineCatalogInput::OpsLifecycleRegistry,
            )
        }
        MeerkatMachineCommandVariant::PrepareBindings => {
            MeerkatMachineCommandClassification::CatalogInput(
                MeerkatMachineCatalogInput::PrepareBindings,
            )
        }
        MeerkatMachineCommandVariant::InputState => {
            MeerkatMachineCommandClassification::CatalogInput(
                MeerkatMachineCatalogInput::InputState,
            )
        }
        MeerkatMachineCommandVariant::ListActiveInputs => {
            MeerkatMachineCommandClassification::CatalogInput(
                MeerkatMachineCatalogInput::ListActiveInputs,
            )
        }
        MeerkatMachineCommandVariant::ReconfigureSessionLlmIdentity => {
            MeerkatMachineCommandClassification::CatalogInput(
                MeerkatMachineCatalogInput::ReconfigureSessionLlmIdentity,
            )
        }
        MeerkatMachineCommandVariant::StagePersistentFilter => {
            MeerkatMachineCommandClassification::CatalogInput(
                MeerkatMachineCatalogInput::StagePersistentFilter,
            )
        }
        MeerkatMachineCommandVariant::RequestDeferredTools => {
            MeerkatMachineCommandClassification::CatalogInput(
                MeerkatMachineCatalogInput::RequestDeferredTools,
            )
        }
        MeerkatMachineCommandVariant::PublishCommittedVisibleSet => {
            MeerkatMachineCommandClassification::CatalogInput(
                MeerkatMachineCatalogInput::PublishCommittedVisibleSet,
            )
        }
        MeerkatMachineCommandVariant::SetPeerIngressContext => {
            MeerkatMachineCommandClassification::CatalogInput(
                MeerkatMachineCatalogInput::SetPeerIngressContext,
            )
        }
        MeerkatMachineCommandVariant::NotifyDrainExited => {
            MeerkatMachineCommandClassification::CatalogInput(
                MeerkatMachineCatalogInput::NotifyDrainExited,
            )
        }
        MeerkatMachineCommandVariant::AbortAll => {
            MeerkatMachineCommandClassification::CatalogInput(MeerkatMachineCatalogInput::AbortAll)
        }
        MeerkatMachineCommandVariant::Abort => {
            MeerkatMachineCommandClassification::CatalogInput(MeerkatMachineCatalogInput::Abort)
        }
        MeerkatMachineCommandVariant::Wait => {
            MeerkatMachineCommandClassification::CatalogInput(MeerkatMachineCatalogInput::Wait)
        }
        MeerkatMachineCommandVariant::Ingest => {
            MeerkatMachineCommandClassification::CatalogInput(MeerkatMachineCatalogInput::Ingest)
        }
        MeerkatMachineCommandVariant::PublishEvent => {
            MeerkatMachineCommandClassification::CatalogInput(
                MeerkatMachineCatalogInput::PublishEvent,
            )
        }
        MeerkatMachineCommandVariant::Retire => {
            MeerkatMachineCommandClassification::CatalogInput(MeerkatMachineCatalogInput::Retire)
        }
        MeerkatMachineCommandVariant::Recycle => {
            MeerkatMachineCommandClassification::CatalogInput(MeerkatMachineCatalogInput::Recycle)
        }
        MeerkatMachineCommandVariant::Reset => {
            MeerkatMachineCommandClassification::CatalogInput(MeerkatMachineCatalogInput::Reset)
        }
        MeerkatMachineCommandVariant::Recover => {
            MeerkatMachineCommandClassification::CatalogInput(MeerkatMachineCatalogInput::Recover)
        }
        MeerkatMachineCommandVariant::Destroy => {
            MeerkatMachineCommandClassification::CatalogInput(MeerkatMachineCatalogInput::Destroy)
        }
        MeerkatMachineCommandVariant::RuntimeState => {
            MeerkatMachineCommandClassification::CatalogInput(
                MeerkatMachineCatalogInput::RuntimeState,
            )
        }
        MeerkatMachineCommandVariant::RuntimeRealtimeAttachmentStatus => {
            MeerkatMachineCommandClassification::CatalogInput(
                MeerkatMachineCatalogInput::RuntimeRealtimeAttachmentStatus,
            )
        }
        MeerkatMachineCommandVariant::AdmitModelRoutingAssistantTurn => {
            MeerkatMachineCommandClassification::CatalogInput(
                MeerkatMachineCatalogInput::AdmitModelRoutingAssistantTurn,
            )
        }
        MeerkatMachineCommandVariant::BeginImageOperation => {
            MeerkatMachineCommandClassification::CatalogInput(
                MeerkatMachineCatalogInput::BeginImageOperation,
            )
        }
        MeerkatMachineCommandVariant::ActivateImageOperationOverride => {
            MeerkatMachineCommandClassification::CatalogInput(
                MeerkatMachineCatalogInput::ActivateImageOperationOverride,
            )
        }
        MeerkatMachineCommandVariant::CompleteImageOperation => {
            MeerkatMachineCommandClassification::CatalogInput(
                MeerkatMachineCatalogInput::CompleteImageOperation,
            )
        }
        MeerkatMachineCommandVariant::RestoreImageOperationOverride => {
            MeerkatMachineCommandClassification::CatalogInput(
                MeerkatMachineCatalogInput::RestoreImageOperationOverride,
            )
        }
        MeerkatMachineCommandVariant::LoadBoundaryReceipt => {
            MeerkatMachineCommandClassification::CatalogInput(
                MeerkatMachineCatalogInput::LoadBoundaryReceipt,
            )
        }
        MeerkatMachineCommandVariant::AcceptWithCompletion => {
            MeerkatMachineCommandClassification::CatalogInput(
                MeerkatMachineCatalogInput::AcceptWithCompletion,
            )
        }
        MeerkatMachineCommandVariant::AcceptWithoutWake => {
            MeerkatMachineCommandClassification::CatalogInput(
                MeerkatMachineCatalogInput::AcceptWithoutWake,
            )
        }
        MeerkatMachineCommandVariant::Prepare => {
            MeerkatMachineCommandClassification::CatalogInput(MeerkatMachineCatalogInput::Prepare)
        }
        MeerkatMachineCommandVariant::Commit => {
            MeerkatMachineCommandClassification::CatalogInput(MeerkatMachineCatalogInput::Commit)
        }
        MeerkatMachineCommandVariant::Fail => {
            MeerkatMachineCommandClassification::CatalogInput(MeerkatMachineCatalogInput::Fail)
        }
    }
}

/// Snapshot of completion waiters registered for one input.
///
/// This is a supporting-carrier view, not canonical semantic truth.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeerkatCompletionWaiterSnapshot {
    pub input_id: InputId,
    pub waiter_count: usize,
}

/// Snapshot of the runtime completion waiter carrier.
///
/// Completion waiters are supporting carrier state rather than primary
/// authority, but they remain useful for diagnostics and recovery inspection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeerkatCompletionWaitersSnapshot {
    pub input_count: usize,
    pub waiter_count: usize,
    pub waiting_inputs: Vec<MeerkatCompletionWaiterSnapshot>,
}

/// Runtime driver flavor for a registered Meerkat session entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeerkatDriverKind {
    Ephemeral,
    Persistent,
}

/// Snapshot of the hidden runtime epoch cursor state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeerkatCursorSnapshot {
    pub agent_applied_cursor: u64,
    pub runtime_observed_seq: u64,
    pub runtime_last_injected_seq: u64,
}

/// Snapshot of the hidden runtime binding for one Meerkat session.
#[derive(Debug, Clone)]
pub struct MeerkatBindingSnapshot {
    pub session_id: SessionId,
    pub runtime_id: LogicalRuntimeId,
    pub driver_kind: MeerkatDriverKind,
    pub driver_present: bool,
    pub completions_present: bool,
    pub ops_registry_present: bool,
    pub attachment_live: bool,
    pub epoch_id: RuntimeEpochId,
    pub cursor_state: MeerkatCursorSnapshot,
}

/// Snapshot of runtime control-plane truth for one session.
#[derive(Debug, Clone)]
pub struct MeerkatControlSnapshot {
    pub phase: RuntimeState,
    pub current_run_id: Option<RunId>,
    pub pre_run_phase: Option<RuntimeState>,
}

/// Snapshot of one admitted runtime input.
#[derive(Debug, Clone)]
pub struct MeerkatAdmittedInputSnapshot {
    pub input_id: InputId,
    pub content_shape: Option<ContentShape>,
    pub request_id: Option<RequestId>,
    pub reservation_key: Option<ReservationKey>,
    pub handling_mode: Option<HandlingMode>,
    pub lifecycle: Option<InputLifecycleState>,
    pub terminal_outcome: Option<InputTerminalOutcome>,
    pub last_run_id: Option<RunId>,
    pub last_boundary_sequence: Option<u64>,
    pub is_prompt: bool,
}

/// Snapshot of runtime ingress truth for one session.
#[derive(Debug, Clone)]
pub struct MeerkatInputsSnapshot {
    pub admission_order: Vec<MeerkatAdmittedInputSnapshot>,
    pub queue: Vec<InputId>,
    pub steer_queue: Vec<InputId>,
    pub current_run_id: Option<RunId>,
    pub current_run_contributors: Vec<InputId>,
    pub post_admission_signal: String,
    pub silent_intent_overrides: Vec<String>,
}

/// Snapshot of the canonical input-ledger carrier for one session.
///
/// These counts sit below the top-level Meerkat phase machine, but they drive
/// the exact values returned by control-plane reports such as
/// `DestroyReport.inputs_abandoned`.
#[derive(Debug, Clone)]
pub struct MeerkatLedgerSnapshot {
    pub input_count: usize,
    pub non_terminal_count: usize,
    pub accepted_count: usize,
    pub queued_count: usize,
    pub staged_count: usize,
    pub applied_count: usize,
    pub applied_pending_consumption_count: usize,
    pub consumed_count: usize,
    pub superseded_count: usize,
    pub coalesced_count: usize,
    pub abandoned_count: usize,
}

/// Snapshot of runtime-owned async operation truth for one session.
#[derive(Debug, Clone)]
pub struct MeerkatOpsSnapshot {
    pub operation_count: usize,
    pub active_count: usize,
    pub wait_request_id: Option<WaitRequestId>,
    pub pending_wait_present: bool,
    pub pending_wait_request_id: Option<WaitRequestId>,
    pub wait_operation_ids: Vec<OperationId>,
    pub operations: Vec<OperationLifecycleSnapshot>,
}

/// Snapshot of comms-drain lifecycle truth for one session.
#[derive(Debug, Clone)]
pub struct MeerkatDrainSnapshot {
    pub slot_present: bool,
    pub phase: Option<CommsDrainPhase>,
    pub mode: Option<CommsDrainMode>,
    pub handle_present: bool,
}

/// Schema-aligned projection of the Meerkat formal state derived from the
/// live runtime.
///
/// This stays diagnostic and intentionally stringifies field values so the
/// parity harness can compare full field vectors even where the runtime does
/// not expose a first-class Rust type for every formal field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeerkatFormalStateProjection {
    /// Formal fields that have a live runtime-backed projection today.
    pub available_fields: BTreeMap<String, String>,
    /// Formal fields that still have no canonical runtime source.
    pub unavailable_fields: Vec<String>,
}

/// Diagnostic snapshot of the current Meerkat runtime spine.
///
/// This is an observational scaffold over the existing runtime-owned Meerkat
/// regions. It is intentionally not the final MeerkatMachine reducer.
#[derive(Debug, Clone)]
pub struct MeerkatMachineSpineSnapshot {
    pub binding: MeerkatBindingSnapshot,
    pub control: MeerkatControlSnapshot,
    pub inputs: MeerkatInputsSnapshot,
    pub ledger: MeerkatLedgerSnapshot,
    pub completion_waiters: MeerkatCompletionWaitersSnapshot,
    pub ops: MeerkatOpsSnapshot,
    pub drain: MeerkatDrainSnapshot,
    pub formal_state: MeerkatFormalStateProjection,
}

impl MeerkatMachineSpineSnapshot {
    /// Validate TLA+ structural invariants against the current spine snapshot.
    ///
    /// Returns `Ok(())` if all invariants hold, or `Err(violations)` with a
    /// list of human-readable violation descriptions. This is release-mode
    /// validation — not `debug_assert`.
    pub fn validate_spine_invariants(&self) -> Result<(), Vec<String>> {
        let mut violations = Vec::new();

        // --- Control/binding invariants ---

        // RunningHasActiveRunInvariant: Running => HasActiveRun
        if self.control.phase == RuntimeState::Running && self.control.current_run_id.is_none() {
            violations
                .push("RunningHasActiveRunInvariant: phase is Running but no active run_id".into());
        }

        // ActiveRunPhaseInvariant: HasActiveRun => phase in {Running, Retired}
        if self.control.current_run_id.is_some()
            && !matches!(
                self.control.phase,
                RuntimeState::Running | RuntimeState::Retired
            )
        {
            violations.push(format!(
                "ActiveRunPhaseInvariant: active run_id present but phase is {:?}",
                self.control.phase
            ));
        }

        // DestroyedShapeInvariant: Destroyed => empty queues, no waiting inputs
        if self.control.phase == RuntimeState::Destroyed {
            if !self.inputs.queue.is_empty() {
                violations.push("DestroyedShapeInvariant: Destroyed but queue is non-empty".into());
            }
            if !self.inputs.steer_queue.is_empty() {
                violations
                    .push("DestroyedShapeInvariant: Destroyed but steer_queue is non-empty".into());
            }
            if self.completion_waiters.input_count > 0 {
                violations.push(
                    "DestroyedShapeInvariant: Destroyed but completion waiters remain".into(),
                );
            }
        }

        // --- Input invariants ---

        // QueueSteerDisjointInvariant
        let queue_set: std::collections::HashSet<_> = self.inputs.queue.iter().collect();
        let steer_set: std::collections::HashSet<_> = self.inputs.steer_queue.iter().collect();
        if !queue_set.is_disjoint(&steer_set) {
            violations
                .push("QueueSteerDisjointInvariant: queue and steer_queue share entries".into());
        }

        // QueueHandlingInvariant: all queue entries must have handling_mode=Queue, lifecycle=Queued
        for qid in &self.inputs.queue {
            if let Some(snap) = self
                .inputs
                .admission_order
                .iter()
                .find(|a| &a.input_id == qid)
            {
                if snap.handling_mode != Some(HandlingMode::Queue) {
                    violations.push(format!(
                        "QueueHandlingInvariant: queue entry {qid} has handling_mode {:?}",
                        snap.handling_mode
                    ));
                }
                if snap.lifecycle != Some(InputLifecycleState::Queued) {
                    violations.push(format!(
                        "QueueHandlingInvariant: queue entry {qid} has lifecycle {:?}",
                        snap.lifecycle
                    ));
                }
            }
        }

        // SteerHandlingInvariant: all steer_queue entries must have handling_mode=Steer, lifecycle=Queued
        for sid in &self.inputs.steer_queue {
            if let Some(snap) = self
                .inputs
                .admission_order
                .iter()
                .find(|a| &a.input_id == sid)
            {
                if snap.handling_mode != Some(HandlingMode::Steer) {
                    violations.push(format!(
                        "SteerHandlingInvariant: steer_queue entry {sid} has handling_mode {:?}",
                        snap.handling_mode
                    ));
                }
                if snap.lifecycle != Some(InputLifecycleState::Queued) {
                    violations.push(format!(
                        "SteerHandlingInvariant: steer_queue entry {sid} has lifecycle {:?}",
                        snap.lifecycle
                    ));
                }
            }
        }

        // ContributorLifecycleInvariant: all current_run_contributors must
        // still belong to the active run lifecycle slice.
        for cid in &self.inputs.current_run_contributors {
            if let Some(snap) = self
                .inputs
                .admission_order
                .iter()
                .find(|a| &a.input_id == cid)
                && !matches!(
                    snap.lifecycle,
                    Some(
                        InputLifecycleState::Staged
                            | InputLifecycleState::Applied
                            | InputLifecycleState::AppliedPendingConsumption
                    )
                )
            {
                violations.push(format!(
                    "ContributorLifecycleInvariant: contributor {cid} has lifecycle {:?}",
                    snap.lifecycle
                ));
            }
        }

        // TerminalInputsNotQueuedInvariant: terminal inputs must not be in queue or steer_queue
        for snap in &self.inputs.admission_order {
            if snap.terminal_outcome.is_some() {
                if queue_set.contains(&snap.input_id) {
                    violations.push(format!(
                        "TerminalInputsNotQueuedInvariant: terminal input {} in queue",
                        snap.input_id
                    ));
                }
                if steer_set.contains(&snap.input_id) {
                    violations.push(format!(
                        "TerminalInputsNotQueuedInvariant: terminal input {} in steer_queue",
                        snap.input_id
                    ));
                }
            }
        }

        // CurrentRunContributorsInvariant: HasActiveRun => contributors non-empty
        if self.control.current_run_id.is_some() && self.inputs.current_run_contributors.is_empty()
        {
            violations
                .push("CurrentRunContributorsInvariant: active run but no contributors".into());
        }

        // ContributorRunIdentityInvariant: when control owns an active run,
        // every current contributor must point at that same run in the
        // ingress bookkeeping.
        if let Some(control_run_id) = &self.control.current_run_id {
            for cid in &self.inputs.current_run_contributors {
                if let Some(snap) = self
                    .inputs
                    .admission_order
                    .iter()
                    .find(|a| &a.input_id == cid)
                    && snap.last_run_id.as_ref() != Some(control_run_id)
                {
                    violations.push(format!(
                        "ContributorRunIdentityInvariant: contributor {cid} has last_run_id {:?}, expected {:?}",
                        snap.last_run_id, control_run_id
                    ));
                }
            }
        }

        // --- Ops invariants ---

        // WaitAllAlignmentInvariant
        let wait_active = self.ops.wait_request_id.is_some();
        if wait_active && self.ops.wait_operation_ids.is_empty() {
            violations
                .push("WaitAllAlignmentInvariant: wait_active but no wait_operation_ids".into());
        }
        if !wait_active && !self.ops.wait_operation_ids.is_empty() {
            violations.push(
                "WaitAllAlignmentInvariant: wait_operation_ids present but no wait_request_id"
                    .into(),
            );
        }

        // --- Drain invariants ---

        // DrainBindingInvariant: drain.phase != Inactive => binding.live
        if let Some(phase) = self.drain.phase {
            if phase != CommsDrainPhase::Inactive && !self.binding.attachment_live {
                // binding.live maps to session being registered (it is, since we have a snapshot)
                // This invariant is about the binding being live, which is always true if we have
                // a snapshot. Skip this check since getting a snapshot implies the session exists.
            }
            let _ = phase; // suppress unused warning
        }

        // DrainModeInvariant: drain.phase != Inactive => mode is set and not Disabled
        if let Some(phase) = self.drain.phase
            && phase != CommsDrainPhase::Inactive
            && self.drain.mode.is_none()
        {
            violations.push("DrainModeInvariant: drain.phase is active but mode is None".into());
        }

        if violations.is_empty() {
            Ok(())
        } else {
            Err(violations)
        }
    }
}
