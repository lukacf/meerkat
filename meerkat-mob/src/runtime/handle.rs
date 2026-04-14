use super::*;
use crate::MobRuntimeMode;
use crate::mob_machine::{MobMachineCommand, MobMachineCommandResult};
use crate::roster::MobMemberKickoffSnapshot;
use crate::runtime::mob_member_lifecycle_authority::{
    CanonicalMemberSnapshotMaterial, CanonicalMemberStatus, CanonicalSessionObservation,
    MobMemberLifecycleAuthority, MobMemberLifecycleInput, MobMemberTerminalClass,
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
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct MobMemberSnapshot {
    /// Current lifecycle status.
    pub status: MobMemberStatus,
    /// Identity-native runtime ID for this incarnation.
    pub agent_runtime_id: AgentRuntimeId,
    /// Fence token for the current incarnation.
    pub fence_token: FenceToken,
    /// Preview of the last assistant output (if any).
    pub output_preview: Option<String>,
    /// Error description (if the member errored).
    pub error: Option<String>,
    /// Cumulative token usage.
    pub tokens_used: u64,
    /// Whether the member has reached a terminal state.
    pub is_final: bool,
    /// Bridge-internal session binding — compatibility alias for the current bridge session.
    #[serde(skip)]
    pub(crate) current_session_id: Option<SessionId>,
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
}

#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
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
    #[serde(skip)]
    pub(crate) meerkat_id: MeerkatId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) peer_id: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) external_peer_specs: BTreeMap<MeerkatId, TrustedPeerSpec>,
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
    /// Member has no current session bridge to retire.
    #[error("no current session bridge for member {identity}")]
    NoSessionBridge { identity: AgentIdentity },

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

impl From<MeerkatId> for PeerTarget {
    fn from(value: MeerkatId) -> Self {
        Self::Local(AgentIdentity::from(value.as_str()))
    }
}

impl From<AgentIdentity> for PeerTarget {
    fn from(value: AgentIdentity) -> Self {
        Self::Local(value)
    }
}

struct MobMemberTerminalClassifier;

impl MobMemberTerminalClassifier {
    fn classify(material: &CanonicalMemberSnapshotMaterial) -> MobMemberTerminalClass {
        if !material.member_present {
            return MobMemberTerminalClass::TerminalUnknown;
        }
        match material.status {
            // Retiring remains non-terminal while canonical roster membership
            // still exists, even if the session read is already inactive/missing.
            CanonicalMemberStatus::Retiring => MobMemberTerminalClass::Running,
            CanonicalMemberStatus::Broken => MobMemberTerminalClass::TerminalFailure,
            CanonicalMemberStatus::Active => match material.session_observation {
                CanonicalSessionObservation::Active
                | CanonicalSessionObservation::Inactive
                | CanonicalSessionObservation::Unknown => MobMemberTerminalClass::Running,
                CanonicalSessionObservation::Missing => MobMemberTerminalClass::TerminalCompleted,
            },
            CanonicalMemberStatus::Completed => MobMemberTerminalClass::TerminalCompleted,
            CanonicalMemberStatus::Unknown => MobMemberTerminalClass::TerminalUnknown,
        }
    }

    fn is_terminal(material: &CanonicalMemberSnapshotMaterial) -> bool {
        matches!(
            Self::classify(material),
            MobMemberTerminalClass::TerminalFailure
                | MobMemberTerminalClass::TerminalUnknown
                | MobMemberTerminalClass::TerminalCompleted
        )
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
    pub(super) state: Arc<AtomicU8>,
    pub(super) events: Arc<dyn MobEventStore>,
    pub(super) flow_streams:
        Arc<tokio::sync::Mutex<BTreeMap<RunId, mpsc::Sender<meerkat_core::ScopedAgentEvent>>>>,
    pub(super) session_service: Arc<dyn MobSessionService>,
    pub(super) restore_diagnostics: Arc<RwLock<HashMap<MeerkatId, RestoreFailureDiagnostic>>>,
}

#[derive(Debug, Clone)]
pub(crate) struct RestoreFailureDiagnostic {
    pub(crate) bridge_session_id: SessionId,
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
    meerkat_id: MeerkatId,
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
    pub(crate) launch_mode: crate::launch::MemberLaunchMode,
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
        }
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
    pub(crate) fn with_resume_bridge_session_id(
        mut self,
        id: meerkat_core::types::SessionId,
    ) -> Self {
        self.launch_mode = crate::launch::MemberLaunchMode::Resume {
            bridge_session_id: id,
        };
        self
    }

    pub(crate) fn with_launch_mode(mut self, mode: crate::launch::MemberLaunchMode) -> Self {
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
        meerkat_id: &MeerkatId,
    ) -> Option<RestoreFailureDiagnostic> {
        self.restore_diagnostics
            .read()
            .await
            .get(meerkat_id)
            .cloned()
    }

    fn restore_failure_error(meerkat_id: &MeerkatId, diag: RestoreFailureDiagnostic) -> MobError {
        MobError::MemberRestoreFailed {
            member_id: meerkat_id.clone(),
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
            MobMachineCommand::Retire { meerkat_id } => {
                self.send_actor_command(|reply_tx| MobCommand::Retire {
                    meerkat_id,
                    reply_tx,
                })
                .await??;
                Ok(MobMachineCommandResult::Unit)
            }
            MobMachineCommand::Respawn {
                meerkat_id,
                initial_message,
            } => {
                let receipt = self
                    .send_actor_command(|reply_tx| MobCommand::Respawn {
                        meerkat_id,
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
            MobMachineCommand::ExternalTurn {
                meerkat_id,
                content,
                handling_mode,
                render_metadata,
            } => {
                let bridge_session_id = self
                    .send_actor_command(|reply_tx| MobCommand::ExternalTurn {
                        meerkat_id,
                        content,
                        handling_mode,
                        render_metadata,
                        reply_tx,
                    })
                    .await??;
                Ok(MobMachineCommandResult::BridgeSessionId(bridge_session_id))
            }
            MobMachineCommand::InternalTurn {
                meerkat_id,
                content,
            } => {
                let bridge_session_id = self
                    .send_actor_command(|reply_tx| MobCommand::InternalTurn {
                        meerkat_id,
                        content,
                        reply_tx,
                    })
                    .await??;
                Ok(MobMachineCommandResult::BridgeSessionId(bridge_session_id))
            }
            MobMachineCommand::SubmitWork {
                runtime_id,
                fence_token,
                work_ref,
                spec,
            } => {
                let meerkat_id = MeerkatId::from(runtime_id.identity.as_str());
                let entry = self
                    .roster
                    .read()
                    .await
                    .entry(&meerkat_id)
                    .ok_or_else(|| MobError::MeerkatNotFound(meerkat_id.clone()))?;
                if entry.fence_token != fence_token {
                    return Err(MobError::StaleFenceToken {
                        runtime_id,
                        expected: entry.fence_token,
                        actual: fence_token,
                    });
                }
                let content = meerkat_core::types::ContentInput::from(spec.content);
                let bridge_session_id = match spec.origin {
                    WorkOrigin::External => {
                        self.send_actor_command(|reply_tx| MobCommand::ExternalTurn {
                            meerkat_id,
                            content,
                            handling_mode: HandlingMode::Queue,
                            render_metadata: None,
                            reply_tx,
                        })
                        .await??
                    }
                    WorkOrigin::Internal => {
                        self.send_actor_command(|reply_tx| MobCommand::InternalTurn {
                            meerkat_id,
                            content,
                            reply_tx,
                        })
                        .await??
                    }
                };
                let _ = bridge_session_id;
                Ok(MobMachineCommandResult::WorkReceipt { work_ref })
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
                let meerkat_id = MeerkatId::from(runtime_id.identity.as_str());
                let entry = self
                    .roster
                    .read()
                    .await
                    .entry(&meerkat_id)
                    .ok_or_else(|| MobError::MeerkatNotFound(meerkat_id.clone()))?;
                if entry.fence_token != fence_token {
                    return Err(MobError::StaleFenceToken {
                        runtime_id,
                        expected: entry.fence_token,
                        actual: fence_token,
                    });
                }
                self.send_actor_command(|reply_tx| MobCommand::ForceCancel {
                    meerkat_id,
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
                self.send_actor_command(|reply_tx| MobCommand::Destroy { reply_tx })
                    .await??;
                Ok(MobMachineCommandResult::Unit)
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
            MobMachineCommand::MemberStatus { meerkat_id } => {
                let snapshot = self.canonical_member_snapshot_material(&meerkat_id).await;
                Ok(MobMachineCommandResult::MemberStatus(
                    snapshot.to_snapshot(),
                ))
            }
            MobMachineCommand::SubscribeAgentEvents { meerkat_id } => {
                let stream = self
                    .send_actor_command(|reply_tx| MobCommand::SubscribeAgentEvents {
                        meerkat_id,
                        reply_tx,
                    })
                    .await??;
                Ok(MobMachineCommandResult::EventStream(stream))
            }
            MobMachineCommand::SubscribeAllAgentEvents => {
                let streams = self
                    .send_actor_command(|reply_tx| MobCommand::SubscribeAllAgentEvents { reply_tx })
                    .await?;
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
                let events = if self.status() == MobState::Destroyed {
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
                let events = if self.status() == MobState::Destroyed {
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
            MobMachineCommand::GetMember { meerkat_id } => {
                let member = self.roster.read().await.entry(&meerkat_id);
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
            MobMachineCommand::ForceCancel { meerkat_id } => {
                self.send_actor_command(|reply_tx| MobCommand::ForceCancel {
                    meerkat_id,
                    reply_tx,
                })
                .await??;
                Ok(MobMachineCommandResult::Unit)
            }
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

    /// Current mob lifecycle state (lock-free read).
    pub fn status(&self) -> MobState {
        MobState::from_u8(self.state.load(Ordering::Acquire))
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
        format!("{}/{}/{}", self.definition.id, entry.role, entry.meerkat_id)
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
            let wired_peer_meerkat = MeerkatId::from(wired_peer.as_str());
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
                .canonical_member_list_material(&entry.meerkat_id)
                .await
                .to_snapshot();
            let current_bridge_session_id = snapshot.current_bridge_session_id().cloned();
            projected.push(
                MobMemberListEntry {
                    agent_identity: entry.agent_identity,
                    agent_runtime_id: entry.agent_runtime_id,
                    fence_token: entry.fence_token,
                    role: entry.role,
                    meerkat_id: entry.meerkat_id,
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
        meerkat_id: &MeerkatId,
    ) -> CanonicalMemberSnapshotMaterial {
        self.canonical_member_material(
            meerkat_id,
            PeerConnectivityProjection::Omit,
            SessionObservationProjection::Omit,
        )
        .await
    }

    async fn canonical_member_snapshot_material(
        &self,
        meerkat_id: &MeerkatId,
    ) -> CanonicalMemberSnapshotMaterial {
        self.canonical_member_material(
            meerkat_id,
            PeerConnectivityProjection::Include,
            SessionObservationProjection::Full,
        )
        .await
    }

    async fn canonical_member_material(
        &self,
        meerkat_id: &MeerkatId,
        connectivity: PeerConnectivityProjection,
        observation: SessionObservationProjection,
    ) -> CanonicalMemberSnapshotMaterial {
        let (roster_snapshot, roster_entry, roster_state, current_bridge_session_id) = {
            let roster = self.roster.read().await;
            match roster.get(meerkat_id) {
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
                .get(meerkat_id)
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
                        AgentRuntimeId::initial(AgentIdentity::from(meerkat_id.as_str()))
                    }),
                fence_token: roster_entry
                    .as_ref()
                    .map(|e| e.fence_token)
                    .unwrap_or(FenceToken::new(0)),
                current_bridge_session_id: Some(diag.bridge_session_id),
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
                agent_runtime_id: AgentRuntimeId::initial(AgentIdentity::from(meerkat_id.as_str())),
                fence_token: FenceToken::new(0),
                current_bridge_session_id: None,
                peer_connectivity: None,
                kickoff: None,
            }),
            (Some(roster_state), None) => {
                MobMemberLifecycleAuthority::materialize(MobMemberLifecycleInput {
                    member_present: true,
                    roster_state: Some(roster_state),
                    session_observation: CanonicalSessionObservation::Missing,
                    restore_failure: None,
                    output_preview: None,
                    tokens_used: 0,
                    agent_runtime_id: roster_entry
                        .as_ref()
                        .map(|e| e.agent_runtime_id.clone())
                        .unwrap_or_else(|| {
                            AgentRuntimeId::initial(AgentIdentity::from(meerkat_id.as_str()))
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
                            AgentRuntimeId::initial(AgentIdentity::from(meerkat_id.as_str()))
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
        let meerkat_id = MeerkatId::from(identity.as_str());
        match self
            .execute_machine_command(MobMachineCommand::GetMember { meerkat_id })
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
        meerkat_id: &MeerkatId,
    ) -> Option<RosterEntry> {
        self.get_member(&AgentIdentity::from(meerkat_id.as_str()))
            .await
    }

    /// Resolve the backing bridge session ID for a member by identity.
    ///
    /// This is an internal routing helper for surfaces that need to call
    /// `SessionService` methods on a member's backing session. Returns `None`
    /// if the member is not found or has no bridge session binding.
    #[doc(hidden)]
    pub async fn resolve_bridge_session_id(&self, identity: &AgentIdentity) -> Option<SessionId> {
        self.get_member(identity)
            .await
            .and_then(|entry| entry.member_ref.bridge_session_id().cloned())
    }

    /// Acquire a capability-bearing handle for a specific active member.
    pub async fn member(&self, identity: &AgentIdentity) -> Result<MemberHandle, MobError> {
        let meerkat_id = MeerkatId::from(identity.as_str());
        if let Some(diag) = self.restore_failure_for(&meerkat_id).await {
            return Err(Self::restore_failure_error(&meerkat_id, diag));
        }
        let entry = self
            .get_member(identity)
            .await
            .ok_or_else(|| MobError::MeerkatNotFound(meerkat_id.clone()))?;
        if entry.state != crate::roster::MemberState::Active {
            return Err(MobError::MeerkatNotFound(meerkat_id.clone()));
        }
        Ok(MemberHandle {
            mob: self.clone(),
            meerkat_id,
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
    /// Returns `MobError::MeerkatNotFound` if the member is not in the
    /// roster or has no backing bridge session.
    pub async fn subscribe_agent_events(
        &self,
        identity: &AgentIdentity,
    ) -> Result<EventStream, MobError> {
        match self
            .execute_machine_command(MobMachineCommand::SubscribeAgentEvents {
                meerkat_id: MeerkatId::from(identity.as_str()),
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
    pub async fn subscribe_all_agent_events(&self) -> Vec<(AgentIdentity, EventStream)> {
        match self
            .execute_machine_command(MobMachineCommand::SubscribeAllAgentEvents)
            .await
        {
            Ok(MobMachineCommandResult::AllAgentEventStreams(streams)) => streams
                .into_iter()
                .map(|(mid, stream)| (AgentIdentity::from(mid.as_str()), stream))
                .collect(),
            Ok(_) => {
                tracing::error!("unexpected command result variant");
                Default::default()
            }
            Err(_) => Vec::new(),
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
        meerkat_id: MeerkatId,
        initial_message: Option<ContentInput>,
    ) -> Result<MemberRef, MobError> {
        self.spawn_with_options(profile_name, meerkat_id, initial_message, None, None)
            .await
    }

    /// Spawn a new member with an explicit runtime binding.
    #[cfg(test)]
    pub(crate) async fn spawn_with_binding(
        &self,
        profile_name: ProfileName,
        meerkat_id: MeerkatId,
        initial_message: Option<ContentInput>,
        binding: crate::RuntimeBinding,
    ) -> Result<MemberRef, MobError> {
        let mut spec = SpawnMemberSpec::new(profile_name, meerkat_id);
        spec.initial_message = initial_message;
        spec.binding = Some(binding);
        self.spawn_spec_internal(spec).await
    }

    /// Spawn a new member from a profile with explicit backend override.
    #[cfg(test)]
    pub(crate) async fn spawn_with_backend(
        &self,
        profile_name: ProfileName,
        meerkat_id: MeerkatId,
        initial_message: Option<ContentInput>,
        backend: Option<MobBackendKind>,
    ) -> Result<MemberRef, MobError> {
        self.spawn_with_options(profile_name, meerkat_id, initial_message, None, backend)
            .await
    }

    /// Spawn a new member from a profile with explicit runtime mode/backend overrides.
    #[cfg(test)]
    pub(crate) async fn spawn_with_options(
        &self,
        profile_name: ProfileName,
        meerkat_id: MeerkatId,
        initial_message: Option<ContentInput>,
        runtime_mode: Option<crate::MobRuntimeMode>,
        backend: Option<MobBackendKind>,
    ) -> Result<MemberRef, MobError> {
        let mut spec = SpawnMemberSpec::new(profile_name, meerkat_id);
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
        meerkat_id: MeerkatId,
        session_id: meerkat_core::types::SessionId,
        runtime_mode: Option<crate::MobRuntimeMode>,
        backend: Option<MobBackendKind>,
    ) -> Result<MemberRef, MobError> {
        let mut spec = SpawnMemberSpec::new(profile_name, meerkat_id);
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
        meerkat_id: MeerkatId,
        session_id: meerkat_core::types::SessionId,
    ) -> Result<MemberRef, MobError> {
        self.attach_existing_session(profile_name, meerkat_id, session_id, None, None)
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
        let meerkat_id = MeerkatId::from(identity.as_str());
        match self
            .execute_machine_command(MobMachineCommand::Retire { meerkat_id })
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
        let meerkat_id = MeerkatId::from(identity.as_str());
        let reply = match self
            .execute_machine_command(MobMachineCommand::Respawn {
                meerkat_id,
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

    /// Wire a local member to either another local member or an external peer.
    pub async fn wire<T>(&self, local: AgentIdentity, target: T) -> Result<(), MobError>
    where
        T: Into<PeerTarget>,
    {
        match self
            .execute_machine_command(MobMachineCommand::Wire {
                local: MeerkatId::from(local.as_str()),
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
                local: MeerkatId::from(local.as_str()),
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
    pub async fn internal_turn(
        &self,
        identity: AgentIdentity,
        message: impl Into<meerkat_core::types::ContentInput>,
    ) -> Result<MemberDeliveryReceipt, MobError> {
        let meerkat_id = MeerkatId::from(identity.as_str());
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
        meerkat_id: MeerkatId,
        message: meerkat_core::types::ContentInput,
        handling_mode: HandlingMode,
        render_metadata: Option<RenderMetadata>,
    ) -> Result<meerkat_core::types::SessionId, MobError> {
        match self
            .execute_machine_command(MobMachineCommand::ExternalTurn {
                meerkat_id,
                content: message,
                handling_mode,
                render_metadata,
            })
            .await?
        {
            MobMachineCommandResult::BridgeSessionId(bridge_session_id) => Ok(bridge_session_id),
            _ => Err(MobError::Internal(
                "unexpected command result variant".into(),
            )),
        }
    }

    pub(super) async fn internal_turn_for_member(
        &self,
        meerkat_id: MeerkatId,
        message: meerkat_core::types::ContentInput,
    ) -> Result<SessionId, MobError> {
        match self
            .execute_machine_command(MobMachineCommand::InternalTurn {
                meerkat_id,
                content: message,
            })
            .await?
        {
            MobMachineCommandResult::BridgeSessionId(bridge_session_id) => Ok(bridge_session_id),
            _ => Err(MobError::Internal(
                "unexpected command result variant".into(),
            )),
        }
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
        match self
            .execute_machine_command(MobMachineCommand::SubmitWork {
                runtime_id: runtime_id.clone(),
                fence_token,
                work_ref: work_ref.clone(),
                spec,
            })
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
    /// Like `destroy()` but keeps the actor alive and transitions to `Running`
    /// instead of `Destroyed`. The handle remains usable after reset.
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
    pub async fn destroy(&self) -> Result<(), MobError> {
        match self
            .execute_machine_command(MobMachineCommand::Destroy)
            .await?
        {
            MobMachineCommandResult::Unit => Ok(()),
            _ => Err(MobError::Internal(
                "unexpected command result variant".into(),
            )),
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
                meerkat_id: MeerkatId::from(identity.as_str()),
            })
            .await?
        {
            MobMachineCommandResult::Unit => Ok(()),
            _ => Err(MobError::Internal(
                "unexpected command result variant".into(),
            )),
        }
    }

    fn kickoff_wait_is_satisfied(entry: &RosterEntry) -> bool {
        if entry.runtime_mode != crate::MobRuntimeMode::AutonomousHost {
            return true;
        }

        match entry.kickoff.as_ref().map(|kickoff| kickoff.phase) {
            None
            | Some(
                MobMemberKickoffPhase::Started
                | MobMemberKickoffPhase::Failed
                | MobMemberKickoffPhase::Cancelled,
            ) => true,
            Some(
                MobMemberKickoffPhase::Pending
                | MobMemberKickoffPhase::Starting
                | MobMemberKickoffPhase::CallbackPending,
            ) => false,
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
            let entries = self
                .list_all_members()
                .await
                .into_iter()
                .map(|entry| (entry.meerkat_id.clone(), entry))
                .collect::<HashMap<_, _>>();

            let pending_member_ids = target_ids
                .iter()
                .filter(|id| {
                    entries
                        .get(*id)
                        .is_some_and(|entry| !Self::kickoff_wait_is_satisfied(entry))
                })
                .cloned()
                .collect::<Vec<_>>();

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

    async fn wait_one_material(
        &self,
        meerkat_id: &MeerkatId,
    ) -> Result<CanonicalMemberSnapshotMaterial, MobError> {
        loop {
            let material = self.canonical_member_list_material(meerkat_id).await;
            if MobMemberTerminalClassifier::is_terminal(&material) {
                return Ok(material);
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }

    /// Get a point-in-time execution snapshot for a member.
    ///
    /// This is the deep inspection surface. Unlike list projections, it
    /// resolves live peer connectivity when a comms runtime is available.
    pub async fn member_status(
        &self,
        identity: &AgentIdentity,
    ) -> Result<MobMemberSnapshot, MobError> {
        match self
            .execute_machine_command(MobMachineCommand::MemberStatus {
                meerkat_id: MeerkatId::from(identity.as_str()),
            })
            .await?
        {
            MobMachineCommandResult::MemberStatus(snapshot) => Ok(snapshot),
            _ => Err(MobError::Internal(
                "unexpected command result variant".into(),
            )),
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
            .map(|entry| entry.meerkat_id)
            .collect::<Vec<_>>();
        let identities: Vec<AgentIdentity> = target_ids
            .iter()
            .map(|id| AgentIdentity::from(id.as_str()))
            .collect();
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
        let target_meerkat_ids: Vec<MeerkatId> =
            ids.iter().map(|id| MeerkatId::from(id.as_str())).collect();
        self.wait_for_kickoff_resolution(&target_meerkat_ids, timeout)
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
        let meerkat_id = MeerkatId::from(identity.as_str());
        let material = self.wait_one_material(&meerkat_id).await?;
        Ok(material.to_snapshot())
    }

    /// Wait for all specified members to reach terminal states.
    pub async fn wait_all(
        &self,
        identities: &[AgentIdentity],
    ) -> Result<Vec<MobMemberSnapshot>, MobError> {
        let meerkat_ids: Vec<MeerkatId> = identities
            .iter()
            .map(|id| MeerkatId::from(id.as_str()))
            .collect();
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
        let meerkat_id = MeerkatId::from(identity.as_str());
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
        let meerkat_id = MeerkatId::from(identity.as_str());
        let source_member_id = MeerkatId::from(source_identity.as_str());
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
        AgentIdentity::from(self.meerkat_id.as_str())
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
                self.meerkat_id.clone(),
                content.into(),
                handling_mode,
                render_metadata,
            )
            .await?;
        let material = self
            .mob
            .canonical_member_list_material(&self.meerkat_id)
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
            .internal_turn_for_member(self.meerkat_id.clone(), content.into())
            .await?;
        let material = self
            .mob
            .canonical_member_list_material(&self.meerkat_id)
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
