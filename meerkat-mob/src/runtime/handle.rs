use super::*;
use crate::MobRuntimeMode;
use crate::mob_machine::{MobMachineCommand, MobMachineCommandResult};
use crate::roster::MobMemberKickoffSnapshot;
#[cfg(test)]
use crate::runtime::MobLifecycleSnapshot;
use crate::runtime::mob_member_lifecycle_authority::{
    CanonicalMemberSnapshotMaterial, CanonicalMemberStatus, CanonicalSessionObservation,
    MobMemberLifecycleAuthority, MobMemberLifecycleInput,
};
use crate::runtime::reconcile::{
    EnsureMemberOutcome, MemberFilter, ReconcileFailure, ReconcileOptions, ReconcileReport,
    ReconcileStage,
};
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use meerkat_core::comms::{
    PeerDirectoryEntry, PeerReachability, PeerReachabilityReason, TrustedPeerSpec,
};
use meerkat_core::ops::OperationId;
use meerkat_core::ops_lifecycle::OpsLifecycleRegistry;
use meerkat_core::service::{MobToolAuthorityContext, SessionError};
use meerkat_core::time_compat::Instant;
use meerkat_core::types::{HandlingMode, RenderMetadata, SessionId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::time::Duration;

const DEFAULT_KICKOFF_WAIT_TIMEOUT: Duration = Duration::from_secs(600);
const DEFAULT_READY_WAIT_TIMEOUT: Duration = Duration::from_secs(600);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PeerConnectivityProjection {
    Omit,
    Include,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionObservationProjection {
    Omit,
    Full,
}

/// Point-in-time snapshot of a mob member's execution state.
/// Serializable projection of the member's current realtime attachment
/// state, mapped one-to-one from `meerkat_runtime::RealtimeAttachmentStatus`.
///
/// Semantic ownership remains with MeerkatMachine (capability-driven
/// transport). This enum is a transport shape so `mob/member_status`
/// consumers can observe the attachment lifecycle for a member without
/// reaching into runtime internals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum MobRealtimeAttachmentStatus {
    Unattached,
    IntentPresentUnbound,
    BindingNotReady,
    BindingReady,
    ReplacementPending,
    ReattachRequired,
}

#[cfg(feature = "runtime-adapter")]
fn map_runtime_realtime_attachment_status(
    status: meerkat_runtime::RealtimeAttachmentStatus,
) -> MobRealtimeAttachmentStatus {
    use meerkat_runtime::RealtimeAttachmentStatus as Rt;
    match status {
        Rt::Unattached => MobRealtimeAttachmentStatus::Unattached,
        Rt::IntentPresentUnbound => MobRealtimeAttachmentStatus::IntentPresentUnbound,
        Rt::BindingNotReady => MobRealtimeAttachmentStatus::BindingNotReady,
        Rt::BindingReady => MobRealtimeAttachmentStatus::BindingReady,
        Rt::ReplacementPending => MobRealtimeAttachmentStatus::ReplacementPending,
        Rt::ReattachRequired => MobRealtimeAttachmentStatus::ReattachRequired,
    }
}

#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct MobMemberSnapshot {
    /// Current lifecycle status.
    pub status: MobMemberStatus,
    /// Identity-native runtime ID for this incarnation.
    pub agent_runtime_id: AgentRuntimeId,
    /// Fence token for the current incarnation.
    pub fence_token: FenceToken,
    /// Preview of the current bridge session's last committed assistant text.
    pub output_preview: Option<String>,
    /// Error description (if the member errored).
    pub error: Option<String>,
    /// Cumulative token usage.
    pub tokens_used: u64,
    /// Whether the member has reached a terminal state.
    pub is_final: bool,
    /// Current realtime attachment state of the member's bridge session,
    /// projected from MeerkatMachine's capability-driven transport.
    /// `None` when no bridge session exists yet or the runtime adapter is
    /// unavailable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realtime_attachment_status: Option<MobRealtimeAttachmentStatus>,
    /// Identity-first session id for the member's current bridge session.
    ///
    /// Exposed over the wire so realtime routing can do the canonical
    /// `mob/member_status → current_session_id → realtime/open_info`
    /// navigation. Phase 5G/T5i removed the `mob_member_target`
    /// shortcut; `RealtimeChannelTarget` only accepts `session_target`
    /// now, and this field is the only path from mob membership to a
    /// realtime session handle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_session_id: Option<SessionId>,
    /// Bridge-internal session binding — not part of the public identity contract.
    #[serde(skip)]
    pub(crate) current_bridge_session_id: Option<SessionId>,
    /// Live comms connectivity for currently wired peers, when available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peer_connectivity: Option<MobPeerConnectivitySnapshot>,
    /// Initial autonomous-turn kickoff state, when this member has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kickoff: Option<MobMemberKickoffSnapshot>,
}

impl MobMemberSnapshot {
    pub(crate) fn with_current_bridge_session_id(
        mut self,
        current_bridge_session_id: Option<SessionId>,
    ) -> Self {
        self.current_session_id = current_bridge_session_id.clone();
        self.current_bridge_session_id = current_bridge_session_id;
        self
    }

    pub(crate) fn current_bridge_session_id(&self) -> Option<&SessionId> {
        self.current_bridge_session_id.as_ref()
    }

    /// Convenience accessor for the canonical member identity. Equivalent to
    /// `&self.agent_runtime_id.identity` but saves every consumer from
    /// reaching through the runtime-id wrapper.
    #[must_use]
    pub fn agent_identity(&self) -> &AgentIdentity {
        &self.agent_runtime_id.identity
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct MobMemberListEntry {
    /// Canonical member identity.
    pub agent_identity: AgentIdentity,
    /// Identity-native runtime ID for this incarnation.
    pub agent_runtime_id: AgentRuntimeId,
    /// Fence token for the current incarnation.
    pub fence_token: FenceToken,
    /// Member role (profile name).
    pub role: ProfileName,
    pub runtime_mode: MobRuntimeMode,
    #[serde(default)]
    pub state: crate::roster::MemberState,
    pub wired_to: BTreeSet<AgentIdentity>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
    pub status: MobMemberStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub is_final: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kickoff: Option<MobMemberKickoffSnapshot>,
    // --- Bridge internals (pub(crate)) ---
    // `list_members` stays the lightweight roster view: no session_id
    // in the wire shape (see `tests.rs::test_identity_first_list_members_returns_identity_native_entries`
    // which regression-asserts that). Callers that need to bridge a
    // member to a realtime session use `mob/member_status`, which
    // surfaces `current_session_id` explicitly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) peer_id: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) external_peer_specs: BTreeMap<AgentIdentity, TrustedPeerSpec>,
    #[serde(skip)]
    pub(crate) current_session_id: Option<SessionId>,
    #[serde(skip)]
    pub(crate) current_bridge_session_id: Option<SessionId>,
}

impl MobMemberListEntry {
    pub(crate) fn with_current_bridge_session_id(
        mut self,
        current_bridge_session_id: Option<SessionId>,
    ) -> Self {
        self.current_session_id = current_bridge_session_id.clone();
        self.current_bridge_session_id = current_bridge_session_id;
        self
    }
}

/// Live connectivity summary for a member's currently wired peers.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct MobPeerConnectivitySnapshot {
    pub reachable_peer_count: usize,
    pub unknown_peer_count: usize,
    pub unreachable_peers: Vec<MobUnreachablePeer>,
}

/// One currently wired peer that is known to be unreachable.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct MobUnreachablePeer {
    pub peer: String,
    pub reason: Option<PeerReachabilityReason>,
}

/// Execution status for a mob member.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum MobMemberStatus {
    /// Member is active and potentially running.
    Active,
    /// Member is in the process of retiring.
    Retiring,
    /// Member failed to restore durable session state and needs repair.
    Broken,
    /// Member has completed (session archived or not found).
    Completed,
    /// Member is not in the roster.
    Unknown,
}

/// Receipt returned by a successful member respawn.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct MemberRespawnReceipt {
    /// The member identity that was respawned.
    pub identity: AgentIdentity,
    /// Runtime id for the current incarnation after respawn completes.
    pub agent_runtime_id: AgentRuntimeId,
    /// Fence token for the superseded incarnation.
    pub previous_fence_token: FenceToken,
    /// Fence token for the current incarnation.
    pub fence_token: FenceToken,
}

impl MemberRespawnReceipt {
    pub fn new(
        identity: AgentIdentity,
        agent_runtime_id: AgentRuntimeId,
        previous_fence_token: FenceToken,
        fence_token: FenceToken,
    ) -> Self {
        Self {
            identity,
            agent_runtime_id,
            previous_fence_token,
            fence_token,
        }
    }
}

/// Report returned after rotating a mob-owned supervisor authority.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct SupervisorRotationReport {
    /// Supervisor epoch before rotation.
    pub previous_epoch: u64,
    /// Supervisor epoch after rotation.
    pub current_epoch: u64,
    /// Public peer id for the new supervisor keypair.
    pub public_peer_id: String,
}

/// Structured report returned from mob destroy.
#[derive(Debug, Clone, Default, Serialize)]
#[non_exhaustive]
pub struct MobDestroyReport {
    /// Members that required force-destroy semantics during cleanup.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub force_destroyed_members: Vec<AgentIdentity>,
    /// Remote members whose cleanup could not be completed before destroy ended.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub orphaned_remote_members: Vec<AgentIdentity>,
    /// Whether aggregate remote cleanup exceeded its deadline.
    #[serde(default)]
    pub remote_cleanup_deadline_exceeded: bool,
    /// Whether runtime metadata was scrubbed.
    #[serde(default)]
    pub metadata_scrubbed: bool,
    /// Whether persisted mob events were cleared.
    #[serde(default)]
    pub events_cleared: bool,
    /// Whether namespace cleanup completed.
    #[serde(default)]
    pub namespace_cleaned: bool,
    /// Human-readable cleanup errors captured while destroying.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
}

impl MobDestroyReport {
    pub(crate) fn push_error(&mut self, error: impl Into<String>) {
        self.errors.push(error.into());
    }

    fn error_summary(&self) -> String {
        if self.errors.is_empty() {
            "destroy cleanup did not complete".to_string()
        } else {
            self.errors.join("; ")
        }
    }
}

/// Structured error returned by mob destroy.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum MobDestroyError {
    /// Destroy performed partial cleanup but could not finish the full contract.
    #[error("destroy incomplete: {}", report.error_summary())]
    Incomplete { report: MobDestroyReport },

    /// A preflight or actor-level mob error occurred before partial reporting.
    #[error(transparent)]
    Mob(#[from] MobError),
}

/// Structured evidence captured when respawn cannot prove the old member is gone.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct PreviousMemberCleanupReport {
    /// Stable member identity.
    pub identity: AgentIdentity,
    /// Runtime id of the incarnation being replaced.
    pub agent_runtime_id: AgentRuntimeId,
    /// Fence token of the incarnation being replaced.
    pub fence_token: FenceToken,
    /// Whether graceful retire was attempted.
    pub retire_attempted: bool,
    /// Error returned from the graceful retire attempt, when any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retire_error: Option<String>,
    /// Whether a confirmatory observation probe was attempted.
    #[serde(default)]
    pub confirmatory_observation_attempted: bool,
    /// Observation probe detail, when any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirmatory_observation: Option<String>,
    /// Whether force-destroy was attempted.
    #[serde(default)]
    pub destroy_attempted: bool,
    /// Error returned from the force-destroy attempt, when any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destroy_error: Option<String>,
}

/// Receipt returned by a successful member spawn.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub(crate) struct MemberSpawnReceipt {
    /// The member identity that was provisioned and committed into the roster.
    pub(crate) member_ref: MemberRef,
    /// Canonical mob child operation for the spawned member lifecycle.
    pub(crate) operation_id: OperationId,
}

/// Public result from a successful member spawn.
///
/// Carries identity-native fields only — no session IDs or internal refs.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct SpawnResult {
    /// Stable member identity.
    pub agent_identity: AgentIdentity,
    /// Identity-native runtime ID for this incarnation.
    pub agent_runtime_id: AgentRuntimeId,
    /// Fence token for stale-command rejection.
    pub fence_token: FenceToken,
}

impl SpawnResult {
    /// Create a new spawn result from identity-native fields.
    pub fn new(
        agent_identity: AgentIdentity,
        agent_runtime_id: AgentRuntimeId,
        fence_token: FenceToken,
    ) -> Self {
        Self {
            agent_identity,
            agent_runtime_id,
            fence_token,
        }
    }
}

#[derive(Clone)]
pub(crate) struct CanonicalOpsOwnerContext {
    pub(crate) owner_bridge_session_id: SessionId,
    pub(crate) ops_registry: Arc<dyn OpsLifecycleRegistry>,
}

/// Structured error for direct-Rust respawn failures.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum MobRespawnError {
    /// Member has no runtime control channel for replacement.
    #[error("no runtime control channel for member {identity}")]
    NoRuntimeControl { identity: AgentIdentity },

    /// Spawn failed after the old member was retired.
    #[error("spawn failed after retire for member {identity}: {reason}")]
    SpawnAfterRetire {
        identity: AgentIdentity,
        reason: String,
    },

    /// Topology restore failed after replacement spawn.
    /// The replacement receipt is carried so callers can still use the new session.
    #[error("topology restore failed for member {}: {} peer(s) failed", receipt.identity, failed_peer_ids.len())]
    TopologyRestoreFailed {
        receipt: MemberRespawnReceipt,
        failed_peer_ids: Vec<AgentIdentity>,
    },

    /// Retire cleanup progressed far enough that the old member may still exist,
    /// but respawn could not prove it was fully cleaned up.
    #[error("previous member cleanup ambiguous for member {}", report.identity)]
    PreviousMemberCleanupAmbiguous { report: PreviousMemberCleanupReport },

    /// An underlying mob error occurred before mutation.
    #[error(transparent)]
    Mob(#[from] MobError),
}

/// Receipt returned by member message delivery.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct MemberDeliveryReceipt {
    /// The member identity.
    pub identity: AgentIdentity,
    /// Runtime id for the incarnation that accepted the work.
    pub agent_runtime_id: AgentRuntimeId,
    /// Fence token for the incarnation that accepted the work.
    pub fence_token: FenceToken,
    /// How the message was handled.
    pub handling_mode: HandlingMode,
}

/// Receipt confirming that a unit of work was accepted by the work lane.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct WorkDeliveryReceipt {
    /// The work reference for the submitted unit.
    pub work_ref: WorkRef,
    /// The runtime ID of the target member.
    pub runtime_id: AgentRuntimeId,
}

/// Options for helper convenience spawns.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct HelperOptions {
    /// Role name (profile key) to use. If None, requires a default profile in the definition.
    pub role_name: Option<ProfileName>,
    /// Runtime mode override.
    pub runtime_mode: Option<crate::MobRuntimeMode>,
    /// Backend override.
    pub backend: Option<MobBackendKind>,
    /// Tool access policy for the helper.
    pub tool_access_policy: Option<meerkat_core::ops::ToolAccessPolicy>,
}

/// Result from a helper spawn-and-wait operation.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct HelperResult {
    /// The member's final output text.
    pub output: Option<String>,
    /// Total tokens used by the helper.
    pub tokens_used: u64,
    /// Stable member identity for the helper run.
    pub agent_identity: AgentIdentity,
    /// Runtime id for the helper incarnation.
    pub agent_runtime_id: AgentRuntimeId,
    /// Fence token for the helper incarnation.
    pub fence_token: FenceToken,
}

/// Target for a wire operation from a local mob member.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PeerTarget {
    /// Another member in the same mob roster.
    Local(AgentIdentity),
    /// A trusted peer that lives outside the local mob roster.
    External(TrustedPeerSpec),
}

// DELETE_ME A5 DSL-schema migration: `MeerkatId` is now a type alias
// for `AgentIdentity`, so the two `From<...> for PeerTarget` impls
// that used to exist collapse into one.
impl From<AgentIdentity> for PeerTarget {
    fn from(value: AgentIdentity) -> Self {
        Self::Local(value)
    }
}

// ---------------------------------------------------------------------------
// MobHandle
// ---------------------------------------------------------------------------

/// Clone-cheap, thread-safe handle for interacting with a running mob.
///
/// All mutation commands are sent through an mpsc channel to the actor.
/// Public orchestration, event, and task surfaces are routed through the
/// top-level machine command seam. A few immutable/shared projections still
/// read canonical shared state directly inside that seam's implementation.
/// The persisted event ledger is also retained here for terminal read-only
/// fallback after `Destroy`, when the actor has exited by contract.
#[derive(Clone)]
pub struct MobHandle {
    pub(super) command_tx: mpsc::Sender<MobCommand>,
    pub(super) roster: Arc<RwLock<RosterAuthority>>,
    pub(super) definition: Arc<MobDefinition>,
    pub(super) events: Arc<dyn MobEventStore>,
    pub(super) flow_streams:
        Arc<tokio::sync::Mutex<BTreeMap<RunId, mpsc::Sender<meerkat_core::ScopedAgentEvent>>>>,
    pub(super) session_service: Arc<dyn MobSessionService>,
    #[cfg(feature = "runtime-adapter")]
    #[allow(dead_code)]
    pub(super) runtime_adapter: Option<Arc<meerkat_runtime::MeerkatMachine>>,
    pub(super) restore_diagnostics: Arc<RwLock<HashMap<MeerkatId, RestoreFailureDiagnostic>>>,
    /// Read-only receiver for the actor's terminal-phase projection. The
    /// actor (sole writer) publishes the current DSL phase after every
    /// phase-changing transition and once more before exiting. Used by
    /// `status()` as the fallback when the command channel has closed
    /// (actor has exited). Dogma-#13 projection: source truth is the DSL
    /// authority inside the actor; this seam is rebuildable (replay) and
    /// read-only on the handle side.
    pub(super) phase_watch_rx: tokio::sync::watch::Receiver<MobState>,
    /// Optional realtime session factory injected via
    /// [`super::MobBuilder::with_realtime_session_factory`] (W2-E / issue
    /// #264). Test harnesses retrieve it via
    /// [`MobHandle::realtime_session_factory`] so a `RealtimeWsHost`
    /// bound to the same runtime can be configured against a
    /// deterministic in-process mock. `None` when no factory was
    /// provided (production mob paths typically wire the factory at the
    /// surface layer directly).
    pub(super) realtime_session_factory: Option<Arc<dyn meerkat_client::RealtimeSessionFactory>>,
    /// W3-H: broadcast sender the actor uses to publish
    /// `MemberRealtimeBindingEvent`s; handle holds a clone so callers can
    /// `subscribe()` for new receivers. Dogma-#13 projection: DSL binding
    /// map is the source of truth, events are rebuilable from transition
    /// replay.
    pub(super) realtime_binding_tx:
        tokio::sync::broadcast::Sender<super::state::MemberRealtimeBindingEvent>,
}

impl MobHandle {
    /// Accessor for the realtime session factory carried from
    /// [`super::MobBuilder::with_realtime_session_factory`] (W2-E).
    pub fn realtime_session_factory(
        &self,
    ) -> Option<Arc<dyn meerkat_client::RealtimeSessionFactory>> {
        self.realtime_session_factory.as_ref().map(Arc::clone)
    }

    /// W3-H: subscribe to this mob's `MemberRealtimeBindingEvent`s — the
    /// stream of Set / Rotated / Released effects the MobMachine emits for
    /// identity→session rebindings. Consumed by the realtime WS surface in
    /// meerkat-rpc to atomically rotate a `MobMember` channel's target
    /// session when the MobMachine rotates the binding (respawn flow) and
    /// to close with a typed terminal frame when the binding is released.
    pub fn subscribe_realtime_binding_events(
        &self,
    ) -> tokio::sync::broadcast::Receiver<super::state::MemberRealtimeBindingEvent> {
        self.realtime_binding_tx.subscribe()
    }

    /// W3-H: read the current bridge session id bound to `agent_identity`
    /// in this mob, projected from the MobMachine's canonical
    /// `member_realtime_bindings` map. Returns `None` if the identity has
    /// no binding. Used by the realtime WS surface at `MobMember` channel
    /// open time to initialize the task-local current_session_id before
    /// subscribing to binding events.
    pub async fn current_realtime_binding(
        &self,
        agent_identity: crate::ids::AgentIdentity,
    ) -> Result<Option<meerkat_core::types::SessionId>, crate::MobError> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.command_tx
            .send(super::state::MobCommand::CurrentRealtimeBinding {
                agent_identity,
                reply_tx,
            })
            .await
            .map_err(|_| {
                crate::MobError::Internal(
                    "mob actor exited before responding to CurrentRealtimeBinding".to_string(),
                )
            })?;
        reply_rx.await.map_err(|_| {
            crate::MobError::Internal("mob actor dropped CurrentRealtimeBinding reply".to_string())
        })
    }
}

#[derive(Debug, Clone)]
pub(crate) struct RestoreFailureDiagnostic {
    pub(crate) bridge_session_id: Option<SessionId>,
    pub(crate) reason: String,
}

/// Clone-cheap, capability-bearing handle for interacting with one mob member.
///
/// This is the target 0.5 API surface for message/turn submission. The mob
/// handle remains orchestration/control-plane oriented, while member-directed
/// delivery goes through this narrower capability.
#[derive(Clone)]
pub struct MemberHandle {
    mob: MobHandle,
    agent_identity: MeerkatId,
}

#[derive(Clone)]
pub struct MobEventsView {
    handle: MobHandle,
}

/// Spawn request for first-class batch member provisioning.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct SpawnMemberSpec {
    /// The role name (profile key) for this member in the mob roster.
    ///
    /// When `tooling` is present it controls model/tool resolution;
    /// `role_name` remains a roster/topology label.
    pub role_name: ProfileName,
    pub identity: AgentIdentity,
    pub initial_message: Option<ContentInput>,
    pub runtime_mode: Option<crate::MobRuntimeMode>,
    pub backend: Option<MobBackendKind>,
    /// Runtime binding for this member. When set, takes precedence over
    /// `backend` and carries concrete binding details (e.g., external process
    /// comms identity). First step toward identity-first mobs.
    pub binding: Option<crate::RuntimeBinding>,
    /// Opaque application context passed through to the agent build pipeline.
    pub context: Option<serde_json::Value>,
    /// Application-defined labels for this member.
    pub labels: Option<std::collections::BTreeMap<String, String>>,
    /// How this member should be launched (fresh, resume, or fork).
    ///
    /// Public spawn-policy seam (DELETE_ME A3 + C1): external consumers
    /// use [`Self::with_launch_mode`] /
    /// [`Self::with_resume_bridge_session_id`] to configure session
    /// adoption. See [`crate::launch::MemberLaunchMode`] for the
    /// variants and [`crate::launch::ForkContext`] for fork
    /// configuration.
    pub launch_mode: crate::launch::MemberLaunchMode,
    /// Tool access policy for this member.
    pub tool_access_policy: Option<meerkat_core::ops::ToolAccessPolicy>,
    /// How to split budget from the orchestrator to this member.
    pub budget_split_policy: Option<crate::launch::BudgetSplitPolicy>,
    /// When true, automatically wire this member to its spawner.
    pub auto_wire_parent: bool,
    /// Additional instruction sections appended to the system prompt for this member.
    pub additional_instructions: Option<Vec<String>>,
    /// Per-agent environment variables injected into shell tool subprocesses.
    pub shell_env: Option<std::collections::HashMap<String, String>>,
    /// Pre-resolved inherited tool filter from spawn tooling resolution.
    ///
    /// When set, stored as `INHERITED_TOOL_FILTER_METADATA_KEY` on the child
    /// session metadata so `AgentBuilder::build()` recovers it as a base filter.
    pub inherited_tool_filter: Option<meerkat_core::tool_scope::ToolFilter>,
    /// Override profile resolved from `SpawnTooling::Profile` source.
    ///
    /// When set, the spawn path uses this profile instead of looking up by
    /// `role_name` from the mob definition. This allows agent-owned spawn
    /// tooling to specify a different model/skills/tools via inline or
    /// realm-scoped profiles.
    pub override_profile: Option<crate::profile::Profile>,
    /// Per-member auth binding (deferral §1). When set, this member's
    /// agent builds with `AgentBuildConfig.connection_ref = Some(this)`
    /// — scoping credential resolution to the named realm + binding.
    /// `None` means the member uses env-default / config-realm fallback
    /// (mob members do NOT currently auto-inherit the spawner's
    /// binding; that's a separate plumbing concern per dogma §19 —
    /// we don't add a tri-state until Inherit has real semantics).
    pub connection_ref: Option<meerkat_core::ConnectionRef>,
}

impl SpawnMemberSpec {
    pub fn new(profile: impl Into<ProfileName>, identity: impl Into<AgentIdentity>) -> Self {
        Self {
            role_name: profile.into(),
            identity: identity.into(),
            initial_message: None,
            runtime_mode: None,
            backend: None,
            binding: None,
            context: None,
            labels: None,
            launch_mode: crate::launch::MemberLaunchMode::Fresh,
            tool_access_policy: None,
            budget_split_policy: None,
            auto_wire_parent: false,
            additional_instructions: None,
            shell_env: None,
            inherited_tool_filter: None,
            override_profile: None,
            connection_ref: None,
        }
    }

    /// Set the per-member auth binding (deferral §1).
    pub fn with_connection_ref(mut self, conn_ref: meerkat_core::ConnectionRef) -> Self {
        self.connection_ref = Some(conn_ref);
        self
    }

    pub fn with_shell_env(mut self, env: std::collections::HashMap<String, String>) -> Self {
        self.shell_env = Some(env);
        self
    }

    pub fn with_initial_message(mut self, message: impl Into<ContentInput>) -> Self {
        self.initial_message = Some(message.into());
        self
    }

    pub fn with_runtime_mode(mut self, mode: crate::MobRuntimeMode) -> Self {
        self.runtime_mode = Some(mode);
        self
    }

    pub fn with_backend(mut self, backend: MobBackendKind) -> Self {
        self.backend = Some(backend);
        self
    }

    pub fn with_context(mut self, context: serde_json::Value) -> Self {
        self.context = Some(context);
        self
    }

    pub fn with_labels(mut self, labels: std::collections::BTreeMap<String, String>) -> Self {
        self.labels = Some(labels);
        self
    }

    /// Set launch mode to resume an existing bridge session.
    ///
    /// DELETE_ME A3 + C1: public session-adoption seam. Callers holding
    /// a bridge session id (for example from a prior
    /// [`crate::runtime::MobHandle::resolve_bridge_session_id`] lookup
    /// or from durable mob-event replay) use this builder method to
    /// spawn a member whose backing session continues that binding
    /// instead of starting fresh.
    pub fn with_resume_bridge_session_id(mut self, id: meerkat_core::types::SessionId) -> Self {
        self.launch_mode = crate::launch::MemberLaunchMode::Resume {
            bridge_session_id: id,
        };
        self
    }

    /// Set an explicit [`crate::launch::MemberLaunchMode`].
    ///
    /// DELETE_ME A3 + C1: public session-adoption seam for callers that
    /// construct their own `MemberLaunchMode` value (e.g. fork-from-
    /// sibling with a caller-chosen [`crate::launch::ForkContext`]).
    /// For the common "resume a specific bridge session" case prefer
    /// [`Self::with_resume_bridge_session_id`].
    pub fn with_launch_mode(mut self, mode: crate::launch::MemberLaunchMode) -> Self {
        self.launch_mode = mode;
        self
    }

    pub fn with_tool_access_policy(mut self, policy: meerkat_core::ops::ToolAccessPolicy) -> Self {
        self.tool_access_policy = Some(policy);
        self
    }

    pub fn with_budget_split_policy(mut self, policy: crate::launch::BudgetSplitPolicy) -> Self {
        self.budget_split_policy = Some(policy);
        self
    }

    pub fn with_auto_wire_parent(mut self, auto_wire: bool) -> Self {
        self.auto_wire_parent = auto_wire;
        self
    }

    pub fn with_additional_instructions(mut self, instructions: Vec<String>) -> Self {
        self.additional_instructions = Some(instructions);
        self
    }

    pub fn from_wire(
        profile: String,
        agent_identity: String,
        initial_message: Option<ContentInput>,
        runtime_mode: Option<crate::MobRuntimeMode>,
        backend: Option<MobBackendKind>,
    ) -> Self {
        let mut spec = Self::new(profile, agent_identity);
        spec.initial_message = initial_message;
        spec.runtime_mode = runtime_mode;
        spec.backend = backend;
        spec
    }
}

impl MobEventsView {
    pub async fn poll(
        &self,
        after_cursor: u64,
        limit: usize,
    ) -> Result<Vec<crate::event::MobEvent>, MobError> {
        match self
            .handle
            .execute_machine_command(MobMachineCommand::PollEvents {
                after_cursor,
                limit,
            })
            .await?
        {
            MobMachineCommandResult::MobEvents(events) => Ok(events),
            _ => Err(MobError::Internal(
                "unexpected command result variant".into(),
            )),
        }
    }

    pub async fn replay_all(&self) -> Result<Vec<crate::event::MobEvent>, MobError> {
        match self
            .handle
            .execute_machine_command(MobMachineCommand::ReplayAllEvents)
            .await?
        {
            MobMachineCommandResult::MobEvents(events) => Ok(events),
            _ => Err(MobError::Internal(
                "unexpected command result variant".into(),
            )),
        }
    }
}

impl MobHandle {
    async fn restore_failure_for(
        &self,
        agent_identity: &MeerkatId,
    ) -> Option<RestoreFailureDiagnostic> {
        self.restore_diagnostics
            .read()
            .await
            .get(agent_identity)
            .cloned()
    }

    fn restore_failure_error(
        agent_identity: &MeerkatId,
        diag: RestoreFailureDiagnostic,
    ) -> MobError {
        MobError::MemberRestoreFailed {
            member_id: agent_identity.clone(),
            session_id: diag.bridge_session_id,
            reason: diag.reason,
        }
    }

    async fn send_actor_command<R>(
        &self,
        build: impl FnOnce(oneshot::Sender<R>) -> MobCommand,
    ) -> Result<R, MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(build(reply_tx))
            .await
            .map_err(|_| MobError::Internal("actor task dropped".into()))?;
        reply_rx
            .await
            .map_err(|_| MobError::Internal("actor reply dropped".into()))
    }

    async fn execute_machine_command(
        &self,
        command: MobMachineCommand,
    ) -> Result<MobMachineCommandResult, MobError> {
        match command {
            MobMachineCommand::RunFlow {
                flow_id,
                activation_params,
                scoped_event_tx,
            } => {
                let run_id = self
                    .send_actor_command(|reply_tx| MobCommand::RunFlow {
                        flow_id,
                        activation_params,
                        scoped_event_tx,
                        reply_tx,
                    })
                    .await??;
                Ok(MobMachineCommandResult::RunId(run_id))
            }
            MobMachineCommand::CancelFlow { run_id } => {
                self.send_actor_command(|reply_tx| MobCommand::CancelFlow { run_id, reply_tx })
                    .await??;
                Ok(MobMachineCommandResult::Unit)
            }
            MobMachineCommand::FlowStatus { run_id } => {
                let status = self
                    .send_actor_command(|reply_tx| MobCommand::FlowStatus { run_id, reply_tx })
                    .await??;
                Ok(MobMachineCommandResult::FlowStatus(status))
            }
            MobMachineCommand::Spawn {
                spec,
                owner_context,
            } => {
                let (owner_bridge_session_id, ops_registry) = match owner_context {
                    Some(ctx) => (Some(ctx.owner_bridge_session_id), Some(ctx.ops_registry)),
                    None => (None, None),
                };
                let receipt = self
                    .send_actor_command(|reply_tx| MobCommand::Spawn {
                        spec,
                        owner_bridge_session_id,
                        ops_registry,
                        reply_tx,
                    })
                    .await??;
                Ok(MobMachineCommandResult::SpawnReceipt(receipt))
            }
            MobMachineCommand::EnsureMember { spec } => {
                let outcome = self.handle_ensure_member(*spec).await?;
                Ok(MobMachineCommandResult::EnsureMember(outcome))
            }
            MobMachineCommand::Reconcile { desired, options } => {
                let report = self.handle_reconcile(desired, options).await;
                Ok(MobMachineCommandResult::Reconcile(Box::new(report)))
            }
            MobMachineCommand::ListMembersMatching { filter } => {
                let members = self.handle_list_members_matching(*filter).await;
                Ok(MobMachineCommandResult::ListMembers(members))
            }
            MobMachineCommand::Retire { agent_identity } => {
                self.send_actor_command(|reply_tx| MobCommand::Retire {
                    agent_identity,
                    reply_tx,
                })
                .await??;
                Ok(MobMachineCommandResult::Unit)
            }
            MobMachineCommand::Respawn {
                agent_identity,
                initial_message,
            } => {
                let receipt = self
                    .send_actor_command(|reply_tx| MobCommand::Respawn {
                        agent_identity,
                        initial_message,
                        reply_tx,
                    })
                    .await?;
                Ok(MobMachineCommandResult::Respawn(receipt))
            }
            MobMachineCommand::RetireAll => {
                self.send_actor_command(|reply_tx| MobCommand::RetireAll { reply_tx })
                    .await??;
                Ok(MobMachineCommandResult::Unit)
            }
            MobMachineCommand::Wire { local, target } => {
                self.send_actor_command(|reply_tx| MobCommand::Wire {
                    local,
                    target,
                    reply_tx,
                })
                .await??;
                Ok(MobMachineCommandResult::Unit)
            }
            MobMachineCommand::Unwire { local, target } => {
                self.send_actor_command(|reply_tx| MobCommand::Unwire {
                    local,
                    target,
                    reply_tx,
                })
                .await??;
                Ok(MobMachineCommandResult::Unit)
            }
            MobMachineCommand::SubmitWork(cmd) => {
                // Shell dispatch is a thin forward: the mob actor owns
                // work-origin legality via the MobMachine DSL. There is no
                // origin re-decision here — `spec.origin` is forwarded
                // verbatim and the DSL accepts or rejects.
                let crate::mob_machine::SubmitWorkCommand {
                    runtime_id,
                    fence_token,
                    work_ref,
                    spec,
                    handling_mode,
                    render_metadata,
                } = *cmd;
                let receipt_work_ref = work_ref.clone();
                let payload = Box::new(super::state::SubmitWorkPayload {
                    runtime_id,
                    fence_token,
                    work_ref,
                    content: spec.content,
                    origin: spec.origin,
                    handling_mode,
                    render_metadata,
                });
                self.send_actor_command(|reply_tx| MobCommand::SubmitWork { payload, reply_tx })
                    .await??;
                Ok(MobMachineCommandResult::WorkReceipt {
                    work_ref: receipt_work_ref,
                })
            }
            MobMachineCommand::CancelWork { work_ref } => {
                // Work tracking ledger is introduced in C7. Until then,
                // individual work cancellation is not supported.
                Err(MobError::WorkNotFound(work_ref))
            }
            MobMachineCommand::CancelAllWork {
                runtime_id,
                fence_token,
            } => {
                // Identity derivation is a projection, not a decision: the
                // MobMachine DSL CancelAllWork guards own live-runtime
                // membership legality; the fence check is a shell-level
                // concurrency freshness invariant. The actor's unified
                // `handle_cancel_all_work` forwards both to the DSL and
                // then dispatches the interrupt when the machine accepts.
                self.send_actor_command(|reply_tx| MobCommand::CancelAllWork {
                    runtime_id,
                    fence_token,
                    reply_tx,
                })
                .await??;
                Ok(MobMachineCommandResult::Unit)
            }
            MobMachineCommand::Stop => {
                self.send_actor_command(|reply_tx| MobCommand::Stop { reply_tx })
                    .await??;
                Ok(MobMachineCommandResult::Unit)
            }
            MobMachineCommand::Resume => {
                self.send_actor_command(|reply_tx| MobCommand::ResumeLifecycle { reply_tx })
                    .await??;
                Ok(MobMachineCommandResult::Unit)
            }
            MobMachineCommand::Complete => {
                self.send_actor_command(|reply_tx| MobCommand::Complete { reply_tx })
                    .await??;
                Ok(MobMachineCommandResult::Unit)
            }
            MobMachineCommand::Reset => {
                self.send_actor_command(|reply_tx| MobCommand::Reset { reply_tx })
                    .await??;
                Ok(MobMachineCommandResult::Unit)
            }
            MobMachineCommand::Destroy => {
                let reply = self
                    .send_actor_command(|reply_tx| MobCommand::Destroy { reply_tx })
                    .await?;
                match reply {
                    Ok(report) => Ok(MobMachineCommandResult::DestroyReport(report)),
                    Err(MobDestroyError::Mob(error)) => Err(error),
                    Err(MobDestroyError::Incomplete { report }) => Err(MobError::Internal(
                        format!("destroy incomplete: {}", report.error_summary()),
                    )),
                }
            }
            MobMachineCommand::TaskCreate {
                subject,
                description,
                blocked_by,
            } => {
                let task_id = self
                    .send_actor_command(|reply_tx| MobCommand::TaskCreate {
                        subject,
                        description,
                        blocked_by,
                        reply_tx,
                    })
                    .await??;
                Ok(MobMachineCommandResult::TaskId(task_id))
            }
            MobMachineCommand::TaskUpdate {
                task_id,
                status,
                owner,
            } => {
                self.send_actor_command(|reply_tx| MobCommand::TaskUpdate {
                    task_id,
                    status,
                    owner,
                    reply_tx,
                })
                .await??;
                Ok(MobMachineCommandResult::Unit)
            }
            MobMachineCommand::TaskList => {
                let tasks = self
                    .send_actor_command(|reply_tx| MobCommand::TaskList { reply_tx })
                    .await?;
                Ok(MobMachineCommandResult::TaskList(tasks))
            }
            MobMachineCommand::TaskGet { task_id } => {
                let task = self
                    .send_actor_command(|reply_tx| MobCommand::TaskGet { task_id, reply_tx })
                    .await?;
                Ok(MobMachineCommandResult::TaskGet(task))
            }
            MobMachineCommand::McpServerStates => {
                let states = self
                    .send_actor_command(|reply_tx| MobCommand::McpServerStates { reply_tx })
                    .await?;
                Ok(MobMachineCommandResult::McpServerStates(states))
            }
            MobMachineCommand::RosterSnapshot => {
                let roster = self.roster.read().await.snapshot();
                Ok(MobMachineCommandResult::RosterSnapshot(roster))
            }
            MobMachineCommand::ListMembers => {
                let entries: Vec<_> = {
                    let roster = self.roster.read().await;
                    roster.list().cloned().collect()
                };
                let members = self.project_member_list(entries.iter()).await;
                Ok(MobMachineCommandResult::ListMembers(members))
            }
            MobMachineCommand::ListMembersIncludingRetiring => {
                let entries: Vec<_> = {
                    let roster = self.roster.read().await;
                    roster.list_all().cloned().collect()
                };
                let members = self.project_member_list(entries.iter()).await;
                Ok(MobMachineCommandResult::ListMembersIncludingRetiring(
                    members,
                ))
            }
            MobMachineCommand::ListAllMembers => {
                let members = self.roster.read().await.list_all().cloned().collect();
                Ok(MobMachineCommandResult::ListAllMembers(members))
            }
            MobMachineCommand::MemberStatus { agent_identity } => {
                let snapshot = self
                    .canonical_member_snapshot_material(&agent_identity)
                    .await;
                Ok(MobMachineCommandResult::MemberStatus(
                    snapshot.to_snapshot(),
                ))
            }
            MobMachineCommand::SubscribeAgentEvents { agent_identity } => {
                let stream = self
                    .send_actor_command(|reply_tx| MobCommand::SubscribeAgentEvents {
                        agent_identity,
                        reply_tx,
                    })
                    .await??;
                Ok(MobMachineCommandResult::EventStream(stream))
            }
            MobMachineCommand::SubscribeAllAgentEvents => {
                let streams = self
                    .send_actor_command(|reply_tx| MobCommand::SubscribeAllAgentEvents { reply_tx })
                    .await??;
                Ok(MobMachineCommandResult::AllAgentEventStreams(streams))
            }
            MobMachineCommand::SubscribeMobEvents { config } => {
                Ok(MobMachineCommandResult::MobEventRouter(
                    super::event_router::spawn_event_router(self.clone(), config),
                ))
            }
            MobMachineCommand::PollEvents {
                after_cursor,
                limit,
            } => {
                let events = if self.status().await? == MobState::Destroyed {
                    self.events
                        .poll(after_cursor, limit)
                        .await
                        .map_err(MobError::from)?
                } else {
                    self.send_actor_command(|reply_tx| MobCommand::PollEvents {
                        after_cursor,
                        limit,
                        reply_tx,
                    })
                    .await??
                };
                Ok(MobMachineCommandResult::MobEvents(events))
            }
            MobMachineCommand::ReplayAllEvents => {
                let events = if self.status().await? == MobState::Destroyed {
                    self.events.replay_all().await.map_err(MobError::from)?
                } else {
                    self.send_actor_command(|reply_tx| MobCommand::ReplayAllEvents { reply_tx })
                        .await??
                };
                Ok(MobMachineCommandResult::MobEvents(events))
            }
            MobMachineCommand::RecordOperatorActionProvenance {
                tool_name,
                authority_context,
            } => {
                self.send_actor_command(|reply_tx| MobCommand::RecordOperatorActionProvenance {
                    tool_name,
                    authority_context,
                    reply_tx,
                })
                .await??;
                Ok(MobMachineCommandResult::Unit)
            }
            MobMachineCommand::GetMember { agent_identity } => {
                let member = self.roster.read().await.entry(&agent_identity);
                Ok(MobMachineCommandResult::GetMember(member))
            }
            #[cfg(test)]
            MobMachineCommand::FlowTrackerCounts => {
                let counts = self
                    .send_actor_command(|reply_tx| MobCommand::FlowTrackerCounts { reply_tx })
                    .await?;
                Ok(MobMachineCommandResult::FlowTrackerCounts(counts))
            }
            #[cfg(test)]
            MobMachineCommand::OrchestratorSnapshot => {
                let snapshot = self
                    .send_actor_command(|reply_tx| MobCommand::OrchestratorSnapshot { reply_tx })
                    .await?;
                Ok(MobMachineCommandResult::OrchestratorSnapshot(snapshot))
            }
            #[cfg(test)]
            MobMachineCommand::LifecycleSnapshot => {
                let snapshot = self
                    .send_actor_command(|reply_tx| MobCommand::LifecycleSnapshot { reply_tx })
                    .await?;
                Ok(MobMachineCommandResult::LifecycleSnapshot(snapshot))
            }
            #[cfg(test)]
            MobMachineCommand::DslT2Snapshot => {
                let snapshot = self
                    .send_actor_command(|reply_tx| MobCommand::DslT2Snapshot { reply_tx })
                    .await?;
                Ok(MobMachineCommandResult::DslT2Snapshot(snapshot))
            }
            MobMachineCommand::SetSpawnPolicy { policy } => {
                self.send_actor_command(|reply_tx| MobCommand::SetSpawnPolicy { policy, reply_tx })
                    .await?;
                Ok(MobMachineCommandResult::Unit)
            }
            MobMachineCommand::Shutdown => {
                self.send_actor_command(|reply_tx| MobCommand::Shutdown { reply_tx })
                    .await??;
                Ok(MobMachineCommandResult::Unit)
            }
            MobMachineCommand::ForceCancel { agent_identity } => {
                self.send_actor_command(|reply_tx| MobCommand::ForceCancel {
                    agent_identity,
                    reply_tx,
                })
                .await??;
                Ok(MobMachineCommandResult::Unit)
            }
        }
    }

    async fn execute_destroy_machine_command(
        &self,
        command: MobMachineCommand,
    ) -> Result<MobMachineCommandResult, MobDestroyError> {
        match command {
            MobMachineCommand::Destroy => {
                let reply = self
                    .send_actor_command(|reply_tx| MobCommand::Destroy { reply_tx })
                    .await
                    .map_err(MobDestroyError::from)?;
                match reply {
                    Ok(report) => Ok(MobMachineCommandResult::DestroyReport(report)),
                    Err(error) => Err(error),
                }
            }
            _ => Err(MobDestroyError::from(MobError::Internal(
                "unsupported destroy machine command".into(),
            ))),
        }
    }

    /// Poll mob events from the underlying store.
    pub async fn poll_events(
        &self,
        after_cursor: u64,
        limit: usize,
    ) -> Result<Vec<crate::event::MobEvent>, MobError> {
        match self
            .execute_machine_command(MobMachineCommand::PollEvents {
                after_cursor,
                limit,
            })
            .await?
        {
            MobMachineCommandResult::MobEvents(events) => Ok(events),
            _ => Err(MobError::Internal(
                "unexpected command result variant".into(),
            )),
        }
    }

    /// Current mob lifecycle state, read directly from the DSL authority
    /// via the actor command channel. There is no atomic shadow — the DSL
    /// authority is the single source of truth (dogma #1, #13, #17).
    ///
    /// After `Destroy` has terminated the actor, the command channel is
    /// closed and this returns `Ok(MobState::Destroyed)`; callers that need
    /// post-destroy event replay should go through the `MobEventsView`.
    pub async fn status(&self) -> Result<MobState, MobError> {
        match self
            .send_actor_command(|reply_tx| MobCommand::QueryPhase { reply_tx })
            .await
        {
            Ok(state) => Ok(state),
            // If the actor task has exited (after Shutdown / Destroy) the
            // command channel send or receive fails. Fall back to the
            // terminal-phase watch, which the actor updates after every
            // DSL phase transition so its last observed value is the
            // authoritative terminal phase.
            Err(MobError::Internal(_)) => Ok(*self.phase_watch_rx.borrow()),
            Err(other) => Err(other),
        }
    }

    /// Access the mob definition.
    pub fn definition(&self) -> &MobDefinition {
        &self.definition
    }

    /// Mob ID.
    pub fn mob_id(&self) -> &MobId {
        &self.definition.id
    }

    /// Snapshot of the current roster.
    pub async fn roster(&self) -> Roster {
        match self
            .execute_machine_command(MobMachineCommand::RosterSnapshot)
            .await
        {
            Ok(MobMachineCommandResult::RosterSnapshot(roster)) => roster,
            Ok(_) => {
                tracing::error!("unexpected command result variant");
                Default::default()
            }
            Err(_) => Roster::new(),
        }
    }

    fn derived_comms_name(&self, entry: &RosterEntry) -> String {
        format!(
            "{}/{}/{}",
            self.definition.id, entry.role, entry.agent_identity
        )
    }

    async fn resolve_peer_connectivity(
        &self,
        entry: &RosterEntry,
        bridge_session_id: &SessionId,
        roster_snapshot: &Roster,
    ) -> Option<MobPeerConnectivitySnapshot> {
        let comms = self
            .session_service
            .comms_runtime(bridge_session_id)
            .await?;
        let peers = comms.peers().await;
        let peers_by_id: HashMap<&str, &PeerDirectoryEntry> = peers
            .iter()
            .map(|peer| (peer.peer_id.as_str(), peer))
            .collect();
        let peers_by_name: HashMap<&str, &PeerDirectoryEntry> = peers
            .iter()
            .map(|peer| (peer.name.as_str(), peer))
            .collect();

        let mut reachable_peer_count = 0usize;
        let mut unknown_peer_count = 0usize;
        let mut unreachable_peers = Vec::new();

        for wired_peer in &entry.wired_to {
            let wired_peer_meerkat = MeerkatId::from(wired_peer);
            let matched = if let Some(spec) = entry.external_peer_specs.get(&wired_peer_meerkat) {
                peers_by_id
                    .get(spec.peer_id.as_str())
                    .copied()
                    .or_else(|| peers_by_name.get(spec.name.as_str()).copied())
            } else {
                let local_entry = roster_snapshot.get(&wired_peer_meerkat);
                let live_peer_id = match local_entry
                    .and_then(|peer_entry| peer_entry.member_ref.bridge_session_id())
                {
                    Some(target_session_id) => self
                        .session_service
                        .comms_runtime(target_session_id)
                        .await
                        .and_then(|runtime| runtime.public_key()),
                    None => None,
                };
                live_peer_id
                    .as_deref()
                    .and_then(|peer_id| peers_by_id.get(peer_id).copied())
                    .or_else(|| {
                        local_entry
                            .and_then(|peer_entry| peer_entry.peer_id.as_deref())
                            .and_then(|peer_id| peers_by_id.get(peer_id).copied())
                    })
                    .or_else(|| {
                        local_entry
                            .map(|peer_entry| self.derived_comms_name(peer_entry))
                            .and_then(|name| peers_by_name.get(name.as_str()).copied())
                    })
            };

            match matched {
                Some(peer) => match peer.reachability {
                    PeerReachability::Reachable => reachable_peer_count += 1,
                    PeerReachability::Unknown => unknown_peer_count += 1,
                    PeerReachability::Unreachable => unreachable_peers.push(MobUnreachablePeer {
                        peer: peer.name.as_string(),
                        reason: peer.last_unreachable_reason,
                    }),
                },
                None => unknown_peer_count += 1,
            }
        }

        Some(MobPeerConnectivitySnapshot {
            reachable_peer_count,
            unknown_peer_count,
            unreachable_peers,
        })
    }

    /// List members as an operational projection surface.
    ///
    /// This includes structural roster fields plus current runtime status,
    /// error/finality state, and the current session binding when known.
    /// It intentionally skips live peer-connectivity fanout so ordinary
    /// membership polling cannot stall on comms reachability lookups.
    /// For low-level structural roster visibility without runtime projection,
    /// use [`list_all_members`](Self::list_all_members).
    pub async fn list_members(&self) -> Vec<MobMemberListEntry> {
        match self
            .execute_machine_command(MobMachineCommand::ListMembers)
            .await
        {
            Ok(MobMachineCommandResult::ListMembers(entries)) => entries,
            Ok(_) => {
                tracing::error!("unexpected command result variant");
                Default::default()
            }
            Err(_) => Vec::new(),
        }
    }

    /// List all members including those in `Retiring` state, with canonical
    /// lifecycle/session projection.
    ///
    /// Like [`list_members`](Self::list_members), this intentionally avoids
    /// live peer-connectivity fanout. Use [`member_status`](Self::member_status)
    /// for deep per-member inspection including live comms reachability.
    pub async fn list_members_including_retiring(&self) -> Vec<MobMemberListEntry> {
        match self
            .execute_machine_command(MobMachineCommand::ListMembersIncludingRetiring)
            .await
        {
            Ok(MobMachineCommandResult::ListMembersIncludingRetiring(entries)) => entries,
            Ok(_) => {
                tracing::error!("unexpected command result variant");
                Default::default()
            }
            Err(_) => Vec::new(),
        }
    }

    async fn project_member_list<'a>(
        &self,
        entries: impl Iterator<Item = &'a crate::roster::RosterEntry>,
    ) -> Vec<MobMemberListEntry> {
        let entries: Vec<_> = entries.cloned().collect();
        let mut projected = Vec::with_capacity(entries.len());
        for entry in entries {
            let snapshot = self
                .canonical_member_list_material(&entry.agent_identity)
                .await
                .to_snapshot();
            let current_bridge_session_id = snapshot.current_bridge_session_id().cloned();
            projected.push(
                MobMemberListEntry {
                    agent_identity: entry.agent_identity,
                    agent_runtime_id: entry.agent_runtime_id,
                    fence_token: entry.fence_token,
                    role: entry.role,
                    runtime_mode: entry.runtime_mode,
                    peer_id: entry.peer_id,
                    state: entry.state,
                    wired_to: entry.wired_to,
                    external_peer_specs: entry.external_peer_specs,
                    labels: entry.labels,
                    status: snapshot.status,
                    error: snapshot.error,
                    is_final: snapshot.is_final,
                    current_session_id: None,
                    current_bridge_session_id: None,
                    kickoff: snapshot.kickoff,
                }
                .with_current_bridge_session_id(current_bridge_session_id),
            );
        }
        projected
    }

    /// List members currently eligible for runtime work dispatch.
    ///
    /// Excludes retiring, completed, broken, or unknown members even if they
    /// still appear in the public operational projection.
    pub(crate) async fn list_runnable_members(&self) -> Vec<MobMemberListEntry> {
        self.list_members()
            .await
            .into_iter()
            .filter(|entry| {
                entry.state == crate::roster::MemberState::Active
                    && entry.status == MobMemberStatus::Active
            })
            .collect()
    }

    /// List all members including those in `Retiring` state.
    ///
    /// The `state` field on each [`RosterEntry`] distinguishes `Active` from
    /// `Retiring`. Use this for observability and membership inspection where
    /// in-flight retires should be visible.
    pub async fn list_all_members(&self) -> Vec<RosterEntry> {
        match self
            .execute_machine_command(MobMachineCommand::ListAllMembers)
            .await
        {
            Ok(MobMachineCommandResult::ListAllMembers(entries)) => entries,
            Ok(_) => {
                tracing::error!("unexpected command result variant");
                Default::default()
            }
            Err(_) => Vec::new(),
        }
    }

    async fn canonical_member_list_material(
        &self,
        agent_identity: &MeerkatId,
    ) -> CanonicalMemberSnapshotMaterial {
        self.canonical_member_material(
            agent_identity,
            PeerConnectivityProjection::Omit,
            SessionObservationProjection::Omit,
        )
        .await
    }

    async fn canonical_member_snapshot_material(
        &self,
        agent_identity: &MeerkatId,
    ) -> CanonicalMemberSnapshotMaterial {
        self.canonical_member_material(
            agent_identity,
            PeerConnectivityProjection::Include,
            SessionObservationProjection::Full,
        )
        .await
    }

    async fn canonical_member_material(
        &self,
        agent_identity: &MeerkatId,
        connectivity: PeerConnectivityProjection,
        observation: SessionObservationProjection,
    ) -> CanonicalMemberSnapshotMaterial {
        let (roster_snapshot, roster_entry, roster_state, current_bridge_session_id) = {
            let roster = self.roster.read().await;
            match roster.get(agent_identity) {
                Some(entry) => (
                    roster.snapshot(),
                    Some(entry.clone()),
                    Some(entry.state),
                    entry.member_ref.bridge_session_id().cloned(),
                ),
                None => (roster.snapshot(), None, None, None),
            }
        };

        let restore_failure = {
            self.restore_diagnostics
                .read()
                .await
                .get(agent_identity)
                .cloned()
        };
        if let Some(diag) = restore_failure {
            return MobMemberLifecycleAuthority::materialize(MobMemberLifecycleInput {
                member_present: roster_state.is_some(),
                roster_state,
                session_observation: CanonicalSessionObservation::Missing,
                restore_failure: Some(diag.reason),
                output_preview: None,
                tokens_used: 0,
                agent_runtime_id: roster_entry
                    .as_ref()
                    .map(|e| e.agent_runtime_id.clone())
                    .unwrap_or_else(|| {
                        AgentRuntimeId::initial(AgentIdentity::from(agent_identity.as_str()))
                    }),
                fence_token: roster_entry
                    .as_ref()
                    .map(|e| e.fence_token)
                    .unwrap_or(FenceToken::new(0)),
                current_bridge_session_id: diag.bridge_session_id,
                peer_connectivity: None,
                kickoff: roster_entry
                    .as_ref()
                    .and_then(|entry| entry.kickoff.clone()),
            });
        }

        match (roster_state, current_bridge_session_id) {
            (None, _) => MobMemberLifecycleAuthority::materialize(MobMemberLifecycleInput {
                member_present: false,
                roster_state: None,
                session_observation: CanonicalSessionObservation::Missing,
                restore_failure: None,
                output_preview: None,
                tokens_used: 0,
                agent_runtime_id: AgentRuntimeId::initial(AgentIdentity::from(
                    agent_identity.as_str(),
                )),
                fence_token: FenceToken::new(0),
                current_bridge_session_id: None,
                peer_connectivity: None,
                kickoff: None,
            }),
            (Some(roster_state), None) => {
                let session_observation = match roster_entry.as_ref().map(|entry| &entry.member_ref)
                {
                    Some(MemberRef::BackendPeer {
                        session_id: None, ..
                    }) => CanonicalSessionObservation::Unknown,
                    _ => CanonicalSessionObservation::Missing,
                };
                MobMemberLifecycleAuthority::materialize(MobMemberLifecycleInput {
                    member_present: true,
                    roster_state: Some(roster_state),
                    session_observation,
                    restore_failure: None,
                    output_preview: None,
                    tokens_used: 0,
                    agent_runtime_id: roster_entry
                        .as_ref()
                        .map(|e| e.agent_runtime_id.clone())
                        .unwrap_or_else(|| {
                            AgentRuntimeId::initial(AgentIdentity::from(agent_identity.as_str()))
                        }),
                    fence_token: roster_entry
                        .as_ref()
                        .map(|e| e.fence_token)
                        .unwrap_or(FenceToken::new(0)),
                    current_bridge_session_id: None,
                    peer_connectivity: None,
                    kickoff: roster_entry
                        .as_ref()
                        .and_then(|entry| entry.kickoff.clone()),
                })
            }
            (Some(roster_state), Some(bridge_session_id)) => {
                let (output_preview, tokens_used, observation) = match observation {
                    SessionObservationProjection::Omit => {
                        (None, 0, CanonicalSessionObservation::Unknown)
                    }
                    SessionObservationProjection::Full => {
                        match self.session_service.read(&bridge_session_id).await {
                            Ok(view) => (
                                view.state.last_assistant_text.clone(),
                                view.billing.total_tokens,
                                if view.state.is_active {
                                    CanonicalSessionObservation::Active
                                } else {
                                    CanonicalSessionObservation::Inactive
                                },
                            ),
                            Err(SessionError::NotFound { .. }) => {
                                (None, 0, CanonicalSessionObservation::Missing)
                            }
                            Err(_) => (None, 0, CanonicalSessionObservation::Unknown),
                        }
                    }
                };
                let peer_connectivity = if connectivity == PeerConnectivityProjection::Include {
                    match roster_entry.as_ref() {
                        Some(entry) => {
                            self.resolve_peer_connectivity(
                                entry,
                                &bridge_session_id,
                                &roster_snapshot,
                            )
                            .await
                        }
                        None => None,
                    }
                } else {
                    None
                };
                MobMemberLifecycleAuthority::materialize(MobMemberLifecycleInput {
                    member_present: true,
                    roster_state: Some(roster_state),
                    session_observation: observation,
                    restore_failure: None,
                    output_preview,
                    tokens_used,
                    agent_runtime_id: roster_entry
                        .as_ref()
                        .map(|e| e.agent_runtime_id.clone())
                        .unwrap_or_else(|| {
                            AgentRuntimeId::initial(AgentIdentity::from(agent_identity.as_str()))
                        }),
                    fence_token: roster_entry
                        .as_ref()
                        .map(|e| e.fence_token)
                        .unwrap_or(FenceToken::new(0)),
                    current_bridge_session_id: Some(bridge_session_id),
                    peer_connectivity,
                    kickoff: roster_entry.and_then(|entry| entry.kickoff),
                })
            }
        }
    }

    /// Get a specific member entry by identity.
    pub async fn get_member(&self, identity: &AgentIdentity) -> Option<RosterEntry> {
        let meerkat_id = MeerkatId::from(identity);
        match self
            .execute_machine_command(MobMachineCommand::GetMember {
                agent_identity: meerkat_id,
            })
            .await
        {
            Ok(MobMachineCommandResult::GetMember(entry)) => entry,
            Ok(_) => {
                tracing::error!("unexpected command result variant");
                Default::default()
            }
            Err(_) => None,
        }
    }

    /// Get a specific member entry by legacy MeerkatId (bridge helper).
    pub(crate) async fn get_member_by_meerkat_id(
        &self,
        agent_identity: &MeerkatId,
    ) -> Option<RosterEntry> {
        self.get_member(&AgentIdentity::from(agent_identity.as_str()))
            .await
    }

    /// Resolve the backing bridge session ID for a member by identity.
    ///
    /// # When to use this
    ///
    /// This is the canonical identity → bridge session mapping used by
    /// **surface implementations** (RPC/MCP/REST handlers, web-runtime
    /// wrappers) that must delegate a mob-identity action to a
    /// session-scoped canonical API — e.g. `mob/turn_start` delegating to
    /// the runtime's `turn/start`, or a delegation tool projecting
    /// assistant output from a helper's backing session. Returns `None` if
    /// the member is not found or has no bridge session binding.
    ///
    /// # When not to use it
    ///
    /// Application code acting on a mob should prefer the identity-native
    /// [`MobHandle`] APIs: [`MobHandle::member`] to acquire a
    /// capability-bearing handle, [`MobHandle::internal_turn`] to deliver
    /// content without the RPC turn-start dance, [`MobHandle::peer_send`]
    /// / [`MobHandle::member_send`] for peer comms, etc. Those hide the
    /// session_id entirely.
    ///
    /// # Dogma fit (A8)
    ///
    /// DELETE_ME finding A8 flagged this method as contradicting the
    /// "hide session_id from callers" principle of identity-first mobs.
    /// The apparent contradiction was a scoping confusion: identity-first
    /// hides session_id from **consumers of the public mob surface**
    /// (application code, end-users, SDK clients). Surface implementations
    /// must still bridge identity to session when delegating to the
    /// canonical session-scoped runtime APIs they don't own themselves —
    /// that delegation is explicitly permitted by
    /// `docs/architecture/meerkat-runtime-dogma.md` principle #3
    /// ("shell owns mechanics, not meaning"). The resolver reads the
    /// roster's canonical identity→bridge mapping; no parallel truth is
    /// introduced. Regression
    /// `resolve_bridge_session_id_is_lookup_not_mutation` proves this is
    /// a pure read against the single owner (the mob roster).
    pub async fn resolve_bridge_session_id(&self, identity: &AgentIdentity) -> Option<SessionId> {
        self.get_member(identity)
            .await
            .and_then(|entry| entry.member_ref.bridge_session_id().cloned())
    }

    /// Acquire a capability-bearing handle for a specific active member.
    pub async fn member(&self, identity: &AgentIdentity) -> Result<MemberHandle, MobError> {
        let meerkat_id = MeerkatId::from(identity);
        if let Some(diag) = self.restore_failure_for(&meerkat_id).await {
            return Err(Self::restore_failure_error(&meerkat_id, diag));
        }
        let entry = self
            .get_member(identity)
            .await
            .ok_or_else(|| MobError::MemberNotFound(meerkat_id.clone()))?;
        if entry.state != crate::roster::MemberState::Active {
            return Err(MobError::MemberNotFound(meerkat_id.clone()));
        }
        Ok(MemberHandle {
            mob: self.clone(),
            agent_identity: meerkat_id,
        })
    }

    /// Access a read-only events view for polling/replay.
    pub fn events(&self) -> MobEventsView {
        MobEventsView {
            handle: self.clone(),
        }
    }

    /// Append a dispatcher-owned operator provenance projection.
    ///
    /// This is audit/projection data only. It must never become
    /// authorization truth.
    pub async fn record_operator_action_provenance(
        &self,
        tool_name: &str,
        authority_context: &MobToolAuthorityContext,
    ) -> Result<(), MobError> {
        match self
            .execute_machine_command(MobMachineCommand::RecordOperatorActionProvenance {
                tool_name: tool_name.to_string(),
                authority_context: authority_context.clone(),
            })
            .await?
        {
            MobMachineCommandResult::Unit => Ok(()),
            _ => Err(MobError::Internal(
                "unexpected command result variant".into(),
            )),
        }
    }

    /// Subscribe to agent-level events for a specific member.
    ///
    /// Looks up the member's backing bridge session from the roster, then
    /// subscribes to the session-level event stream via [`MobSessionService`].
    ///
    /// Returns `MobError::MemberNotFound` if the member is not in the
    /// roster or has no backing bridge session.
    pub async fn subscribe_agent_events(
        &self,
        identity: &AgentIdentity,
    ) -> Result<EventStream, MobError> {
        match self
            .execute_machine_command(MobMachineCommand::SubscribeAgentEvents {
                agent_identity: MeerkatId::from(identity),
            })
            .await?
        {
            MobMachineCommandResult::EventStream(stream) => Ok(stream),
            _ => Err(MobError::Internal(
                "unexpected command result variant".into(),
            )),
        }
    }

    /// Subscribe to agent events for all active members (point-in-time snapshot).
    ///
    /// Returns one stream per active member that has a live bridge binding. Members
    /// spawned after this call are not included — use [`subscribe_mob_events`]
    /// for a continuously updated view.
    pub async fn subscribe_all_agent_events(
        &self,
    ) -> Result<Vec<(AgentIdentity, EventStream)>, MobError> {
        match self
            .execute_machine_command(MobMachineCommand::SubscribeAllAgentEvents)
            .await
        {
            Ok(MobMachineCommandResult::AllAgentEventStreams(streams)) => Ok(streams
                .into_iter()
                .map(|(mid, stream)| (AgentIdentity::from(mid.as_str()), stream))
                .collect()),
            Ok(_) => {
                tracing::error!("unexpected command result variant");
                Err(MobError::Internal(
                    "unexpected command result variant".into(),
                ))
            }
            Err(error) => Err(error),
        }
    }

    /// Subscribe to a continuously-updated, mob-level event bus.
    ///
    /// Spawns an independent task that merges per-member session streams,
    /// tags each event with [`AttributedEvent`], and tracks roster changes
    /// (spawns/retires) automatically. Drop the returned handle to stop
    /// the router.
    pub async fn subscribe_mob_events(&self) -> super::event_router::MobEventRouterHandle {
        self.subscribe_mob_events_with_config(super::event_router::MobEventRouterConfig::default())
            .await
    }

    /// Like [`subscribe_mob_events`](Self::subscribe_mob_events) with explicit config.
    pub async fn subscribe_mob_events_with_config(
        &self,
        config: super::event_router::MobEventRouterConfig,
    ) -> super::event_router::MobEventRouterHandle {
        match self
            .execute_machine_command(MobMachineCommand::SubscribeMobEvents { config })
            .await
        {
            Ok(MobMachineCommandResult::MobEventRouter(handle)) => handle,
            Ok(_) => {
                tracing::error!("unexpected command result variant for subscribe_mob_events");
                super::event_router::spawn_event_router(self.clone(), config)
            }
            Err(_) => super::event_router::spawn_event_router(self.clone(), config),
        }
    }

    /// Snapshot of MCP server lifecycle state tracked by this runtime.
    pub async fn mcp_server_states(&self) -> BTreeMap<String, bool> {
        match self
            .execute_machine_command(MobMachineCommand::McpServerStates)
            .await
        {
            Ok(MobMachineCommandResult::McpServerStates(states)) => states,
            Ok(_) => {
                tracing::error!("unexpected command result variant");
                Default::default()
            }
            Err(_) => BTreeMap::new(),
        }
    }

    /// Start a flow run and return its run ID.
    pub async fn run_flow(
        &self,
        flow_id: FlowId,
        params: serde_json::Value,
    ) -> Result<RunId, MobError> {
        self.run_flow_with_stream(flow_id, params, None).await
    }

    /// Start a flow run with an optional scoped stream sink.
    pub async fn run_flow_with_stream(
        &self,
        flow_id: FlowId,
        params: serde_json::Value,
        scoped_event_tx: Option<mpsc::Sender<meerkat_core::ScopedAgentEvent>>,
    ) -> Result<RunId, MobError> {
        match self
            .execute_machine_command(MobMachineCommand::RunFlow {
                flow_id,
                activation_params: params,
                scoped_event_tx,
            })
            .await?
        {
            MobMachineCommandResult::RunId(run_id) => Ok(run_id),
            _ => Err(MobError::Internal(
                "unexpected command result variant".into(),
            )),
        }
    }

    /// Request cancellation of an in-flight flow run.
    pub async fn cancel_flow(&self, run_id: RunId) -> Result<(), MobError> {
        match self
            .execute_machine_command(MobMachineCommand::CancelFlow { run_id })
            .await?
        {
            MobMachineCommandResult::Unit => Ok(()),
            _ => Err(MobError::Internal(
                "unexpected command result variant".into(),
            )),
        }
    }

    /// Fetch a flow run snapshot from the run store.
    pub async fn flow_status(&self, run_id: RunId) -> Result<Option<MobRun>, MobError> {
        match self
            .execute_machine_command(MobMachineCommand::FlowStatus { run_id })
            .await?
        {
            MobMachineCommandResult::FlowStatus(status) => Ok(status),
            _ => Err(MobError::Internal(
                "unexpected command result variant".into(),
            )),
        }
    }

    /// List all configured flow IDs in this mob definition.
    pub fn list_flows(&self) -> Vec<FlowId> {
        self.definition.flows.keys().cloned().collect()
    }

    /// Spawn a new member from a profile and return its member reference.
    #[cfg(test)]
    pub(crate) async fn spawn(
        &self,
        profile_name: ProfileName,
        agent_identity: MeerkatId,
        initial_message: Option<ContentInput>,
    ) -> Result<MemberRef, MobError> {
        self.spawn_with_options(profile_name, agent_identity, initial_message, None, None)
            .await
    }

    /// Spawn a new member with an explicit runtime binding.
    #[cfg(test)]
    pub(crate) async fn spawn_with_binding(
        &self,
        profile_name: ProfileName,
        agent_identity: MeerkatId,
        initial_message: Option<ContentInput>,
        binding: crate::RuntimeBinding,
    ) -> Result<MemberRef, MobError> {
        let mut spec = SpawnMemberSpec::new(profile_name, agent_identity);
        spec.initial_message = initial_message;
        spec.binding = Some(binding);
        self.spawn_spec_internal(spec).await
    }

    /// Spawn a new member from a profile with explicit backend override.
    #[cfg(test)]
    pub(crate) async fn spawn_with_backend(
        &self,
        profile_name: ProfileName,
        agent_identity: MeerkatId,
        initial_message: Option<ContentInput>,
        backend: Option<MobBackendKind>,
    ) -> Result<MemberRef, MobError> {
        self.spawn_with_options(profile_name, agent_identity, initial_message, None, backend)
            .await
    }

    /// Spawn a new member from a profile with explicit runtime mode/backend overrides.
    #[cfg(test)]
    pub(crate) async fn spawn_with_options(
        &self,
        profile_name: ProfileName,
        agent_identity: MeerkatId,
        initial_message: Option<ContentInput>,
        runtime_mode: Option<crate::MobRuntimeMode>,
        backend: Option<MobBackendKind>,
    ) -> Result<MemberRef, MobError> {
        let mut spec = SpawnMemberSpec::new(profile_name, agent_identity);
        spec.initial_message = initial_message;
        spec.runtime_mode = runtime_mode;
        spec.backend = backend;
        self.spawn_spec_internal(spec).await
    }

    /// Attach an existing session by reusing the mob spawn control-plane path.
    #[cfg(test)]
    pub(crate) async fn attach_existing_session(
        &self,
        profile_name: ProfileName,
        agent_identity: MeerkatId,
        session_id: meerkat_core::types::SessionId,
        runtime_mode: Option<crate::MobRuntimeMode>,
        backend: Option<MobBackendKind>,
    ) -> Result<MemberRef, MobError> {
        let mut spec = SpawnMemberSpec::new(profile_name, agent_identity);
        spec.launch_mode = crate::launch::MemberLaunchMode::Resume {
            bridge_session_id: session_id,
        };
        spec.runtime_mode = runtime_mode;
        spec.backend = backend;
        self.spawn_spec_internal(spec).await
    }

    /// Attach an existing session as a regular mob member.
    #[cfg(test)]
    pub(crate) async fn attach_existing_session_as_member(
        &self,
        profile_name: ProfileName,
        agent_identity: MeerkatId,
        session_id: meerkat_core::types::SessionId,
    ) -> Result<MemberRef, MobError> {
        self.attach_existing_session(profile_name, agent_identity, session_id, None, None)
            .await
    }

    /// Spawn a member from a fully-specified [`SpawnMemberSpec`].
    pub async fn spawn_spec(&self, spec: SpawnMemberSpec) -> Result<SpawnResult, MobError> {
        let identity = spec.identity.clone();
        self.spawn_spec_internal(spec).await?;
        // The roster is updated synchronously during spawn finalization,
        // so the entry is guaranteed to be present by the time the reply
        // arrives.
        let entry = self.get_member(&identity).await.ok_or_else(|| {
            MobError::Internal(format!(
                "spawn succeeded but roster entry missing for '{identity}'"
            ))
        })?;
        Ok(SpawnResult {
            agent_identity: entry.agent_identity,
            agent_runtime_id: entry.agent_runtime_id,
            fence_token: entry.fence_token,
        })
    }

    /// Internal spawn that returns the raw `MemberRef` for crate-internal callers.
    pub(crate) async fn spawn_spec_internal(
        &self,
        spec: SpawnMemberSpec,
    ) -> Result<MemberRef, MobError> {
        match self
            .execute_machine_command(MobMachineCommand::Spawn {
                spec: Box::new(spec),
                owner_context: None,
            })
            .await?
        {
            MobMachineCommandResult::SpawnReceipt(receipt) => Ok(receipt.member_ref),
            _ => Err(MobError::Internal(
                "unexpected command result variant".into(),
            )),
        }
    }

    pub(super) async fn spawn_spec_receipt_with_owner_context(
        &self,
        spec: SpawnMemberSpec,
        owner_context: CanonicalOpsOwnerContext,
    ) -> Result<MemberSpawnReceipt, MobError> {
        match self
            .execute_machine_command(MobMachineCommand::Spawn {
                spec: Box::new(spec),
                owner_context: Some(owner_context),
            })
            .await?
        {
            MobMachineCommandResult::SpawnReceipt(receipt) => Ok(receipt),
            _ => Err(MobError::Internal(
                "unexpected command result variant".into(),
            )),
        }
    }

    /// Spawn multiple members in parallel.
    ///
    /// Results preserve input order.
    pub async fn spawn_many(
        &self,
        specs: Vec<SpawnMemberSpec>,
    ) -> Vec<Result<SpawnResult, MobError>> {
        futures::future::join_all(specs.into_iter().map(|spec| self.spawn_spec(spec))).await
    }

    pub(super) async fn spawn_many_receipts_with_owner_context(
        &self,
        specs: Vec<SpawnMemberSpec>,
        owner_context: CanonicalOpsOwnerContext,
    ) -> Vec<Result<MemberSpawnReceipt, MobError>> {
        futures::future::join_all(
            specs.into_iter().map(|spec| {
                self.spawn_spec_receipt_with_owner_context(spec, owner_context.clone())
            }),
        )
        .await
    }

    /// Retire a member, archiving its session and removing trust.
    pub async fn retire(&self, identity: AgentIdentity) -> Result<(), MobError> {
        let meerkat_id = MeerkatId::from(&identity);
        match self
            .execute_machine_command(MobMachineCommand::Retire {
                agent_identity: meerkat_id,
            })
            .await?
        {
            MobMachineCommandResult::Unit => Ok(()),
            _ => Err(MobError::Internal(
                "unexpected command result variant".into(),
            )),
        }
    }

    /// Retire a member and respawn with the same profile, labels, wiring, and mode.
    ///
    /// This is a helper convenience over primitive mob behavior, not a
    /// machine-owned primitive. Returns a receipt on full success, or a
    /// structured error on failure. No rollback is attempted after retire.
    pub async fn respawn(
        &self,
        identity: AgentIdentity,
        initial_message: Option<ContentInput>,
    ) -> Result<MemberRespawnReceipt, MobRespawnError> {
        let meerkat_id = MeerkatId::from(&identity);
        let reply = match self
            .execute_machine_command(MobMachineCommand::Respawn {
                agent_identity: meerkat_id,
                initial_message,
            })
            .await?
        {
            MobMachineCommandResult::Respawn(reply) => reply,
            _ => {
                return Err(MobRespawnError::from(MobError::Internal(
                    "unexpected command result variant".into(),
                )));
            }
        };
        match reply {
            Ok(receipt) => Ok(receipt),
            Err(err) => Err(err),
        }
    }

    /// Retire all roster members concurrently in a single actor command.
    pub async fn retire_all(&self) -> Result<(), MobError> {
        match self
            .execute_machine_command(MobMachineCommand::RetireAll)
            .await?
        {
            MobMachineCommandResult::Unit => Ok(()),
            _ => Err(MobError::Internal(
                "unexpected command result variant".into(),
            )),
        }
    }

    /// Core `ensure_member` worker invoked by `execute_machine_command`.
    ///
    /// Tries to spawn the member; on [`MobError::MemberAlreadyExists`],
    /// resolves the existing member via [`list_members`] and wraps it as
    /// [`EnsureMemberOutcome::Existed`]. Other spawn errors propagate
    /// unchanged.
    async fn handle_ensure_member(
        &self,
        spec: SpawnMemberSpec,
    ) -> Result<EnsureMemberOutcome, MobError> {
        let identity = spec.identity.clone();
        // `Box::pin` breaks the compiler-visible recursion:
        // handle_ensure_member -> spawn_spec -> execute_machine_command ->
        // (MobMachineCommand::Spawn arm, which never re-enters this fn).
        match Box::pin(self.spawn_spec(spec)).await {
            Ok(spawn_result) => Ok(EnsureMemberOutcome::Spawned(spawn_result)),
            Err(MobError::MemberAlreadyExists(_)) => {
                let existing = Box::pin(self.list_members())
                    .await
                    .into_iter()
                    .find(|entry| entry.agent_identity == identity)
                    .ok_or_else(|| {
                        MobError::Internal(format!(
                            "ensure_member: member '{identity}' reported existing but not found in roster"
                        ))
                    })?;
                Ok(EnsureMemberOutcome::Existed(Box::new(existing)))
            }
            Err(other) => Err(other),
        }
    }

    /// Core `reconcile` worker invoked by `execute_machine_command`.
    ///
    /// Compares `desired` against the current roster:
    /// * Desired identities present in the roster become `retained`.
    /// * Desired identities absent are spawned; successes land in
    ///   `spawned`, per-identity failures land in `failures` tagged with
    ///   [`ReconcileStage::Spawn`].
    /// * When [`ReconcileOptions::retire_stale`] is set, identities in the
    ///   roster that are not in `desired` are retired; failures land in
    ///   `failures` tagged with [`ReconcileStage::Retire`].
    async fn handle_reconcile(
        &self,
        desired: Vec<SpawnMemberSpec>,
        options: ReconcileOptions,
    ) -> ReconcileReport {
        let mut report = ReconcileReport {
            desired: desired.iter().map(|spec| spec.identity.clone()).collect(),
            ..ReconcileReport::default()
        };

        let current: std::collections::BTreeSet<AgentIdentity> = Box::pin(self.list_members())
            .await
            .into_iter()
            .map(|entry| entry.agent_identity)
            .collect();
        let desired_ids: std::collections::BTreeSet<AgentIdentity> =
            desired.iter().map(|spec| spec.identity.clone()).collect();

        for spec in desired {
            let identity = spec.identity.clone();
            if current.contains(&identity) {
                report.retained.push(identity);
                continue;
            }
            match Box::pin(self.spawn_spec(spec)).await {
                Ok(spawn_result) => report.spawned.push(spawn_result),
                Err(error) => report.failures.push(ReconcileFailure {
                    agent_identity: identity,
                    error,
                    stage: ReconcileStage::Spawn,
                }),
            }
        }

        if options.retire_stale {
            for identity in current.difference(&desired_ids).cloned() {
                match Box::pin(self.retire(identity.clone())).await {
                    Ok(()) => report.retired.push(identity),
                    Err(error) => report.failures.push(ReconcileFailure {
                        agent_identity: identity,
                        error,
                        stage: ReconcileStage::Retire,
                    }),
                }
            }
        }

        report
    }

    /// Core `list_members_matching` worker invoked by
    /// `execute_machine_command`. Composition over
    /// [`list_members`](Self::list_members) with each constraint applied
    /// conjunctively. An empty filter matches every member.
    async fn handle_list_members_matching(&self, filter: MemberFilter) -> Vec<MobMemberListEntry> {
        Box::pin(self.list_members())
            .await
            .into_iter()
            .filter(|entry| {
                if let Some(role) = &filter.role
                    && entry.role != *role
                {
                    return false;
                }
                if let Some(state) = filter.state
                    && entry.state != state
                {
                    return false;
                }
                for (key, value) in &filter.labels {
                    if entry.labels.get(key).is_none_or(|v| v != value) {
                        return false;
                    }
                }
                true
            })
            .collect()
    }

    /// Declarative: spawn the member described by `spec` if absent; otherwise
    /// return the existing roster entry unchanged.
    ///
    /// Composition over [`spawn_spec`](Self::spawn_spec) +
    /// [`get_member`](Self::get_member). Idempotent with respect to
    /// [`SpawnMemberSpec::identity`]. The spec's `initial_message`, launch
    /// mode, and other per-spawn options are applied only when a new member
    /// is created.
    pub async fn ensure_member(
        &self,
        spec: SpawnMemberSpec,
    ) -> Result<EnsureMemberOutcome, MobError> {
        match self
            .execute_machine_command(MobMachineCommand::EnsureMember {
                spec: Box::new(spec),
            })
            .await?
        {
            MobMachineCommandResult::EnsureMember(outcome) => Ok(outcome),
            _ => Err(MobError::Internal(
                "unexpected command result variant".into(),
            )),
        }
    }

    /// Declarative: drive the roster toward the `desired` set of specs.
    ///
    /// For each desired spec, spawn if absent or retain if present. When
    /// [`ReconcileOptions::retire_stale`] is set, members whose identity is
    /// not in the desired set are retired. Failures are collected per-
    /// identity in [`ReconcileReport::failures`] rather than short-circuiting.
    ///
    /// Composition over spawn + retire + list_members; no new lifecycle.
    pub async fn reconcile(
        &self,
        desired: Vec<SpawnMemberSpec>,
        options: ReconcileOptions,
    ) -> Result<ReconcileReport, MobError> {
        match self
            .execute_machine_command(MobMachineCommand::Reconcile { desired, options })
            .await?
        {
            MobMachineCommandResult::Reconcile(report) => Ok(*report),
            _ => Err(MobError::Internal(
                "unexpected command result variant".into(),
            )),
        }
    }

    /// Declarative: list members matching every constraint in `filter`.
    ///
    /// Composition over [`list_members`](Self::list_members) followed by
    /// in-process filtering. An empty filter matches every currently active
    /// member. Only the `labels` pairs in `filter` must match (extra labels
    /// on the member are allowed); `role`, `state`, and `has_realtime_intent`
    /// each apply only when set.
    pub async fn list_members_matching(&self, filter: MemberFilter) -> Vec<MobMemberListEntry> {
        match self
            .execute_machine_command(MobMachineCommand::ListMembersMatching {
                filter: Box::new(filter),
            })
            .await
        {
            Ok(MobMachineCommandResult::ListMembers(entries)) => entries,
            Ok(_) => {
                tracing::error!("unexpected command result variant");
                Default::default()
            }
            Err(_) => Vec::new(),
        }
    }

    /// Rotate the persisted mob supervisor authority.
    ///
    /// # Scope: mob-wide
    ///
    /// The supervisor authority is a **single per-mob fact** persisted in
    /// [`SupervisorAuthorityRecord`](crate::store::SupervisorAuthorityRecord) keyed by
    /// `mob_id`. Rotation generates a fresh authority (new public peer id,
    /// incremented epoch) and broadcasts
    /// [`BridgeCommand::AuthorizeSupervisor`](meerkat_contracts::wire::supervisor_bridge::BridgeCommand)
    /// to **every** remote member binding currently on the roster, then
    /// atomically advances the persisted local authority when the remote
    /// dispatch succeeds (or partially advances — see below — on partial
    /// failure).
    ///
    /// There is no per-member scope here, and no scoping parameter is
    /// missing. Per-member [`BridgeBootstrapToken`](meerkat_contracts::wire::supervisor_bridge::BridgeBootstrapToken)s
    /// carried on `MemberRef::BackendPeer` are the **bootstrap proof** that
    /// authorizes a specific member's bridge to (re)establish under the
    /// current supervisor — they are not a separate supervisor identity.
    /// One supervisor, many bootstrap tokens.
    ///
    /// # Partial-failure semantics
    ///
    /// If some remote bindings accept the rotation and others reject it,
    /// the local authority is advanced to match the partially applied
    /// next authority rather than reverted (see
    /// `MobActor::handle_rotate_supervisor` in `actor.rs` for the
    /// authoritative implementation). Callers observing
    /// [`MobError::WiringError`] with a message containing
    /// `"rollback failures"` should treat it as
    /// "rotation completed then local authority advanced", not
    /// "fully reverted rotation". This matches the top-level `CLAUDE.md`
    /// warning: once a remote has rotated forward, rolling it back is
    /// best-effort.
    ///
    /// # Dogma fit (B4)
    ///
    /// DELETE_ME finding B4 flagged the `&self`-only signature as
    /// potentially missing a scoping parameter. After audit the
    /// supervisor is unambiguously mob-wide (one
    /// `SupervisorAuthorityRecord` per `mob_id`, one persistence key,
    /// one rotation broadcast), so a scoping parameter would be
    /// fictional. Per dogma principle #1 ("one semantic fact, one
    /// owner") the signature already matches the data model.
    /// Regression coverage lives in `meerkat-mob/src/runtime/tests.rs`:
    /// `test_rotate_supervisor_updates_runtime_metadata`,
    /// `test_rotate_supervisor_reauthorizes_live_remote_members_and_rejects_stale_epoch`,
    /// `test_rotate_supervisor_bind_fallback_binds_next_authority`, and
    /// `test_rotate_supervisor_advances_local_authority_when_rollback_fails`.
    pub async fn rotate_supervisor(&self) -> Result<SupervisorRotationReport, MobError> {
        self.send_actor_command(|reply_tx| MobCommand::RotateSupervisor { reply_tx })
            .await?
    }

    /// Wire a local member to either another local member or an external peer.
    pub async fn wire<T>(&self, local: AgentIdentity, target: T) -> Result<(), MobError>
    where
        T: Into<PeerTarget>,
    {
        match self
            .execute_machine_command(MobMachineCommand::Wire {
                local: MeerkatId::from(&local),
                target: target.into(),
            })
            .await?
        {
            MobMachineCommandResult::Unit => Ok(()),
            _ => Err(MobError::Internal(
                "unexpected command result variant".into(),
            )),
        }
    }

    /// Unwire a local member from either another local member or an external peer.
    pub async fn unwire<T>(&self, local: AgentIdentity, target: T) -> Result<(), MobError>
    where
        T: Into<PeerTarget>,
    {
        match self
            .execute_machine_command(MobMachineCommand::Unwire {
                local: MeerkatId::from(&local),
                target: target.into(),
            })
            .await?
        {
            MobMachineCommandResult::Unit => Ok(()),
            _ => Err(MobError::Internal(
                "unexpected command result variant".into(),
            )),
        }
    }

    /// Compatibility wrapper for internal-turn submission.
    ///
    /// Prefer [`MobHandle::member`] plus [`MemberHandle::internal_turn`] for
    /// the target 0.5 API shape.
    ///
    /// DELETE_ME B5: three operations that sometimes get mistaken for the
    /// same thing are actually three distinct slices of "deliver content to
    /// a member". [`MobHandle::internal_turn`] / [`MemberHandle::internal_turn`]
    /// (this) is Rust in-process direct write into the member's pending
    /// turn slot — no peer comms, no handling-mode selection. `mob/turn_start`
    /// (RPC) resolves the identity to the bridge session and delegates to
    /// the canonical `turn/start` handler with turn-level overrides.
    /// `mob/member_send` (RPC) is peer-delivery shape over comms with
    /// `HandlingMode` + `RenderMetadata`; it lands in the member's comms
    /// inbox, not as a new turn. The three surfaces share a name fragment
    /// but diverge on who authorizes the delivery, what the member's
    /// runtime does with it, and what the caller gets back. Keep them
    /// separate — collapsing them would erase real policy distinctions.
    pub async fn internal_turn(
        &self,
        identity: AgentIdentity,
        message: impl Into<meerkat_core::types::ContentInput>,
    ) -> Result<MemberDeliveryReceipt, MobError> {
        let meerkat_id = MeerkatId::from(&identity);
        self.internal_turn_for_member(meerkat_id.clone(), message.into())
            .await?;
        let material = self.canonical_member_list_material(&meerkat_id).await;
        Ok(MemberDeliveryReceipt {
            identity,
            agent_runtime_id: material.agent_runtime_id,
            fence_token: material.fence_token,
            handling_mode: HandlingMode::Queue,
        })
    }

    pub(super) async fn external_turn_for_member(
        &self,
        agent_identity: MeerkatId,
        message: meerkat_core::types::ContentInput,
        handling_mode: HandlingMode,
        render_metadata: Option<RenderMetadata>,
    ) -> Result<(), MobError> {
        let material = self.canonical_member_list_material(&agent_identity).await;
        let cmd = Box::new(crate::mob_machine::SubmitWorkCommand {
            runtime_id: material.agent_runtime_id,
            fence_token: material.fence_token,
            work_ref: WorkRef::new(),
            spec: WorkSpec::new(message, WorkOrigin::External),
            handling_mode,
            render_metadata,
        });
        self.execute_machine_command(MobMachineCommand::SubmitWork(cmd))
            .await?;
        Ok(())
    }

    pub(super) async fn internal_turn_for_member(
        &self,
        agent_identity: MeerkatId,
        message: meerkat_core::types::ContentInput,
    ) -> Result<(), MobError> {
        let material = self.canonical_member_list_material(&agent_identity).await;
        let cmd = Box::new(crate::mob_machine::SubmitWorkCommand {
            runtime_id: material.agent_runtime_id,
            fence_token: material.fence_token,
            work_ref: WorkRef::new(),
            spec: WorkSpec::new(message, WorkOrigin::Internal),
            handling_mode: HandlingMode::Queue,
            render_metadata: None,
        });
        self.execute_machine_command(MobMachineCommand::SubmitWork(cmd))
            .await?;
        Ok(())
    }

    // -----------------------------------------------------------------
    // Work lane
    // -----------------------------------------------------------------

    /// Submit a unit of work to a mob member.
    ///
    /// The fence token is validated against the member's current incarnation at
    /// the dispatch boundary. If the token is stale (i.e., the member has been
    /// respawned or reset since the caller obtained the token), the submission
    /// is rejected with [`MobError::StaleFenceToken`].
    pub async fn submit_work(
        &self,
        runtime_id: AgentRuntimeId,
        fence_token: FenceToken,
        work_ref: WorkRef,
        spec: WorkSpec,
    ) -> Result<WorkDeliveryReceipt, MobError> {
        let cmd = Box::new(crate::mob_machine::SubmitWorkCommand {
            runtime_id: runtime_id.clone(),
            fence_token,
            work_ref: work_ref.clone(),
            spec,
            handling_mode: HandlingMode::Queue,
            render_metadata: None,
        });
        match self
            .execute_machine_command(MobMachineCommand::SubmitWork(cmd))
            .await?
        {
            MobMachineCommandResult::WorkReceipt { work_ref: ref_out } => Ok(WorkDeliveryReceipt {
                work_ref: ref_out,
                runtime_id,
            }),
            _ => Err(MobError::Internal(
                "unexpected command result variant".into(),
            )),
        }
    }

    /// Cancel a previously submitted unit of work.
    ///
    /// Returns `Ok(())` if the work was found and cancellation was initiated.
    /// Returns [`MobError::WorkNotFound`] if no in-flight work with the given
    /// reference exists.
    pub async fn cancel_work(&self, work_ref: WorkRef) -> Result<(), MobError> {
        match self
            .execute_machine_command(MobMachineCommand::CancelWork { work_ref })
            .await?
        {
            MobMachineCommandResult::Unit => Ok(()),
            _ => Err(MobError::Internal(
                "unexpected command result variant".into(),
            )),
        }
    }

    /// Cancel all in-flight work for a mob member.
    ///
    /// The fence token is validated before cancellation proceeds.
    pub async fn cancel_all_work(
        &self,
        runtime_id: AgentRuntimeId,
        fence_token: FenceToken,
    ) -> Result<(), MobError> {
        match self
            .execute_machine_command(MobMachineCommand::CancelAllWork {
                runtime_id,
                fence_token,
            })
            .await?
        {
            MobMachineCommandResult::Unit => Ok(()),
            _ => Err(MobError::Internal(
                "unexpected command result variant".into(),
            )),
        }
    }

    /// Transition Running -> Stopped. Mutation commands are rejected while stopped.
    pub async fn stop(&self) -> Result<(), MobError> {
        match self
            .execute_machine_command(MobMachineCommand::Stop)
            .await?
        {
            MobMachineCommandResult::Unit => Ok(()),
            _ => Err(MobError::Internal(
                "unexpected command result variant".into(),
            )),
        }
    }

    /// Transition Stopped -> Running.
    pub async fn resume(&self) -> Result<(), MobError> {
        match self
            .execute_machine_command(MobMachineCommand::Resume)
            .await?
        {
            MobMachineCommandResult::Unit => Ok(()),
            _ => Err(MobError::Internal(
                "unexpected command result variant".into(),
            )),
        }
    }

    /// Archive all members, emit MobCompleted, and transition to Completed.
    pub async fn complete(&self) -> Result<(), MobError> {
        match self
            .execute_machine_command(MobMachineCommand::Complete)
            .await?
        {
            MobMachineCommandResult::Unit => Ok(()),
            _ => Err(MobError::Internal(
                "unexpected command result variant".into(),
            )),
        }
    }

    /// Wipe all runtime state and transition back to `Running`.
    ///
    /// # Scope vs `destroy`
    ///
    /// `reset` and [`Self::destroy`] look similar (both wipe runtime
    /// state, both teardown MCP servers, both append epoch-marker
    /// events) but they have **deliberately different semantics**:
    ///
    /// | aspect               | `reset()`                                          | `destroy()`                                                 |
    /// |----------------------|----------------------------------------------------|-------------------------------------------------------------|
    /// | actor                | **stays alive**, transitions to `Running`          | terminates, transitions to `Destroyed`                      |
    /// | member teardown      | `retire_all_members` (idempotent, all-or-retry)    | `destroy_all_members_for_destroy` (force-fallback, atomic)  |
    /// | return               | `Result<(), MobError>` — clean or retry            | [`Result<MobDestroyReport, MobDestroyError>`]               |
    /// | partial outcomes     | retire-idempotent → reissuing reset retries safely | structured report carries force-destroyed / orphaned / errs |
    /// | event marker         | `MobCreated` + `MobReset` (new epoch, replayable)  | `MobDestroyed` (terminal)                                   |
    /// | handle usable after? | yes                                                | no                                                          ||
    ///
    /// The `()` return is not hiding partial-state information: retire
    /// is idempotent by construction (see `handle_retire` in
    /// `actor.rs` — "cleanup errors are best-effort. If any member
    /// fails to retire the operation is aborted — the caller can retry
    /// since already-retired members are idempotent"), so on error
    /// the contract is "retry `reset()`" rather than "read the partial
    /// outcome from the report." `destroy`'s richer return exists
    /// because force-fallback produces **genuinely new state**
    /// (force-destroyed members, orphaned remote bindings that
    /// couldn't be cleanly dismantled) that the caller needs to see;
    /// `reset` by design avoids that regime and so has no equivalent
    /// data to surface.
    ///
    /// # Dogma fit (B3)
    ///
    /// DELETE_ME finding B3 flagged the divergent return types as an
    /// API asymmetry. After audit the asymmetry is load-bearing: the
    /// return types match the underlying member-teardown shape
    /// (idempotent retire vs force-fallback destroy). Per dogma
    /// principle #5 ("typed truth, never string folklore") the reset
    /// return does not need to pretend to carry a report it cannot
    /// produce; and per principle #1 ("one semantic fact, one
    /// owner") this matches the single underlying model: the
    /// teardown path authors the outcome shape, the handle signature
    /// reflects it. Regression coverage lives in
    /// `test_reset_clears_roster_events_and_returns_to_running`,
    /// `test_reset_allows_spawn_after_reset`, and the
    /// supervisor-escalation reset tests in
    /// `meerkat-mob/src/runtime/tests.rs`.
    pub async fn reset(&self) -> Result<(), MobError> {
        match self
            .execute_machine_command(MobMachineCommand::Reset)
            .await?
        {
            MobMachineCommandResult::Unit => Ok(()),
            _ => Err(MobError::Internal(
                "unexpected command result variant".into(),
            )),
        }
    }

    /// Retire active members, clear persisted mob storage, and terminate the actor.
    pub async fn destroy(&self) -> Result<MobDestroyReport, MobDestroyError> {
        match self
            .execute_destroy_machine_command(MobMachineCommand::Destroy)
            .await?
        {
            MobMachineCommandResult::DestroyReport(report) => Ok(report),
            _ => Err(MobDestroyError::from(MobError::Internal(
                "unexpected command result variant".into(),
            ))),
        }
    }

    /// Create a task in the shared mob task board.
    pub async fn task_create(
        &self,
        subject: String,
        description: String,
        blocked_by: Vec<TaskId>,
    ) -> Result<TaskId, MobError> {
        match self
            .execute_machine_command(MobMachineCommand::TaskCreate {
                subject,
                description,
                blocked_by,
            })
            .await?
        {
            MobMachineCommandResult::TaskId(task_id) => Ok(task_id),
            _ => Err(MobError::Internal(
                "unexpected command result variant".into(),
            )),
        }
    }

    /// Update task status/owner in the shared mob task board.
    pub async fn task_update(
        &self,
        task_id: TaskId,
        status: TaskStatus,
        owner: Option<AgentIdentity>,
    ) -> Result<(), MobError> {
        match self
            .execute_machine_command(MobMachineCommand::TaskUpdate {
                task_id,
                status,
                owner,
            })
            .await?
        {
            MobMachineCommandResult::Unit => Ok(()),
            _ => Err(MobError::Internal(
                "unexpected command result variant".into(),
            )),
        }
    }

    /// List tasks from the in-memory task board projection.
    pub async fn task_list(&self) -> Result<Vec<MobTask>, MobError> {
        match self
            .execute_machine_command(MobMachineCommand::TaskList)
            .await?
        {
            MobMachineCommandResult::TaskList(tasks) => Ok(tasks),
            _ => Err(MobError::Internal(
                "unexpected command result variant".into(),
            )),
        }
    }

    /// Get a task by ID from the in-memory task board projection.
    pub async fn task_get(&self, task_id: &TaskId) -> Result<Option<MobTask>, MobError> {
        match self
            .execute_machine_command(MobMachineCommand::TaskGet {
                task_id: task_id.clone(),
            })
            .await?
        {
            MobMachineCommandResult::TaskGet(task) => Ok(task),
            _ => Err(MobError::Internal(
                "unexpected command result variant".into(),
            )),
        }
    }

    #[cfg(test)]
    pub async fn debug_flow_tracker_counts(&self) -> Result<(usize, usize), MobError> {
        match self
            .execute_machine_command(MobMachineCommand::FlowTrackerCounts)
            .await?
        {
            MobMachineCommandResult::FlowTrackerCounts(counts) => Ok(counts),
            _ => Err(MobError::Internal(
                "unexpected command result variant".into(),
            )),
        }
    }

    #[cfg(test)]
    pub(crate) async fn debug_orchestrator_snapshot(
        &self,
    ) -> Result<super::MobOrchestratorSnapshot, MobError> {
        match self
            .execute_machine_command(MobMachineCommand::OrchestratorSnapshot)
            .await?
        {
            MobMachineCommandResult::OrchestratorSnapshot(snapshot) => Ok(snapshot),
            _ => Err(MobError::Internal(
                "unexpected command result variant".into(),
            )),
        }
    }

    #[cfg(test)]
    pub(crate) async fn debug_lifecycle_snapshot(&self) -> Result<MobLifecycleSnapshot, MobError> {
        match self
            .execute_machine_command(MobMachineCommand::LifecycleSnapshot)
            .await?
        {
            MobMachineCommandResult::LifecycleSnapshot(snapshot) => Ok(snapshot),
            _ => Err(MobError::Internal(
                "unexpected command result variant".into(),
            )),
        }
    }

    #[cfg(test)]
    pub(crate) async fn debug_dsl_t2_snapshot(&self) -> Result<super::MobDslT2Snapshot, MobError> {
        match self
            .execute_machine_command(MobMachineCommand::DslT2Snapshot)
            .await?
        {
            MobMachineCommandResult::DslT2Snapshot(snapshot) => Ok(snapshot),
            _ => Err(MobError::Internal(
                "unexpected command result variant".into(),
            )),
        }
    }

    /// Set or clear the spawn policy for automatic member provisioning.
    ///
    /// When set, external turns targeting an unknown member identity will
    /// consult the policy before returning `MeerkatNotFound`.
    pub async fn set_spawn_policy(
        &self,
        policy: Option<Arc<dyn super::spawn_policy::SpawnPolicy>>,
    ) -> Result<(), MobError> {
        match self
            .execute_machine_command(MobMachineCommand::SetSpawnPolicy { policy })
            .await?
        {
            MobMachineCommandResult::Unit => Ok(()),
            _ => Err(MobError::Internal(
                "unexpected command result variant".into(),
            )),
        }
    }

    /// Shut down the actor. After this, no more commands are accepted.
    pub async fn shutdown(&self) -> Result<(), MobError> {
        match self
            .execute_machine_command(MobMachineCommand::Shutdown)
            .await?
        {
            MobMachineCommandResult::Unit => Ok(()),
            _ => Err(MobError::Internal(
                "unexpected command result variant".into(),
            )),
        }
    }

    /// Force-cancel a member's in-flight turn via session interrupt.
    ///
    /// Unlike [`retire`](Self::retire), this does not archive the session or
    /// remove the member from the roster — it only cancels the current turn.
    pub async fn force_cancel_member(&self, identity: AgentIdentity) -> Result<(), MobError> {
        match self
            .execute_machine_command(MobMachineCommand::ForceCancel {
                agent_identity: MeerkatId::from(&identity),
            })
            .await?
        {
            MobMachineCommandResult::Unit => Ok(()),
            _ => Err(MobError::Internal(
                "unexpected command result variant".into(),
            )),
        }
    }

    async fn startup_kickoff_snapshot(
        &self,
    ) -> Result<super::state::MobStartupKickoffSnapshot, MobError> {
        self.send_actor_command(|reply_tx| MobCommand::StartupKickoffSnapshot { reply_tx })
            .await
    }

    fn kickoff_wait_is_satisfied(
        entry: &RosterEntry,
        material: &CanonicalMemberSnapshotMaterial,
        pending_kickoff_member_ids: &BTreeSet<String>,
    ) -> bool {
        if entry.runtime_mode != crate::MobRuntimeMode::AutonomousHost {
            return true;
        }
        match material.status {
            CanonicalMemberStatus::Unknown => false,
            CanonicalMemberStatus::Active => {
                !pending_kickoff_member_ids.contains(entry.agent_identity.as_str())
            }
            CanonicalMemberStatus::Retiring
            | CanonicalMemberStatus::Broken
            | CanonicalMemberStatus::Completed => true,
        }
    }

    fn ready_wait_is_satisfied(
        entry: &RosterEntry,
        material: &CanonicalMemberSnapshotMaterial,
        ready_runtime_ids: &BTreeSet<String>,
    ) -> bool {
        if entry.runtime_mode != crate::MobRuntimeMode::AutonomousHost {
            return true;
        }
        match material.status {
            CanonicalMemberStatus::Unknown => false,
            CanonicalMemberStatus::Active => {
                ready_runtime_ids.contains(&entry.agent_runtime_id.to_string())
            }
            CanonicalMemberStatus::Retiring
            | CanonicalMemberStatus::Broken
            | CanonicalMemberStatus::Completed => true,
        }
    }

    async fn wait_for_kickoff_resolution(
        &self,
        target_ids: &[MeerkatId],
        timeout: Option<Duration>,
    ) -> Result<(), MobError> {
        if target_ids.is_empty() {
            return Ok(());
        }

        let deadline = Instant::now() + timeout.unwrap_or(DEFAULT_KICKOFF_WAIT_TIMEOUT);
        loop {
            let snapshot = self.startup_kickoff_snapshot().await?;
            let entries = self
                .list_all_members()
                .await
                .into_iter()
                .map(|entry| (entry.agent_identity.clone(), entry))
                .collect::<HashMap<_, _>>();

            let mut pending_member_ids = Vec::new();
            for id in target_ids {
                let Some(entry) = entries.get(id) else {
                    continue;
                };
                let material = self.canonical_member_list_material(id).await;
                if !Self::kickoff_wait_is_satisfied(
                    entry,
                    &material,
                    &snapshot.pending_kickoff_member_ids,
                ) {
                    pending_member_ids.push(id.clone());
                }
            }

            if pending_member_ids.is_empty() {
                return Ok(());
            }

            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(MobError::KickoffWaitTimedOut { pending_member_ids });
            }

            tokio::time::sleep(std::cmp::min(remaining, Duration::from_millis(50))).await;
        }
    }

    async fn wait_for_ready_resolution(
        &self,
        target_ids: &[MeerkatId],
        timeout: Option<Duration>,
    ) -> Result<(), MobError> {
        if target_ids.is_empty() {
            return Ok(());
        }

        let deadline = Instant::now() + timeout.unwrap_or(DEFAULT_READY_WAIT_TIMEOUT);
        loop {
            let snapshot = self.startup_kickoff_snapshot().await?;
            let entries = self
                .list_all_members()
                .await
                .into_iter()
                .map(|entry| (entry.agent_identity.clone(), entry))
                .collect::<HashMap<_, _>>();

            let mut pending_member_ids = Vec::new();
            for id in target_ids {
                let Some(entry) = entries.get(id) else {
                    continue;
                };
                let material = self.canonical_member_list_material(id).await;
                if !Self::ready_wait_is_satisfied(entry, &material, &snapshot.ready_runtime_ids) {
                    pending_member_ids.push(id.clone());
                }
            }

            if pending_member_ids.is_empty() {
                return Ok(());
            }

            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(MobError::ReadyWaitTimedOut { pending_member_ids });
            }

            tokio::time::sleep(std::cmp::min(remaining, Duration::from_millis(50))).await;
        }
    }

    async fn wait_one_material(
        &self,
        agent_identity: &MeerkatId,
    ) -> Result<CanonicalMemberSnapshotMaterial, MobError> {
        loop {
            let material = self.canonical_member_list_material(agent_identity).await;
            if MobMemberLifecycleAuthority::is_terminal(&material) {
                return Ok(material);
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }

    /// Get a point-in-time execution snapshot for a member.
    ///
    /// This is the deep inspection surface. Unlike list projections, it
    /// resolves live peer connectivity when a comms runtime is available,
    /// and projects the current realtime attachment status from the
    /// MeerkatMachine (when the runtime adapter is available).
    pub async fn member_status(
        &self,
        identity: &AgentIdentity,
    ) -> Result<MobMemberSnapshot, MobError> {
        let mut snapshot = match self
            .execute_machine_command(MobMachineCommand::MemberStatus {
                agent_identity: MeerkatId::from(identity),
            })
            .await?
        {
            MobMachineCommandResult::MemberStatus(snapshot) => snapshot,
            _ => {
                return Err(MobError::Internal(
                    "unexpected command result variant".into(),
                ));
            }
        };
        snapshot.realtime_attachment_status =
            self.project_realtime_attachment_status(&snapshot).await;
        Ok(snapshot)
    }

    /// Project the current realtime attachment status for the given member
    /// snapshot by consulting the MeerkatMachine runtime adapter. Returns
    /// `None` when the adapter is unavailable, the session is not yet bound
    /// in the runtime (bridge session unknown), or the runtime query fails.
    async fn project_realtime_attachment_status(
        &self,
        snapshot: &MobMemberSnapshot,
    ) -> Option<MobRealtimeAttachmentStatus> {
        #[cfg(feature = "runtime-adapter")]
        {
            use meerkat_runtime::service_ext::SessionServiceRuntimeExt as _;
            let session_id = snapshot.current_bridge_session_id().cloned()?;
            let runtime = self.runtime_adapter.as_ref()?.as_ref();
            let status = runtime.realtime_attachment_status(&session_id).await.ok()?;
            Some(map_runtime_realtime_attachment_status(status))
        }
        #[cfg(not(feature = "runtime-adapter"))]
        {
            let _ = snapshot;
            None
        }
    }

    /// Wait until all current autonomous members resolve their initial kickoff.
    ///
    /// In 0.6 autonomous members no longer run a synthetic second kickoff turn,
    /// but their initial prompt still resolves asynchronously through the
    /// runtime-backed input path. This barrier is satisfied once each targeted
    /// autonomous member leaves `pending` / `starting` / `callback_pending`
    /// and reaches a terminal kickoff phase.
    pub async fn wait_for_kickoff_complete(
        &self,
        timeout: Option<Duration>,
    ) -> Result<Vec<(AgentIdentity, MobMemberSnapshot)>, MobError> {
        let target_ids = self
            .list_all_members()
            .await
            .into_iter()
            .map(|entry| entry.agent_identity)
            .collect::<Vec<_>>();
        let identities: Vec<AgentIdentity> = target_ids.clone();
        self.wait_for_kickoff_resolution(&target_ids, timeout)
            .await?;

        let mut snapshots = Vec::with_capacity(identities.len());
        for identity in identities {
            snapshots.push((identity.clone(), self.member_status(&identity).await?));
        }
        Ok(snapshots)
    }

    /// Wait until the given members resolve their initial kickoff.
    ///
    /// See [`wait_for_kickoff_complete`](Self::wait_for_kickoff_complete) for details.
    pub async fn wait_for_members_kickoff_complete(
        &self,
        ids: &[AgentIdentity],
        timeout: Option<Duration>,
    ) -> Result<Vec<(AgentIdentity, MobMemberSnapshot)>, MobError> {
        let target_meerkat_ids: Vec<MeerkatId> = ids.iter().map(MeerkatId::from).collect();
        self.wait_for_kickoff_resolution(&target_meerkat_ids, timeout)
            .await?;

        let mut snapshots = Vec::with_capacity(ids.len());
        for identity in ids {
            snapshots.push((identity.clone(), self.member_status(identity).await?));
        }
        Ok(snapshots)
    }

    /// Wait until all current members are startup-ready for orchestration.
    pub async fn wait_for_ready(
        &self,
        timeout: Option<Duration>,
    ) -> Result<Vec<(AgentIdentity, MobMemberSnapshot)>, MobError> {
        let target_ids = self
            .list_all_members()
            .await
            .into_iter()
            .map(|entry| entry.agent_identity)
            .collect::<Vec<_>>();
        let identities: Vec<AgentIdentity> = target_ids.clone();
        self.wait_for_ready_resolution(&target_ids, timeout).await?;

        let mut snapshots = Vec::with_capacity(identities.len());
        for identity in identities {
            snapshots.push((identity.clone(), self.member_status(&identity).await?));
        }
        Ok(snapshots)
    }

    /// Wait until the given members are startup-ready for orchestration.
    pub async fn wait_for_members_ready(
        &self,
        ids: &[AgentIdentity],
        timeout: Option<Duration>,
    ) -> Result<Vec<(AgentIdentity, MobMemberSnapshot)>, MobError> {
        let target_meerkat_ids: Vec<MeerkatId> = ids.iter().map(MeerkatId::from).collect();
        self.wait_for_ready_resolution(&target_meerkat_ids, timeout)
            .await?;

        let mut snapshots = Vec::with_capacity(ids.len());
        for identity in ids {
            snapshots.push((identity.clone(), self.member_status(identity).await?));
        }
        Ok(snapshots)
    }

    /// Wait for a specific member to reach a terminal state, then return its snapshot.
    ///
    /// Polls canonical member classification until terminal.
    pub async fn wait_one(&self, identity: &AgentIdentity) -> Result<MobMemberSnapshot, MobError> {
        let meerkat_id = MeerkatId::from(identity);
        let material = self.wait_one_material(&meerkat_id).await?;
        Ok(material.to_snapshot())
    }

    /// Wait for all specified members to reach terminal states.
    pub async fn wait_all(
        &self,
        identities: &[AgentIdentity],
    ) -> Result<Vec<MobMemberSnapshot>, MobError> {
        let meerkat_ids: Vec<MeerkatId> = identities.iter().map(MeerkatId::from).collect();
        let futs = meerkat_ids
            .iter()
            .map(|mid| self.wait_one_material(mid))
            .collect::<Vec<_>>();
        let results = futures::future::join_all(futs).await;
        results
            .into_iter()
            .map(|result| result.map(|material| material.to_snapshot()))
            .collect()
    }

    /// Collect snapshots for all members that have reached terminal states.
    pub async fn collect_completed(&self) -> Vec<(AgentIdentity, MobMemberSnapshot)> {
        let entries = self.list_all_members().await;
        let mut completed = Vec::new();
        for entry in entries {
            if let Ok(snapshot) = self.member_status(&entry.agent_identity).await
                && snapshot.is_final
            {
                completed.push((entry.agent_identity, snapshot));
            }
        }
        completed
    }

    /// Spawn a fresh helper, wait for it to complete, retire it, and return its result.
    ///
    /// Helpers are short-lived TurnDriven tasks by default. Their completion
    /// truth is the spawn/create boundary plus the canonical post-spawn member
    /// snapshot, not full member terminality in the mob lifecycle.
    pub async fn spawn_helper(
        &self,
        identity: AgentIdentity,
        task: impl Into<String>,
        options: HelperOptions,
    ) -> Result<HelperResult, MobError> {
        let profile_name = options
            .role_name
            .or_else(|| self.definition.profiles.keys().next().cloned())
            .ok_or_else(|| {
                MobError::Internal("no profile specified and definition has no profiles".into())
            })?;
        let task_text = task.into();
        let meerkat_id = MeerkatId::from(&identity);
        let mut spec = SpawnMemberSpec::new(profile_name, identity.clone());
        spec.initial_message = Some(task_text.into());
        spec.runtime_mode = Some(
            options
                .runtime_mode
                .unwrap_or(crate::MobRuntimeMode::TurnDriven),
        );
        spec.backend = options.backend;
        spec.tool_access_policy = options.tool_access_policy;
        spec.auto_wire_parent = true;

        self.spawn_spec(spec).await?;
        let helper_material = self.canonical_member_list_material(&meerkat_id).await;
        let _ = self.retire(identity).await;

        Ok(helper_material.to_helper_result())
    }

    /// Fork from an existing member's context, wait for completion, retire, and return.
    ///
    /// Like `spawn_helper` but uses `MemberLaunchMode::Fork` to share
    /// conversation context with the source member.
    pub async fn fork_helper(
        &self,
        source_identity: &AgentIdentity,
        identity: AgentIdentity,
        task: impl Into<String>,
        fork_context: crate::launch::ForkContext,
        options: HelperOptions,
    ) -> Result<HelperResult, MobError> {
        let profile_name = options
            .role_name
            .or_else(|| self.definition.profiles.keys().next().cloned())
            .ok_or_else(|| {
                MobError::Internal("no profile specified and definition has no profiles".into())
            })?;
        let task_text = task.into();
        let meerkat_id = MeerkatId::from(&identity);
        let source_member_id = MeerkatId::from(source_identity);
        let mut spec = SpawnMemberSpec::new(profile_name, identity.clone());
        spec.initial_message = Some(task_text.into());
        spec.runtime_mode = Some(
            options
                .runtime_mode
                .unwrap_or(crate::MobRuntimeMode::TurnDriven),
        );
        spec.backend = options.backend;
        spec.tool_access_policy = options.tool_access_policy;
        spec.auto_wire_parent = true;
        spec.launch_mode = crate::launch::MemberLaunchMode::Fork {
            source_member_id,
            fork_context,
        };

        self.spawn_spec(spec).await?;
        let helper_material = self.canonical_member_list_material(&meerkat_id).await;
        let _ = self.retire(identity).await;

        Ok(helper_material.to_helper_result())
    }
}

impl MemberHandle {
    /// Target member identity.
    pub fn identity(&self) -> AgentIdentity {
        AgentIdentity::from(self.agent_identity.as_str())
    }

    /// Submit external work to this member through the canonical runtime path.
    pub async fn send(
        &self,
        content: impl Into<meerkat_core::types::ContentInput>,
        handling_mode: HandlingMode,
    ) -> Result<MemberDeliveryReceipt, MobError> {
        self.send_with_render_metadata(content, handling_mode, None)
            .await
    }

    /// Submit external work with explicit normalized render metadata.
    pub async fn send_with_render_metadata(
        &self,
        content: impl Into<meerkat_core::types::ContentInput>,
        handling_mode: HandlingMode,
        render_metadata: Option<RenderMetadata>,
    ) -> Result<MemberDeliveryReceipt, MobError> {
        self.mob
            .external_turn_for_member(
                self.agent_identity.clone(),
                content.into(),
                handling_mode,
                render_metadata,
            )
            .await?;
        let material = self
            .mob
            .canonical_member_list_material(&self.agent_identity)
            .await;
        Ok(MemberDeliveryReceipt {
            identity: self.identity(),
            agent_runtime_id: material.agent_runtime_id,
            fence_token: material.fence_token,
            handling_mode,
        })
    }

    /// Submit internal work to this member without external addressability checks.
    pub async fn internal_turn(
        &self,
        content: impl Into<meerkat_core::types::ContentInput>,
    ) -> Result<MemberDeliveryReceipt, MobError> {
        self.mob
            .internal_turn_for_member(self.agent_identity.clone(), content.into())
            .await?;
        let material = self
            .mob
            .canonical_member_list_material(&self.agent_identity)
            .await;
        Ok(MemberDeliveryReceipt {
            identity: self.identity(),
            agent_runtime_id: material.agent_runtime_id,
            fence_token: material.fence_token,
            handling_mode: HandlingMode::Queue,
        })
    }

    /// Current bridge session ID for this member, if a session bridge exists.
    #[cfg(test)]
    pub(crate) async fn current_bridge_session_id(&self) -> Result<Option<SessionId>, MobError> {
        let status = self.status().await?;
        Ok(status.current_bridge_session_id().cloned())
    }

    /// Get a point-in-time execution snapshot for this member.
    pub async fn status(&self) -> Result<MobMemberSnapshot, MobError> {
        self.mob.member_status(&self.identity()).await
    }

    /// Subscribe to this member's agent events.
    pub async fn events(&self) -> Result<EventStream, MobError> {
        self.mob.subscribe_agent_events(&self.identity()).await
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::ids::Generation;

    #[test]
    fn member_projection_types_omit_bridge_session_fields_in_serialized_output() {
        let sid = SessionId::new();

        let snapshot = MobMemberSnapshot {
            status: MobMemberStatus::Active,
            agent_runtime_id: AgentRuntimeId::initial(AgentIdentity::from("worker")),
            fence_token: FenceToken::new(0),
            output_preview: None,
            error: None,
            tokens_used: 0,
            is_final: false,
            realtime_attachment_status: None,
            current_session_id: None,
            current_bridge_session_id: None,
            peer_connectivity: None,
            kickoff: None,
        }
        .with_current_bridge_session_id(Some(sid.clone()));
        let snapshot_value =
            serde_json::to_value(&snapshot).expect("snapshot should serialize to json");
        // 0.6 clean break: session fields are #[serde(skip)] and must not appear
        assert!(snapshot_value.get("current_bridge_session_id").is_none());
        // Identity-native fields must be present
        assert!(!snapshot_value["agent_runtime_id"].is_null());
        assert!(!snapshot_value["fence_token"].is_null());
    }

    #[test]
    fn mob_member_snapshot_exposes_agent_identity_convenience() {
        // Regression for DELETE_ME C9: every consumer used to reach through
        // `snapshot.agent_runtime_id.identity`; the snapshot now exposes a
        // direct accessor.
        let snapshot = MobMemberSnapshot {
            status: MobMemberStatus::Active,
            agent_runtime_id: AgentRuntimeId::initial(AgentIdentity::from("singer")),
            fence_token: FenceToken::new(0),
            output_preview: None,
            error: None,
            tokens_used: 0,
            is_final: false,
            realtime_attachment_status: None,
            current_session_id: None,
            current_bridge_session_id: None,
            peer_connectivity: None,
            kickoff: None,
        };
        assert_eq!(
            snapshot.agent_identity(),
            &AgentIdentity::from("singer"),
            "agent_identity() must return the canonical identity without requiring callers to reach through agent_runtime_id",
        );
    }

    #[test]
    fn canonical_member_material_populates_bridge_binding_from_canonical_state() {
        let sid = SessionId::new();
        let snapshot = CanonicalMemberSnapshotMaterial {
            member_present: true,
            status: CanonicalMemberStatus::Active,
            session_observation: CanonicalSessionObservation::Active,
            error: None,
            output_preview: None,
            tokens_used: 0,
            agent_runtime_id: AgentRuntimeId::initial(AgentIdentity::from("worker")),
            fence_token: FenceToken::new(0),
            current_bridge_session_id: Some(sid.clone()),
            peer_connectivity: None,
            kickoff: None,
        }
        .to_snapshot();

        assert_eq!(snapshot.current_bridge_session_id(), Some(&sid));
        assert_eq!(snapshot.current_bridge_session_id, Some(sid));
    }

    #[test]
    fn member_receipt_types_omit_bridge_session_fields_in_serialized_output() {
        let runtime_id = AgentRuntimeId::new(AgentIdentity::from("worker"), Generation::new(1));
        let receipt = MemberRespawnReceipt::new(
            AgentIdentity::from("worker"),
            runtime_id.clone(),
            FenceToken::new(7),
            FenceToken::new(8),
        );
        let receipt_value =
            serde_json::to_value(&receipt).expect("respawn receipt should serialize to json");
        assert_eq!(receipt_value["identity"], "worker");
        assert_eq!(
            receipt_value["agent_runtime_id"],
            serde_json::to_value(&runtime_id).expect("runtime id should serialize to json")
        );
        assert_eq!(receipt_value["previous_fence_token"], 7);
        assert_eq!(receipt_value["fence_token"], 8);

        let delivery = MemberDeliveryReceipt {
            identity: AgentIdentity::from("worker"),
            agent_runtime_id: runtime_id,
            fence_token: FenceToken::new(8),
            handling_mode: HandlingMode::Queue,
        };
        let delivery_value =
            serde_json::to_value(&delivery).expect("delivery receipt should serialize to json");
        assert_eq!(delivery_value["identity"], "worker");
        assert_eq!(delivery_value["fence_token"], 8);
    }

    #[test]
    fn helper_result_serializes_identity_native_runtime_fields() {
        let runtime_id = AgentRuntimeId::new(AgentIdentity::from("worker"), Generation::new(2));
        let result = HelperResult {
            output: Some("done".to_string()),
            tokens_used: 7,
            agent_identity: AgentIdentity::from("worker"),
            agent_runtime_id: runtime_id.clone(),
            fence_token: FenceToken::new(9),
        };

        let value = serde_json::to_value(&result).expect("helper result should serialize to json");
        assert_eq!(value["agent_identity"], "worker");
        assert_eq!(
            value["agent_runtime_id"],
            serde_json::to_value(&runtime_id).expect("runtime id should serialize to json")
        );
        assert_eq!(value["fence_token"], 9);
        assert!(value.get("session_id").is_none());
        assert!(value.get("bridge_session_id").is_none());
    }

    #[test]
    fn spawn_member_spec_resume_bridge_session_accessors_stay_additive() {
        let sid = SessionId::new();
        let spec =
            SpawnMemberSpec::new("worker", "worker-1").with_resume_bridge_session_id(sid.clone());

        assert_eq!(spec.launch_mode.resume_bridge_session_id(), Some(&sid));
        assert_eq!(spec.launch_mode.resume_bridge_session_id(), Some(&sid));
    }
}
