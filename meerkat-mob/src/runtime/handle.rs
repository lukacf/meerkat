use super::*;
use crate::MobRuntimeMode;
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use futures::stream::{FuturesUnordered, StreamExt};
use meerkat_core::comms::{
    PeerDirectoryEntry, PeerReachability, PeerReachabilityReason, TrustedPeerSpec,
};
use meerkat_core::ops::OperationId;
use meerkat_core::ops_lifecycle::OpsLifecycleRegistry;
use meerkat_core::service::SessionError;
use meerkat_core::types::{HandlingMode, RenderMetadata, SessionId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::time::Duration;

const DEFAULT_KICKOFF_WAIT_TIMEOUT: Duration = Duration::from_secs(600);

/// Point-in-time snapshot of a mob member's execution state.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct MobMemberSnapshot {
    /// Current lifecycle status.
    pub status: MobMemberStatus,
    /// Preview of the last assistant output (if any).
    pub output_preview: Option<String>,
    /// Error description (if the member errored).
    pub error: Option<String>,
    /// Cumulative token usage.
    pub tokens_used: u64,
    /// Whether the member has reached a terminal state.
    pub is_final: bool,
    /// Current session ID (if a session bridge exists).
    pub current_session_id: Option<SessionId>,
    /// Live comms connectivity for currently wired peers, when available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peer_connectivity: Option<MobPeerConnectivitySnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct MobMemberListEntry {
    pub meerkat_id: MeerkatId,
    pub profile: ProfileName,
    pub member_ref: MemberRef,
    pub runtime_mode: MobRuntimeMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_id: Option<String>,
    #[serde(default)]
    pub state: crate::roster::MemberState,
    pub wired_to: BTreeSet<MeerkatId>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub external_peer_specs: BTreeMap<MeerkatId, TrustedPeerSpec>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
    pub status: MobMemberStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub is_final: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_session_id: Option<SessionId>,
}

impl MobMemberListEntry {
    pub fn session_id(&self) -> Option<&SessionId> {
        self.member_ref.session_id()
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
    pub member_id: MeerkatId,
    /// Session ID of the retired (old) session.
    pub old_session_id: Option<SessionId>,
    /// Session ID of the newly spawned session.
    pub new_session_id: Option<SessionId>,
}

impl MemberRespawnReceipt {
    pub fn new(
        member_id: MeerkatId,
        old_session_id: Option<SessionId>,
        new_session_id: Option<SessionId>,
    ) -> Self {
        Self {
            member_id,
            old_session_id,
            new_session_id,
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

#[derive(Clone)]
pub(crate) struct CanonicalOpsOwnerContext {
    pub(crate) owner_session_id: SessionId,
    pub(crate) ops_registry: Arc<dyn OpsLifecycleRegistry>,
}

/// Structured error for direct-Rust respawn failures.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum MobRespawnError {
    /// Member has no current session bridge to retire.
    #[error("no current session bridge for member {member_id}")]
    NoSessionBridge { member_id: MeerkatId },

    /// Spawn failed after the old member was retired.
    #[error("spawn failed after retire for member {member_id}: {reason}")]
    SpawnAfterRetire {
        member_id: MeerkatId,
        reason: String,
    },

    /// Topology restore failed after replacement spawn.
    /// The replacement receipt is carried so callers can still use the new session.
    #[error("topology restore failed for member {}: {} peer(s) failed", receipt.member_id, failed_peer_ids.len())]
    TopologyRestoreFailed {
        receipt: MemberRespawnReceipt,
        failed_peer_ids: Vec<MeerkatId>,
    },

    /// An underlying mob error occurred before mutation.
    #[error(transparent)]
    Mob(#[from] MobError),
}

/// Receipt returned by member message delivery.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct MemberDeliveryReceipt {
    /// The member that received the message.
    pub member_id: MeerkatId,
    /// The session ID the message was delivered to.
    pub session_id: SessionId,
    /// How the message was handled.
    pub handling_mode: HandlingMode,
}

/// Reference to a member's current session bridge.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct MemberSessionRef {
    /// The member identity.
    pub member_id: MeerkatId,
    /// The current session ID.
    pub session_id: SessionId,
}

/// Options for helper convenience spawns.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct HelperOptions {
    /// Profile to use. If None, requires a default profile in the definition.
    pub profile_name: Option<ProfileName>,
    /// Runtime mode override.
    pub runtime_mode: Option<crate::MobRuntimeMode>,
    /// Backend override.
    pub backend: Option<MobBackendKind>,
    /// Tool access policy for the helper.
    pub tool_access_policy: Option<meerkat_core::ops::ToolAccessPolicy>,
}

/// Result from a helper spawn-and-wait operation.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct HelperResult {
    /// The member's final output text.
    pub output: Option<String>,
    /// Total tokens used by the helper.
    pub tokens_used: u64,
    /// The session ID that was used.
    pub session_id: Option<meerkat_core::types::SessionId>,
}

/// Target for a wire operation from a local mob member.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PeerTarget {
    /// Another member in the same mob roster.
    Local(MeerkatId),
    /// A trusted peer that lives outside the local mob roster.
    External(TrustedPeerSpec),
}

impl From<MeerkatId> for PeerTarget {
    fn from(value: MeerkatId) -> Self {
        Self::Local(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CanonicalMemberStatus {
    Unknown,
    Active,
    Retiring,
    Broken,
    Completed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CanonicalSessionObservation {
    Active,
    Inactive,
    Missing,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MobMemberTerminalClass {
    Running,
    TerminalFailure,
    TerminalUnknown,
    TerminalCompleted,
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

    fn has_canonical_member(material: &CanonicalMemberSnapshotMaterial) -> bool {
        material.member_present
    }
}

#[derive(Debug, Clone)]
struct CanonicalMemberSnapshotMaterial {
    member_present: bool,
    status: CanonicalMemberStatus,
    session_observation: CanonicalSessionObservation,
    error: Option<String>,
    output_preview: Option<String>,
    tokens_used: u64,
    current_session_id: Option<SessionId>,
    peer_connectivity: Option<MobPeerConnectivitySnapshot>,
}

impl CanonicalMemberSnapshotMaterial {
    fn to_snapshot(&self) -> MobMemberSnapshot {
        let status = match self.status {
            CanonicalMemberStatus::Unknown => MobMemberStatus::Unknown,
            CanonicalMemberStatus::Active => MobMemberStatus::Active,
            CanonicalMemberStatus::Retiring => MobMemberStatus::Retiring,
            CanonicalMemberStatus::Broken => MobMemberStatus::Broken,
            CanonicalMemberStatus::Completed => MobMemberStatus::Completed,
        };
        let is_final = MobMemberTerminalClassifier::is_terminal(self);
        MobMemberSnapshot {
            status,
            output_preview: self.output_preview.clone(),
            error: self.error.clone(),
            tokens_used: self.tokens_used,
            is_final,
            current_session_id: self.current_session_id.clone(),
            peer_connectivity: self.peer_connectivity.clone(),
        }
    }

    fn to_helper_result(&self) -> HelperResult {
        HelperResult {
            output: self.output_preview.clone(),
            tokens_used: self.tokens_used,
            session_id: self.current_session_id.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// MobHandle
// ---------------------------------------------------------------------------

/// Clone-cheap, thread-safe handle for interacting with a running mob.
///
/// All mutation commands are sent through an mpsc channel to the actor.
/// Read-only operations (roster, state) bypass the actor and read from
/// shared `Arc` state directly.
#[derive(Clone)]
pub struct MobHandle {
    pub(super) command_tx: mpsc::Sender<MobCommand>,
    pub(super) roster: Arc<RwLock<RosterAuthority>>,
    pub(super) task_board: Arc<RwLock<TaskBoard>>,
    pub(super) definition: Arc<MobDefinition>,
    pub(super) state: Arc<AtomicU8>,
    pub(super) events: Arc<dyn MobEventStore>,
    pub(super) mcp_servers: Arc<tokio::sync::Mutex<BTreeMap<String, actor::McpServerEntry>>>,
    pub(super) flow_streams:
        Arc<tokio::sync::Mutex<BTreeMap<RunId, mpsc::Sender<meerkat_core::ScopedAgentEvent>>>>,
    pub(super) session_service: Arc<dyn MobSessionService>,
    pub(super) restore_diagnostics: Arc<RwLock<HashMap<MeerkatId, RestoreFailureDiagnostic>>>,
}

#[derive(Debug, Clone)]
pub(crate) struct RestoreFailureDiagnostic {
    pub(crate) session_id: SessionId,
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
    inner: Arc<dyn MobEventStore>,
}

/// Spawn request for first-class batch member provisioning.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct SpawnMemberSpec {
    pub profile_name: ProfileName,
    pub meerkat_id: MeerkatId,
    pub initial_message: Option<ContentInput>,
    pub runtime_mode: Option<crate::MobRuntimeMode>,
    pub backend: Option<MobBackendKind>,
    /// Opaque application context passed through to the agent build pipeline.
    pub context: Option<serde_json::Value>,
    /// Application-defined labels for this member.
    pub labels: Option<std::collections::BTreeMap<String, String>>,
    /// How this member should be launched (fresh, resume, or fork).
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
}

impl SpawnMemberSpec {
    pub fn new(profile: impl Into<ProfileName>, meerkat_id: impl Into<MeerkatId>) -> Self {
        Self {
            profile_name: profile.into(),
            meerkat_id: meerkat_id.into(),
            initial_message: None,
            runtime_mode: None,
            backend: None,
            context: None,
            labels: None,
            launch_mode: crate::launch::MemberLaunchMode::Fresh,
            tool_access_policy: None,
            budget_split_policy: None,
            auto_wire_parent: false,
            additional_instructions: None,
            shell_env: None,
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

    /// Set launch mode to resume an existing session.
    ///
    /// This is a convenience method equivalent to setting
    /// `launch_mode = MemberLaunchMode::Resume { session_id }`.
    pub fn with_resume_session_id(mut self, id: meerkat_core::types::SessionId) -> Self {
        self.launch_mode = crate::launch::MemberLaunchMode::Resume { session_id: id };
        self
    }

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
        meerkat_id: String,
        initial_message: Option<ContentInput>,
        runtime_mode: Option<crate::MobRuntimeMode>,
        backend: Option<MobBackendKind>,
    ) -> Self {
        let mut spec = Self::new(profile, meerkat_id);
        spec.initial_message = initial_message;
        spec.runtime_mode = runtime_mode;
        spec.backend = backend;
        spec
    }

    /// Extract the resume session ID if the launch mode is `Resume`.
    pub fn resume_session_id(&self) -> Option<&meerkat_core::types::SessionId> {
        match &self.launch_mode {
            crate::launch::MemberLaunchMode::Resume { session_id } => Some(session_id),
            _ => None,
        }
    }
}

impl MobEventsView {
    pub async fn poll(
        &self,
        after_cursor: u64,
        limit: usize,
    ) -> Result<Vec<crate::event::MobEvent>, MobError> {
        self.inner.poll(after_cursor, limit).await
    }

    pub async fn replay_all(&self) -> Result<Vec<crate::event::MobEvent>, MobError> {
        self.inner.replay_all().await
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
            session_id: diag.session_id,
            reason: diag.reason,
        }
    }

    /// Poll mob events from the underlying store.
    pub async fn poll_events(
        &self,
        after_cursor: u64,
        limit: usize,
    ) -> Result<Vec<crate::event::MobEvent>, MobError> {
        self.events.poll(after_cursor, limit).await
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
        self.roster.read().await.snapshot()
    }

    fn derived_comms_name(&self, entry: &RosterEntry) -> String {
        format!(
            "{}/{}/{}",
            self.definition.id, entry.profile, entry.meerkat_id
        )
    }

    async fn resolve_peer_connectivity(
        &self,
        entry: &RosterEntry,
        session_id: &SessionId,
        roster_snapshot: &Roster,
    ) -> Option<MobPeerConnectivitySnapshot> {
        let comms = self.session_service.comms_runtime(session_id).await?;
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
            let matched = if let Some(spec) = entry.external_peer_specs.get(wired_peer) {
                peers_by_id
                    .get(spec.peer_id.as_str())
                    .copied()
                    .or_else(|| peers_by_name.get(spec.name.as_str()).copied())
            } else {
                let local_entry = roster_snapshot.get(wired_peer);
                let live_peer_id =
                    match local_entry.and_then(|peer_entry| peer_entry.member_ref.session_id()) {
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
    /// For low-level structural roster visibility without runtime projection,
    /// use [`list_all_members`](Self::list_all_members).
    pub async fn list_members(&self) -> Vec<MobMemberListEntry> {
        self.project_member_list(self.roster.read().await.list())
            .await
    }

    /// List all members including those in `Retiring` state, with full
    /// enriched status projection.
    ///
    /// Use this for observability surfaces (RPC, MCP) where in-flight
    /// retires should remain visible with their transitional state.
    pub async fn list_members_including_retiring(&self) -> Vec<MobMemberListEntry> {
        self.project_member_list(self.roster.read().await.list_all())
            .await
    }

    async fn project_member_list<'a>(
        &self,
        entries: impl Iterator<Item = &'a crate::roster::RosterEntry>,
    ) -> Vec<MobMemberListEntry> {
        let entries: Vec<_> = entries.cloned().collect();
        let mut projected = Vec::with_capacity(entries.len());
        for entry in entries {
            let snapshot = self.member_status(&entry.meerkat_id).await.ok();
            let (status, error, is_final, current_session_id) = match snapshot {
                Some(snapshot) => (
                    snapshot.status,
                    snapshot.error,
                    snapshot.is_final,
                    snapshot.current_session_id,
                ),
                None => (
                    MobMemberStatus::Unknown,
                    None,
                    true,
                    entry.session_id().cloned(),
                ),
            };
            projected.push(MobMemberListEntry {
                meerkat_id: entry.meerkat_id,
                profile: entry.profile,
                member_ref: entry.member_ref,
                runtime_mode: entry.runtime_mode,
                peer_id: entry.peer_id,
                state: entry.state,
                wired_to: entry.wired_to,
                external_peer_specs: entry.external_peer_specs,
                labels: entry.labels,
                status,
                error,
                is_final,
                current_session_id,
            });
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
        self.roster.read().await.list_all().cloned().collect()
    }

    /// Get a specific member entry.
    pub async fn get_member(&self, meerkat_id: &MeerkatId) -> Option<RosterEntry> {
        self.roster.read().await.get(meerkat_id).cloned()
    }

    /// Acquire a capability-bearing handle for a specific active member.
    pub async fn member(&self, meerkat_id: &MeerkatId) -> Result<MemberHandle, MobError> {
        if let Some(diag) = self.restore_failure_for(meerkat_id).await {
            return Err(Self::restore_failure_error(meerkat_id, diag));
        }
        let entry = self
            .get_member(meerkat_id)
            .await
            .ok_or_else(|| MobError::MeerkatNotFound(meerkat_id.clone()))?;
        if entry.state != crate::roster::MemberState::Active {
            return Err(MobError::MeerkatNotFound(meerkat_id.clone()));
        }
        Ok(MemberHandle {
            mob: self.clone(),
            meerkat_id: meerkat_id.clone(),
        })
    }

    /// Access a read-only events view for polling/replay.
    pub fn events(&self) -> MobEventsView {
        MobEventsView {
            inner: self.events.clone(),
        }
    }

    /// Subscribe to agent-level events for a specific meerkat.
    ///
    /// Looks up the meerkat's session ID from the roster, then subscribes
    /// to the session-level event stream via [`MobSessionService`].
    ///
    /// Returns `MobError::MeerkatNotFound` if the meerkat is not in the
    /// roster or has no session ID.
    pub async fn subscribe_agent_events(
        &self,
        meerkat_id: &MeerkatId,
    ) -> Result<EventStream, MobError> {
        let session_id = {
            let roster = self.roster.read().await;
            roster
                .session_id(meerkat_id)
                .cloned()
                .ok_or_else(|| MobError::MeerkatNotFound(meerkat_id.clone()))?
        };
        SessionService::subscribe_session_events(self.session_service.as_ref(), &session_id)
            .await
            .map_err(|e| {
                MobError::Internal(format!(
                    "failed to subscribe to agent events for '{meerkat_id}': {e}"
                ))
            })
    }

    /// Subscribe to agent events for all active members (point-in-time snapshot).
    ///
    /// Returns one stream per active member that has a session ID. Members
    /// spawned after this call are not included — use [`subscribe_mob_events`]
    /// for a continuously updated view.
    pub async fn subscribe_all_agent_events(&self) -> Vec<(MeerkatId, EventStream)> {
        let entries: Vec<_> = {
            let roster = self.roster.read().await;
            roster
                .list()
                .filter_map(|e| {
                    e.member_ref
                        .session_id()
                        .map(|sid| (e.meerkat_id.clone(), sid.clone()))
                })
                .collect()
        };
        let mut streams = Vec::with_capacity(entries.len());
        for (meerkat_id, session_id) in entries {
            if let Ok(stream) =
                SessionService::subscribe_session_events(self.session_service.as_ref(), &session_id)
                    .await
            {
                streams.push((meerkat_id, stream));
            }
        }
        streams
    }

    /// Subscribe to a continuously-updated, mob-level event bus.
    ///
    /// Spawns an independent task that merges per-member session streams,
    /// tags each event with [`AttributedEvent`], and tracks roster changes
    /// (spawns/retires) automatically. Drop the returned handle to stop
    /// the router.
    pub fn subscribe_mob_events(&self) -> super::event_router::MobEventRouterHandle {
        self.subscribe_mob_events_with_config(super::event_router::MobEventRouterConfig::default())
    }

    /// Like [`subscribe_mob_events`](Self::subscribe_mob_events) with explicit config.
    pub fn subscribe_mob_events_with_config(
        &self,
        config: super::event_router::MobEventRouterConfig,
    ) -> super::event_router::MobEventRouterHandle {
        super::event_router::spawn_event_router(
            self.session_service.clone(),
            self.events.clone(),
            self.roster.clone(),
            config,
        )
    }

    /// Snapshot of MCP server lifecycle state tracked by this runtime.
    pub async fn mcp_server_states(&self) -> BTreeMap<String, bool> {
        self.mcp_servers
            .lock()
            .await
            .iter()
            .map(|(name, entry)| (name.clone(), entry.running))
            .collect()
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
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(MobCommand::RunFlow {
                flow_id,
                activation_params: params,
                scoped_event_tx,
                reply_tx,
            })
            .await
            .map_err(|_| MobError::Internal("actor task dropped".into()))?;
        reply_rx
            .await
            .map_err(|_| MobError::Internal("actor reply dropped".into()))?
    }

    /// Request cancellation of an in-flight flow run.
    pub async fn cancel_flow(&self, run_id: RunId) -> Result<(), MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(MobCommand::CancelFlow { run_id, reply_tx })
            .await
            .map_err(|_| MobError::Internal("actor task dropped".into()))?;
        reply_rx
            .await
            .map_err(|_| MobError::Internal("actor reply dropped".into()))?
    }

    /// Fetch a flow run snapshot from the run store.
    pub async fn flow_status(&self, run_id: RunId) -> Result<Option<MobRun>, MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(MobCommand::FlowStatus { run_id, reply_tx })
            .await
            .map_err(|_| MobError::Internal("actor task dropped".into()))?;
        reply_rx
            .await
            .map_err(|_| MobError::Internal("actor reply dropped".into()))?
    }

    /// List all configured flow IDs in this mob definition.
    pub fn list_flows(&self) -> Vec<FlowId> {
        self.definition.flows.keys().cloned().collect()
    }

    /// Spawn a new member from a profile and return its member reference.
    pub async fn spawn(
        &self,
        profile_name: ProfileName,
        meerkat_id: MeerkatId,
        initial_message: Option<ContentInput>,
    ) -> Result<MemberRef, MobError> {
        self.spawn_with_options(profile_name, meerkat_id, initial_message, None, None)
            .await
    }

    /// Spawn a new member from a profile with explicit backend override.
    pub async fn spawn_with_backend(
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
    pub async fn spawn_with_options(
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
        self.spawn_spec(spec).await
    }

    /// Attach an existing session by reusing the mob spawn control-plane path.
    pub async fn attach_existing_session(
        &self,
        profile_name: ProfileName,
        meerkat_id: MeerkatId,
        session_id: meerkat_core::types::SessionId,
        runtime_mode: Option<crate::MobRuntimeMode>,
        backend: Option<MobBackendKind>,
    ) -> Result<MemberRef, MobError> {
        let mut spec = SpawnMemberSpec::new(profile_name, meerkat_id);
        spec.launch_mode = crate::launch::MemberLaunchMode::Resume { session_id };
        spec.runtime_mode = runtime_mode;
        spec.backend = backend;
        self.spawn_spec(spec).await
    }

    /// Attach an existing session as the mob orchestrator.
    pub async fn attach_existing_session_as_orchestrator(
        &self,
        profile_name: ProfileName,
        meerkat_id: MeerkatId,
        session_id: meerkat_core::types::SessionId,
    ) -> Result<MemberRef, MobError> {
        self.attach_existing_session(profile_name, meerkat_id, session_id, None, None)
            .await
    }

    /// Attach an existing session as a regular mob member.
    pub async fn attach_existing_session_as_member(
        &self,
        profile_name: ProfileName,
        meerkat_id: MeerkatId,
        session_id: meerkat_core::types::SessionId,
    ) -> Result<MemberRef, MobError> {
        self.attach_existing_session(profile_name, meerkat_id, session_id, None, None)
            .await
    }

    /// Spawn a member from a fully-specified [`SpawnMemberSpec`].
    pub async fn spawn_spec(&self, spec: SpawnMemberSpec) -> Result<MemberRef, MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(MobCommand::Spawn {
                spec: Box::new(spec),
                owner_session_id: None,
                ops_registry: None,
                reply_tx,
            })
            .await
            .map_err(|_| MobError::Internal("actor task dropped".into()))?;
        reply_rx
            .await
            .map_err(|_| MobError::Internal("actor reply dropped".into()))?
            .map(|receipt| receipt.member_ref)
    }

    pub(super) async fn spawn_spec_receipt_with_owner_context(
        &self,
        spec: SpawnMemberSpec,
        owner_context: CanonicalOpsOwnerContext,
    ) -> Result<MemberSpawnReceipt, MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(MobCommand::Spawn {
                spec: Box::new(spec),
                owner_session_id: Some(owner_context.owner_session_id),
                ops_registry: Some(owner_context.ops_registry),
                reply_tx,
            })
            .await
            .map_err(|_| MobError::Internal("actor task dropped".into()))?;
        reply_rx
            .await
            .map_err(|_| MobError::Internal("actor reply dropped".into()))?
    }

    /// Spawn multiple members in parallel.
    ///
    /// Results preserve input order.
    pub async fn spawn_many(
        &self,
        specs: Vec<SpawnMemberSpec>,
    ) -> Vec<Result<MemberRef, MobError>> {
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
    pub async fn retire(&self, meerkat_id: MeerkatId) -> Result<(), MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(MobCommand::Retire {
                meerkat_id,
                reply_tx,
            })
            .await
            .map_err(|_| MobError::Internal("actor task dropped".into()))?;
        reply_rx
            .await
            .map_err(|_| MobError::Internal("actor reply dropped".into()))?
    }

    /// Retire a member and respawn with the same profile, labels, wiring, and mode.
    ///
    /// This is a helper convenience over primitive mob behavior, not a
    /// machine-owned primitive. Returns a receipt on full success, or a
    /// structured error on failure. No rollback is attempted after retire.
    pub async fn respawn(
        &self,
        meerkat_id: MeerkatId,
        initial_message: Option<ContentInput>,
    ) -> Result<MemberRespawnReceipt, MobRespawnError> {
        let old_session_id_before = self
            .canonical_member_snapshot_material(&meerkat_id)
            .await
            .current_session_id;
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(MobCommand::Respawn {
                meerkat_id,
                initial_message,
                reply_tx,
            })
            .await
            .map_err(|_| MobError::Internal("actor task dropped".into()))?;
        let reply = reply_rx
            .await
            .map_err(|_| MobError::Internal("actor reply dropped".into()))?;
        let mut receipt = match reply {
            Ok(receipt) => receipt,
            Err(MobRespawnError::TopologyRestoreFailed {
                mut receipt,
                failed_peer_ids,
            }) => {
                if receipt.old_session_id.is_none() {
                    receipt.old_session_id = old_session_id_before;
                }
                let post_material = self
                    .canonical_member_snapshot_material(&receipt.member_id)
                    .await;
                if MobMemberTerminalClassifier::has_canonical_member(&post_material) {
                    receipt.new_session_id = post_material.current_session_id;
                }
                return Err(MobRespawnError::TopologyRestoreFailed {
                    receipt,
                    failed_peer_ids,
                });
            }
            Err(err) => return Err(err),
        };
        if receipt.old_session_id.is_none() {
            receipt.old_session_id = old_session_id_before;
        }
        let post_material = self
            .canonical_member_snapshot_material(&receipt.member_id)
            .await;
        if MobMemberTerminalClassifier::has_canonical_member(&post_material) {
            receipt.new_session_id = post_material.current_session_id;
        }
        Ok(receipt)
    }

    /// Retire all roster members concurrently in a single actor command.
    pub async fn retire_all(&self) -> Result<(), MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(MobCommand::RetireAll { reply_tx })
            .await
            .map_err(|_| MobError::Internal("actor task dropped".into()))?;
        reply_rx
            .await
            .map_err(|_| MobError::Internal("actor reply dropped".into()))?
    }

    /// Wire a local member to either another local member or an external peer.
    pub async fn wire<T>(&self, local: MeerkatId, target: T) -> Result<(), MobError>
    where
        T: Into<PeerTarget>,
    {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(MobCommand::Wire {
                local,
                target: target.into(),
                reply_tx,
            })
            .await
            .map_err(|_| MobError::Internal("actor task dropped".into()))?;
        reply_rx
            .await
            .map_err(|_| MobError::Internal("actor reply dropped".into()))?
    }

    /// Unwire a local member from either another local member or an external peer.
    pub async fn unwire<T>(&self, local: MeerkatId, target: T) -> Result<(), MobError>
    where
        T: Into<PeerTarget>,
    {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(MobCommand::Unwire {
                local,
                target: target.into(),
                reply_tx,
            })
            .await
            .map_err(|_| MobError::Internal("actor task dropped".into()))?;
        reply_rx
            .await
            .map_err(|_| MobError::Internal("actor reply dropped".into()))?
    }

    /// Compatibility wrapper for internal-turn submission.
    ///
    /// Prefer [`MobHandle::member`] plus [`MemberHandle::internal_turn`] for
    /// the target 0.5 API shape.
    pub async fn internal_turn(
        &self,
        meerkat_id: MeerkatId,
        message: impl Into<meerkat_core::types::ContentInput>,
    ) -> Result<MemberDeliveryReceipt, MobError> {
        let session_id = self
            .internal_turn_for_member(meerkat_id.clone(), message.into())
            .await?;
        Ok(MemberDeliveryReceipt {
            member_id: meerkat_id,
            session_id,
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
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(MobCommand::ExternalTurn {
                meerkat_id,
                content: message,
                handling_mode,
                render_metadata,
                reply_tx,
            })
            .await
            .map_err(|_| MobError::Internal("actor task dropped".into()))?;
        reply_rx
            .await
            .map_err(|_| MobError::Internal("actor reply dropped".into()))?
    }

    pub(super) async fn internal_turn_for_member(
        &self,
        meerkat_id: MeerkatId,
        message: meerkat_core::types::ContentInput,
    ) -> Result<SessionId, MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(MobCommand::InternalTurn {
                meerkat_id,
                content: message,
                reply_tx,
            })
            .await
            .map_err(|_| MobError::Internal("actor task dropped".into()))?;
        reply_rx
            .await
            .map_err(|_| MobError::Internal("actor reply dropped".into()))?
    }

    /// Transition Running -> Stopped. Mutation commands are rejected while stopped.
    pub async fn stop(&self) -> Result<(), MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(MobCommand::Stop { reply_tx })
            .await
            .map_err(|_| MobError::Internal("actor task dropped".into()))?;
        reply_rx
            .await
            .map_err(|_| MobError::Internal("actor reply dropped".into()))?
    }

    /// Transition Stopped -> Running.
    pub async fn resume(&self) -> Result<(), MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(MobCommand::ResumeLifecycle { reply_tx })
            .await
            .map_err(|_| MobError::Internal("actor task dropped".into()))?;
        reply_rx
            .await
            .map_err(|_| MobError::Internal("actor reply dropped".into()))?
    }

    /// Archive all members, emit MobCompleted, and transition to Completed.
    pub async fn complete(&self) -> Result<(), MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(MobCommand::Complete { reply_tx })
            .await
            .map_err(|_| MobError::Internal("actor task dropped".into()))?;
        reply_rx
            .await
            .map_err(|_| MobError::Internal("actor reply dropped".into()))?
    }

    /// Wipe all runtime state and transition back to `Running`.
    ///
    /// Like `destroy()` but keeps the actor alive and transitions to `Running`
    /// instead of `Destroyed`. The handle remains usable after reset.
    pub async fn reset(&self) -> Result<(), MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(MobCommand::Reset { reply_tx })
            .await
            .map_err(|_| MobError::Internal("actor task dropped".into()))?;
        reply_rx
            .await
            .map_err(|_| MobError::Internal("actor reply dropped".into()))?
    }

    /// Retire active members and clear persisted mob storage.
    pub async fn destroy(&self) -> Result<(), MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(MobCommand::Destroy { reply_tx })
            .await
            .map_err(|_| MobError::Internal("actor task dropped".into()))?;
        reply_rx
            .await
            .map_err(|_| MobError::Internal("actor reply dropped".into()))?
    }

    /// Create a task in the shared mob task board.
    pub async fn task_create(
        &self,
        subject: String,
        description: String,
        blocked_by: Vec<TaskId>,
    ) -> Result<TaskId, MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(MobCommand::TaskCreate {
                subject,
                description,
                blocked_by,
                reply_tx,
            })
            .await
            .map_err(|_| MobError::Internal("actor task dropped".into()))?;
        reply_rx
            .await
            .map_err(|_| MobError::Internal("actor reply dropped".into()))?
    }

    /// Update task status/owner in the shared mob task board.
    pub async fn task_update(
        &self,
        task_id: TaskId,
        status: TaskStatus,
        owner: Option<MeerkatId>,
    ) -> Result<(), MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(MobCommand::TaskUpdate {
                task_id,
                status,
                owner,
                reply_tx,
            })
            .await
            .map_err(|_| MobError::Internal("actor task dropped".into()))?;
        reply_rx
            .await
            .map_err(|_| MobError::Internal("actor reply dropped".into()))?
    }

    /// List tasks from the in-memory task board projection.
    pub async fn task_list(&self) -> Result<Vec<MobTask>, MobError> {
        Ok(self.task_board.read().await.list().cloned().collect())
    }

    /// Get a task by ID from the in-memory task board projection.
    pub async fn task_get(&self, task_id: &TaskId) -> Result<Option<MobTask>, MobError> {
        Ok(self.task_board.read().await.get(task_id).cloned())
    }

    #[cfg(test)]
    pub async fn debug_flow_tracker_counts(&self) -> Result<(usize, usize), MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(MobCommand::FlowTrackerCounts { reply_tx })
            .await
            .map_err(|_| MobError::Internal("actor task dropped".into()))?;
        reply_rx
            .await
            .map_err(|_| MobError::Internal("actor reply dropped".into()))
    }

    #[cfg(test)]
    pub async fn debug_orchestrator_snapshot(
        &self,
    ) -> Result<super::MobOrchestratorSnapshot, MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(MobCommand::OrchestratorSnapshot { reply_tx })
            .await
            .map_err(|_| MobError::Internal("actor task dropped".into()))?;
        reply_rx
            .await
            .map_err(|_| MobError::Internal("actor reply dropped".into()))
    }

    /// Set or clear the spawn policy for automatic member provisioning.
    ///
    /// When set, external turns targeting an unknown meerkat ID will
    /// consult the policy before returning `MeerkatNotFound`.
    pub async fn set_spawn_policy(
        &self,
        policy: Option<Arc<dyn super::spawn_policy::SpawnPolicy>>,
    ) -> Result<(), MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(MobCommand::SetSpawnPolicy { policy, reply_tx })
            .await
            .map_err(|_| MobError::Internal("actor task dropped".into()))?;
        reply_rx
            .await
            .map_err(|_| MobError::Internal("actor reply dropped".into()))?;
        Ok(())
    }

    /// Shut down the actor. After this, no more commands are accepted.
    pub async fn shutdown(&self) -> Result<(), MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(MobCommand::Shutdown { reply_tx })
            .await
            .map_err(|_| MobError::Internal("actor task dropped".into()))?;
        reply_rx
            .await
            .map_err(|_| MobError::Internal("actor reply dropped".into()))??;
        Ok(())
    }

    /// Force-cancel a member's in-flight turn via session interrupt.
    ///
    /// Unlike [`retire`](Self::retire), this does not archive the session or
    /// remove the member from the roster — it only cancels the current turn.
    pub async fn force_cancel_member(&self, meerkat_id: MeerkatId) -> Result<(), MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(MobCommand::ForceCancel {
                meerkat_id,
                reply_tx,
            })
            .await
            .map_err(|_| MobError::Internal("actor task dropped".into()))?;
        reply_rx
            .await
            .map_err(|_| MobError::Internal("actor reply dropped".into()))?
    }

    async fn canonical_member_snapshot_material(
        &self,
        meerkat_id: &MeerkatId,
    ) -> CanonicalMemberSnapshotMaterial {
        // Canonical helper-surface classification is derived only from roster
        // membership/state plus session-service activity, never side tables.
        let (roster_snapshot, roster_entry, roster_state, current_session_id) = {
            let roster = self.roster.read().await;
            match roster.get(meerkat_id) {
                Some(entry) => (
                    roster.snapshot(),
                    Some(entry.clone()),
                    Some(entry.state),
                    entry.member_ref.session_id().cloned(),
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
            let member_present = roster_state.is_some();
            return CanonicalMemberSnapshotMaterial {
                member_present,
                status: if member_present {
                    CanonicalMemberStatus::Broken
                } else {
                    CanonicalMemberStatus::Unknown
                },
                session_observation: CanonicalSessionObservation::Missing,
                error: Some(diag.reason),
                output_preview: None,
                tokens_used: 0,
                current_session_id: Some(diag.session_id),
                peer_connectivity: None,
            };
        }

        match (roster_state, current_session_id) {
            (None, _) => CanonicalMemberSnapshotMaterial {
                member_present: false,
                status: CanonicalMemberStatus::Unknown,
                session_observation: CanonicalSessionObservation::Missing,
                error: None,
                output_preview: None,
                tokens_used: 0,
                current_session_id: None,
                peer_connectivity: None,
            },
            (Some(crate::roster::MemberState::Retiring), None) => CanonicalMemberSnapshotMaterial {
                member_present: true,
                status: CanonicalMemberStatus::Retiring,
                session_observation: CanonicalSessionObservation::Missing,
                error: None,
                output_preview: None,
                tokens_used: 0,
                current_session_id: None,
                peer_connectivity: None,
            },
            (Some(crate::roster::MemberState::Active), None) => CanonicalMemberSnapshotMaterial {
                member_present: true,
                status: CanonicalMemberStatus::Completed,
                session_observation: CanonicalSessionObservation::Missing,
                error: None,
                output_preview: None,
                tokens_used: 0,
                current_session_id: None,
                peer_connectivity: None,
            },
            (Some(roster_state), Some(session_id)) => {
                let (output_preview, tokens_used, observation) =
                    match self.session_service.read(&session_id).await {
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
                    };
                let status = match observation {
                    CanonicalSessionObservation::Active => match roster_state {
                        crate::roster::MemberState::Active => CanonicalMemberStatus::Active,
                        crate::roster::MemberState::Retiring => CanonicalMemberStatus::Retiring,
                    },
                    CanonicalSessionObservation::Inactive => match roster_state {
                        crate::roster::MemberState::Active => CanonicalMemberStatus::Active,
                        crate::roster::MemberState::Retiring => CanonicalMemberStatus::Retiring,
                    },
                    CanonicalSessionObservation::Missing => match roster_state {
                        crate::roster::MemberState::Active => CanonicalMemberStatus::Completed,
                        crate::roster::MemberState::Retiring => CanonicalMemberStatus::Retiring,
                    },
                    // Transport/read faults are not terminal truth.
                    CanonicalSessionObservation::Unknown => match roster_state {
                        crate::roster::MemberState::Active => CanonicalMemberStatus::Active,
                        crate::roster::MemberState::Retiring => CanonicalMemberStatus::Retiring,
                    },
                };
                let peer_connectivity = match roster_entry.as_ref() {
                    Some(entry) => {
                        self.resolve_peer_connectivity(entry, &session_id, &roster_snapshot)
                            .await
                    }
                    None => None,
                };
                CanonicalMemberSnapshotMaterial {
                    member_present: true,
                    status,
                    session_observation: observation,
                    error: None,
                    output_preview,
                    tokens_used,
                    current_session_id: Some(session_id),
                    peer_connectivity,
                }
            }
        }
    }

    async fn snapshot_kickoff_waiters(
        &self,
        meerkat_ids: Vec<MeerkatId>,
    ) -> Result<Vec<(MeerkatId, tokio::sync::watch::Receiver<bool>)>, MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(MobCommand::KickoffBarrierSnapshot {
                meerkat_ids,
                reply_tx,
            })
            .await
            .map_err(|_| MobError::Internal("mob actor dropped".into()))?;
        reply_rx
            .await
            .map_err(|_| MobError::Internal("actor reply dropped".into()))
    }

    async fn wait_for_kickoff_receivers(
        &self,
        target_ids: &[MeerkatId],
        waiters: Vec<(MeerkatId, tokio::sync::watch::Receiver<bool>)>,
        timeout: Option<Duration>,
    ) -> Result<(), MobError> {
        if waiters.is_empty() {
            return Ok(());
        }

        let deadline =
            tokio::time::Instant::now() + timeout.unwrap_or(DEFAULT_KICKOFF_WAIT_TIMEOUT);
        let mut pending = waiters
            .iter()
            .map(|(id, _)| id.clone())
            .collect::<std::collections::HashSet<_>>();
        let mut futures = FuturesUnordered::new();

        for (id, mut rx) in waiters {
            if *rx.borrow() {
                pending.remove(&id);
                continue;
            }
            futures.push(async move {
                loop {
                    if *rx.borrow() {
                        break;
                    }
                    if rx.changed().await.is_err() {
                        break;
                    }
                }
                id
            });
        }

        while !futures.is_empty() {
            match tokio::time::timeout_at(deadline, futures.next()).await {
                Ok(Some(id)) => {
                    pending.remove(&id);
                }
                Ok(None) => break,
                Err(_) => {
                    let pending_member_ids = target_ids
                        .iter()
                        .filter(|id| pending.contains(*id))
                        .cloned()
                        .collect();
                    return Err(MobError::KickoffWaitTimedOut { pending_member_ids });
                }
            }
        }

        Ok(())
    }

    async fn wait_one_material(
        &self,
        meerkat_id: &MeerkatId,
    ) -> Result<CanonicalMemberSnapshotMaterial, MobError> {
        loop {
            let material = self.canonical_member_snapshot_material(meerkat_id).await;
            if MobMemberTerminalClassifier::is_terminal(&material) {
                return Ok(material);
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }

    /// Get a point-in-time execution snapshot for a member.
    pub async fn member_status(
        &self,
        meerkat_id: &MeerkatId,
    ) -> Result<MobMemberSnapshot, MobError> {
        let material = self.canonical_member_snapshot_material(meerkat_id).await;
        Ok(material.to_snapshot())
    }

    /// Wait for all currently-running autonomous kickoff turns in the current roster snapshot.
    pub async fn wait_for_kickoff_complete(
        &self,
        timeout: Option<Duration>,
    ) -> Result<Vec<(MeerkatId, MobMemberSnapshot)>, MobError> {
        let target_ids = self
            .list_all_members()
            .await
            .into_iter()
            .map(|entry| entry.meerkat_id)
            .collect::<Vec<_>>();
        self.wait_for_members_kickoff_complete(&target_ids, timeout)
            .await
    }

    /// Wait for currently-running autonomous kickoff turns for the given member ids.
    pub async fn wait_for_members_kickoff_complete(
        &self,
        ids: &[MeerkatId],
        timeout: Option<Duration>,
    ) -> Result<Vec<(MeerkatId, MobMemberSnapshot)>, MobError> {
        let target_ids = ids.to_vec();
        let waiters = self.snapshot_kickoff_waiters(target_ids.clone()).await?;
        self.wait_for_kickoff_receivers(&target_ids, waiters, timeout)
            .await?;

        let mut snapshots = Vec::with_capacity(target_ids.len());
        for id in target_ids {
            snapshots.push((id.clone(), self.member_status(&id).await?));
        }
        Ok(snapshots)
    }

    /// Wait for a specific member to reach a terminal state, then return its snapshot.
    ///
    /// Polls canonical member classification until terminal.
    pub async fn wait_one(&self, meerkat_id: &MeerkatId) -> Result<MobMemberSnapshot, MobError> {
        let material = self.wait_one_material(meerkat_id).await?;
        Ok(material.to_snapshot())
    }

    /// Wait for all specified members to reach terminal states.
    pub async fn wait_all(
        &self,
        meerkat_ids: &[MeerkatId],
    ) -> Result<Vec<MobMemberSnapshot>, MobError> {
        let futs = meerkat_ids
            .iter()
            .map(|id| self.wait_one_material(id))
            .collect::<Vec<_>>();
        let results = futures::future::join_all(futs).await;
        results
            .into_iter()
            .map(|result| result.map(|material| material.to_snapshot()))
            .collect()
    }

    /// Collect snapshots for all members that have reached terminal states.
    pub async fn collect_completed(&self) -> Vec<(MeerkatId, MobMemberSnapshot)> {
        let entries = self.list_all_members().await;
        let mut completed = Vec::new();
        for entry in entries {
            if let Ok(snapshot) = self.member_status(&entry.meerkat_id).await
                && snapshot.is_final
            {
                completed.push((entry.meerkat_id, snapshot));
            }
        }
        completed
    }

    /// Spawn a fresh helper, wait for it to complete, retire it, and return its result.
    ///
    /// This is a convenience wrapper around spawn → wait → collect → retire for
    /// short-lived sub-tasks.
    pub async fn spawn_helper(
        &self,
        meerkat_id: MeerkatId,
        task: impl Into<String>,
        options: HelperOptions,
    ) -> Result<HelperResult, MobError> {
        let profile_name = options
            .profile_name
            .or_else(|| self.definition.profiles.keys().next().cloned())
            .ok_or_else(|| {
                MobError::Internal("no profile specified and definition has no profiles".into())
            })?;
        let task_text = task.into();
        let mut spec = SpawnMemberSpec::new(profile_name, meerkat_id.clone());
        spec.initial_message = Some(task_text.into());
        spec.runtime_mode = options.runtime_mode;
        spec.backend = options.backend;
        spec.tool_access_policy = options.tool_access_policy;
        spec.auto_wire_parent = true;

        self.spawn_spec(spec).await?;
        let terminal_material = self.wait_one_material(&meerkat_id).await?;
        let _ = self.retire(meerkat_id.clone()).await;

        Ok(terminal_material.to_helper_result())
    }

    /// Fork from an existing member's context, wait for completion, retire, and return.
    ///
    /// Like `spawn_helper` but uses `MemberLaunchMode::Fork` to share conversation
    /// context with the source member.
    pub async fn fork_helper(
        &self,
        source_member_id: &MeerkatId,
        meerkat_id: MeerkatId,
        task: impl Into<String>,
        fork_context: crate::launch::ForkContext,
        options: HelperOptions,
    ) -> Result<HelperResult, MobError> {
        let profile_name = options
            .profile_name
            .or_else(|| self.definition.profiles.keys().next().cloned())
            .ok_or_else(|| {
                MobError::Internal("no profile specified and definition has no profiles".into())
            })?;
        let task_text = task.into();
        let mut spec = SpawnMemberSpec::new(profile_name, meerkat_id.clone());
        spec.initial_message = Some(task_text.into());
        spec.runtime_mode = options.runtime_mode;
        spec.backend = options.backend;
        spec.tool_access_policy = options.tool_access_policy;
        spec.auto_wire_parent = true;
        spec.launch_mode = crate::launch::MemberLaunchMode::Fork {
            source_member_id: source_member_id.clone(),
            fork_context,
        };

        self.spawn_spec(spec).await?;
        let terminal_material = self.wait_one_material(&meerkat_id).await?;
        let _ = self.retire(meerkat_id.clone()).await;

        Ok(terminal_material.to_helper_result())
    }
}

impl MemberHandle {
    /// Target member id for this capability.
    pub fn meerkat_id(&self) -> &MeerkatId {
        &self.meerkat_id
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
        let session_id = self
            .mob
            .external_turn_for_member(
                self.meerkat_id.clone(),
                content.into(),
                handling_mode,
                render_metadata,
            )
            .await?;
        Ok(MemberDeliveryReceipt {
            member_id: self.meerkat_id.clone(),
            session_id,
            handling_mode,
        })
    }

    /// Submit internal work to this member without external addressability checks.
    pub async fn internal_turn(
        &self,
        content: impl Into<meerkat_core::types::ContentInput>,
    ) -> Result<MemberDeliveryReceipt, MobError> {
        let session_id = self
            .mob
            .internal_turn_for_member(self.meerkat_id.clone(), content.into())
            .await?;
        Ok(MemberDeliveryReceipt {
            member_id: self.meerkat_id.clone(),
            session_id,
            handling_mode: HandlingMode::Queue,
        })
    }

    /// Current session ID for this member, if a session bridge exists.
    pub async fn current_session_id(&self) -> Result<Option<SessionId>, MobError> {
        let roster = self.mob.roster.read().await;
        Ok(roster
            .get(&self.meerkat_id)
            .and_then(|e| e.member_ref.session_id().cloned()))
    }

    /// Session reference for this member, if a session bridge exists.
    pub async fn session_ref(&self) -> Result<Option<MemberSessionRef>, MobError> {
        let roster = self.mob.roster.read().await;
        Ok(roster
            .get(&self.meerkat_id)
            .and_then(|e| e.member_ref.session_id().cloned())
            .map(|session_id| MemberSessionRef {
                member_id: self.meerkat_id.clone(),
                session_id,
            }))
    }

    /// Get a point-in-time execution snapshot for this member.
    pub async fn status(&self) -> Result<MobMemberSnapshot, MobError> {
        self.mob.member_status(&self.meerkat_id).await
    }

    /// Subscribe to this member's agent events.
    pub async fn events(&self) -> Result<EventStream, MobError> {
        self.mob.subscribe_agent_events(&self.meerkat_id).await
    }
}
