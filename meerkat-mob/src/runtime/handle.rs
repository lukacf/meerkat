use super::*;
use crate::MobRuntimeMode;
use crate::generated::adaptive_mob_bundle as adaptive_bundle;
use crate::machines::mob_machine as mob_dsl;
use crate::mob_machine::{MobMachineCommand, MobMachineCommandResult};
use crate::roster::MobMemberKickoffSnapshot;
use crate::run::{MobMachineFlowRunCommand, flow_run};
#[cfg(test)]
use crate::runtime::MobLifecycleSnapshot;
use crate::runtime::flow_frame_engine::FlowFrameLoopStorePlan;
#[cfg(test)]
use crate::runtime::mob_member_lifecycle_projection::{
    CanonicalMemberSnapshotMaterial, CanonicalMemberStatus,
};
use crate::runtime::mob_member_lifecycle_projection::{
    MobMemberLifecycleInput, MobMemberLifecycleProjection, kickoff_snapshot_from_machine_state,
};
use crate::runtime::reconcile::{
    EnsureMemberOutcome, MemberFilter, ReconcileFailure, ReconcileOptions, ReconcileReport,
    ReconcileStage,
};
use crate::runtime::terminalization::{TerminalizationOutcome, TerminalizationTarget};
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use meerkat_core::agent::CommsRuntime;
use meerkat_core::comms::{CommsCommand, PeerId, SendReceipt, TrustedPeerDescriptor};
use meerkat_core::lifecycle::run_primitive::{
    KeepAliveDirective, ModelId, ProviderParamsOverride, RuntimeTurnMetadata, TurnInstruction,
    TurnMetadataOverride,
};
use meerkat_core::ops::OperationId;
use meerkat_core::ops_lifecycle::OpsLifecycleRegistry;
use meerkat_core::service::{MobToolAuthorityContext, SessionError};
use meerkat_core::skills::SkillKey;
use meerkat_core::time_compat::Instant;
use meerkat_core::types::{HandlingMode, RenderMetadata, SessionId};
use meerkat_core::{AuthBindingRef, Provider, TurnToolOverlay};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::sync::RwLock as StdRwLock;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

use crate::store::MobMemberOperatorRequestKey;

const DEFAULT_KICKOFF_WAIT_TIMEOUT: Duration = Duration::from_secs(600);
const DEFAULT_READY_WAIT_TIMEOUT: Duration = Duration::from_secs(60);
pub(super) const HOST_STATUS_POLL_INTERVAL: Duration = Duration::from_secs(5);
const REACHABILITY_STALE_AFTER: Duration = Duration::from_secs(15);
#[cfg(not(test))]
const READY_WAIT_BRIDGE_SESSION_RECHECK_INTERVAL: Duration = Duration::from_secs(1);
#[cfg(test)]
const READY_WAIT_BRIDGE_SESSION_RECHECK_INTERVAL: Duration = Duration::from_millis(25);

#[derive(Debug, Clone)]
pub(super) struct ReachabilityObservationView {
    pub reachability: meerkat_contracts::wire::WireReachability,
    pub last_seen_ms: Option<u64>,
    pub freshness_reason: String,
}

#[derive(Debug, Clone)]
struct ReachabilityObservationRecord {
    reachability: meerkat_contracts::wire::WireReachability,
    last_verified: Option<Instant>,
    freshness_reason: String,
}

/// Observer-local liveness projection shared by the actor, event pumps, and
/// read-only handles. It is deliberately process-local and monotonic-time
/// based: reachability is a rebuildable projection, never durable membership.
#[derive(Debug, Default)]
pub(super) struct ReachabilityObservations {
    hosts: StdRwLock<BTreeMap<String, ReachabilityObservationRecord>>,
    members: StdRwLock<BTreeMap<String, ReachabilityObservationRecord>>,
}

impl ReachabilityObservations {
    fn success_record(reason: &str) -> ReachabilityObservationRecord {
        ReachabilityObservationRecord {
            reachability: meerkat_contracts::wire::WireReachability::Reachable,
            last_verified: Some(Instant::now()),
            freshness_reason: reason.to_string(),
        }
    }

    pub(super) fn mark_host_success(&self, host_id: &str) {
        self.mark_host_success_with_reason(host_id, "host_status_ack");
    }

    pub(super) fn mark_host_member_events_success(&self, host_id: &str) {
        self.mark_host_success_with_reason(host_id, "member_event_page_ack");
    }

    fn mark_host_success_with_reason(&self, host_id: &str, reason: &str) {
        self.hosts
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(host_id.to_string(), Self::success_record(reason));
    }

    pub(super) fn mark_host_failure(&self, host_id: &str, error: &MobError) {
        let mut hosts = self
            .hosts
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let previous_last_verified = hosts.get(host_id).and_then(|row| row.last_verified);
        let timed_out = matches!(error, MobError::BridgeRequestTimedOut { .. });
        let reachability = if timed_out {
            if previous_last_verified.is_some() {
                meerkat_contracts::wire::WireReachability::Stale
            } else {
                meerkat_contracts::wire::WireReachability::Unknown
            }
        } else {
            meerkat_contracts::wire::WireReachability::Unreachable
        };
        hosts.insert(
            host_id.to_string(),
            ReachabilityObservationRecord {
                reachability,
                last_verified: previous_last_verified,
                freshness_reason: if timed_out {
                    "host_status_timeout"
                } else {
                    "host_status_unreachable"
                }
                .to_string(),
            },
        );
    }

    pub(super) fn mark_member_progress(&self, identity: &AgentIdentity) {
        self.members
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(
                identity.to_string(),
                Self::success_record("event_pump_progress"),
            );
    }

    pub(super) fn mark_member_poll_failure(&self, identity: &AgentIdentity, timed_out: bool) {
        let mut members = self
            .members
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let previous_last_verified = members
            .get(identity.as_str())
            .and_then(|row| row.last_verified);
        members.insert(
            identity.to_string(),
            ReachabilityObservationRecord {
                reachability: if timed_out && previous_last_verified.is_some() {
                    meerkat_contracts::wire::WireReachability::Stale
                } else if timed_out {
                    meerkat_contracts::wire::WireReachability::Unknown
                } else {
                    meerkat_contracts::wire::WireReachability::Unreachable
                },
                last_verified: previous_last_verified,
                freshness_reason: if timed_out {
                    "event_pump_timeout"
                } else {
                    "event_pump_unreachable"
                }
                .to_string(),
            },
        );
    }

    pub(super) fn clear_host(&self, host_id: &str) {
        self.hosts
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(host_id);
    }

    pub(super) fn clear_member(&self, identity: &AgentIdentity) {
        self.members
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(identity.as_str());
    }

    fn view(record: &ReachabilityObservationRecord) -> ReachabilityObservationView {
        let elapsed = record.last_verified.map(|instant| instant.elapsed());
        let auto_stale = matches!(
            record.reachability,
            meerkat_contracts::wire::WireReachability::Reachable
        ) && elapsed.is_some_and(|age| age > REACHABILITY_STALE_AFTER);
        ReachabilityObservationView {
            reachability: if auto_stale {
                meerkat_contracts::wire::WireReachability::Stale
            } else {
                record.reachability
            },
            last_seen_ms: elapsed.map(|age| u64::try_from(age.as_millis()).unwrap_or(u64::MAX)),
            freshness_reason: if auto_stale {
                "observation_stale".to_string()
            } else {
                record.freshness_reason.clone()
            },
        }
    }

    pub(super) fn host(&self, host_id: &str) -> Option<ReachabilityObservationView> {
        self.hosts
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(host_id)
            .map(Self::view)
    }

    pub(super) fn member(&self, identity: &AgentIdentity) -> Option<ReachabilityObservationView> {
        self.members
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(identity.as_str())
            .map(Self::view)
    }
}

fn adaptive_bundle_layer_terminal_store_plan()
-> Result<adaptive_bundle::AdaptiveMobBundleStorePlan, MobError> {
    let work = adaptive_bundle::AdaptiveMobBundleWork::new(
        adaptive_bundle::producers::layer_mob_instance_id(),
        adaptive_bundle::effects::layer_mob::flow_run_public_result_classified(),
    );
    let decision = adaptive_bundle::AdaptiveMobBundleDriver::decide(&work);
    adaptive_bundle::AdaptiveMobBundleDriver::store_plan(decision).ok_or_else(|| {
        MobError::Internal(
            "adaptive bundle generated driver has no route for layer terminal feedback".into(),
        )
    })
}

fn sanitize_flow_member_segment(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        "member".to_string()
    } else {
        out
    }
}

/// Machine-decided spawn-member operator admission verdict, mirrored by tool
/// surfaces. `Denied` maps to a tool `access_denied` error; `Allowed` proceeds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpawnMemberAdmission {
    Allowed,
    Denied,
}

/// Opaque capability minted when a MobMachine accepts
/// `InitializeAdaptiveRun`.
///
/// The private field makes this unforgeable outside `meerkat-mob`; adaptive
/// driver code can hold and pass it back to the narrow adaptive seam, but it
/// cannot manufacture one or write raw machine inputs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdaptiveDriverCapability {
    adaptive_run_id: String,
    _private: (),
}

impl AdaptiveDriverCapability {
    #[must_use]
    pub fn adaptive_run_id(&self) -> &str {
        &self.adaptive_run_id
    }
}

/// Complete numeric limit record currently owned by the landed adaptive
/// MobMachine kernel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdaptiveRunLimits {
    pub max_depth: u64,
    pub max_total_decisions: u64,
    pub max_repair_attempts: u64,
    pub max_layer_failures: u64,
    pub max_attempts_per_layer: u64,
    pub max_members_per_layer: u64,
    pub max_total_spawned_members: u64,
    pub max_active_members: u64,
    pub max_retained_layer_mobs: u64,
    pub max_aggregate_tokens: u64,
    pub max_aggregate_tool_calls: u64,
    pub allowed_model_classes: BTreeSet<String>,
    pub allowed_tool_classes: BTreeSet<String>,
    pub allowed_skill_identities: BTreeSet<String>,
    pub allowed_auth_binding_refs: BTreeSet<String>,
    pub deadline_ms: u64,
}

/// Initialization payload for the adaptive run kernel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitializeAdaptiveRunRequest {
    pub adaptive_run_id: String,
    pub limits: AdaptiveRunLimits,
}

/// FlowMaster decision kind recorded by the adaptive kernel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdaptivePlanningDecisionKind {
    RunLayer,
    Finish,
}

impl From<AdaptivePlanningDecisionKind> for mob_dsl::AdaptiveDecisionKind {
    fn from(value: AdaptivePlanningDecisionKind) -> Self {
        match value {
            AdaptivePlanningDecisionKind::RunLayer => Self::RunLayer,
            AdaptivePlanningDecisionKind::Finish => Self::Finish,
        }
    }
}

/// Kernel admission verdict for one adaptive layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdaptiveLayerAdmission {
    Allowed,
    Denied,
}

impl From<mob_dsl::AdaptiveLayerAdmissionKind> for AdaptiveLayerAdmission {
    fn from(value: mob_dsl::AdaptiveLayerAdmissionKind) -> Self {
        match value {
            mob_dsl::AdaptiveLayerAdmissionKind::Allowed => Self::Allowed,
            mob_dsl::AdaptiveLayerAdmissionKind::Denied => Self::Denied,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdaptiveRunPhaseView {
    Active,
    CleanupRequired,
    EvidenceMissing,
    Finished,
    Failed,
    Canceled,
}

impl From<mob_dsl::AdaptiveRunPhase> for AdaptiveRunPhaseView {
    fn from(value: mob_dsl::AdaptiveRunPhase) -> Self {
        match value {
            mob_dsl::AdaptiveRunPhase::Active => Self::Active,
            mob_dsl::AdaptiveRunPhase::CleanupRequired => Self::CleanupRequired,
            mob_dsl::AdaptiveRunPhase::EvidenceMissing => Self::EvidenceMissing,
            mob_dsl::AdaptiveRunPhase::Finished => Self::Finished,
            mob_dsl::AdaptiveRunPhase::Failed => Self::Failed,
            mob_dsl::AdaptiveRunPhase::Canceled => Self::Canceled,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdaptiveStopReasonView {
    FinishDecision,
    DepthLimit,
    PlanLimit,
    RepairLimit,
    FailureLimit,
    BudgetExhausted,
    DeadlineExceeded,
    HostCancel,
}

impl From<mob_dsl::AdaptiveStopReason> for AdaptiveStopReasonView {
    fn from(value: mob_dsl::AdaptiveStopReason) -> Self {
        match value {
            mob_dsl::AdaptiveStopReason::FinishDecision => Self::FinishDecision,
            mob_dsl::AdaptiveStopReason::DepthLimit => Self::DepthLimit,
            mob_dsl::AdaptiveStopReason::PlanLimit => Self::PlanLimit,
            mob_dsl::AdaptiveStopReason::RepairLimit => Self::RepairLimit,
            mob_dsl::AdaptiveStopReason::FailureLimit => Self::FailureLimit,
            mob_dsl::AdaptiveStopReason::BudgetExhausted => Self::BudgetExhausted,
            mob_dsl::AdaptiveStopReason::DeadlineExceeded => Self::DeadlineExceeded,
            mob_dsl::AdaptiveStopReason::HostCancel => Self::HostCancel,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdaptiveLayerPhaseView {
    Validating,
    Admitted,
    Provisioning,
    Running,
    Collecting,
    Completed,
    SetupFailed,
    RunFailed,
    ResultInvalid,
    Canceled,
}

impl From<mob_dsl::AdaptiveLayerPhase> for AdaptiveLayerPhaseView {
    fn from(value: mob_dsl::AdaptiveLayerPhase) -> Self {
        match value {
            mob_dsl::AdaptiveLayerPhase::Validating => Self::Validating,
            mob_dsl::AdaptiveLayerPhase::Admitted => Self::Admitted,
            mob_dsl::AdaptiveLayerPhase::Provisioning => Self::Provisioning,
            mob_dsl::AdaptiveLayerPhase::Running => Self::Running,
            mob_dsl::AdaptiveLayerPhase::Collecting => Self::Collecting,
            mob_dsl::AdaptiveLayerPhase::Completed => Self::Completed,
            mob_dsl::AdaptiveLayerPhase::SetupFailed => Self::SetupFailed,
            mob_dsl::AdaptiveLayerPhase::RunFailed => Self::RunFailed,
            mob_dsl::AdaptiveLayerPhase::ResultInvalid => Self::ResultInvalid,
            mob_dsl::AdaptiveLayerPhase::Canceled => Self::Canceled,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdaptiveLayerAdmissionRequest {
    pub layer_id: String,
    pub attempt: u64,
    pub plan_digest: String,
    pub child_mob_id: String,
    pub member_count: u64,
    pub token_reservation: u64,
    pub tool_call_reservation: u64,
    pub used_model_classes: BTreeSet<String>,
    pub used_tool_classes: BTreeSet<String>,
    pub used_skill_identities: BTreeSet<String>,
    pub used_auth_binding_refs: BTreeSet<String>,
    pub observed_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdaptiveLayerAttempt {
    pub layer_id: String,
    pub attempt: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdaptiveLayerRunStart {
    pub layer_id: String,
    pub attempt: u64,
    pub child_run_id: RunId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdaptiveLayerResultDigest {
    pub layer_id: String,
    pub attempt: u64,
    pub result_digest: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdaptiveLayerSetupFault {
    MobCreateFailed,
    SpawnFailed,
    WiringFailed,
    CanceledDuringSetup,
    Interrupted,
}

impl From<AdaptiveLayerSetupFault> for mob_dsl::AdaptiveLayerSetupFaultKind {
    fn from(value: AdaptiveLayerSetupFault) -> Self {
        match value {
            AdaptiveLayerSetupFault::MobCreateFailed => Self::MobCreateFailed,
            AdaptiveLayerSetupFault::SpawnFailed => Self::SpawnFailed,
            AdaptiveLayerSetupFault::WiringFailed => Self::WiringFailed,
            AdaptiveLayerSetupFault::CanceledDuringSetup => Self::CanceledDuringSetup,
            AdaptiveLayerSetupFault::Interrupted => Self::Interrupted,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdaptiveLayerSetupFaultObservation {
    pub layer_id: String,
    pub attempt: u64,
    pub fault: AdaptiveLayerSetupFault,
    pub spawned_members: u64,
    pub requested_members: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdaptiveLayerDisposition {
    Destroyed,
    Retained,
    RetainedAsEvidence,
}

impl From<AdaptiveLayerDisposition> for mob_dsl::AdaptiveLayerDispositionKind {
    fn from(value: AdaptiveLayerDisposition) -> Self {
        match value {
            AdaptiveLayerDisposition::Destroyed => Self::Destroyed,
            AdaptiveLayerDisposition::Retained => Self::Retained,
            AdaptiveLayerDisposition::RetainedAsEvidence => Self::RetainedAsEvidence,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdaptiveLayerRetention {
    pub layer_id: String,
    pub attempt: u64,
    pub disposition: AdaptiveLayerDisposition,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdaptiveLayerSnapshot {
    pub layer_id: String,
    pub phase: AdaptiveLayerPhaseView,
    pub attempt: u64,
    pub child_run_id: Option<RunId>,
    pub result_digest: Option<String>,
    pub plan_digest: Option<String>,
    pub child_mob_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdaptiveRunSnapshot {
    pub adaptive_run_id: String,
    pub phase: Option<AdaptiveRunPhaseView>,
    pub stop_reason: Option<AdaptiveStopReasonView>,
    pub depth: u64,
    pub total_decisions: u64,
    pub repair_attempts: u64,
    pub layer_failures: u64,
    pub total_spawned_members: u64,
    pub active_members: u64,
    pub retained_layer_mobs: u64,
    pub aggregate_token_reserved: u64,
    pub aggregate_token_actual: u64,
    pub aggregate_tool_call_reserved: u64,
    pub aggregate_tool_call_actual: u64,
    pub missing_body_digest: Option<String>,
    pub layers: BTreeMap<String, AdaptiveLayerSnapshot>,
}

/// Raw, atomic spawn-member admission observations a tool surface extracts and
/// feeds to MobMachine WITHOUT pre-composing them.
///
/// Each `privileged_*_present` field is a pure per-argument presence
/// observation (the surface fills only the arguments its own spawn-member tool
/// accepts; absent fields default to `false`). MobMachine — not the tool
/// surface — owns the privileged-argument SET membership policy (which
/// arguments are privileged) by OR-ing these facts, and composes the
/// `manage_scope_present || profile_scope_contains` profile-scope disjunction.
/// The surface must NOT pre-reduce these into a single "any privileged arg" or
/// "can spawn this profile" bool: the SET and the disjunction are POLICY owned
/// by the machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SpawnMemberAdmissionObservations {
    /// Whether the operator holds manage scope over the target mob (a
    /// machine-owned operator-scope membership projection).
    pub manage_scope_present: bool,
    /// Whether the operator's spawn-profile scope set for the target mob
    /// CONTAINS the requested profile (a RAW per-profile set-membership
    /// projection — NOT OR'd with manage scope; the machine composes the
    /// disjunction).
    pub profile_scope_contains: bool,
    /// Presence of `resume_bridge_session_id` on the spawn request.
    pub resume_bridge_session_present: bool,
    /// Presence of `resume_session_id` on the spawn request.
    pub resume_session_present: bool,
    /// Presence of an explicit `backend` on the spawn request.
    pub backend_present: bool,
    /// Presence of an explicit `runtime_mode` on the spawn request.
    pub runtime_mode_present: bool,
    /// Presence of an explicit `launch_mode` on the spawn request.
    pub launch_mode_present: bool,
    /// Presence of an explicit `tool_access_policy` on the spawn request.
    pub tool_access_policy_present: bool,
    /// Presence of an explicit `tooling` selection on the spawn request.
    pub tooling_present: bool,
    /// Presence of an explicit `auth_binding` on the spawn request.
    pub auth_binding_present: bool,
}

/// Machine-decided per-mob operator admission verdict for current-mob-scoped
/// tools, mirrored by tool surfaces. `Denied` maps to a tool `access_denied`
/// error; `Allowed` proceeds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurrentMobAdmission {
    Allowed,
    Denied,
}

/// Machine-decided coarse spawn-tool admission verdict for the spawn-member
/// tool surfaces (`spawn_member` / `spawn_many_members`), mirrored by tool
/// surfaces. `Denied` maps to a tool `access_denied` error; `Allowed` proceeds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpawnToolAdmission {
    Allowed,
    Denied,
}

/// Machine-decided member-operator upcall admission verdict (multi-host §15
/// R6), mirrored by the controlling-side upcall responder. `Rejected` carries
/// the machine's typed cause verbatim (ADJ-14 maps it onto the wire).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemberOperatorAdmissionVerdict {
    Admitted,
    Rejected(mob_dsl::MemberOperatorRejectKind),
}

/// Raw facts for one member-originated operator admission decision.
///
/// The durable request key owns every requester-incarnation atom; ingress adds
/// only the authenticated sender peer. The actor forwards these facts to the
/// generated MobMachine without pre-composing an admission verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MemberOperatorAdmissionRequest {
    pub(crate) request_key: MobMemberOperatorRequestKey,
    pub(crate) sender_peer_id: PeerId,
}

#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct MobMemberSnapshot {
    /// Current lifecycle status.
    pub status: MobMemberStatus,
    /// Canonical member identity.
    ///
    /// Kept as bridge-internal projection state because existing public
    /// surfaces wrap snapshots with their own explicit identity fields.
    #[serde(skip)]
    pub(crate) agent_identity: AgentIdentity,
    /// Identity-native runtime ID for this incarnation.
    ///
    /// Binding-era atom: bridge-internal, `pub(crate)` + `#[serde(skip)]`
    /// so external consumers use `agent_identity()` as the public identity
    /// contract. Absent when MobMachine has no current runtime binding.
    #[serde(skip)]
    pub(crate) agent_runtime_id: Option<AgentRuntimeId>,
    /// Fence token for the current incarnation.
    ///
    /// Binding-era atom used by the bridge for stale-command rejection.
    /// `pub(crate)` + `#[serde(skip)]` so it does not leak into
    /// app-facing payloads. Absent when MobMachine has no current runtime
    /// binding.
    #[serde(skip)]
    pub(crate) fence_token: Option<FenceToken>,
    /// Preview of the current bridge session's last committed assistant text.
    pub output_preview: Option<String>,
    /// Error description (if the member errored).
    pub error: Option<String>,
    /// Cumulative token usage.
    pub tokens_used: u64,
    /// Whether the member has reached a terminal state.
    pub is_final: bool,
    /// Diagnostic session id for the member's current bridge session.
    /// Observable for status/continuity diagnostics only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_session_id: Option<SessionId>,
    /// Bridge-internal session binding — not part of the public identity contract.
    #[serde(skip)]
    pub(crate) current_bridge_session_id: Option<SessionId>,
    /// Live comms connectivity for currently wired peers, projected as the
    /// closed tri-state ([`meerkat_contracts::WirePeerConnectivity`]) so a probe
    /// timeout is distinguishable from a member that has no bridge session.
    /// `None` only when the deep-inspection projection has not been computed
    /// (e.g. lightweight roster projections that skip connectivity fanout).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peer_connectivity: Option<meerkat_contracts::WirePeerConnectivity>,
    /// Initial autonomous-turn kickoff state, when this member has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kickoff: Option<MobMemberKickoffSnapshot>,
    /// External-member observation projection, when this member is backed by
    /// an external peer rather than a local session.
    ///
    /// This is a read-only projection over the roster member ref and restore
    /// diagnostics. It intentionally avoids peer ids, transport addresses,
    /// bootstrap tokens, runtime incarnation ids, and fence tokens; those are
    /// binding mechanics, not app-facing authority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_member: Option<ExternalMemberObservationSnapshot>,
    /// Runtime-owned resolved LLM capability projection for the member's
    /// current bridge session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_capabilities: Option<meerkat_contracts::WireResolvedModelCapabilities>,
    /// Machine-owned live execution/progress projection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<MemberProgressSnapshot>,
    /// Machine-recorded remote placement (owning host peer id) for this
    /// member (phase 7, ADJ-P7-2: produced from the `member_placement`
    /// machine fact at the status projection). `None` = local to the
    /// controlling host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placement: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub control_reachability: Option<meerkat_contracts::wire::WireReachability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comms_reachability: Option<meerkat_contracts::wire::WireReachability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_seen_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub freshness_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lifecycle_capabilities: Option<meerkat_contracts::wire::WireMemberLifecycleCapabilities>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub non_portable_disabled: Option<Vec<meerkat_contracts::wire::WireNonPortableResourceKind>>,
}

/// Whether the member currently owns an open execution run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemberRunState {
    Idle,
    RunOpen,
    Unknown,
}

/// Machine-owned liveness classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemberHealthClass {
    Healthy,
    Degraded,
    Wedged,
    Unknown,
}

/// Typed last-progress observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemberProgressEvent {
    ExecutionAdvanced,
    BecameIdle,
    Unchanged,
}

/// Point-in-time execution health for a member.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberProgressSnapshot {
    pub run_state: MemberRunState,
    pub in_flight_work: u64,
    pub last_progress_at_ms: u64,
    pub last_progress_event: MemberProgressEvent,
    pub health: MemberHealthClass,
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
    /// the identity supplied by MobMachine-backed projection material.
    #[must_use]
    pub fn agent_identity(&self) -> &AgentIdentity {
        &self.agent_identity
    }

    /// Runtime incarnation identity for diagnostic/control projections.
    ///
    /// These atoms stay out of generic `Serialize` output so app-facing
    /// receipts do not couple callers to bridge internals. Surfaces that own a
    /// control contract, such as `mob/member_status`, must opt in through this
    /// accessor and project the fields explicitly.
    #[must_use]
    pub fn runtime_identity_fields(&self) -> Option<(&AgentRuntimeId, FenceToken)> {
        match (&self.agent_runtime_id, self.fence_token) {
            (Some(agent_runtime_id), Some(fence_token)) => Some((agent_runtime_id, fence_token)),
            _ => None,
        }
    }

    pub(crate) fn require_runtime_identity_fields(
        &self,
        context: &str,
    ) -> Result<(&AgentRuntimeId, FenceToken), MobError> {
        self.runtime_identity_fields().ok_or_else(|| {
            MobError::Internal(format!(
                "{context} requires MobMachine runtime binding for '{}'",
                self.agent_identity
            ))
        })
    }

    /// Project this deep-inspection snapshot into the typed public
    /// `mob/member_status` wire result.
    ///
    /// `member_ref` is the server-resolved opaque handle the caller supplies so
    /// the member identity travels in-band rather than being spliced into the
    /// JSON out-of-band. The `peer_connectivity` tri-state is carried as-is so
    /// consumers distinguish a timed-out probe from a not-applicable binding. A
    /// serialization failure on the typed `kickoff`/`external_member`
    /// projections is a typed fault, not laundered into a fabricated empty
    /// object.
    pub fn to_member_status_result(
        &self,
        member_ref: meerkat_contracts::WireMemberRef,
    ) -> Result<meerkat_contracts::MobMemberStatusResult, MobError> {
        let kickoff = match &self.kickoff {
            Some(kickoff) => Some(serde_json::to_value(kickoff).map_err(|error| {
                MobError::Internal(format!(
                    "mob member status kickoff projection failed for '{}': {error}",
                    self.agent_identity
                ))
            })?),
            None => None,
        };
        let external_member = match &self.external_member {
            Some(external) => Some(serde_json::to_value(external).map_err(|error| {
                MobError::Internal(format!(
                    "mob member status external-member projection failed for '{}': {error}",
                    self.agent_identity
                ))
            })?),
            None => None,
        };
        Ok(meerkat_contracts::MobMemberStatusResult {
            status: wire_member_status(self.status),
            member_ref,
            output_preview: self.output_preview.clone(),
            error: self.error.clone(),
            tokens_used: self.tokens_used,
            is_final: self.is_final,
            current_session_id: self.current_session_id.as_ref().map(ToString::to_string),
            peer_connectivity: self.peer_connectivity.clone(),
            kickoff,
            external_member,
            resolved_capabilities: self.resolved_capabilities.clone(),
            progress: self.progress.as_ref().map(|progress| {
                meerkat_contracts::WireMemberProgressSnapshot {
                    run_state: match progress.run_state {
                        MemberRunState::Idle => meerkat_contracts::WireMemberRunState::Idle,
                        MemberRunState::RunOpen => meerkat_contracts::WireMemberRunState::RunOpen,
                        MemberRunState::Unknown => meerkat_contracts::WireMemberRunState::Unknown,
                    },
                    in_flight_work: progress.in_flight_work,
                    last_progress_at_ms: progress.last_progress_at_ms,
                    last_progress_event: match progress.last_progress_event {
                        MemberProgressEvent::ExecutionAdvanced => {
                            meerkat_contracts::WireMemberProgressEvent::ExecutionAdvanced
                        }
                        MemberProgressEvent::BecameIdle => {
                            meerkat_contracts::WireMemberProgressEvent::BecameIdle
                        }
                        MemberProgressEvent::Unchanged => {
                            meerkat_contracts::WireMemberProgressEvent::Unchanged
                        }
                    },
                    health: match progress.health {
                        MemberHealthClass::Healthy => {
                            meerkat_contracts::WireMemberHealthClass::Healthy
                        }
                        MemberHealthClass::Degraded => {
                            meerkat_contracts::WireMemberHealthClass::Degraded
                        }
                        MemberHealthClass::Wedged => {
                            meerkat_contracts::WireMemberHealthClass::Wedged
                        }
                        MemberHealthClass::Unknown => {
                            meerkat_contracts::WireMemberHealthClass::Unknown
                        }
                    },
                }
            }),
            // Placement (phase 7, ADJ-P7-2): the `member_placement` machine
            // fact, attached to the snapshot by the status projection.
            placement: self
                .placement
                .clone()
                .map(meerkat_contracts::wire::WireHostRef),
            control_reachability: self.control_reachability,
            comms_reachability: self.comms_reachability,
            last_seen_ms: self.last_seen_ms,
            freshness_reason: self.freshness_reason.clone(),
            lifecycle_capabilities: self.lifecycle_capabilities,
            non_portable_disabled: self.non_portable_disabled.clone(),
        })
    }
}

/// Project a domain `MobMemberStatus` into its closed wire twin.
fn wire_member_status(status: MobMemberStatus) -> meerkat_contracts::WireMobMemberStatus {
    match status {
        MobMemberStatus::Active => meerkat_contracts::WireMobMemberStatus::Active,
        MobMemberStatus::Retiring => meerkat_contracts::WireMobMemberStatus::Retiring,
        MobMemberStatus::Broken => meerkat_contracts::WireMobMemberStatus::Broken,
        MobMemberStatus::Completed => meerkat_contracts::WireMobMemberStatus::Completed,
        MobMemberStatus::Unknown => meerkat_contracts::WireMobMemberStatus::Unknown,
    }
}

/// Project a domain [`crate::Profile`] into the public wire profile contract.
///
/// Runtime-owned `rust_bundles` are intentionally not part of the public wire
/// shape and are dropped from the projection. This is the canonical
/// `Profile -> WireMobProfile` direction reused by every mob surface.
#[must_use]
pub fn profile_to_wire(profile: &crate::Profile) -> meerkat_contracts::WireMobProfile {
    let tools = &profile.tools;
    meerkat_contracts::WireMobProfile {
        model: profile.model.clone(),
        provider: profile.provider,
        self_hosted_server_id: profile.self_hosted_server_id.clone(),
        image_generation_provider: profile.image_generation_provider,
        auto_compact_threshold: profile.auto_compact_threshold,
        resume_overrides: profile
            .resume_overrides
            .iter()
            .map(|field| match field {
                crate::profile::ResumeOverrideField::Model => {
                    meerkat_contracts::WireMobResumeOverrideField::Model
                }
                crate::profile::ResumeOverrideField::Provider => {
                    meerkat_contracts::WireMobResumeOverrideField::Provider
                }
                crate::profile::ResumeOverrideField::ProviderParams => {
                    meerkat_contracts::WireMobResumeOverrideField::ProviderParams
                }
            })
            .collect(),
        skills: profile.skills.clone(),
        tools: meerkat_contracts::WireMobToolConfig {
            builtins: tools.builtins,
            shell: tools.shell,
            comms: tools.comms,
            memory: tools.memory,
            workgraph: tools.workgraph,
            mob: tools.mob,
            schedule: tools.schedule,
            image_generation: tools.image_generation,
            mcp: tools.mcp.clone(),
        },
        peer_description: profile.peer_description.clone(),
        external_addressable: profile.external_addressable,
        backend: profile.backend.map(|kind| match kind {
            crate::MobBackendKind::Session => meerkat_contracts::WireMobBackendKind::Session,
            crate::MobBackendKind::External => meerkat_contracts::WireMobBackendKind::External,
        }),
        runtime_mode: match profile.runtime_mode {
            crate::MobRuntimeMode::AutonomousHost => {
                meerkat_contracts::WireMobRuntimeMode::AutonomousHost
            }
            crate::MobRuntimeMode::TurnDriven => meerkat_contracts::WireMobRuntimeMode::TurnDriven,
        },
        max_inline_peer_notifications: profile.max_inline_peer_notifications,
        // Read-only wire projection of the typed, validated schema owner.
        output_schema: profile
            .output_schema
            .as_ref()
            .map(|schema| schema.as_value().clone()),
        provider_params: profile.provider_params.clone().map(Into::into),
    }
}

/// Project a stored realm profile into the public lookup result contract.
#[must_use]
pub fn stored_realm_profile_to_wire(
    stored: &crate::StoredRealmProfile,
) -> meerkat_contracts::MobProfileLookupResult {
    meerkat_contracts::MobProfileLookupResult {
        not_found: false,
        name: stored.name.clone(),
        profile: Some(profile_to_wire(&stored.profile)),
        revision: Some(stored.revision),
        created_at: Some(stored.created_at.to_rfc3339()),
        updated_at: Some(stored.updated_at.to_rfc3339()),
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct MobMemberListEntry {
    /// Canonical member identity.
    pub agent_identity: AgentIdentity,
    /// Member role (profile name).
    pub role: ProfileName,
    pub runtime_mode: MobRuntimeMode,
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
    // which regression-asserts that). `current_session_id` is kept as
    // bridge-internal projection state; public realtime callers use the
    // stable mob-member realtime target instead of routing through this id.
    //
    // `agent_runtime_id` and `fence_token` are binding-era atoms used
    // by the bridge for wiring and stale-command rejection. They are
    // optional because a public list projection may include a machine-known
    // `Unknown` member after recovery without current runtime material.
    // Control paths must route through `binding_atoms()` and fail closed
    // when MobMachine has not supplied a current binding.
    #[serde(skip)]
    pub(crate) agent_runtime_id: Option<AgentRuntimeId>,
    #[serde(skip)]
    pub(crate) fence_token: Option<FenceToken>,
    /// Canonical comms routing ID for bridge-internal peer lookup.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) peer_id: Option<PeerId>,
    /// Transport/auth public key material, separate from canonical `peer_id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) transport_public_key: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) external_peer_specs: BTreeMap<AgentIdentity, TrustedPeerDescriptor>,
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

    /// Typed helper for server-side control dispatch that resolves an
    /// app-facing `WireMemberRef` into the current incarnation before it
    /// enters the work lane. Keeps the fields `pub(crate)` + `#[serde(skip)]`
    /// so they never leak through Serialize/Debug-derived paths.
    pub fn binding_atoms(&self) -> Option<(AgentRuntimeId, FenceToken)> {
        match (&self.agent_runtime_id, self.fence_token) {
            (Some(agent_runtime_id), Some(fence_token)) => {
                Some((agent_runtime_id.clone(), fence_token))
            }
            _ => None,
        }
    }

    pub(crate) fn require_binding_atoms(
        &self,
        context: &str,
    ) -> Result<(AgentRuntimeId, FenceToken), MobError> {
        self.binding_atoms().ok_or_else(|| {
            MobError::Internal(format!(
                "{context} requires MobMachine runtime binding for '{}'",
                self.agent_identity
            ))
        })
    }
}

impl WorkDeliveryReceipt {
    /// Typed accessor for the submitting runtime identity. See
    /// `MobMemberListEntry::binding_atoms` for rationale.
    pub fn runtime_id(&self) -> &AgentRuntimeId {
        &self.runtime_id
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
    pub reason: Option<String>,
}

/// Project a domain peer-connectivity snapshot into its typed wire twin.
fn peer_connectivity_snapshot_to_wire(
    snapshot: MobPeerConnectivitySnapshot,
) -> meerkat_contracts::WirePeerConnectivitySnapshot {
    meerkat_contracts::WirePeerConnectivitySnapshot {
        reachable_peer_count: snapshot.reachable_peer_count,
        unknown_peer_count: snapshot.unknown_peer_count,
        unreachable_peers: snapshot
            .unreachable_peers
            .into_iter()
            .map(|peer| meerkat_contracts::WireUnreachablePeer {
                peer: peer.peer,
                reason: peer.reason,
            })
            .collect(),
    }
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

/// Identity-native owner reference for external-member observation and hooks.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub struct ExternalMemberOwnerRef {
    pub mob_id: MobId,
    pub agent_identity: AgentIdentity,
}

/// Whether an external member is still bridged through a local session or is
/// a peer-only binding.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ExternalMemberBindingMode {
    BridgeSessionBacked,
    PeerOnly,
}

/// Current external-member reachability observation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ExternalMemberReachability {
    /// No live probe has been executed as part of this snapshot.
    Unknown,
    /// The member is known unavailable because canonical restore/binding
    /// projection has failed.
    Unavailable { reason: String },
}

/// Whether the supervisor has enough durable proof to rebind this external
/// member after reconnect.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ExternalMemberRebindStatus {
    /// A bridge session is still present; peer-only rebind is not required.
    NotRequired,
    /// A peer-only binding carries the typed bootstrap proof needed for
    /// supervisor bind fallback.
    Available,
    /// The peer-only binding is missing rebind proof.
    Unavailable { reason: String },
    /// Restore/binding normalization failed.
    Failed { reason: String },
}

/// Declared external-member forwarding hook status.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ExternalMemberForwardingStatus {
    /// The hook owner is typed and can be used by future artifact/approval
    /// forwarding code; this slice does not execute forwarding.
    Declared,
}

/// Stable forwarding hook reference for a mob-owned external member.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub struct ExternalMemberForwardingHookRef {
    pub owner: ExternalMemberOwnerRef,
    pub status: ExternalMemberForwardingStatus,
}

/// External-member artifact/approval forwarding hook projection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub struct ExternalMemberForwardingHooks {
    pub artifacts: ExternalMemberForwardingHookRef,
    pub approvals: ExternalMemberForwardingHookRef,
}

/// Read-only observation block for an external mob member.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub struct ExternalMemberObservationSnapshot {
    pub owner: ExternalMemberOwnerRef,
    pub binding_mode: ExternalMemberBindingMode,
    pub bridge_session_present: bool,
    pub reachability: ExternalMemberReachability,
    pub rebind: ExternalMemberRebindStatus,
    pub forwarding: ExternalMemberForwardingHooks,
}

impl ExternalMemberObservationSnapshot {
    fn hook(owner: &ExternalMemberOwnerRef) -> ExternalMemberForwardingHookRef {
        ExternalMemberForwardingHookRef {
            owner: owner.clone(),
            status: ExternalMemberForwardingStatus::Declared,
        }
    }

    fn forwarding(owner: &ExternalMemberOwnerRef) -> ExternalMemberForwardingHooks {
        ExternalMemberForwardingHooks {
            artifacts: Self::hook(owner),
            approvals: Self::hook(owner),
        }
    }
}

/// Receipt returned by a successful member respawn.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct MemberRespawnReceipt {
    /// The member identity that was respawned.
    pub identity: AgentIdentity,
    /// Binding-era atom: bridge-internal, `pub(crate)` + `#[serde(skip)]`.
    #[serde(skip)]
    pub(crate) agent_runtime_id: AgentRuntimeId,
    /// Binding-era atom: bridge-internal, `pub(crate)` + `#[serde(skip)]`.
    #[serde(skip)]
    pub(crate) previous_fence_token: FenceToken,
    /// Binding-era atom: bridge-internal, `pub(crate)` + `#[serde(skip)]`.
    #[serde(skip)]
    pub(crate) fence_token: FenceToken,
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

/// Domain-side host bind request (§7.2 step 2), built from a
/// [`WireHostBindingDescriptor`](super::bridge_protocol::WireHostBindingDescriptor)
/// by callers. Identity-first (D1): the canonical peer id derives from the
/// descriptor's Ed25519 identity, never from a display name.
#[derive(Debug, Clone)]
pub struct HostBindRequest {
    /// Canonical host peer id derived from the descriptor identity pubkey.
    pub expected_peer_id: PeerId,
    /// The host's Ed25519 signing public key.
    pub pubkey: [u8; 32],
    /// The host acceptor's advertised address.
    pub address: String,
    /// One-time ceremony token from the descriptor (redacted-Debug newtype;
    /// never a principal credential — D3).
    pub bootstrap_token: super::bridge_protocol::BridgeBootstrapToken,
    /// Advertised ws/wss live base URL from the descriptor; the bind reply's
    /// declaration is authoritative (restart truthfulness, DL5).
    pub live_endpoint: Option<String>,
}

impl HostBindRequest {
    /// Build the domain-side bind request from the descriptor `rkat mob host`
    /// wrote (§7.2 plane a). Fails typed when the descriptor identity does not
    /// resolve to a canonical Ed25519 peer identity.
    pub fn from_descriptor(
        descriptor: &super::bridge_protocol::WireHostBindingDescriptor,
    ) -> Result<Self, MobError> {
        let resolved = descriptor.identity.resolve().map_err(|error| {
            MobError::WiringError(format!("invalid host binding descriptor identity: {error}"))
        })?;
        Ok(Self {
            expected_peer_id: resolved.peer_id,
            pubkey: resolved.pubkey,
            address: descriptor.address.clone(),
            bootstrap_token: descriptor.bootstrap_token.clone(),
            live_endpoint: descriptor.live_endpoint.clone(),
        })
    }
}

/// Domain projection of a bound host's declared capability record (the §6.1
/// single enumeration, flattened exactly like the MobMachine host capability
/// maps).
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct HostCapabilityReport {
    pub protocol_min: u64,
    pub protocol_max: u64,
    pub engine_version: String,
    pub durable_sessions: bool,
    pub autonomous_members: bool,
    pub hard_cancel_member: bool,
    pub tracked_input_cancel: bool,
    pub memory_store: bool,
    pub mcp: bool,
    pub resolvable_providers: BTreeSet<String>,
    pub approval_forwarding: bool,
    /// Advertised ws/wss live base URL; `None` = live-incapable (DL5:
    /// presence IS the capability).
    pub live_endpoint: Option<String>,
}

impl HostCapabilityReport {
    /// THE domain→wire capability conversion (ADJ-P7-2): lossless by
    /// construction post the ADJ-P7-1 `WireHostCapabilityFlags` amendment
    /// (`u64` protocol bounds, open `BTreeSet<String>` provider vocabulary —
    /// no silent caps). Shared by `mob/bind_host` result rendering and the
    /// [`MobHandle::hosts`] projection so the field mapping cannot drift
    /// per call site.
    #[must_use]
    pub fn to_wire(&self) -> meerkat_contracts::wire::WireHostCapabilityFlags {
        meerkat_contracts::wire::WireHostCapabilityFlags {
            protocol_min: self.protocol_min,
            protocol_max: self.protocol_max,
            engine_version: self.engine_version.clone(),
            durable_sessions: self.durable_sessions,
            autonomous_members: self.autonomous_members,
            hard_cancel_member: self.hard_cancel_member,
            tracked_input_cancel: self.tracked_input_cancel,
            memory_store: self.memory_store,
            mcp: self.mcp,
            resolvable_providers: self.resolvable_providers.clone(),
            approval_forwarding: self.approval_forwarding,
            live_endpoint: self.live_endpoint.clone(),
        }
    }
}

/// Report returned after a committed host bind (§7.2 step 2).
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct HostBindReport {
    /// Identity-first host id: the host's canonical comms peer id.
    pub host_id: String,
    /// Supervisor authority epoch recorded for the binding (DEC-P2-9: the
    /// supervisor epoch, never a second counter).
    pub epoch: u64,
    /// Capability record declared by the host in the bind reply.
    pub capabilities: HostCapabilityReport,
}

/// Report returned after a committed host revocation (phase 7, ADJ-P7-2:
/// the actor arm knows what it released — the wire
/// `MobRevokeHostResult.released_members` promise is honored, never
/// fabricated surface-side).
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct HostRevokeReport {
    /// Canonical peer id of the revoked host.
    pub host_id: String,
    /// Member identities whose machine-recorded placement pointed at the
    /// revoked host when the revocation committed. Their materializations
    /// lose binding + recipient trust with the host; the placement entries
    /// themselves stay recorded — the §9 revival ladder owns re-placement.
    pub released_members: Vec<AgentIdentity>,
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
    /// Binding-era atom: bridge-internal, `pub(crate)` + `#[serde(skip)]`.
    #[serde(skip)]
    pub(crate) agent_runtime_id: AgentRuntimeId,
    /// Binding-era atom: bridge-internal, `pub(crate)` + `#[serde(skip)]`.
    #[serde(skip)]
    pub(crate) fence_token: FenceToken,
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
    pub(crate) session_origin: super::provisioner::ProvisionSessionOrigin,
    /// Exact executor attachment that may be retired if the enclosing roster
    /// transaction fails after provisioning succeeds. Resumed runtime-backed
    /// sessions always carry this; fresh or external provisions do not.
    #[serde(skip)]
    pub(crate) rollback_authority: Option<super::provisioner::ResumedMemberRollbackAuthority>,
    /// Typed materialize ack for host-materialized members (multi-host
    /// §7.3). `None` for every local/external provisioning path; `Some` iff
    /// the receipt came from `MobProvisioner::materialize_member`, carrying
    /// the transport-validated ack facts the remote commit consumes.
    #[serde(skip)]
    pub(crate) materialized_ack: Option<Box<super::provisioner::MaterializedMemberAck>>,
    /// Actor-classified respawn topology failures carried back through the
    /// deferred placed-spawn reply. Empty for ordinary spawns.
    #[serde(skip)]
    pub(crate) failed_restore_peer_ids: Vec<crate::ids::RespawnTopologyPeerId>,
}

/// Public result from a successful member spawn.
///
/// The identity-native `agent_identity` is the public contract — it is
/// what app-facing payloads surface. `agent_runtime_id` and `fence_token`
/// are carried for crate-internal bridging (provisioning, wiring) but
/// `pub(crate)` so external consumers must route through a `MobMemberView`
/// or the identity seam rather than reading these binding-era atoms
/// directly.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct SpawnResult {
    /// Stable member identity — the one app-facing identity atom.
    pub agent_identity: AgentIdentity,
    /// Composite runtime id. `pub(crate)` — binding-era detail, not
    /// an app-facing identity.
    #[serde(skip)]
    pub(crate) agent_runtime_id: AgentRuntimeId,
    /// Fence token for stale-command rejection. `pub(crate)` — the
    /// bridge uses it; app-facing payloads do not surface it.
    #[serde(skip)]
    pub(crate) fence_token: FenceToken,
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

/// Per-row failure returned by `mob/spawn_many` after MobMachine has classified
/// the public failure cause.
#[derive(Debug)]
#[non_exhaustive]
pub struct MobSpawnManyFailure {
    cause: meerkat_contracts::MobSpawnManyFailureCause,
    error: MobError,
}

impl MobSpawnManyFailure {
    #[must_use]
    pub fn cause(&self) -> meerkat_contracts::MobSpawnManyFailureCause {
        self.cause
    }

    #[must_use]
    pub fn error(&self) -> &MobError {
        &self.error
    }
}

impl std::fmt::Display for MobSpawnManyFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.error.fmt(f)
    }
}

impl std::error::Error for MobSpawnManyFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

fn spawn_many_failure_observation(error: &MobError) -> mob_dsl::MobSpawnManyFailureObservationKind {
    match error {
        MobError::MobNotFound(_) => mob_dsl::MobSpawnManyFailureObservationKind::Internal,
        MobError::ProfileNotFound(_) => {
            mob_dsl::MobSpawnManyFailureObservationKind::ProfileNotFound
        }
        MobError::MemberNotFound(_) => mob_dsl::MobSpawnManyFailureObservationKind::MemberNotFound,
        MobError::MemberAlreadyExists(_) => {
            mob_dsl::MobSpawnManyFailureObservationKind::MemberAlreadyExists
        }
        MobError::NotExternallyAddressable(_) => {
            mob_dsl::MobSpawnManyFailureObservationKind::NotExternallyAddressable
        }
        MobError::InvalidTransition { .. } => {
            mob_dsl::MobSpawnManyFailureObservationKind::InvalidTransition
        }
        MobError::MobMachineRejected { .. } => {
            mob_dsl::MobSpawnManyFailureObservationKind::InvalidTransition
        }
        // The spawn ladder's own machine-composed denial (multi-host
        // placement/portability arms): same family as MobMachineRejected —
        // the machine refused the requested transition.
        MobError::SpawnMemberAdmissionDenied { .. } => {
            mob_dsl::MobSpawnManyFailureObservationKind::InvalidTransition
        }
        // Bridge reply deadline: transport-class (matches
        // `MobError::failure_class`), folded with the comms transport lane.
        MobError::BridgeRequestTimedOut { .. } => {
            mob_dsl::MobSpawnManyFailureObservationKind::CommsError
        }
        // Fork-source session history unavailable (typed W-G retype): the
        // source SESSION could not serve the read — session-class failure.
        MobError::ForkSourceUnavailable { .. } => {
            mob_dsl::MobSpawnManyFailureObservationKind::SessionError
        }
        // Capability-contract replacement is a host bind/rebind concern, not
        // a spawn-many provisioning result.
        MobError::HostCapabilityContractViolation { .. } => {
            mob_dsl::MobSpawnManyFailureObservationKind::Internal
        }
        MobError::WiringError(_) | MobError::RetirementTopologyIncomplete(_) => {
            mob_dsl::MobSpawnManyFailureObservationKind::WiringError
        }
        MobError::SupervisorRotationIncomplete { .. } => {
            mob_dsl::MobSpawnManyFailureObservationKind::SupervisorRotationIncomplete
        }
        // Epoch exhaustion is not reachable from spawn-many provisioning; keep
        // the exhaustive classifier fail-closed if it is ever threaded here.
        MobError::SupervisorEpochExhausted { .. } => {
            mob_dsl::MobSpawnManyFailureObservationKind::Internal
        }
        MobError::BridgeCommandRejected { .. } => {
            mob_dsl::MobSpawnManyFailureObservationKind::BridgeCommandRejected
        }
        MobError::MemberRestoreFailed { .. } => {
            mob_dsl::MobSpawnManyFailureObservationKind::MemberRestoreFailed
        }
        MobError::RuntimeEffectRefused { kind, .. } => match kind {
            crate::error::RuntimeEffectKind::RuntimeBinding => {
                mob_dsl::MobSpawnManyFailureObservationKind::MemberRestoreFailed
            }
            crate::error::RuntimeEffectKind::RuntimeIngress
            | crate::error::RuntimeEffectKind::RuntimeRetire => {
                mob_dsl::MobSpawnManyFailureObservationKind::Internal
            }
        },
        MobError::KickoffWaitTimedOut { .. } => {
            mob_dsl::MobSpawnManyFailureObservationKind::KickoffWaitTimedOut
        }
        MobError::ReadyWaitTimedOut { .. } => {
            mob_dsl::MobSpawnManyFailureObservationKind::ReadyWaitTimedOut
        }
        MobError::DefinitionError(_) => {
            mob_dsl::MobSpawnManyFailureObservationKind::DefinitionError
        }
        MobError::FlowNotFound(_) => mob_dsl::MobSpawnManyFailureObservationKind::FlowNotFound,
        MobError::FlowFailed { .. } => mob_dsl::MobSpawnManyFailureObservationKind::FlowFailed,
        MobError::RunNotFound(_) => mob_dsl::MobSpawnManyFailureObservationKind::RunNotFound,
        MobError::RunCanceled(_) => mob_dsl::MobSpawnManyFailureObservationKind::RunCanceled,
        MobError::FlowTurnTimedOut => mob_dsl::MobSpawnManyFailureObservationKind::FlowTurnTimedOut,
        MobError::FrameDepthLimitExceeded { .. } => {
            mob_dsl::MobSpawnManyFailureObservationKind::FrameDepthLimitExceeded
        }
        MobError::FrameAtomicPersistenceUnavailable { .. } => {
            mob_dsl::MobSpawnManyFailureObservationKind::FrameAtomicPersistenceUnavailable
        }
        MobError::SpecRevisionConflict { .. } => {
            mob_dsl::MobSpawnManyFailureObservationKind::SpecRevisionConflict
        }
        MobError::SchemaValidation { .. } => {
            mob_dsl::MobSpawnManyFailureObservationKind::SchemaValidation
        }
        MobError::InsufficientTargets { .. } => {
            mob_dsl::MobSpawnManyFailureObservationKind::InsufficientTargets
        }
        MobError::TopologyViolation { .. } => {
            mob_dsl::MobSpawnManyFailureObservationKind::TopologyViolation
        }
        MobError::BridgeDeliveryRejected { .. } => {
            mob_dsl::MobSpawnManyFailureObservationKind::BridgeDeliveryRejected
        }
        MobError::SupervisorEscalation(_) => {
            mob_dsl::MobSpawnManyFailureObservationKind::SupervisorEscalation
        }
        MobError::UnsupportedForMode { .. } => {
            mob_dsl::MobSpawnManyFailureObservationKind::UnsupportedForMode
        }
        MobError::MissingMemberCapability { .. } => {
            mob_dsl::MobSpawnManyFailureObservationKind::MissingMemberCapability
        }
        MobError::ResetBarrier => mob_dsl::MobSpawnManyFailureObservationKind::ResetBarrier,
        MobError::StorageError(_) => mob_dsl::MobSpawnManyFailureObservationKind::StorageError,
        MobError::SessionError(_) => mob_dsl::MobSpawnManyFailureObservationKind::SessionError,
        MobError::CommsError(_) => mob_dsl::MobSpawnManyFailureObservationKind::CommsError,
        MobError::CallbackPending { .. } => {
            mob_dsl::MobSpawnManyFailureObservationKind::CallbackPending
        }
        MobError::StaleFenceToken { .. } | MobError::StaleMemberOperatorAuthority { .. } => {
            mob_dsl::MobSpawnManyFailureObservationKind::StaleFenceToken
        }
        MobError::StaleEventCursor { .. } => {
            mob_dsl::MobSpawnManyFailureObservationKind::StaleEventCursor
        }
        MobError::WorkNotFound(_) => mob_dsl::MobSpawnManyFailureObservationKind::WorkNotFound,
        // Per-unit work cancellation is never a spawn-many failure cause;
        // spawn-many provisioning never invokes `cancel_work`. Classify as
        // Internal so the exhaustive match stays total.
        MobError::WorkCancellationUnsupported(_) => {
            mob_dsl::MobSpawnManyFailureObservationKind::Internal
        }
        // Spawn kickoff dispatch never carries injected context (the slot
        // belongs to the submit-work lane), so this cannot be a spawn-many
        // failure cause. Classify as Internal to keep the match total.
        MobError::InjectedContextUndeliverable { .. } => {
            mob_dsl::MobSpawnManyFailureObservationKind::Internal
        }
        // Placed-interaction validation/terminal outcomes and lifecycle
        // cleanup barriers belong to submit-work or lifecycle commands, never
        // spawn-many provisioning. Keep the exhaustive classifier honest
        // without laundering them into a false spawn failure cause.
        MobError::PlacedInteractionIdAlreadyUsed { .. }
        | MobError::InvalidPlacedInteractionId { .. }
        | MobError::PlacedCompletionDeliveryRejected
        | MobError::PlacedCompletionHostNoEffect
        | MobError::PlacedCompletionHostCancelled
        | MobError::PlacedCompletionDisposed
        | MobError::PlacedCompletionCleanupPending { .. }
        | MobError::PlacedKickoffCleanupPending { .. }
        | MobError::AutonomousStopInterruptsPending { .. }
        | MobError::LifecycleOperationPending { .. } => {
            mob_dsl::MobSpawnManyFailureObservationKind::Internal
        }
        MobError::ActorCommandChannelClosed | MobError::ActorReplyChannelClosed => {
            mob_dsl::MobSpawnManyFailureObservationKind::Internal
        }
        MobError::BridgeSessionNotInLiveAuthority { .. } => {
            mob_dsl::MobSpawnManyFailureObservationKind::Internal
        }
        // Member comms-name resolution and flow condition-eval failures are not
        // spawn-many provisioning causes; spawn-many never resolves comms names
        // nor evaluates flow conditions. Classify as Internal to keep the match
        // total.
        MobError::MemberCommsName(_) | MobError::ConditionEval { .. } => {
            mob_dsl::MobSpawnManyFailureObservationKind::Internal
        }
        // Flow-step dispatch classification never runs during spawn-many
        // provisioning (it is the flow lane's admission gate). Classify as
        // Internal to keep the match total.
        MobError::FlowStepDispatchRejected { .. } => {
            mob_dsl::MobSpawnManyFailureObservationKind::Internal
        }
        MobError::Internal(_) | MobError::ExternalMemberCleanupUncertain { .. } => {
            mob_dsl::MobSpawnManyFailureObservationKind::Internal
        }
        // Chokepoint-(a) scope denial: the caller's principal lacked the verb's
        // required ControlScope, so the spawn was refused at admission — the
        // same denied-admission family as SpawnMemberAdmissionDenied above.
        MobError::ScopeDenied(_) => mob_dsl::MobSpawnManyFailureObservationKind::InvalidTransition,
    }
}

fn spawn_many_failure_cause_from_dsl(
    cause: mob_dsl::MobSpawnManyFailureCauseKind,
) -> meerkat_contracts::MobSpawnManyFailureCause {
    match cause {
        mob_dsl::MobSpawnManyFailureCauseKind::ProfileNotFound => {
            meerkat_contracts::MobSpawnManyFailureCause::ProfileNotFound
        }
        mob_dsl::MobSpawnManyFailureCauseKind::MemberNotFound => {
            meerkat_contracts::MobSpawnManyFailureCause::MemberNotFound
        }
        mob_dsl::MobSpawnManyFailureCauseKind::MemberAlreadyExists => {
            meerkat_contracts::MobSpawnManyFailureCause::MemberAlreadyExists
        }
        mob_dsl::MobSpawnManyFailureCauseKind::NotExternallyAddressable => {
            meerkat_contracts::MobSpawnManyFailureCause::NotExternallyAddressable
        }
        mob_dsl::MobSpawnManyFailureCauseKind::InvalidTransition => {
            meerkat_contracts::MobSpawnManyFailureCause::InvalidTransition
        }
        mob_dsl::MobSpawnManyFailureCauseKind::WiringError => {
            meerkat_contracts::MobSpawnManyFailureCause::WiringError
        }
        mob_dsl::MobSpawnManyFailureCauseKind::BridgeCommandRejected => {
            meerkat_contracts::MobSpawnManyFailureCause::BridgeCommandRejected
        }
        mob_dsl::MobSpawnManyFailureCauseKind::MemberRestoreFailed => {
            meerkat_contracts::MobSpawnManyFailureCause::MemberRestoreFailed
        }
        mob_dsl::MobSpawnManyFailureCauseKind::KickoffWaitTimedOut => {
            meerkat_contracts::MobSpawnManyFailureCause::KickoffWaitTimedOut
        }
        mob_dsl::MobSpawnManyFailureCauseKind::ReadyWaitTimedOut => {
            meerkat_contracts::MobSpawnManyFailureCause::ReadyWaitTimedOut
        }
        mob_dsl::MobSpawnManyFailureCauseKind::DefinitionError => {
            meerkat_contracts::MobSpawnManyFailureCause::DefinitionError
        }
        mob_dsl::MobSpawnManyFailureCauseKind::FlowNotFound => {
            meerkat_contracts::MobSpawnManyFailureCause::FlowNotFound
        }
        mob_dsl::MobSpawnManyFailureCauseKind::FlowFailed => {
            meerkat_contracts::MobSpawnManyFailureCause::FlowFailed
        }
        mob_dsl::MobSpawnManyFailureCauseKind::RunNotFound => {
            meerkat_contracts::MobSpawnManyFailureCause::RunNotFound
        }
        mob_dsl::MobSpawnManyFailureCauseKind::RunCanceled => {
            meerkat_contracts::MobSpawnManyFailureCause::RunCanceled
        }
        mob_dsl::MobSpawnManyFailureCauseKind::FlowTurnTimedOut => {
            meerkat_contracts::MobSpawnManyFailureCause::FlowTurnTimedOut
        }
        mob_dsl::MobSpawnManyFailureCauseKind::FrameDepthLimitExceeded => {
            meerkat_contracts::MobSpawnManyFailureCause::FrameDepthLimitExceeded
        }
        mob_dsl::MobSpawnManyFailureCauseKind::FrameAtomicPersistenceUnavailable => {
            meerkat_contracts::MobSpawnManyFailureCause::FrameAtomicPersistenceUnavailable
        }
        mob_dsl::MobSpawnManyFailureCauseKind::SpecRevisionConflict => {
            meerkat_contracts::MobSpawnManyFailureCause::SpecRevisionConflict
        }
        mob_dsl::MobSpawnManyFailureCauseKind::SchemaValidation => {
            meerkat_contracts::MobSpawnManyFailureCause::SchemaValidation
        }
        mob_dsl::MobSpawnManyFailureCauseKind::InsufficientTargets => {
            meerkat_contracts::MobSpawnManyFailureCause::InsufficientTargets
        }
        mob_dsl::MobSpawnManyFailureCauseKind::TopologyViolation => {
            meerkat_contracts::MobSpawnManyFailureCause::TopologyViolation
        }
        mob_dsl::MobSpawnManyFailureCauseKind::BridgeDeliveryRejected => {
            meerkat_contracts::MobSpawnManyFailureCause::BridgeDeliveryRejected
        }
        mob_dsl::MobSpawnManyFailureCauseKind::SupervisorEscalation => {
            meerkat_contracts::MobSpawnManyFailureCause::SupervisorEscalation
        }
        mob_dsl::MobSpawnManyFailureCauseKind::UnsupportedForMode => {
            meerkat_contracts::MobSpawnManyFailureCause::UnsupportedForMode
        }
        mob_dsl::MobSpawnManyFailureCauseKind::MissingMemberCapability => {
            meerkat_contracts::MobSpawnManyFailureCause::MissingMemberCapability
        }
        mob_dsl::MobSpawnManyFailureCauseKind::ResetBarrier => {
            meerkat_contracts::MobSpawnManyFailureCause::ResetBarrier
        }
        mob_dsl::MobSpawnManyFailureCauseKind::StorageError => {
            meerkat_contracts::MobSpawnManyFailureCause::StorageError
        }
        mob_dsl::MobSpawnManyFailureCauseKind::SessionError => {
            meerkat_contracts::MobSpawnManyFailureCause::SessionError
        }
        mob_dsl::MobSpawnManyFailureCauseKind::CommsError => {
            meerkat_contracts::MobSpawnManyFailureCause::CommsError
        }
        mob_dsl::MobSpawnManyFailureCauseKind::CallbackPending => {
            meerkat_contracts::MobSpawnManyFailureCause::CallbackPending
        }
        mob_dsl::MobSpawnManyFailureCauseKind::StaleFenceToken => {
            meerkat_contracts::MobSpawnManyFailureCause::StaleFenceToken
        }
        mob_dsl::MobSpawnManyFailureCauseKind::StaleEventCursor => {
            meerkat_contracts::MobSpawnManyFailureCause::StaleEventCursor
        }
        mob_dsl::MobSpawnManyFailureCauseKind::WorkNotFound => {
            meerkat_contracts::MobSpawnManyFailureCause::WorkNotFound
        }
        mob_dsl::MobSpawnManyFailureCauseKind::Internal => {
            meerkat_contracts::MobSpawnManyFailureCause::Internal
        }
    }
}

/// Project a [`MobError`] into its closed public wire failure code.
///
/// Reuses the canonical MobMachine-owned classification table: the shell
/// extracts the raw `MobError -> ObservationKind` observation, drives a
/// throwaway machine authority to obtain the machine's typed cause ruling, and
/// lowers that ruling into the public [`meerkat_contracts::MobSpawnManyFailureCause`]
/// vocabulary. The verdict is the machine's, not the shell's; if the machine
/// emits no typed cause the projection fails closed to `Internal` rather than
/// fabricating a more specific class.
#[must_use]
pub fn mob_error_wire_code(error: &MobError) -> meerkat_contracts::MobSpawnManyFailureCause {
    let observation = spawn_many_failure_observation(error);
    let mut authority = mob_dsl::MobMachineAuthority::new();
    let cause = mob_dsl::MobMachineMutator::apply(
        &mut authority,
        mob_dsl::MobMachineInput::ClassifySpawnManyFailure { observation },
    )
    .ok()
    .and_then(|transition| {
        transition.effects().iter().find_map(|effect| match effect {
            mob_dsl::MobMachineEffect::SpawnManyFailureClassified {
                observation: effect_observation,
                cause,
            } if *effect_observation == observation => Some(*cause),
            _ => None,
        })
    });
    match cause {
        Some(cause) => spawn_many_failure_cause_from_dsl(cause),
        None => meerkat_contracts::MobSpawnManyFailureCause::Internal,
    }
}

#[derive(Clone)]
pub(crate) struct CanonicalOpsOwnerContext {
    pub(crate) owner_bridge_session_id: SessionId,
    pub(crate) ops_registry: Arc<dyn OpsLifecycleRegistry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[doc(hidden)]
pub struct OwnerBridgeSessionLifecycleAuthority {
    pub bridge_session_id: SessionId,
    pub destroy_on_owner_archive: bool,
    pub implicit_delegation_mob: bool,
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
        failed_peer_ids: Vec<crate::ids::RespawnTopologyPeerId>,
    },

    /// Retire cleanup progressed far enough that the old member may still exist,
    /// but respawn could not prove it was fully cleaned up.
    #[error("previous member cleanup ambiguous for member {}", report.identity)]
    PreviousMemberCleanupAmbiguous { report: PreviousMemberCleanupReport },

    /// An underlying mob error occurred before mutation.
    #[error(transparent)]
    Mob(#[from] MobError),
}

// NOTE (ADJ-P7-4): `MobRespawnError::wire_detail()` — the delegating §17.4
// console wire projection — lives in `crate::error` beside the `MobError`
// projection (that module owns the variant→code knowledge), not here.

/// Receipt returned by member message delivery.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct MemberDeliveryReceipt {
    /// The member identity.
    pub identity: AgentIdentity,
    /// How the message was handled.
    pub handling_mode: HandlingMode,
    /// Binding-era atom: bridge-internal, `pub(crate)` + `#[serde(skip)]`.
    #[serde(skip)]
    pub(crate) agent_runtime_id: AgentRuntimeId,
    /// Binding-era atom: bridge-internal, `pub(crate)` + `#[serde(skip)]`.
    #[serde(skip)]
    pub(crate) fence_token: FenceToken,
}

/// Sender for the live event stream of a single member turn.
///
/// Runtime-backed members forward nonterminal events live. Terminal events
/// are withheld until the MeerkatMachine commits the corresponding boundary.
pub type MemberTurnEventSender =
    tokio::sync::mpsc::Sender<meerkat_core::EventEnvelope<meerkat_core::AgentEvent>>;

pub(crate) type MemberTurnLlmIdentityAppliedSender =
    tokio::sync::oneshot::Sender<Result<Option<meerkat_core::SessionLlmIdentity>, MobError>>;

/// Host-owned observation channels attached to one external member turn.
///
/// Keeping these channels together makes the admission carrier explicit and
/// prevents the canonical external-turn seam from growing one positional
/// argument for every independently observed lifecycle fact.
#[derive(Default)]
pub(super) struct MemberTurnObservers {
    pub(super) event_tx: Option<MemberTurnEventSender>,
    pub(super) completion_tx: Option<tokio::sync::oneshot::Sender<Result<(), MobError>>>,
    pub(super) llm_identity_applied_tx: Option<MemberTurnLlmIdentityAppliedSender>,
}

/// Host-owned options for one member turn.
///
/// Runtime-authored metadata (execution classification, terminal peer intent,
/// and run identity) is deliberately absent. The mob runtime stamps those
/// facts at admission instead of accepting them from callers. The full
/// [`TurnToolOverlay`] remains available to trusted in-process hosts, including
/// caller-authored dispatch context; public wire surfaces use the narrower
/// `PublicTurnToolOverlay` instead.
#[derive(Debug, Clone, Default, PartialEq)]
#[non_exhaustive]
pub struct MemberTurnOptions {
    pub model: Option<ModelId>,
    pub provider: Option<Provider>,
    /// Exact local-server route for self-hosted models. This is deliberately
    /// part of the typed turn identity instead of being inferred from a model
    /// id that may be shared by multiple local servers.
    pub self_hosted_server_id: Option<String>,
    pub provider_params: Option<TurnMetadataOverride<ProviderParamsOverride>>,
    pub auth_binding: Option<TurnMetadataOverride<AuthBindingRef>>,
    pub skill_references: Option<Vec<SkillKey>>,
    pub turn_tool_overlay: Option<TurnToolOverlay>,
    pub additional_instructions: Option<Vec<TurnInstruction>>,
    pub keep_alive: Option<KeepAliveDirective>,
    pub render_metadata: Option<RenderMetadata>,
    pub interaction_id: Option<meerkat_core::interaction::InteractionId>,
    pub objective_id: Option<meerkat_core::interaction::ObjectiveId>,
}

impl MemberTurnOptions {
    /// Construct empty host-owned options for one member turn.
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_model(mut self, model: ModelId) -> Self {
        self.model = Some(model);
        self
    }

    pub fn with_provider(mut self, provider: Provider) -> Self {
        self.provider = Some(provider);
        self
    }

    pub fn with_self_hosted_server_id(mut self, server_id: impl Into<String>) -> Self {
        self.self_hosted_server_id = Some(server_id.into());
        self
    }

    /// Set or explicitly clear provider parameters for this turn.
    pub fn with_provider_params(
        mut self,
        provider_params: TurnMetadataOverride<ProviderParamsOverride>,
    ) -> Self {
        self.provider_params = Some(provider_params);
        self
    }

    /// Set or explicitly clear the auth binding for this turn.
    pub fn with_auth_binding(mut self, auth_binding: TurnMetadataOverride<AuthBindingRef>) -> Self {
        self.auth_binding = Some(auth_binding);
        self
    }

    pub fn with_skill_references(mut self, skill_references: Vec<SkillKey>) -> Self {
        self.skill_references = Some(skill_references);
        self
    }

    pub fn with_turn_tool_overlay(mut self, turn_tool_overlay: TurnToolOverlay) -> Self {
        self.turn_tool_overlay = Some(turn_tool_overlay);
        self
    }

    pub fn with_additional_instructions(
        mut self,
        additional_instructions: Vec<TurnInstruction>,
    ) -> Self {
        self.additional_instructions = Some(additional_instructions);
        self
    }

    pub fn with_keep_alive(mut self, keep_alive: KeepAliveDirective) -> Self {
        self.keep_alive = Some(keep_alive);
        self
    }

    pub fn with_render_metadata(mut self, render_metadata: RenderMetadata) -> Self {
        self.render_metadata = Some(render_metadata);
        self
    }

    pub fn with_interaction_id(
        mut self,
        interaction_id: meerkat_core::interaction::InteractionId,
    ) -> Self {
        self.interaction_id = Some(interaction_id);
        self
    }

    pub fn with_objective_id(
        mut self,
        objective_id: meerkat_core::interaction::ObjectiveId,
    ) -> Self {
        self.objective_id = Some(objective_id);
        self
    }

    fn into_runtime_metadata(self, handling_mode: HandlingMode) -> RuntimeTurnMetadata {
        RuntimeTurnMetadata {
            handling_mode: Some(handling_mode),
            skill_references: self.skill_references,
            turn_tool_overlay: self.turn_tool_overlay,
            additional_instructions: self.additional_instructions,
            model: self.model,
            provider: self.provider,
            self_hosted_server_id: self.self_hosted_server_id,
            provider_params: self.provider_params,
            auth_binding: self.auth_binding,
            keep_alive: self.keep_alive,
            render_metadata: self.render_metadata,
            transcript_identity: meerkat_core::types::TranscriptMessageIdentity {
                interaction_id: self.interaction_id,
                objective_id: self.objective_id,
                run_id: None,
            },
            ..Default::default()
        }
    }
}

/// Completion-bearing handle for an admitted member turn.
///
/// The receipt proves canonical mob admission. [`Self::wait`] resolves only
/// after the runtime's completion boundary commits, and returns an error when
/// execution fails after admission.
#[must_use = "member turns must be awaited or deliberately detached"]
#[derive(Debug)]
pub struct MemberTurnHandle {
    receipt: MemberDeliveryReceipt,
    session_id: Option<SessionId>,
    completion_rx: tokio::sync::oneshot::Receiver<Result<(), MobError>>,
    llm_identity_applied_rx: Option<
        tokio::sync::oneshot::Receiver<Result<Option<meerkat_core::SessionLlmIdentity>, MobError>>,
    >,
}

impl MemberTurnHandle {
    pub fn receipt(&self) -> &MemberDeliveryReceipt {
        &self.receipt
    }

    /// Bridge session admitted for this exact member runtime binding.
    pub fn session_id(&self) -> Option<&SessionId> {
        self.session_id.as_ref()
    }

    /// Wait until the exact serialized executor boundary has either applied
    /// this turn's requested LLM identity or determined that the turn carries
    /// no identity override.
    ///
    /// `Ok(Some(identity))` is authoritative applied live identity and remains
    /// valid even if the later agent turn or its finalization fails. Backing
    /// services that persist reconfiguration also make it durable; ephemeral
    /// or custom hosts need not. `Ok(None)` means this turn did not request an
    /// identity change. This is deliberately distinct from both ingress
    /// admission and [`Self::wait`] terminal completion.
    pub async fn wait_for_applied_llm_identity(
        &mut self,
    ) -> Result<Option<meerkat_core::SessionLlmIdentity>, MobError> {
        let receiver = self.llm_identity_applied_rx.take().ok_or_else(|| {
            MobError::Internal(
                "member turn LLM identity application was already awaited".to_string(),
            )
        })?;
        receiver.await.map_err(|_| {
            MobError::Internal(
                "member turn LLM identity application channel closed before executor boundary"
                    .to_string(),
            )
        })?
    }

    pub async fn wait(self) -> Result<MemberDeliveryReceipt, MobError> {
        match self.completion_rx.await {
            Ok(Ok(())) => Ok(self.receipt),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(MobError::Internal(
                "member turn completion channel closed before terminal outcome".to_string(),
            )),
        }
    }
}

/// Receipt returned by sender-aware mob peer-message delivery.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct PeerMessageReceipt {
    /// Sender mob-member identity.
    pub from: AgentIdentity,
    /// Recipient mob-member identity.
    pub to: AgentIdentity,
    /// Transport envelope id for the typed peer message.
    pub envelope_id: uuid::Uuid,
    /// Strongest delivery fact proved by the selected transport.
    pub delivery: meerkat_core::comms::PeerDeliveryOutcome,
    /// How the recipient should handle the peer message.
    pub handling_mode: HandlingMode,
}

/// Receipt confirming that a unit of work was accepted by the work lane.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct WorkDeliveryReceipt {
    /// The work reference for the submitted unit.
    pub work_ref: WorkRef,
    /// Binding-era atom: bridge-internal, `pub(crate)` + `#[serde(skip)]`.
    #[serde(skip)]
    pub(crate) runtime_id: AgentRuntimeId,
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
    /// Explicit auth binding used for the helper member's agent build.
    pub auth_binding: Option<meerkat_core::AuthBindingRef>,
    /// Parent/composition-authorized inherited tool filter from scheduled or agent-owned tooling resolution.
    pub inherited_tool_filter: Option<meerkat_core::InheritedToolVisibilityAuthority>,
    /// Override profile resolved from scheduled or agent-owned tooling resolution.
    pub override_profile: Option<crate::profile::Profile>,
    /// Field-scoped model override reapplied over the current role profile on
    /// every materialization. Unlike `override_profile`, this does not freeze
    /// unrelated profile fields across definition drift.
    pub model_override: Option<String>,
    /// Objective causality inherited from the spawning turn.
    pub objective_id: Option<meerkat_core::interaction::ObjectiveId>,
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
    /// Identity-native runtime ID for this incarnation.
    ///
    /// Binding-era atom: bridge-internal, `pub(crate)` + `#[serde(skip)]`.
    #[serde(skip)]
    pub(crate) agent_runtime_id: AgentRuntimeId,
    /// Fence token for the current incarnation.
    ///
    /// Binding-era atom: bridge-internal, `pub(crate)` + `#[serde(skip)]`.
    #[serde(skip)]
    pub(crate) fence_token: FenceToken,
}

/// Target for a wire operation from a local mob member.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PeerTarget {
    /// Another member in the same mob roster.
    Local(AgentIdentity),
    /// External peer handle for operations that only need the mob-owned edge.
    ExternalName(meerkat_core::comms::PeerName),
    /// A typed external binding request resolved by the mob actor before trust install.
    ExternalBinding(ExternalPeerBindingSpec),
    /// A trusted peer that lives outside the local mob roster.
    External(TrustedPeerDescriptor),
}

/// Summary for one dense local-member topology materialization pass.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MobWireMembersBatchReport {
    /// Number of edges requested before normalization/deduplication.
    pub requested: usize,
    /// Normalized unique edges already present in the MobMachine graph.
    pub already_wired: Vec<crate::event::MemberWireEdge>,
    /// Normalized unique edges newly admitted by the MobMachine graph.
    pub wired: Vec<crate::event::MemberWireEdge>,
}

/// Typed request to bind a local member to an external peer.
///
/// App-facing surfaces provide this shape instead of comms-owned `peer_id` /
/// `pubkey` atoms. The mob actor resolves the evidence into a
/// `TrustedPeerDescriptor` immediately before the machine admits the external
/// edge and before any trust is installed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalPeerBindingSpec {
    pub name: String,
    pub address: String,
    pub identity: meerkat_contracts::WireTrustedPeerIdentity,
}

impl ExternalPeerBindingSpec {
    pub fn new(
        name: impl Into<String>,
        address: impl Into<String>,
        identity: meerkat_contracts::WireTrustedPeerIdentity,
    ) -> Self {
        Self {
            name: name.into(),
            address: address.into(),
            identity,
        }
    }
}

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
    pub(super) command_tx: mpsc::Sender<super::scope_gate::RoutedMobCommand>,
    /// The authority lane this handle's commands travel on (phase 5,
    /// DEC-P5E-2). `MobBuilder` binds the launched handle to the OWNER
    /// principal console (explicit launch-site mint, A16); surfaces rebind
    /// clones via [`MobHandle::with_command_authority`].
    pub(super) command_authority: crate::control_policy::CommandAuthority,
    pub(super) roster: Arc<RwLock<RosterAuthority>>,
    pub(super) definition: Arc<MobDefinition>,
    pub(super) events: Arc<dyn MobEventStore>,
    pub(super) run_store: Arc<dyn MobRunStore>,
    pub(super) flow_streams:
        Arc<tokio::sync::Mutex<BTreeMap<RunId, mpsc::Sender<meerkat_core::ScopedAgentEvent>>>>,
    pub(super) session_service: Arc<dyn MobSessionService>,
    #[cfg(feature = "runtime-adapter")]
    pub(super) runtime_adapter: Option<Arc<meerkat_runtime::MeerkatMachine>>,
    pub(super) restore_diagnostics: Arc<RwLock<HashMap<AgentIdentity, RestoreFailureDiagnostic>>>,
    pub(super) supervisor_bridge: Arc<MobSupervisorBridge>,
    /// Read-only projection of the actor-owned MobMachine state. The actor is
    /// the sole writer; handles use this only for non-blocking status/list
    /// surfaces that must remain observable while a mutating command is
    /// awaiting shell cleanup.
    pub(super) machine_state_watch_rx: tokio::sync::watch::Receiver<mob_dsl::MobMachineState>,
    pub(super) reachability_observations: Arc<ReachabilityObservations>,
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
}

impl MobHandle {
    /// Rebind a clone of this handle onto `authority` (phase 5, ADJ-P5-10).
    ///
    /// This is the PRODUCTION principal-binding seam: local single-user
    /// surfaces bind `CommandAuthority::principal(MobControlPrincipal::Owner)`
    /// (A16); the v2 bearer-token resolver binds validated `External`
    /// principals through this same method; in-crate agent-lane consumers
    /// rebind with the `pub(crate)` agent-lane authority.
    #[must_use]
    pub fn with_command_authority(
        mut self,
        authority: crate::control_policy::CommandAuthority,
    ) -> MobHandle {
        self.command_authority = authority;
        self
    }

    /// Lane introspection for pins/audit (T-LS4).
    #[must_use]
    pub fn command_authority_kind(&self) -> crate::control_policy::CommandAuthorityKind {
        self.command_authority.kind()
    }

    /// Chokepoint-(b) resolver over the committed machine projection: the
    /// sealed policy for this handle's bound principal at `now_ms`.
    ///
    /// Serves the verbs that never send a MobCommand (watch-read
    /// projections, event-router subscription admission). Actor-routed
    /// verbs are gated once, at chokepoint (a), against the actor's own
    /// serialized state — not here. Agent-lane / internal bindings resolve
    /// as `Unresolved` (zero scopes): those lanes must never satisfy
    /// principal checks (§15.2).
    pub fn resolve_control_policy(
        &self,
        now_ms: u64,
    ) -> Result<crate::control_policy::ResolvedControlPolicy, MobError> {
        let state = self.machine_state_watch_rx.borrow().clone();
        let policy = match self.command_authority.control_principal() {
            Some(principal) => {
                crate::control_policy::ResolvedControlPolicy::resolve(principal, &state, now_ms)
            }
            None => crate::control_policy::ResolvedControlPolicy::resolve(
                &crate::control_policy::MobControlPrincipal::Unresolved,
                &state,
                now_ms,
            ),
        };
        Ok(policy)
    }

    /// Accessor for the realtime session factory carried from
    /// [`super::MobBuilder::with_realtime_session_factory`] (W2-E).
    pub fn realtime_session_factory(
        &self,
    ) -> Option<Arc<dyn meerkat_client::RealtimeSessionFactory>> {
        self.realtime_session_factory.as_ref().map(Arc::clone)
    }

    /// Authorize an external peer to trust a local mob member after
    /// `WireExternalPeer` has established the MobMachine-owned edge.
    pub async fn apply_external_peer_reciprocal_trust(
        &self,
        local: &AgentIdentity,
        external_peer_name: &str,
        target_comms: std::sync::Arc<dyn CommsRuntime>,
        peer: TrustedPeerDescriptor,
    ) -> Result<(), MobError> {
        let key = mob_dsl::ExternalPeerKey::new(
            mob_dsl::AgentIdentity::from_domain(local),
            mob_dsl::PeerName::from(external_peer_name),
        );
        self.send_actor_command(|reply_tx| MobCommand::ApplyExternalPeerReciprocalTrust {
            key,
            target_comms,
            peer,
            reply_tx,
        })
        .await?
    }

    /// Return the routable signed supervisor bridge peer for external
    /// members that must answer mob control-plane requests.
    pub async fn routable_supervisor_peer(&self) -> Result<TrustedPeerDescriptor, MobError> {
        self.supervisor_bridge.routable_supervisor_spec().await
    }

    /// Typed member machine projection (ADJ-P5-18: a denial or transport
    /// failure is an `Err`, never a defaulted projection).
    async fn member_machine_projection(
        &self,
        agent_identity: &AgentIdentity,
    ) -> Result<super::state::MobMemberMachineProjection, MobError> {
        self.send_actor_command(
            |reply_tx| super::state::MobCommand::MemberMachineProjection {
                agent_identity: AgentIdentity::from(agent_identity.as_str()),
                reply_tx,
            },
        )
        .await?
    }
}

#[derive(Debug, Clone)]
pub(crate) struct RestoreFailureDiagnostic {
    pub(crate) bridge_session_id: Option<SessionId>,
    pub(crate) reason: String,
}

/// The machine's per-member subscribe verdict (phase 6): a local
/// session-bound stream or the external (pump-tap) realization — the
/// `AuthorizeExternalAgentEventSubscription` third outcome.
enum AgentEventSubscriptionAuthority {
    Local(SessionId),
    External,
}

/// Clone-cheap, capability-bearing handle for interacting with one mob member.
///
/// This is the target 0.5 API surface for message/turn submission. The mob
/// handle remains orchestration/control-plane oriented, while member-directed
/// delivery goes through this narrower capability.
#[derive(Clone)]
pub struct MemberHandle {
    mob: MobHandle,
    agent_identity: AgentIdentity,
}

#[derive(Clone)]
pub struct MobEventsView {
    handle: MobHandle,
}

/// Configuration for a structural mob event subscription.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct MobEventsSubscriptionConfig {
    /// Cursor to start after. `None` starts at the current latest cursor.
    pub after_cursor: Option<u64>,
    /// Maximum number of persisted events read per catch-up batch.
    pub batch_limit: usize,
    /// Capacity of the output event channel.
    pub channel_capacity: usize,
}

impl Default for MobEventsSubscriptionConfig {
    fn default() -> Self {
        Self {
            after_cursor: None,
            batch_limit: 128,
            channel_capacity: 256,
        }
    }
}

/// Handle for a structural mob event subscription.
///
/// Receives persisted [`crate::event::MobEvent`] records from the mob event
/// ledger. Drop the handle, or call [`Self::cancel`], to stop the background
/// forwarding task.
pub struct MobEventsSubscription {
    pub event_rx: mpsc::Receiver<crate::event::MobEvent>,
    cancel: CancellationToken,
}

impl MobEventsSubscription {
    pub fn cancel(&self) {
        self.cancel.cancel();
    }
}

impl Drop for MobEventsSubscription {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// Typed source of a mob member spawn request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum SpawnSource {
    Consumer,
    AgentSpawnMember,
    HelperSpawn,
    BatchItem,
    FlowProvisioning,
    PolicySpawn,
    Respawn,
    Resume,
    Fork,
    /// Actor-owned level-triggered materialization from an already sealed
    /// `IdentityIntent`. Portable customization has already happened before
    /// apply and must not run again on recovery.
    IdentityReconcile,
}

impl SpawnSource {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Consumer => "consumer",
            Self::AgentSpawnMember => "agent_spawn_member",
            Self::HelperSpawn => "helper_spawn",
            Self::BatchItem => "batch_item",
            Self::FlowProvisioning => "flow_provisioning",
            Self::PolicySpawn => "policy_spawn",
            Self::Respawn => "respawn",
            Self::Resume => "resume",
            Self::Fork => "fork",
            Self::IdentityReconcile => "identity_reconcile",
        }
    }

    #[must_use]
    fn for_launch_mode(base: Self, launch_mode: &crate::launch::MemberLaunchMode) -> Self {
        match launch_mode {
            crate::launch::MemberLaunchMode::Resume { .. } => Self::Resume,
            crate::launch::MemberLaunchMode::Fork { .. } => Self::Fork,
            crate::launch::MemberLaunchMode::Fresh => base,
        }
    }

    #[must_use]
    pub(crate) fn allows_reserved_flow_identity(self) -> bool {
        matches!(self, Self::FlowProvisioning)
    }
}

/// Typed system prompt replacement for a single spawn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum SpawnSystemPromptOverride {
    Replace(String),
    Disable,
}

/// Durable identity continuity intent attached to a spawn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum SpawnContinuityIntent {
    #[default]
    Ephemeral,
    DurableIdentity {
        continuity_key: String,
    },
}

/// Build-boundary context supplied to [`SpawnMemberCustomizer`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct SpawnCustomizationContext {
    pub mob_id: MobId,
    pub spawn_source: SpawnSource,
    pub spawner_identity: Option<AgentIdentity>,
    pub spawner_runtime_id: Option<AgentRuntimeId>,
    pub requested_profile: ProfileName,
}

/// Narrow pre-build mutator for per-spawn construction inputs.
pub trait SpawnMemberCustomizer: Send + Sync {
    fn customize_spawn(
        &self,
        ctx: &SpawnCustomizationContext,
        spec: &mut SpawnMemberSpec,
    ) -> Result<(), MobError>;
}

/// Spawn request for first-class batch member provisioning.
#[derive(Clone)]
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
    /// Hard resource caps for the spawned member session.
    pub budget_limits: Option<meerkat_core::BudgetLimits>,
    /// When true, automatically wire this member to its spawner.
    pub auto_wire_parent: bool,
    /// Additional instruction sections appended to the system prompt for this member.
    pub additional_instructions: Option<Vec<String>>,
    /// Per-agent environment variables injected into shell tool subprocesses.
    pub shell_env: Option<std::collections::HashMap<String, String>>,
    /// Parent/composition-authorized inherited tool filter from spawn tooling resolution.
    ///
    /// When set, carried as an opaque typed handoff so the runtime-backed core
    /// build restores it through the generated visibility owner.
    pub inherited_tool_filter: Option<meerkat_core::InheritedToolVisibilityAuthority>,
    /// Override profile resolved from `SpawnTooling::Profile` source.
    ///
    /// When set, the spawn path uses this profile instead of looking up by
    /// `role_name` from the mob definition. This allows agent-owned spawn
    /// tooling to specify a different model/skills/tools via inline or
    /// realm-scoped profiles.
    pub override_profile: Option<crate::profile::Profile>,
    /// Field-scoped model override.
    ///
    /// The runtime resolves the current role profile first and then applies
    /// this value, including on cold restore, revival, and respawn. This is the
    /// correct seam for model-only re-profiling because tools, skills, and peer
    /// posture continue to follow the definition.
    pub model_override: Option<String>,
    /// Objective causality inherited from the spawning turn.
    pub objective_id: Option<meerkat_core::interaction::ObjectiveId>,
    /// Per-member auth binding. When set, this member's agent builds with
    /// `AgentBuildConfig.auth_binding = Some(this)`, scoping credential
    /// resolution to the named realm + binding. `None` means the caller did not
    /// provide binding authority; build paths that require a binding must reject
    /// the spawn instead of promoting an ambient fallback.
    pub auth_binding: Option<meerkat_core::AuthBindingRef>,
    /// Per-spawn external tool overlay. In-process only; retained by the mob
    /// actor for the member's lifetime so machine-authorized revival
    /// recomposes it (replaced or cleared by respawn replacement semantics,
    /// dropped at retire). Not persisted: re-supply across process restarts
    /// via `SpawnMemberCustomizer` (`SpawnSource::Resume`).
    pub external_tools: Option<Arc<dyn AgentToolDispatcher>>,
    /// Typed prompt replacement for this spawn.
    pub system_prompt_override: Option<SpawnSystemPromptOverride>,
    /// Explicit helper/member continuity intent.
    pub continuity_intent: SpawnContinuityIntent,
    /// Multi-host placement (ADJ-7): the bound member host this member is
    /// materialized on. The value is the host's canonical comms peer id
    /// (identity-first, D1); admission is machine-owned — an unbound or
    /// garbage id is a typed `HostNotBound` denial at the spawn ladder,
    /// never a shell-side probe. `None` = controlling host (local member).
    pub placement: Option<crate::machines::mob_machine::HostId>,
}

impl std::fmt::Debug for SpawnMemberSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpawnMemberSpec")
            .field("role_name", &self.role_name)
            .field("identity", &self.identity)
            .field("initial_message", &self.initial_message)
            .field("runtime_mode", &self.runtime_mode)
            .field("backend", &self.backend)
            .field("binding", &self.binding)
            .field("context", &self.context)
            .field("labels", &self.labels)
            .field("launch_mode", &self.launch_mode)
            .field("tool_access_policy", &self.tool_access_policy)
            .field("budget_limits", &self.budget_limits)
            .field("auto_wire_parent", &self.auto_wire_parent)
            .field("additional_instructions", &self.additional_instructions)
            .field("shell_env", &self.shell_env)
            .field("inherited_tool_filter", &self.inherited_tool_filter)
            .field("override_profile", &self.override_profile)
            .field("model_override", &self.model_override)
            .field("objective_id", &self.objective_id)
            .field("auth_binding", &self.auth_binding)
            .field("external_tools", &self.external_tools.is_some())
            .field("system_prompt_override", &self.system_prompt_override)
            .field("continuity_intent", &self.continuity_intent)
            .field("placement", &self.placement)
            .finish()
    }
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
            budget_limits: None,
            auto_wire_parent: false,
            additional_instructions: None,
            shell_env: None,
            inherited_tool_filter: None,
            override_profile: None,
            model_override: None,
            objective_id: None,
            auth_binding: None,
            external_tools: None,
            system_prompt_override: None,
            continuity_intent: SpawnContinuityIntent::Ephemeral,
            placement: None,
        }
    }

    /// Place this member on a bound member host (multi-host mobs §7.3).
    pub fn with_placement(mut self, host: crate::machines::mob_machine::HostId) -> Self {
        self.placement = Some(host);
        self
    }

    /// Set the per-member auth binding (deferral §1).
    pub fn with_auth_binding(mut self, conn_ref: meerkat_core::AuthBindingRef) -> Self {
        self.auth_binding = Some(conn_ref);
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

    pub fn with_budget_limits(mut self, limits: meerkat_core::BudgetLimits) -> Self {
        self.budget_limits = Some(limits);
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
    pub async fn latest_cursor(&self) -> Result<u64, MobError> {
        self.handle
            .events
            .latest_cursor()
            .await
            .map_err(MobError::from)
    }

    /// Subscribe to structural mob events recorded in the mob event ledger.
    ///
    /// This is distinct from [`MobHandle::subscribe_mob_events`], which routes
    /// member-agent events. The returned stream yields [`crate::event::MobEvent`]
    /// records and starts after the current latest cursor.
    pub async fn subscribe(&self) -> Result<MobEventsSubscription, MobError> {
        self.subscribe_with_config(MobEventsSubscriptionConfig::default())
            .await
    }

    /// Subscribe to structural mob events after an explicit cursor.
    pub async fn subscribe_after(
        &self,
        after_cursor: u64,
    ) -> Result<MobEventsSubscription, MobError> {
        self.subscribe_with_config(MobEventsSubscriptionConfig {
            after_cursor: Some(after_cursor),
            ..MobEventsSubscriptionConfig::default()
        })
        .await
    }

    /// Like [`Self::subscribe`] with explicit catch-up and channel settings.
    pub async fn subscribe_with_config(
        &self,
        config: MobEventsSubscriptionConfig,
    ) -> Result<MobEventsSubscription, MobError> {
        let config = MobEventsSubscriptionConfig {
            batch_limit: config.batch_limit.max(1),
            channel_capacity: config.channel_capacity.max(1),
            ..config
        };
        let explicit_after_cursor = config.after_cursor.is_some();
        let latest_cursor = self.latest_cursor().await?;
        let after_cursor = config.after_cursor.unwrap_or(latest_cursor);
        let batch_limit = u64::try_from(config.batch_limit).map_err(|_| {
            MobError::Internal(
                "structural event batch limit does not fit generated authority input".into(),
            )
        })?;
        let channel_capacity = u64::try_from(config.channel_capacity).map_err(|_| {
            MobError::Internal(
                "structural event channel capacity does not fit generated authority input".into(),
            )
        })?;
        let effects = self
            .handle
            .apply_machine_input_effects(mob_dsl::MobMachineInput::SubscribeStructuralEvents {
                after_cursor,
                latest_cursor,
                explicit_after_cursor,
                batch_limit,
                channel_capacity,
            })
            .await?;
        let (after_cursor, explicit_after_cursor, config) =
            MobHandle::structural_event_subscription_authority_from_effects(effects)?;
        let source_rx = self.handle.events.subscribe().map_err(MobError::from)?;
        Ok(spawn_structural_event_subscription(
            self.clone(),
            source_rx,
            after_cursor,
            explicit_after_cursor,
            config,
        ))
    }

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

    pub async fn poll_strict(
        &self,
        after_cursor: u64,
        limit: usize,
    ) -> Result<Vec<crate::event::MobEvent>, MobError> {
        let latest_cursor = self.latest_cursor().await?;
        let limit = u64::try_from(limit).map_err(|_| {
            MobError::Internal(
                "strict event poll limit does not fit generated authority input".into(),
            )
        })?;
        let effects = self
            .handle
            .apply_machine_input_effects(mob_dsl::MobMachineInput::PollEventsStrict {
                after_cursor,
                latest_cursor,
                limit,
            })
            .await?;
        let (after_cursor, limit) = MobHandle::strict_event_poll_authority_from_effects(effects)?;
        self.poll(after_cursor, limit).await
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

#[allow(clippy::ignored_unit_patterns)]
fn spawn_structural_event_subscription(
    events: MobEventsView,
    mut source_rx: crate::store::MobEventReceiver,
    mut cursor: u64,
    catch_up_on_start: bool,
    config: MobEventsSubscriptionConfig,
) -> MobEventsSubscription {
    let (event_tx, event_rx) = mpsc::channel(config.channel_capacity);
    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();

    tokio::spawn(async move {
        if catch_up_on_start
            && !catch_up_structural_events(&events, &event_tx, &mut cursor, config.batch_limit)
                .await
        {
            return;
        }

        loop {
            tokio::select! {
                () = cancel_clone.cancelled() => break,
                received = source_rx.recv() => {
                    match received {
                        Ok(event) => {
                            if event.cursor > cursor.saturating_add(1)
                                && !catch_up_structural_events(
                                    &events,
                                    &event_tx,
                                    &mut cursor,
                                    config.batch_limit,
                                )
                                .await
                            {
                                return;
                            }
                            if event.cursor > cursor {
                                cursor = event.cursor;
                                if event_tx.send(event).await.is_err() {
                                    return;
                                }
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                            if !catch_up_structural_events(
                                &events,
                                &event_tx,
                                &mut cursor,
                                config.batch_limit,
                            )
                            .await
                            {
                                return;
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            }
        }
    });

    MobEventsSubscription { event_rx, cancel }
}

async fn catch_up_structural_events(
    events: &MobEventsView,
    event_tx: &mpsc::Sender<crate::event::MobEvent>,
    cursor: &mut u64,
    batch_limit: usize,
) -> bool {
    loop {
        let batch = match events.poll(*cursor, batch_limit).await {
            Ok(batch) => batch,
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    "mob structural event subscription stopped after catch-up failure",
                );
                return false;
            }
        };
        if batch.is_empty() {
            return true;
        }

        let is_complete = batch.len() < batch_limit;
        for event in batch {
            if event.cursor <= *cursor {
                continue;
            }
            *cursor = event.cursor;
            if event_tx.send(event).await.is_err() {
                return false;
            }
        }

        if is_complete {
            return true;
        }
    }
}

impl MobHandle {
    pub(crate) async fn observe_host_runtime_incarnation(
        &self,
        expected_member: super::bridge_protocol::BridgeMemberIncarnation,
        runtime_incarnation: super::bridge_protocol::BridgeHostRuntimeIncarnation,
    ) -> Result<(), MobError> {
        self.send_actor_command(|reply_tx| MobCommand::HostRuntimeIncarnationObserved {
            expected_member,
            runtime_incarnation,
            reply_tx,
        })
        .await?
    }

    async fn restore_failure_for(
        &self,
        agent_identity: &AgentIdentity,
    ) -> Option<RestoreFailureDiagnostic> {
        self.restore_diagnostics
            .read()
            .await
            .get(agent_identity)
            .cloned()
    }

    fn restore_failure_error(
        agent_identity: &AgentIdentity,
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
        let command = build(reply_tx);
        let command_kind = command.kind();
        tracing::debug!(
            command_kind,
            "MobHandle::send_actor_command sending command"
        );
        // Chokepoint (a) routing: every command carries the handle's bound
        // authority lane; the actor gate resolves it before any handler.
        self.command_tx
            .send(super::scope_gate::RoutedMobCommand {
                authority: self.command_authority.clone(),
                cmd: command,
            })
            .await
            .map_err(|_| MobError::ActorCommandChannelClosed)?;
        tracing::debug!(
            command_kind,
            "MobHandle::send_actor_command command sent; awaiting reply"
        );
        reply_rx
            .await
            .map_err(|_| MobError::ActorReplyChannelClosed)
    }

    async fn execute_machine_command(
        &self,
        command: MobMachineCommand,
    ) -> Result<MobMachineCommandResult, MobError> {
        match command {
            MobMachineCommand::PreviewRunFlowAdmission => {
                self.send_actor_command(|reply_tx| MobCommand::PreviewRunFlowAdmission {
                    reply_tx,
                })
                .await??;
                Ok(MobMachineCommandResult::Unit)
            }
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
                spawn_source,
                owner_context,
            } => {
                tracing::debug!(
                    member_id = %spec.identity,
                    profile = %spec.role_name,
                    owner_bound = owner_context.is_some(),
                    "MobHandle::execute_machine_command spawn dispatch"
                );
                let (owner_bridge_session_id, ops_registry) = match owner_context {
                    Some(ctx) => (Some(ctx.owner_bridge_session_id), Some(ctx.ops_registry)),
                    None => (None, None),
                };
                let receipt = self
                    .send_actor_command(|reply_tx| MobCommand::Spawn {
                        spec,
                        spawn_source,
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
                let report = self.handle_reconcile(desired, options).await?;
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
                    turn_metadata,
                    event_tx,
                    completion_tx,
                    llm_identity_applied_tx,
                    ack_mode,
                } = *cmd;
                let receipt_work_ref = work_ref.clone();
                let payload = Box::new(super::state::SubmitWorkPayload {
                    runtime_id,
                    fence_token,
                    work_ref,
                    content: spec.content,
                    origin: spec.origin,
                    injected_context: spec.injected_context,
                    interaction_id: spec.interaction_id,
                    objective_id: spec.objective_id,
                    handling_mode,
                    turn_metadata,
                    event_tx,
                    completion_tx,
                    llm_identity_applied_tx,
                    ack_mode,
                });
                self.send_actor_command(|reply_tx| MobCommand::SubmitWork { payload, reply_tx })
                    .await??;
                Ok(MobMachineCommandResult::WorkReceipt {
                    work_ref: receipt_work_ref,
                })
            }
            MobMachineCommand::CancelWork { work_ref } => {
                // No work-tracking ledger backs per-unit cancellation, so
                // there is no authority that can locate and cancel an
                // individual submitted unit. Fail closed with a typed
                // `WorkCancellationUnsupported` so the advertised
                // `mob/cancel_work` surface stays honest (advertise == deliver)
                // instead of returning a phantom `WorkNotFound` that lies about
                // having searched a ledger. Member-scoped `cancel_all_work`
                // remains the supported in-flight cancellation path.
                Err(MobError::WorkCancellationUnsupported(work_ref))
            }
            MobMachineCommand::CancelAllWork {
                runtime_id,
                fence_token,
            } => {
                // Identity derivation is a projection, not a decision: the
                // MobMachine DSL CancelAllWork guards own live-runtime
                // membership, fence-token freshness, and phase legality. The
                // actor's unified `handle_cancel_all_work` forwards both to
                // the DSL and then dispatches the interrupt when accepted.
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
            MobMachineCommand::RosterSnapshot => {
                let roster = self.project_roster_snapshot_from_machine_state().await;
                Ok(MobMachineCommandResult::RosterSnapshot(roster))
            }
            MobMachineCommand::ListMembers => {
                let members = self
                    .send_actor_command(|reply_tx| MobCommand::ProjectMemberList {
                        include_retiring: false,
                        reply_tx,
                    })
                    .await??;
                Ok(MobMachineCommandResult::ListMembers(members))
            }
            MobMachineCommand::ListMembersIncludingRetiring => {
                let members = self
                    .send_actor_command(|reply_tx| MobCommand::ProjectMemberList {
                        include_retiring: true,
                        reply_tx,
                    })
                    .await??;
                Ok(MobMachineCommandResult::ListMembersIncludingRetiring(
                    members,
                ))
            }
            MobMachineCommand::ListAllMembers => {
                let members = self.project_all_roster_entries_from_machine_state().await;
                Ok(MobMachineCommandResult::ListAllMembers(members))
            }
            MobMachineCommand::MemberStatus { agent_identity } => {
                let snapshot = self
                    .send_actor_command(|reply_tx| MobCommand::ProjectMemberStatus {
                        agent_identity: AgentIdentity::from(agent_identity.as_str()),
                        reply_tx,
                    })
                    .await??;
                Ok(MobMachineCommandResult::MemberStatus(snapshot))
            }
            MobMachineCommand::ApplyIdentityDeclarationManifest { manifest } => {
                let outcome =
                    self.send_actor_command(|reply_tx| {
                        MobCommand::ApplyIdentityDeclarationManifest { manifest, reply_tx }
                    })
                    .await??;
                Ok(MobMachineCommandResult::IdentityDeclarationManifestApplied(
                    Box::new(outcome),
                ))
            }
            MobMachineCommand::GetIdentityIntent { agent_identity } => {
                let observation = self
                    .send_actor_command(|reply_tx| MobCommand::GetIdentityIntent {
                        agent_identity,
                        reply_tx,
                    })
                    .await??;
                Ok(MobMachineCommandResult::IdentityIntent(Box::new(
                    observation,
                )))
            }
            MobMachineCommand::GetIdentityDeclarationReceipt {
                scope_id,
                operation_id,
            } => {
                let observation = self
                    .send_actor_command(|reply_tx| MobCommand::GetIdentityDeclarationReceipt {
                        scope_id,
                        operation_id,
                        reply_tx,
                    })
                    .await??;
                Ok(MobMachineCommandResult::IdentityDeclarationReceipt(
                    Box::new(observation),
                ))
            }
            MobMachineCommand::GetIdentityConvergenceStatus { agent_identity } => {
                let observation = self
                    .send_actor_command(|reply_tx| MobCommand::GetIdentityConvergenceStatus {
                        agent_identity,
                        reply_tx,
                    })
                    .await??;
                Ok(MobMachineCommandResult::IdentityConvergenceStatus(
                    observation,
                ))
            }
            MobMachineCommand::ConcludeObjective {
                agent_identity,
                objective_id,
                outcome,
            } => {
                self.send_actor_command(|reply_tx| MobCommand::ConcludeObjective {
                    agent_identity,
                    objective_id,
                    outcome,
                    reply_tx,
                })
                .await??;
                Ok(MobMachineCommandResult::Unit)
            }
            MobMachineCommand::SubscribeAgentEvents { agent_identity } => {
                let effects = self
                    .apply_machine_input_effects(mob_dsl::MobMachineInput::SubscribeAgentEvents {
                        agent_identity: mob_dsl::AgentIdentity(agent_identity.to_string()),
                    })
                    .await?;
                let stream = match Self::agent_event_subscription_authority_from_effects(
                    effects,
                    &agent_identity,
                )? {
                    AgentEventSubscriptionAuthority::Local(session_id) => {
                        self.subscribe_authorized_agent_session_events(&agent_identity, &session_id)
                            .await?
                    }
                    // The machine's THIRD outcome (phase 6): a placed member
                    // subscribes through the member event pump's tap.
                    AgentEventSubscriptionAuthority::External => {
                        self.external_member_event_stream(&agent_identity).await?
                    }
                };
                Ok(MobMachineCommandResult::EventStream(stream))
            }
            MobMachineCommand::SubscribeAllAgentEvents => {
                let machine_state = self.machine_state_watch_rx.borrow().clone();
                let session_bound_runtimes =
                    Self::session_bound_live_runtime_ids_from_machine(&machine_state);
                let effects = self
                    .apply_machine_input_effects(
                        mob_dsl::MobMachineInput::SubscribeAllAgentEvents {
                            session_bound_runtimes,
                            // Phase 6 (DEC-P6E-12 kill site 1): the placed
                            // roster IS the external-member set — the stale
                            // "no producer" empty set is dead.
                            external_members: Self::placed_member_identities_from_machine(
                                &machine_state,
                            ),
                        },
                    )
                    .await?;
                let (authorized_runtimes, authorized_external) =
                    Self::all_agent_event_subscription_authority_from_effects(effects)?;
                let mut streams = Vec::new();
                for (dsl_identity, runtime_id) in &machine_state.identity_to_runtime {
                    if !authorized_runtimes.contains(runtime_id) {
                        continue;
                    }
                    // Placement decides the transport lane (ADJ-24): placed
                    // members ride the authorized-external pump taps below.
                    if machine_state.member_placement.contains_key(dsl_identity) {
                        continue;
                    }
                    let Some(dsl_session_id) =
                        machine_state.member_session_bindings.get(dsl_identity)
                    else {
                        continue;
                    };
                    let agent_identity = AgentIdentity::from(dsl_identity.0.as_str());
                    let session_id =
                        Self::session_id_from_dsl(dsl_session_id, "all-agent event subscription")?;
                    let stream = self
                        .subscribe_authorized_agent_session_events(&agent_identity, &session_id)
                        .await?;
                    streams.push((agent_identity, stream));
                }
                // Authorized external members ride pump taps (same item
                // shape as the local entries).
                for dsl_identity in &authorized_external {
                    let agent_identity = AgentIdentity::from(dsl_identity.0.as_str());
                    let stream = self.external_member_event_stream(&agent_identity).await?;
                    streams.push((agent_identity, stream));
                }
                Ok(MobMachineCommandResult::AllAgentEventStreams(streams))
            }
            MobMachineCommand::SubscribeMobEvents { config } => {
                let machine_state = self.machine_state_watch_rx.borrow().clone();
                let session_bound_runtimes =
                    Self::session_bound_live_runtime_ids_from_machine(&machine_state);
                let initial_cursor = self.events.latest_cursor().await.map_err(MobError::from)?;
                let channel_capacity = u64::try_from(config.channel_capacity).map_err(|_| {
                    MobError::Internal(
                        "mob event router channel capacity does not fit generated authority input"
                            .into(),
                    )
                })?;
                let poll_interval_ms =
                    u64::try_from(config.poll_interval.as_millis()).unwrap_or(u64::MAX);
                let external_members = Self::placed_member_identities_from_machine(&machine_state);
                let effects = self
                    .apply_machine_input_effects(mob_dsl::MobMachineInput::SubscribeMobEvents {
                        initial_cursor,
                        channel_capacity,
                        poll_interval_ms,
                        session_bound_runtimes,
                        external_members: external_members.clone(),
                    })
                    .await?;
                let mut authority = Self::mob_event_router_authority_from_effects(effects)?;
                // Phase 6 (DEC-P6E-12 kill site 4): the mob-wide stream
                // includes placed members — pump taps fan into the same
                // merge. The frozen router-authorization effect carries no
                // external field; the set is the machine's placement facts.
                authority.external_members = external_members;
                Ok(MobMachineCommandResult::MobEventRouter(
                    super::event_router::spawn_event_router(self.clone(), authority),
                ))
            }
            MobMachineCommand::PollEvents {
                after_cursor,
                limit,
            } => {
                // Destroyed-fallback preflight: an INTERNAL mechanism read,
                // never an operator verb — the watch observation avoids
                // riding the caller's principal through the List-gated
                // `QueryPhase` (ADJ-P5-18 made that a typed denial). The
                // operator gate for the poll itself is the actor-routed
                // `PollEvents` chokepoint.
                let events = if self.status_observation_snapshot() == MobState::Destroyed {
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
                // Same internal-observation preflight as `PollEvents` above.
                let events = if self.status_observation_snapshot() == MobState::Destroyed {
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
                let member = self
                    .roster
                    .read()
                    .await
                    .entry(&agent_identity)
                    .map(|entry| {
                        let machine_state = self.machine_state_watch_rx.borrow().clone();
                        Self::project_roster_entry_from_machine_state(entry, &machine_state)
                    });
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
            MobMachineCommand::LifecycleNotificationBurst { count, message } => {
                self.send_actor_command(|reply_tx| MobCommand::LifecycleNotificationBurst {
                    count,
                    message,
                    reply_tx,
                })
                .await??;
                Ok(MobMachineCommandResult::LifecycleNotificationBurst)
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
                    .await??;
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
            MobMachineCommand::Wire { local, target } => {
                self.send_actor_command(|reply_tx| MobCommand::Wire {
                    local,
                    target,
                    reply_tx,
                })
                .await??;
                Ok(MobMachineCommandResult::Unit)
            }
            MobMachineCommand::WireMembersBatch { edges } => {
                let report = self
                    .send_actor_command(|reply_tx| MobCommand::WireMembersBatch { edges, reply_tx })
                    .await??;
                Ok(MobMachineCommandResult::WireMembersBatchReport(report))
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
    /// After generated Shutdown/Complete/Destroy authority has moved the mob
    /// out of a live phase and the actor exits, this may read the actor's
    /// terminal phase watch. A closed actor while the last published phase is
    /// still live is an actor failure, not lifecycle truth.
    pub async fn status(&self) -> Result<MobState, MobError> {
        match self
            .send_actor_command(|reply_tx| MobCommand::QueryPhase { reply_tx })
            .await
        {
            // ADJ-P5-18: the reply channel is typed — a scope denial (or any
            // actor-side error) surfaces as its own `Err`, never as fake
            // lifecycle data.
            Ok(reply) => reply,
            Err(
                error @ (MobError::ActorCommandChannelClosed | MobError::ActorReplyChannelClosed),
            ) => Self::status_from_closed_actor(error, *self.phase_watch_rx.borrow()),
            Err(other) => Err(other),
        }
    }

    fn status_from_closed_actor(error: MobError, observed: MobState) -> Result<MobState, MobError> {
        match observed {
            MobState::Stopped | MobState::Completed | MobState::Destroyed => Ok(observed),
            MobState::Creating | MobState::Running => Err(MobError::Internal(format!(
                "{error}; last actor-published phase is {observed}, so the phase watch is not terminal lifecycle authority"
            ))),
        }
    }

    /// Last actor-published mob lifecycle phase.
    ///
    /// Observation-only surfaces use this when they must remain live while the
    /// mob actor is awaiting an in-flight member turn. Control paths that need
    /// the authoritative current phase must keep using [`Self::status`].
    pub fn status_observation_snapshot(&self) -> MobState {
        *self.phase_watch_rx.borrow()
    }

    /// Access the mob definition.
    pub fn definition(&self) -> &MobDefinition {
        &self.definition
    }

    /// Read-only projection of generated owner bridge-session lifecycle authority.
    #[doc(hidden)]
    pub fn owner_bridge_session_lifecycle_authority(
        &self,
    ) -> Option<OwnerBridgeSessionLifecycleAuthority> {
        let machine_state = self.machine_state_watch_rx.borrow();
        let bridge_session_id = machine_state.owner_bridge_session_id.as_ref()?;
        Some(OwnerBridgeSessionLifecycleAuthority {
            bridge_session_id: SessionId::parse(&bridge_session_id.0).ok()?,
            destroy_on_owner_archive: machine_state.owner_bridge_destroy_on_archive,
            implicit_delegation_mob: machine_state.implicit_delegation_mob,
        })
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

    async fn resolve_peer_connectivity(
        &self,
        entry: &RosterEntry,
        bridge_session_id: &SessionId,
        _roster_snapshot: &Roster,
    ) -> Option<MobPeerConnectivitySnapshot> {
        self.session_service
            .comms_runtime(bridge_session_id)
            .await?;

        let reachable_peer_count = 0usize;
        let machine_state = self.machine_state_watch_rx.borrow().clone();
        let unknown_peer_count =
            Self::machine_wired_to_for_identity(&entry.agent_identity, &machine_state).len();
        let unreachable_peers = Vec::new();

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
    /// It hides machine-terminal completed/unknown rows while keeping broken
    /// rows visible for diagnostics. It intentionally skips live
    /// peer-connectivity fanout so ordinary membership polling cannot stall on
    /// comms connectivity lookups.
    /// For low-level structural roster visibility without runtime projection,
    /// use [`list_all_members`](Self::list_all_members).
    pub async fn list_members(&self) -> Vec<MobMemberListEntry> {
        self.project_member_list_entries_from_current_machine_state(false)
            .await
    }

    /// List operationally visible members including those in `Retiring` state,
    /// with canonical lifecycle/session projection.
    ///
    /// Like [`list_members`](Self::list_members), this intentionally avoids
    /// live peer-connectivity fanout. Use [`member_status`](Self::member_status)
    /// for deep per-member inspection including live comms connectivity.
    pub async fn list_members_including_retiring(&self) -> Vec<MobMemberListEntry> {
        self.project_member_list_entries_from_current_machine_state(true)
            .await
    }

    async fn project_member_list_entries_from_current_machine_state(
        &self,
        include_retiring: bool,
    ) -> Vec<MobMemberListEntry> {
        let entries_by_identity = {
            let roster = self.roster.read().await;
            roster
                .list_all()
                .cloned()
                .map(|entry| (entry.agent_identity.clone(), entry))
                .collect()
        };
        let machine_state = self.machine_state_watch_rx.borrow().clone();
        let entries = self
            .project_member_list_entries_from_machine_state(entries_by_identity, &machine_state);
        entries
            .into_iter()
            .filter(|entry| match entry.status {
                MobMemberStatus::Active | MobMemberStatus::Broken => true,
                MobMemberStatus::Retiring => include_retiring,
                MobMemberStatus::Completed | MobMemberStatus::Unknown => false,
            })
            .collect()
    }

    /// Observation-only member projection that never enters the mob actor queue.
    ///
    /// Source truth is the shared roster projection plus the actor-published
    /// MobMachine state watch. The actor is the sole writer for the machine
    /// watch; this handle only reads the last published value, so the status
    /// fields can be stale until the actor publishes the next transition.
    ///
    /// Use boundary: console/event observation and backfill discovery only.
    /// This includes retiring rows so observers can keep showing in-flight
    /// teardown. Control paths that decide routability, mutation legality, or
    /// active-membership policy must keep using the identity-native command
    /// APIs; this projection must not become a parallel authority.
    pub async fn list_members_observation_snapshot(&self) -> Vec<MobMemberListEntry> {
        let entries = self
            .roster
            .read()
            .await
            .list_all()
            .cloned()
            .map(|entry| (entry.agent_identity.clone(), entry))
            .collect();
        let machine_state = self.machine_state_watch_rx.borrow().clone();
        self.project_member_list_entries_from_machine_state(entries, &machine_state)
    }

    async fn inflight_retiring_member_list(&self) -> Option<Vec<MobMemberListEntry>> {
        let machine_state = self.machine_state_watch_rx.borrow().clone();
        if !machine_state.identity_to_runtime.keys().any(|identity| {
            machine_state.member_lifecycle_for_identity(identity).status
                == mob_dsl::MobMemberLifecycleStatus::Retiring
        }) {
            return None;
        }
        let entries = self
            .roster
            .read()
            .await
            .list_all()
            .cloned()
            .map(|entry| (entry.agent_identity.clone(), entry))
            .collect();
        Some(self.project_member_list_entries_from_machine_state(entries, &machine_state))
    }

    fn project_member_list_entries_from_machine_state(
        &self,
        entries_by_identity: BTreeMap<AgentIdentity, RosterEntry>,
        machine_state: &mob_dsl::MobMachineState,
    ) -> Vec<MobMemberListEntry> {
        machine_state
            .identity_to_runtime
            .keys()
            .filter_map(|identity| {
                Self::project_member_list_entry_from_machine_identity(
                    identity,
                    entries_by_identity.get(&AgentIdentity::from(identity.0.as_str())),
                    machine_state,
                )
            })
            .collect()
    }

    pub(super) fn project_member_list_entry_from_machine_identity(
        identity: &mob_dsl::AgentIdentity,
        roster_entry: Option<&RosterEntry>,
        machine_state: &mob_dsl::MobMachineState,
    ) -> Option<MobMemberListEntry> {
        let domain_identity = AgentIdentity::from(identity.0.as_str());
        let role = match machine_state.member_profile_name_for_identity(identity) {
            Some(profile_name) => ProfileName::from(profile_name),
            None => {
                tracing::error!(
                    agent_identity = %domain_identity,
                    "MobMachine member projection is missing machine-owned profile name"
                );
                return None;
            }
        };
        let runtime_mode = match machine_state.member_runtime_mode_for_identity(identity) {
            Some(runtime_mode) => runtime_mode,
            None => {
                tracing::error!(
                    agent_identity = %domain_identity,
                    "MobMachine member projection is missing machine-owned runtime mode"
                );
                return None;
            }
        };
        let machine_runtime = machine_state
            .member_runtime_material_for_identity(identity)
            .map(|material| material.to_domain_for_identity(&domain_identity));
        let current_bridge_session_id =
            Self::machine_bridge_session_id_for_identity(&domain_identity, machine_state);
        let material = MobMemberLifecycleProjection::materialize(MobMemberLifecycleInput {
            member_present: true,
            machine_lifecycle: machine_state.member_lifecycle_for_identity(identity),
            output_preview: None,
            tokens_used: 0,
            agent_identity: domain_identity.clone(),
            agent_runtime_id: machine_runtime
                .as_ref()
                .map(|(agent_runtime_id, _)| agent_runtime_id.clone()),
            fence_token: machine_runtime
                .as_ref()
                .map(|(_, fence_token)| *fence_token),
            current_bridge_session_id,
            peer_connectivity: None,
            kickoff: kickoff_snapshot_from_machine_state(
                domain_identity.as_str(),
                machine_state,
                roster_entry.and_then(|entry| entry.kickoff.as_ref()),
            ),
            progress: None,
        });
        let snapshot = material.to_snapshot();
        let current_bridge_session_id = snapshot.current_bridge_session_id().cloned();
        Some(
            MobMemberListEntry {
                agent_identity: domain_identity.clone(),
                agent_runtime_id: snapshot.agent_runtime_id,
                fence_token: snapshot.fence_token,
                role,
                runtime_mode,
                peer_id: roster_entry.and_then(|entry| entry.peer_id),
                transport_public_key: roster_entry
                    .and_then(|entry| entry.transport_public_key.clone()),
                wired_to: Self::machine_wired_to_for_identity(&domain_identity, machine_state),
                external_peer_specs: roster_entry
                    .map(|entry| entry.external_peer_specs.clone())
                    .unwrap_or_default(),
                labels: roster_entry
                    .map(|entry| entry.labels.clone())
                    .unwrap_or_default(),
                status: snapshot.status,
                error: snapshot.error,
                is_final: snapshot.is_final,
                current_session_id: None,
                current_bridge_session_id: None,
                kickoff: snapshot.kickoff,
            }
            .with_current_bridge_session_id(current_bridge_session_id),
        )
    }

    fn machine_bridge_session_id_for_identity(
        identity: &crate::ids::AgentIdentity,
        machine_state: &mob_dsl::MobMachineState,
    ) -> Option<SessionId> {
        let dsl_identity = mob_dsl::AgentIdentity::from_domain(identity);
        machine_state
            .member_session_bindings
            .get(&dsl_identity)
            .and_then(|dsl_session_id| SessionId::parse(&dsl_session_id.0).ok())
    }

    fn session_bound_live_runtime_ids_from_machine(
        machine_state: &mob_dsl::MobMachineState,
    ) -> BTreeSet<mob_dsl::AgentRuntimeId> {
        machine_state
            .member_session_bindings
            .iter()
            .filter_map(|(identity, _)| machine_state.identity_to_runtime.get(identity))
            .filter(|runtime_id| machine_state.live_runtime_ids.contains(*runtime_id))
            .cloned()
            .collect()
    }

    fn domain_identity_from_dsl(identity: &mob_dsl::AgentIdentity) -> AgentIdentity {
        AgentIdentity::from(identity.0.as_str())
    }

    fn domain_runtime_from_machine_binding(
        identity: &mob_dsl::AgentIdentity,
        runtime_id: &mob_dsl::AgentRuntimeId,
        machine_state: &mob_dsl::MobMachineState,
    ) -> Option<AgentRuntimeId> {
        let domain_identity = Self::domain_identity_from_dsl(identity);
        let generation = machine_state.identity_runtime_generations.get(identity)?;
        let domain_runtime =
            AgentRuntimeId::new(domain_identity, crate::ids::Generation::new(generation.0));
        (mob_dsl::AgentRuntimeId::from_domain(&domain_runtime) == *runtime_id)
            .then_some(domain_runtime)
    }

    fn session_id_from_dsl(
        session_id: &mob_dsl::SessionId,
        context: &str,
    ) -> Result<SessionId, MobError> {
        SessionId::parse(&session_id.0).map_err(|_| {
            MobError::Internal(format!(
                "MobMachine produced invalid session id for {context}"
            ))
        })
    }

    /// Force-cancel's HARD sibling (phase 6, DEC-P6E-8): bounded convergence
    /// over the immediate user-interrupt authority, `Cancel`-scoped at
    /// chokepoint (a). For a placed member the bridge request is pinned to one
    /// exact observed run and ACKs only after that run is unbound/terminal;
    /// a stale retry never interrupts a newer run. Placed members are also
    /// capability-gated on the recorded host fact BEFORE any bridge dispatch.
    /// RPC/REST/MCP/CLI exposure is phase 7.
    pub async fn hard_cancel_member(
        &self,
        caller: crate::control_policy::MobControlPrincipal,
        identity: AgentIdentity,
        reason: impl Into<String>,
    ) -> Result<(), MobError> {
        let reason = reason.into();
        self.clone()
            .with_command_authority(crate::control_policy::CommandAuthority::principal(caller))
            .send_actor_command(|reply_tx| super::state::MobCommand::HardCancelMember {
                agent_identity: identity,
                reason,
                reply_tx,
            })
            .await?
    }

    /// Placement-switched member transcript read (phase 6, DEC-P6E-21):
    /// `ReadHistory`-scoped at chokepoint (a); local and remote pages share
    /// ONE wire projection. RPC/REST/MCP exposure is phase 7 (the DTOs
    /// already exist).
    pub async fn member_history(
        &self,
        caller: crate::control_policy::MobControlPrincipal,
        identity: AgentIdentity,
        from_index: Option<u64>,
        limit: Option<u32>,
    ) -> Result<super::member_history_proxy::MemberHistoryPageDomain, MobError> {
        self.clone()
            .with_command_authority(crate::control_policy::CommandAuthority::principal(caller))
            .send_actor_command(|reply_tx| super::state::MobCommand::MemberHistory {
                agent_identity: identity,
                from_index,
                limit,
                reply_tx,
            })
            .await?
    }

    /// Open a live realtime channel on a member by identity (phase 6b,
    /// DEC-P6B-C1; placement-blind, DL3). `Live`-scoped at chokepoint (a) —
    /// bootstrap issuance IS the scope-gated act (DL8). The returned
    /// [`super::bridge_protocol::LiveOpenResult`] carries the OWNING host's
    /// absolute WS URL + single-use token VERBATIM (DEC-P6B-C7/C8: never
    /// parsed, rewritten, logged, or retained controlling-side); the caller
    /// connects the WS directly — the socket is the input plane, there is
    /// no bridge frame verb (DL10). `turning_mode: None` defaults to
    /// `ProviderManaged` on the owning host (DEC-P6B-L15, one owner).
    ///
    /// Reply-loss reconciliation contract (DEC-P6B-C9): a
    /// [`MobError::BridgeRequestTimedOut`] open may have minted an
    /// owning-side channel whose reply was lost. An open is NOT resend-safe
    /// (a retry against the orphan rejects `LiveChannelAlreadyBound`, and
    /// no resend can recover the single-use token). The verbs themselves
    /// are the reconciliation primitives, caller-driven:
    /// [`Self::member_live_status`] with `channel_id: None` discovers the
    /// orphan's id, [`Self::member_live_close`] clears exactly the channel
    /// it names, and a fresh open then succeeds. The harness never
    /// auto-probes and no live command joins any resend classifier; the
    /// unconsumed token expires at its 60s TTL regardless.
    ///
    /// RPC/REST/MCP/CLI exposure is phase 7.
    pub async fn member_live_open(
        &self,
        caller: crate::control_policy::MobControlPrincipal,
        identity: AgentIdentity,
        turning_mode: Option<super::bridge_protocol::RealtimeTurningMode>,
        transport: Option<super::bridge_protocol::LiveOpenTransport>,
    ) -> Result<super::bridge_protocol::LiveOpenResult, MobError> {
        let delivery = self
            .clone()
            .with_command_authority(crate::control_policy::CommandAuthority::principal(caller))
            .send_actor_command(|reply_tx| super::state::MobCommand::MemberLiveOpen {
                agent_identity: identity,
                turning_mode,
                transport,
                reply_tx,
            })
            .await??;
        // Durable cleanup custody must be deleted before public ownership is
        // returned. The actor confirms that deletion over this second
        // channel; a crash before confirmation closes this receiver and the
        // caller never observes a success that recovery could later reclaim.
        let (custody_confirm_tx, custody_confirm_rx) = oneshot::channel();
        delivery
            .delivery_ack
            .send(custody_confirm_tx)
            .map_err(|_| MobError::ActorReplyChannelClosed)?;
        custody_confirm_rx
            .await
            .map_err(|_| MobError::ActorReplyChannelClosed)??;
        // There is no cancellation point after confirmed durable transfer.
        Ok(delivery.open)
    }

    /// Close one NAMED live channel on a member (phase 6b, DEC-P6B-C9:
    /// close-what-you-name — a reconciling console can never race-kill a
    /// channel a concurrent legitimate open just minted). `Live`-scoped.
    /// An already-clear channel rejects typed `LiveChannelNotFound` (safe
    /// to treat as "nothing left to clear"). RPC/REST/MCP/CLI exposure is
    /// phase 7.
    pub async fn member_live_close(
        &self,
        caller: crate::control_policy::MobControlPrincipal,
        identity: AgentIdentity,
        channel_id: String,
    ) -> Result<super::bridge_protocol::LiveCloseStatus, MobError> {
        self.clone()
            .with_command_authority(crate::control_policy::CommandAuthority::principal(caller))
            .send_actor_command(|reply_tx| super::state::MobCommand::MemberLiveClose {
                agent_identity: identity,
                channel_id,
                reply_tx,
            })
            .await?
    }

    /// The dedicated live point read (phase 6b, §16.9/DEC-P6B-C10):
    /// `channel_id: None` resolves "the member's active channel" on the
    /// owning host (ADJ-P6B-2) — the reply-loss discovery primitive; no
    /// active channel rejects typed `LiveChannelNotFound` (an honest
    /// "nothing to reconcile"). `member_status` stays bridge-free and
    /// live-field-free — this read is the ONLY remote channel-state path.
    /// `Live`-scoped. RPC/REST/MCP/CLI exposure is phase 7.
    pub async fn member_live_status(
        &self,
        caller: crate::control_policy::MobControlPrincipal,
        identity: AgentIdentity,
        channel_id: Option<String>,
    ) -> Result<super::member_live_proxy::MemberLiveStatusDomain, MobError> {
        self.clone()
            .with_command_authority(crate::control_policy::CommandAuthority::principal(caller))
            .send_actor_command(|reply_tx| super::state::MobCommand::MemberLiveStatus {
                agent_identity: identity,
                channel_id,
                reply_tx,
            })
            .await?
    }

    /// Drive one turn-level live control verb on a member's channel (phase
    /// 6b, DL10's closed vocabulary: commit_input / interrupt / truncate /
    /// refresh). Live `interrupt` is media-plane barge-in and sits inside
    /// `Live`; lifecycle hard-cancel stays under `Cancel`. `Live`-scoped.
    /// RPC/REST/MCP/CLI exposure is phase 7.
    pub async fn member_live_control(
        &self,
        caller: crate::control_policy::MobControlPrincipal,
        identity: AgentIdentity,
        channel_id: String,
        verb: super::bridge_protocol::BridgeLiveControlVerb,
    ) -> Result<super::bridge_protocol::BridgeLiveControlOutcome, MobError> {
        self.clone()
            .with_command_authority(crate::control_policy::CommandAuthority::principal(caller))
            .send_actor_command(|reply_tx| super::state::MobCommand::MemberLiveControl {
                agent_identity: identity,
                channel_id,
                verb,
                reply_tx,
            })
            .await?
    }

    /// A17 pump-liveness poke (the flow lane's named hook,
    /// `ensure_pump_for_obligation`): keeps a placed member's event pump
    /// alive while a remote-turn obligation is outstanding. Internal lane.
    pub(crate) async fn ensure_pump_for_obligation(
        &self,
        identity: &AgentIdentity,
    ) -> Result<(), MobError> {
        self.send_actor_command(|reply_tx| super::state::MobCommand::EnsureMemberEventPump {
            agent_identity: identity.clone(),
            reply_tx,
        })
        .await?
    }

    /// Placed roster identities that still hold a runtime binding — the
    /// external-member set for the mob-wide fan-out (DEC-P6E-12).
    pub(super) fn placed_member_identities_from_machine(
        machine_state: &mob_dsl::MobMachineState,
    ) -> BTreeSet<mob_dsl::AgentIdentity> {
        machine_state
            .member_placement
            .keys()
            .filter(|identity| machine_state.identity_to_runtime.contains_key(*identity))
            .cloned()
            .collect()
    }

    /// Outstanding remote-turn custody across pending, committed, and
    /// resolved/ACK-pending machine facts. Watch-read only; the actor is the
    /// sole writer.
    pub fn remote_turn_obligations_observation(&self) -> Vec<(AgentIdentity, String)> {
        let state = self.machine_state_watch_rx.borrow();
        let mut obligations = state
            .pending_remote_turn_outcomes
            .iter()
            .chain(state.committed_remote_turn_outcomes.iter())
            .chain(state.resolved_remote_turn_outcomes.iter())
            .map(|obligation| {
                (
                    AgentIdentity::from(obligation.agent_identity.0.as_str()),
                    obligation.input_id.0.clone(),
                )
            })
            .collect::<Vec<_>>();
        obligations.extend(
            state
                .pending_placed_kickoff_outcomes
                .iter()
                .chain(state.resolved_placed_kickoff_outcomes.iter())
                .map(|obligation| {
                    (
                        AgentIdentity::from(obligation.agent_identity.0.as_str()),
                        obligation.input_id.0.clone(),
                    )
                }),
        );
        obligations.extend(
            state
                .pending_placed_completion_outcomes
                .iter()
                .chain(state.resolved_placed_completion_outcomes.iter())
                .map(|obligation| {
                    (
                        AgentIdentity::from(obligation.agent_identity.0.as_str()),
                        obligation.input_id.0.clone(),
                    )
                }),
        );
        obligations
    }

    /// Whether any machine-owned remote-turn custody belongs to this exact
    /// host/member residency. Mob scope is implicit in this handle; every
    /// remaining tuple component is compared explicitly.
    pub(crate) fn remote_turn_obligation_for_member_observation(
        &self,
        expected_member: &meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation,
    ) -> bool {
        let matches = |obligation: &mob_dsl::RemoteTurnObligation| {
            obligation.agent_identity.0 == expected_member.agent_identity
                && obligation.host_id.0 == expected_member.host_id
                && obligation.host_binding_generation == expected_member.binding_generation
                && obligation.member_session_id.0 == expected_member.member_session_id
                && obligation.generation.0 == expected_member.generation
                && obligation.fence_token.0 == expected_member.fence_token
        };
        let state = self.machine_state_watch_rx.borrow();
        let flow = state
            .pending_remote_turn_outcomes
            .iter()
            .chain(state.committed_remote_turn_outcomes.iter())
            .chain(state.resolved_remote_turn_outcomes.iter())
            .any(matches);
        let kickoff = |obligation: &mob_dsl::PlacedKickoffObligation| {
            obligation.agent_identity.0 == expected_member.agent_identity
                && obligation.host_id.0 == expected_member.host_id
                && obligation.host_binding_generation == expected_member.binding_generation
                && obligation.member_session_id.0 == expected_member.member_session_id
                && obligation.generation.0 == expected_member.generation
                && obligation.fence_token.0 == expected_member.fence_token
        };
        let completion = |obligation: &mob_dsl::PlacedCompletionObligation| {
            obligation.agent_identity.0 == expected_member.agent_identity
                && obligation.host_id.0 == expected_member.host_id
                && obligation.host_binding_generation == expected_member.binding_generation
                && obligation.member_session_id.0 == expected_member.member_session_id
                && obligation.generation.0 == expected_member.generation
                && obligation.fence_token.0 == expected_member.fence_token
        };
        flow || state
            .pending_placed_completion_outcomes
            .iter()
            .chain(state.resolved_placed_completion_outcomes.iter())
            .any(completion)
            || state
                .pending_placed_kickoff_outcomes
                .iter()
                .chain(state.resolved_placed_kickoff_outcomes.iter())
                .any(kickoff)
    }

    /// Exact host ACKs that must be resent until a poll response proves
    /// application. Reconstructed from machine state after restart.
    pub(crate) fn remote_turn_ack_pending_observation(
        &self,
        expected_member: &meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation,
    ) -> Vec<meerkat_contracts::wire::supervisor_bridge::BridgeTurnOutcomeAck> {
        let state = self.machine_state_watch_rx.borrow();
        let mut acks = state
            .resolved_remote_turn_outcomes
            .iter()
            .filter(|obligation| {
                obligation.agent_identity.0 == expected_member.agent_identity
                    && obligation.host_id.0 == expected_member.host_id
                    && obligation.host_binding_generation == expected_member.binding_generation
                    && obligation.member_session_id.0 == expected_member.member_session_id
                    && obligation.generation.0 == expected_member.generation
                    && obligation.fence_token.0 == expected_member.fence_token
            })
            .map(
                |obligation| meerkat_contracts::wire::supervisor_bridge::BridgeTurnOutcomeAck {
                    generation: obligation.generation.0,
                    fence_token: obligation.fence_token.0,
                    input_id: obligation.input_id.0.clone(),
                },
            )
            .collect::<Vec<_>>();
        acks.extend(
            state
                .resolved_placed_completion_outcomes
                .iter()
                .filter(|obligation| {
                    obligation.agent_identity.0 == expected_member.agent_identity
                        && obligation.host_id.0 == expected_member.host_id
                        && obligation.host_binding_generation == expected_member.binding_generation
                        && obligation.member_session_id.0 == expected_member.member_session_id
                        && obligation.generation.0 == expected_member.generation
                        && obligation.fence_token.0 == expected_member.fence_token
                })
                .map(|obligation| {
                    meerkat_contracts::wire::supervisor_bridge::BridgeTurnOutcomeAck {
                        generation: obligation.generation.0,
                        fence_token: obligation.fence_token.0,
                        input_id: obligation.input_id.0.clone(),
                    }
                }),
        );
        acks.extend(
            state
                .resolved_placed_kickoff_outcomes
                .iter()
                .filter(|obligation| {
                    obligation.agent_identity.0 == expected_member.agent_identity
                        && obligation.host_id.0 == expected_member.host_id
                        && obligation.host_binding_generation == expected_member.binding_generation
                        && obligation.member_session_id.0 == expected_member.member_session_id
                        && obligation.generation.0 == expected_member.generation
                        && obligation.fence_token.0 == expected_member.fence_token
                })
                .map(|obligation| {
                    meerkat_contracts::wire::supervisor_bridge::BridgeTurnOutcomeAck {
                        generation: obligation.generation.0,
                        fence_token: obligation.fence_token.0,
                        input_id: obligation.input_id.0.clone(),
                    }
                }),
        );
        acks
    }

    /// Machine-recorded runtime incarnation for `identity` (watch-read; the
    /// router's re-subscription key).
    pub(super) fn member_runtime_id_observation(
        &self,
        agent_identity: &AgentIdentity,
    ) -> Option<AgentRuntimeId> {
        let machine_state = self.machine_state_watch_rx.borrow().clone();
        let dsl_identity = mob_dsl::AgentIdentity::from_domain(agent_identity);
        let runtime_id = machine_state.identity_to_runtime.get(&dsl_identity)?;
        Self::domain_runtime_from_machine_binding(&dsl_identity, runtime_id, &machine_state)
    }

    /// Whether the machine records a placement for `identity` (the router's
    /// roster-delta placement switch).
    pub(super) fn member_placement_present(&self, agent_identity: &AgentIdentity) -> bool {
        let dsl_identity = mob_dsl::AgentIdentity(agent_identity.to_string());
        self.machine_state_watch_rx
            .borrow()
            .member_placement
            .contains_key(&dsl_identity)
    }

    /// Open a raw `AttributedEvent` pump tap for one placed member (ensures
    /// the pump first; authorization happened at the machine input).
    pub(super) async fn external_member_event_tap(
        &self,
        agent_identity: &AgentIdentity,
    ) -> Result<tokio::sync::mpsc::Receiver<crate::event::AttributedEvent>, MobError> {
        self.send_actor_command(|reply_tx| super::state::MobCommand::EnsureMemberEventTap {
            agent_identity: agent_identity.clone(),
            reply_tx,
        })
        .await?
    }

    /// Realize a machine-authorized external subscription as a pump-tap
    /// stream (DEC-P6E-12 kill site 3): ensure the member's pump, open a
    /// tap, and project `AttributedEvent → EventEnvelope` (attribution is
    /// implicit for a single-member stream).
    pub(super) async fn external_member_event_stream(
        &self,
        agent_identity: &AgentIdentity,
    ) -> Result<EventStream, MobError> {
        let tap = self.external_member_event_tap(agent_identity).await?;
        Ok(Box::pin(futures::stream::unfold(
            tap,
            |mut tap| async move {
                tap.recv()
                    .await
                    .map(|attributed| (attributed.envelope, tap))
            },
        )))
    }

    fn agent_event_subscription_authority_from_effects(
        effects: Vec<mob_dsl::MobMachineEffect>,
        agent_identity: &AgentIdentity,
    ) -> Result<AgentEventSubscriptionAuthority, MobError> {
        let dsl_identity = mob_dsl::AgentIdentity(agent_identity.to_string());
        for effect in effects {
            match effect {
                mob_dsl::MobMachineEffect::AuthorizeAgentEventSubscription {
                    agent_identity: effect_identity,
                    session_id,
                } if effect_identity == dsl_identity => {
                    return Ok(AgentEventSubscriptionAuthority::Local(
                        Self::session_id_from_dsl(&session_id, "agent event subscription")?,
                    ));
                }
                // Phase 6: the machine's THIRD outcome for placed members
                // supersedes the old `NoSessionBinding` reject — no shell
                // reinterpretation.
                mob_dsl::MobMachineEffect::AuthorizeExternalAgentEventSubscription {
                    agent_identity: effect_identity,
                    ..
                } if effect_identity == dsl_identity => {
                    return Ok(AgentEventSubscriptionAuthority::External);
                }
                mob_dsl::MobMachineEffect::RejectAgentEventSubscription {
                    agent_identity: effect_identity,
                    reason,
                } if effect_identity == dsl_identity => {
                    return match reason {
                        mob_dsl::EventSubscriptionRejectReasonKind::MemberNotFound => {
                            Err(MobError::MemberNotFound(agent_identity.clone()))
                        }
                        mob_dsl::EventSubscriptionRejectReasonKind::NoSessionBinding => {
                            Err(MobError::UnsupportedForMode {
                                mode: MobRuntimeMode::TurnDriven,
                                reason: "MobMachine rejected agent event subscription without a session binding".to_string(),
                            })
                        }
                    };
                }
                _ => {}
            }
        }
        Err(MobError::Internal(
            "MobMachine did not emit agent event subscription authority".into(),
        ))
    }

    fn all_agent_event_subscription_authority_from_effects(
        effects: Vec<mob_dsl::MobMachineEffect>,
    ) -> Result<
        (
            BTreeSet<mob_dsl::AgentRuntimeId>,
            BTreeSet<mob_dsl::AgentIdentity>,
        ),
        MobError,
    > {
        effects
            .into_iter()
            .try_fold(None, |authorized, effect| match effect {
                mob_dsl::MobMachineEffect::AuthorizeAllAgentEventSubscription {
                    session_bound_runtimes,
                    external_members,
                } => {
                    // Phase 6 (DEC-P6E-12 kill site 2): the authorized
                    // external set is realized as pump taps by the caller —
                    // the fail-closed "no remote subscription realization
                    // yet" arm is dead.
                    Ok(Some((session_bound_runtimes, external_members)))
                }
                mob_dsl::MobMachineEffect::RejectAllAgentEventSubscription { reason } => {
                    let error = match reason {
                        mob_dsl::EventSubscriptionRejectReasonKind::MemberNotFound => {
                            MobError::Internal(
                                "MobMachine rejected all-agent event subscription: member not found"
                                    .into(),
                            )
                        }
                        mob_dsl::EventSubscriptionRejectReasonKind::NoSessionBinding => {
                            MobError::UnsupportedForMode {
                                mode: MobRuntimeMode::TurnDriven,
                                reason: "MobMachine rejected all-agent event subscription without any session-bound members".to_string(),
                            }
                        }
                    };
                    Err(error)
                }
                _ => Ok(authorized),
            })
            .and_then(|authorized| {
                authorized.ok_or_else(|| {
                MobError::Internal(
                    "MobMachine did not emit all-agent event subscription authority".into(),
                )
                })
            })
    }

    fn mob_event_router_authority_from_effects(
        effects: Vec<mob_dsl::MobMachineEffect>,
    ) -> Result<super::event_router::AuthorizedMobEventRouter, MobError> {
        effects
            .into_iter()
            .find_map(|effect| match effect {
                mob_dsl::MobMachineEffect::AuthorizeMobEventRouter {
                    initial_cursor,
                    channel_capacity,
                    poll_interval_ms,
                    session_bound_runtimes,
                } => {
                    let channel_capacity = usize::try_from(channel_capacity).ok()?.max(1);
                    let poll_interval = Duration::from_millis(poll_interval_ms);
                    Some(super::event_router::AuthorizedMobEventRouter {
                        initial_cursor,
                        config: super::event_router::MobEventRouterConfig {
                            poll_interval,
                            channel_capacity,
                        },
                        session_bound_runtimes,
                        external_members: BTreeSet::new(),
                    })
                }
                _ => None,
            })
            .ok_or_else(|| {
                MobError::Internal("MobMachine did not emit mob event router authority".into())
            })
    }

    fn structural_event_subscription_authority_from_effects(
        effects: Vec<mob_dsl::MobMachineEffect>,
    ) -> Result<(u64, bool, MobEventsSubscriptionConfig), MobError> {
        for effect in effects {
            match effect {
                mob_dsl::MobMachineEffect::AuthorizeStructuralEventSubscription {
                    after_cursor,
                    explicit_after_cursor,
                    batch_limit,
                    channel_capacity,
                } => {
                    let batch_limit = usize::try_from(batch_limit).map_err(|_| {
                        MobError::Internal(
                            "MobMachine produced invalid structural batch limit".into(),
                        )
                    })?;
                    let channel_capacity = usize::try_from(channel_capacity).map_err(|_| {
                        MobError::Internal(
                            "MobMachine produced invalid structural channel capacity".into(),
                        )
                    })?;
                    return Ok((
                        after_cursor,
                        explicit_after_cursor,
                        MobEventsSubscriptionConfig {
                            after_cursor: Some(after_cursor),
                            batch_limit,
                            channel_capacity,
                        },
                    ));
                }
                mob_dsl::MobMachineEffect::RejectStructuralEventSubscription {
                    after_cursor,
                    latest_cursor,
                } => {
                    return Err(MobError::StaleEventCursor {
                        after_cursor,
                        latest_cursor,
                    });
                }
                _ => {}
            }
        }
        Err(MobError::Internal(
            "MobMachine did not emit structural event subscription authority".into(),
        ))
    }

    fn strict_event_poll_authority_from_effects(
        effects: Vec<mob_dsl::MobMachineEffect>,
    ) -> Result<(u64, usize), MobError> {
        for effect in effects {
            match effect {
                mob_dsl::MobMachineEffect::AuthorizeStrictEventPoll {
                    after_cursor,
                    limit,
                } => {
                    let limit = usize::try_from(limit).map_err(|_| {
                        MobError::Internal(
                            "MobMachine produced invalid strict event poll limit".into(),
                        )
                    })?;
                    return Ok((after_cursor, limit));
                }
                mob_dsl::MobMachineEffect::RejectStrictEventPoll {
                    after_cursor,
                    latest_cursor,
                } => {
                    return Err(MobError::StaleEventCursor {
                        after_cursor,
                        latest_cursor,
                    });
                }
                _ => {}
            }
        }
        Err(MobError::Internal(
            "MobMachine did not emit strict event poll authority".into(),
        ))
    }

    fn project_member_ref_session_binding(
        member_ref: &crate::event::MemberRef,
        current_bridge_session_id: Option<SessionId>,
    ) -> crate::event::MemberRef {
        match member_ref {
            // The event roster has no sessionless encoding for a session-backed
            // member. When MobMachine has no active binding, keep the member ref
            // only as a structural roster anchor; behavior paths must resolve
            // bridge sessions through `MobMachineState.member_session_bindings`.
            crate::event::MemberRef::Session { .. } => current_bridge_session_id
                .map(crate::event::MemberRef::from_bridge_session_id)
                .unwrap_or_else(|| member_ref.clone()),
            crate::event::MemberRef::BackendPeer {
                peer_id,
                address,
                pubkey,
                bootstrap_token,
                ..
            } => crate::event::MemberRef::BackendPeer {
                peer_id: peer_id.clone(),
                address: address.clone(),
                pubkey: *pubkey,
                bootstrap_token: bootstrap_token.clone(),
                session_id: current_bridge_session_id,
            },
        }
    }

    fn project_roster_entry_from_machine_state(
        mut entry: RosterEntry,
        machine_state: &mob_dsl::MobMachineState,
    ) -> RosterEntry {
        let current_bridge_session_id =
            Self::machine_bridge_session_id_for_identity(&entry.agent_identity, machine_state);
        entry.member_ref =
            Self::project_member_ref_session_binding(&entry.member_ref, current_bridge_session_id);
        entry.wired_to = Self::machine_wired_to_for_identity(&entry.agent_identity, machine_state);
        entry.kickoff = kickoff_snapshot_from_machine_state(
            entry.agent_identity.as_str(),
            machine_state,
            entry.kickoff.as_ref(),
        );
        entry
    }

    fn machine_wired_to_for_identity(
        identity: &crate::ids::AgentIdentity,
        machine_state: &mob_dsl::MobMachineState,
    ) -> BTreeSet<crate::ids::AgentIdentity> {
        let local = mob_dsl::AgentIdentity::from_domain(identity);
        let mut wired_to = machine_state
            .wiring_edges
            .iter()
            .filter_map(|edge| {
                if edge.a == local {
                    Some(crate::ids::AgentIdentity::from(edge.b.0.as_str()))
                } else if edge.b == local {
                    Some(crate::ids::AgentIdentity::from(edge.a.0.as_str()))
                } else {
                    None
                }
            })
            .collect::<BTreeSet<_>>();
        wired_to.extend(
            machine_state
                .external_peer_edges
                .iter()
                .filter(|edge| edge.local == local)
                .map(|edge| crate::ids::AgentIdentity::from(edge.endpoint.name.0.as_str())),
        );
        wired_to
    }

    fn member_lifecycle_from_machine_state(
        identity: &crate::ids::AgentIdentity,
        machine_state: &mob_dsl::MobMachineState,
    ) -> mob_dsl::MobMemberLifecycleMaterial {
        let domain_identity = crate::ids::AgentIdentity::from(identity.as_str());
        let dsl_identity = mob_dsl::AgentIdentity::from_domain(&domain_identity);
        machine_state.member_lifecycle_for_identity(&dsl_identity)
    }

    fn machine_runtime_identity_fields_for_identity(
        &self,
        identity: &crate::ids::AgentIdentity,
    ) -> Option<(AgentRuntimeId, FenceToken)> {
        let machine_state = self.machine_state_watch_rx.borrow().clone();
        let dsl_identity = mob_dsl::AgentIdentity::from_domain(identity);
        machine_state
            .member_runtime_material_for_identity(&dsl_identity)
            .map(|material| material.to_domain_for_identity(identity))
    }

    async fn project_retiring_member_status_from_machine_state(
        &self,
        identity: &crate::ids::AgentIdentity,
    ) -> Option<MobMemberSnapshot> {
        let machine_state = self.machine_state_watch_rx.borrow().clone();
        let lifecycle = Self::member_lifecycle_from_machine_state(identity, &machine_state);
        if lifecycle.status != mob_dsl::MobMemberLifecycleStatus::Retiring {
            return None;
        }

        let entry = {
            let roster = self.roster.read().await;
            roster.get(identity).cloned()
        };
        let kickoff = entry.as_ref().and_then(|entry| {
            kickoff_snapshot_from_machine_state(
                entry.agent_identity.as_str(),
                &machine_state,
                entry.kickoff.as_ref(),
            )
        });
        let current_bridge_session_id =
            Self::machine_bridge_session_id_for_identity(identity, &machine_state);
        let dsl_identity = mob_dsl::AgentIdentity::from_domain(identity);
        let machine_runtime = machine_state
            .member_runtime_material_for_identity(&dsl_identity)
            .map(|material| material.to_domain_for_identity(identity))?;

        Some(
            MobMemberLifecycleProjection::materialize(MobMemberLifecycleInput {
                member_present: entry.is_some(),
                machine_lifecycle: lifecycle,
                output_preview: None,
                tokens_used: 0,
                agent_identity: identity.clone(),
                agent_runtime_id: Some(machine_runtime.0),
                fence_token: Some(machine_runtime.1),
                current_bridge_session_id,
                peer_connectivity: None,
                kickoff,
                progress: None,
            })
            .to_snapshot(),
        )
    }

    fn project_roster_from_machine_state(
        roster: Roster,
        machine_state: &mob_dsl::MobMachineState,
    ) -> Roster {
        let entries = roster.list_all().cloned().collect();
        Roster::from_projected_entries(Self::project_roster_entries_from_machine_state(
            entries,
            machine_state,
        ))
    }

    async fn project_roster_snapshot_from_machine_state(&self) -> Roster {
        let roster = self.roster.read().await.snapshot();
        let machine_state = self.machine_state_watch_rx.borrow().clone();
        Self::project_roster_from_machine_state(roster, &machine_state)
    }

    async fn project_all_roster_entries_from_machine_state(&self) -> Vec<RosterEntry> {
        let entries = self.roster.read().await.list_all().cloned().collect();
        let machine_state = self.machine_state_watch_rx.borrow().clone();
        Self::project_roster_entries_from_machine_state(entries, &machine_state)
    }

    fn project_roster_entries_from_machine_state(
        entries: Vec<RosterEntry>,
        machine_state: &mob_dsl::MobMachineState,
    ) -> Vec<RosterEntry> {
        entries
            .into_iter()
            .map(|entry| Self::project_roster_entry_from_machine_state(entry, machine_state))
            .collect()
    }

    /// List members currently eligible for runtime work dispatch.
    ///
    /// Excludes retiring, completed, broken, or unknown members even if they
    /// still appear in the public operational projection.
    pub(crate) async fn list_runnable_members(&self) -> Vec<MobMemberListEntry> {
        self.list_members()
            .await
            .into_iter()
            .filter(|entry| entry.status == MobMemberStatus::Active)
            .collect()
    }

    /// List all members including those in `Retiring` state.
    ///
    /// The `state` field on each [`RosterEntry`] is projected from
    /// `MobMachineState`, not from the event roster's compatibility mirror.
    pub async fn list_all_members(&self) -> Vec<RosterEntry> {
        self.project_all_roster_entries_from_machine_state().await
    }

    /// Get a specific member entry by identity.
    ///
    /// Returns `Ok(None)` for a genuinely absent member and `Err(MobError)`
    /// when the underlying machine command itself fails (query/transport
    /// fault). A transport fault must never be laundered into "not found".
    pub async fn get_member(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<RosterEntry>, MobError> {
        let result = self
            .execute_machine_command(MobMachineCommand::GetMember {
                agent_identity: identity.clone(),
            })
            .await?;
        Self::project_get_member_result(result)
    }

    /// Atomically apply one complete provider-scoped identity declaration.
    ///
    /// In the 0.8.2 production slice, a new operation is admitted only for a
    /// non-empty, missing scope whose members all adopt exact verified legacy
    /// sessions, require their existing history, use controlling-session
    /// execution, and declare no initial delivery. Desired wiring remains part
    /// of the sealed intent, but non-empty topology is reported as typed
    /// `RepairBlocked` in this narrow release until a target-atomic actuator
    /// exists. Replaying an already committed operation still returns its exact
    /// original outcome before this admission check.
    pub async fn apply_identity_declaration_manifest(
        &self,
        manifest: crate::identity::IdentityDeclarationManifest,
    ) -> Result<crate::identity::IdentityDeclarationManifestApplyOutcome, MobError> {
        match self
            .execute_machine_command(MobMachineCommand::ApplyIdentityDeclarationManifest {
                manifest: Box::new(manifest),
            })
            .await?
        {
            MobMachineCommandResult::IdentityDeclarationManifestApplied(outcome) => Ok(*outcome),
            _ => Err(MobError::Internal(
                "unexpected identity declaration command result variant".into(),
            )),
        }
    }

    /// Read the total stored observation for one identity intent row.
    pub async fn identity_intent(
        &self,
        identity: &AgentIdentity,
    ) -> Result<
        crate::identity::IdentityStoredObservation<crate::identity::IdentityIntentRecord>,
        MobError,
    > {
        match self
            .execute_machine_command(MobMachineCommand::GetIdentityIntent {
                agent_identity: identity.clone(),
            })
            .await?
        {
            MobMachineCommandResult::IdentityIntent(observation) => Ok(*observation),
            _ => Err(MobError::Internal(
                "unexpected identity intent command result variant".into(),
            )),
        }
    }

    /// Read the immutable receipt for one exact declaration operation.
    ///
    /// The returned total store observation preserves malformed or unsupported
    /// physical evidence. A valid receipt contains the exact original sealed
    /// apply outcome and its request digest, so lost-ack recovery can resume
    /// without recompiling portable material or rewriting desired state. This
    /// is historical custody only: callers must not treat it as the current
    /// intent or as mutation authority.
    pub async fn identity_declaration_receipt(
        &self,
        scope_id: &crate::identity::IdentityDeclarationScopeId,
        operation_id: &meerkat_core::ops::OperationId,
    ) -> Result<
        crate::identity::IdentityStoredObservation<crate::identity::IdentityOperationReceipt>,
        MobError,
    > {
        match self
            .execute_machine_command(MobMachineCommand::GetIdentityDeclarationReceipt {
                scope_id: scope_id.clone(),
                operation_id: operation_id.clone(),
            })
            .await?
        {
            MobMachineCommandResult::IdentityDeclarationReceipt(observation) => Ok(*observation),
            _ => Err(MobError::Internal(
                "unexpected identity declaration receipt command result variant".into(),
            )),
        }
    }

    /// Read replaceable, output-only convergence diagnostics for one identity.
    pub async fn identity_convergence_status(
        &self,
        identity: &AgentIdentity,
    ) -> Result<
        crate::identity::IdentityStoredObservation<crate::identity::IdentityConvergenceStatus>,
        MobError,
    > {
        match self
            .execute_machine_command(MobMachineCommand::GetIdentityConvergenceStatus {
                agent_identity: identity.clone(),
            })
            .await?
        {
            MobMachineCommandResult::IdentityConvergenceStatus(observation) => Ok(observation),
            _ => Err(MobError::Internal(
                "unexpected identity convergence status command result variant".into(),
            )),
        }
    }

    /// Map a `GetMember` command result into the typed member projection.
    ///
    /// Split out so the unexpected-variant fault arm (a genuine command-layer
    /// fault, never "absent member") is testable without laundering it into
    /// `Ok(None)`.
    fn project_get_member_result(
        result: MobMachineCommandResult,
    ) -> Result<Option<RosterEntry>, MobError> {
        match result {
            MobMachineCommandResult::GetMember(entry) => Ok(entry),
            _ => Err(MobError::Internal(
                "unexpected command result variant".into(),
            )),
        }
    }

    /// Test-only probe that drives [`Self::get_member`]'s result mapping with
    /// an explicit command result, proving a command-layer fault surfaces as
    /// `Err(MobError)` and is never collapsed to `Ok(None)`.
    #[cfg(test)]
    pub(crate) fn project_get_member_result_for_test(
        result: MobMachineCommandResult,
    ) -> Result<Option<RosterEntry>, MobError> {
        Self::project_get_member_result(result)
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
    /// capability-bearing handle, [`MemberHandle::internal_turn`] to deliver
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
    /// ("shell owns mechanics, not meaning"). The resolver reads
    /// `MobMachineState.member_session_bindings`; the event roster is only
    /// a compatibility mirror. Regression
    /// `resolve_bridge_session_id_is_lookup_not_mutation` proves this is
    /// a pure read against the generated authority projection.
    pub async fn resolve_bridge_session_id(&self, identity: &AgentIdentity) -> Option<SessionId> {
        let machine_state = self.machine_state_watch_rx.borrow().clone();
        Self::machine_bridge_session_id_for_identity(identity, &machine_state)
    }

    /// Observation-only bridge-session lookup that never enters the actor queue.
    ///
    /// This reads the roster's current member binding as a projection for
    /// observers that need to attach to session event/history streams. It is
    /// not a control admission seam and must not be used to decide membership
    /// legality.
    pub async fn resolve_bridge_session_id_observation(
        &self,
        identity: &AgentIdentity,
    ) -> Option<SessionId> {
        self.roster
            .read()
            .await
            .get_by_identity(identity)
            .and_then(|entry| entry.member_ref.bridge_session_id().cloned())
    }

    /// Acquire a capability-bearing handle for a specific member.
    pub async fn member(&self, identity: &AgentIdentity) -> Result<MemberHandle, MobError> {
        if let Some(diag) = self.restore_failure_for(identity).await {
            return Err(Self::restore_failure_error(identity, diag));
        }
        self.get_member(identity)
            .await?
            .ok_or_else(|| MobError::MemberNotFound(identity.clone()))?;
        Ok(MemberHandle {
            mob: self.clone(),
            agent_identity: identity.clone(),
        })
    }

    /// Access a read-only events view for polling, replay, and subscription.
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
                agent_identity: identity.clone(),
            })
            .await?
        {
            MobMachineCommandResult::EventStream(stream) => Ok(stream),
            _ => Err(MobError::Internal(
                "unexpected command result variant".into(),
            )),
        }
    }

    /// Observation-only event subscription that reads the authoritative
    /// placement and session binding directly instead of sending a command
    /// through the mob actor.
    ///
    /// The subscription authority remains the session service for the resolved
    /// local bridge session. Placed members require the actor-owned external
    /// event pump, so this non-blocking helper rejects them explicitly rather
    /// than treating their host-resident session id as a local session id.
    pub async fn subscribe_agent_events_observation(
        &self,
        identity: &AgentIdentity,
    ) -> Result<EventStream, MobError> {
        let runtime_mode = {
            let roster = self.roster.read().await;
            let entry = roster
                .get_by_identity(identity)
                .ok_or_else(|| MobError::MemberNotFound(identity.clone()))?;
            entry.runtime_mode
        };
        let machine_state = self.machine_state_watch_rx.borrow().clone();
        let dsl_identity = mob_dsl::AgentIdentity::from_domain(identity);
        if super::member_runtime_is_host_owned(&machine_state, identity) {
            return Err(MobError::UnsupportedForMode {
                mode: runtime_mode,
                reason: "observation-only agent event subscriptions for placed members require the actor-owned external event pump"
                    .to_string(),
            });
        }
        let session_id = machine_state
            .member_session_bindings
            .get(&dsl_identity)
            .ok_or_else(|| MobError::UnsupportedForMode {
                mode: runtime_mode,
                reason: "agent event subscriptions are not supported for peer-only members"
                    .to_string(),
            })
            .and_then(|session_id| {
                Self::session_id_from_dsl(session_id, "observation agent event subscription")
            })?;
        crate::runtime::session_service::MobSessionService::subscribe_session_events(
            self.session_service.as_ref(),
            &session_id,
        )
        .await
        .map_err(|error| {
            MobError::Internal(format!(
                "failed to subscribe to agent events for '{identity}': {error}"
            ))
        })
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
    pub async fn subscribe_mob_events(
        &self,
    ) -> Result<super::event_router::MobEventRouterHandle, MobError> {
        self.subscribe_mob_events_with_config(super::event_router::MobEventRouterConfig::default())
            .await
    }

    /// Like [`subscribe_mob_events`](Self::subscribe_mob_events) with explicit config.
    pub async fn subscribe_mob_events_with_config(
        &self,
        config: super::event_router::MobEventRouterConfig,
    ) -> Result<super::event_router::MobEventRouterHandle, MobError> {
        match self
            .execute_machine_command(MobMachineCommand::SubscribeMobEvents { config })
            .await
        {
            Ok(MobMachineCommandResult::MobEventRouter(handle)) => Ok(handle),
            Ok(_) => {
                tracing::error!("unexpected command result variant for subscribe_mob_events");
                Err(MobError::Internal(
                    "unexpected command result variant for subscribe_mob_events".into(),
                ))
            }
            Err(error) => Err(error),
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
        self.execute_machine_command(MobMachineCommand::PreviewRunFlowAdmission)
            .await?;
        self.ensure_flow_targets_provisioned(&flow_id).await?;
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

    async fn ensure_flow_targets_provisioned(&self, flow_id: &FlowId) -> Result<(), MobError> {
        let Some(flow) = self.definition.flows.get(flow_id) else {
            return Err(MobError::FlowNotFound(flow_id.clone()));
        };
        let mut required_roles = BTreeSet::new();
        for step in flow.steps.values() {
            required_roles.insert(step.role.clone());
        }

        for role in required_roles {
            let has_runnable_target = self
                .list_runnable_members()
                .await
                .into_iter()
                .any(|entry| entry.role == role);
            if has_runnable_target {
                continue;
            }
            let identity = AgentIdentity::from(format!(
                "__flow_{}_{}",
                sanitize_flow_member_segment(role.as_str()),
                sanitize_flow_member_segment(flow_id.as_str())
            ));
            if self.get_member(&identity).await?.is_some() {
                continue;
            }
            self.spawn_spec_internal_with_source(
                SpawnMemberSpec::new(role, identity)
                    .with_runtime_mode(crate::MobRuntimeMode::TurnDriven),
                SpawnSource::FlowProvisioning,
            )
            .await?;
        }
        Ok(())
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

    /// List flow runs for this mob, optionally filtered to one flow ID.
    pub async fn list_runs(&self, flow_id: Option<&FlowId>) -> Result<Vec<MobRun>, MobError> {
        self.run_store
            .list_runs(&self.definition.id, flow_id)
            .await
            .map_err(MobError::from)
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
        agent_identity: AgentIdentity,
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
        agent_identity: AgentIdentity,
        initial_message: Option<ContentInput>,
        binding: crate::RuntimeBinding,
    ) -> Result<MemberRef, MobError> {
        let external_binding = matches!(binding, crate::RuntimeBinding::External { .. });
        let mut spec = SpawnMemberSpec::new(profile_name, agent_identity);
        spec.initial_message = initial_message;
        spec.binding = Some(binding);
        if external_binding {
            let owner_context = self.create_generated_ops_owner_context_for_test().await?;
            return self
                .spawn_spec_receipt_with_owner_context(spec, owner_context)
                .await
                .map(|receipt| receipt.member_ref);
        }
        self.spawn_spec_internal(spec).await
    }

    /// Spawn a new member from a profile with explicit backend override.
    #[cfg(test)]
    pub(crate) async fn spawn_with_backend(
        &self,
        profile_name: ProfileName,
        agent_identity: AgentIdentity,
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
        agent_identity: AgentIdentity,
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
        agent_identity: AgentIdentity,
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
        agent_identity: AgentIdentity,
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
        let entry = self.get_member(&identity).await?.ok_or_else(|| {
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

    /// Spawn a member while binding child operations to a machine-minted owner context.
    ///
    /// The owner bridge-session id is only an input to `MeerkatMachine`; the
    /// ops lifecycle registry is obtained from the opaque runtime binding
    /// bundle returned by machine authority.
    pub async fn spawn_spec_with_generated_owner_context(
        &self,
        spec: SpawnMemberSpec,
        owner_bridge_session_id: SessionId,
    ) -> Result<SpawnResult, MobError> {
        let identity = spec.identity.clone();
        let _receipt = self
            .spawn_spec_receipt_with_generated_owner_context(spec, owner_bridge_session_id)
            .await?;
        let entry = self.get_member(&identity).await?.ok_or_else(|| {
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
        self.spawn_spec_internal_with_source(spec, SpawnSource::Consumer)
            .await
    }

    pub(crate) async fn spawn_spec_internal_with_source(
        &self,
        spec: SpawnMemberSpec,
        spawn_source: SpawnSource,
    ) -> Result<MemberRef, MobError> {
        let spawn_source = SpawnSource::for_launch_mode(spawn_source, &spec.launch_mode);
        match self
            .execute_machine_command(MobMachineCommand::Spawn {
                spec: Box::new(spec),
                spawn_source,
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

    async fn generated_ops_owner_context_from_runtime(
        &self,
        owner_bridge_session_id: SessionId,
        context: &'static str,
    ) -> Result<CanonicalOpsOwnerContext, MobError> {
        #[cfg(feature = "runtime-adapter")]
        {
            let adapter = self.runtime_adapter.as_ref().ok_or_else(|| {
                MobError::Internal(
                    "mob handle cannot prepare generated ops owner context without MeerkatMachine"
                        .into(),
                )
            })?;
            let bindings = adapter
                .prepare_local_session_bindings(owner_bridge_session_id.clone())
                .await
                .map_err(|error| {
                    MobError::Internal(format!(
                        "{context} generated operation owner binding failed: {error}"
                    ))
                })?;
            if bindings.session_id() != &owner_bridge_session_id {
                return Err(MobError::Internal(format!(
                    "{context} generated operation owner binding returned session '{}' for requested owner '{}'",
                    bindings.session_id(),
                    owner_bridge_session_id
                )));
            }
            if !meerkat_runtime::session_runtime_bindings_have_machine_authority(&bindings) {
                return Err(MobError::Internal(format!(
                    "{context} generated operation owner binding lacked MeerkatMachine authority"
                )));
            }
            Ok(CanonicalOpsOwnerContext {
                owner_bridge_session_id,
                ops_registry: Arc::clone(bindings.ops_lifecycle()),
            })
        }
        #[cfg(not(feature = "runtime-adapter"))]
        {
            let _ = owner_bridge_session_id;
            Err(MobError::Internal(format!(
                "{context} generated operation owner binding requires the runtime-adapter feature"
            )))
        }
    }

    #[cfg(test)]
    pub(crate) async fn generated_ops_owner_context_for_test(
        &self,
        owner_bridge_session_id: SessionId,
    ) -> Result<CanonicalOpsOwnerContext, MobError> {
        self.generated_ops_owner_context_from_runtime(
            owner_bridge_session_id,
            "test generated operation owner",
        )
        .await
    }

    #[cfg(test)]
    pub(crate) async fn create_generated_ops_owner_context_for_test(
        &self,
    ) -> Result<CanonicalOpsOwnerContext, MobError> {
        let created = self
            .session_service
            .create_session(meerkat_core::service::CreateSessionRequest {
                injected_context: Vec::new(),
                model: "claude-sonnet-4-5".to_string(),
                prompt: ContentInput::from("test generated mob operation owner"),
                system_prompt: meerkat_core::SystemPromptOverride::Inherit,
                max_tokens: None,
                event_tx: None,
                build: Some(meerkat_core::service::SessionBuildOptions::default()),
                initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
                deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
                labels: None,
            })
            .await
            .map_err(MobError::from)?;
        self.generated_ops_owner_context_for_test(created.session_id)
            .await
    }

    pub(super) async fn spawn_spec_receipt_with_owner_context(
        &self,
        spec: SpawnMemberSpec,
        owner_context: CanonicalOpsOwnerContext,
    ) -> Result<MemberSpawnReceipt, MobError> {
        self.spawn_spec_receipt_with_owner_context_and_source(
            spec,
            owner_context,
            SpawnSource::AgentSpawnMember,
        )
        .await
    }

    pub(super) async fn spawn_spec_receipt_with_owner_context_and_source(
        &self,
        spec: SpawnMemberSpec,
        owner_context: CanonicalOpsOwnerContext,
        spawn_source: SpawnSource,
    ) -> Result<MemberSpawnReceipt, MobError> {
        match self
            .execute_machine_command(MobMachineCommand::Spawn {
                spawn_source: SpawnSource::for_launch_mode(spawn_source, &spec.launch_mode),
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

    pub(super) async fn spawn_spec_receipt_with_generated_owner_context(
        &self,
        spec: SpawnMemberSpec,
        owner_bridge_session_id: SessionId,
    ) -> Result<MemberSpawnReceipt, MobError> {
        let owner_context = self
            .generated_ops_owner_context_from_runtime(
                owner_bridge_session_id,
                "mob spawn owner-bound operations",
            )
            .await?;
        self.spawn_spec_receipt_with_owner_context(spec, owner_context)
            .await
    }

    pub(super) async fn spawn_spec_receipt_with_generated_owner_context_and_source(
        &self,
        spec: SpawnMemberSpec,
        owner_bridge_session_id: SessionId,
        spawn_source: SpawnSource,
    ) -> Result<MemberSpawnReceipt, MobError> {
        let owner_context = self
            .generated_ops_owner_context_from_runtime(
                owner_bridge_session_id,
                "mob spawn owner-bound operations",
            )
            .await?;
        self.spawn_spec_receipt_with_owner_context_and_source(spec, owner_context, spawn_source)
            .await
    }

    async fn classify_spawn_many_failure(
        &self,
        error: MobError,
    ) -> Result<MobSpawnManyFailure, MobError> {
        let observation = spawn_many_failure_observation(&error);
        let effects = self
            .apply_machine_input_effects(mob_dsl::MobMachineInput::ClassifySpawnManyFailure {
                observation,
            })
            .await?;
        let (effect_observation, cause) = effects
            .into_iter()
            .find_map(|effect| match effect {
                mob_dsl::MobMachineEffect::SpawnManyFailureClassified { observation, cause } => {
                    Some((observation, cause))
                }
                _ => None,
            })
            .ok_or_else(|| {
                MobError::Internal(
                    "MobMachine accepted spawn_many failure observation but emitted no typed cause"
                        .into(),
                )
            })?;
        if effect_observation != observation {
            return Err(MobError::Internal(format!(
                "MobMachine spawn_many failure classification drift: input={observation:?}, effect={effect_observation:?}"
            )));
        }
        Ok(MobSpawnManyFailure {
            cause: spawn_many_failure_cause_from_dsl(cause),
            error,
        })
    }

    async fn classify_spawn_many_results<T>(
        &self,
        results: Vec<Result<T, MobError>>,
    ) -> Result<Vec<Result<T, MobSpawnManyFailure>>, MobError> {
        let mut classified = Vec::with_capacity(results.len());
        for result in results {
            match result {
                Ok(value) => classified.push(Ok(value)),
                Err(error) => classified.push(Err(self.classify_spawn_many_failure(error).await?)),
            }
        }
        Ok(classified)
    }

    /// Spawn multiple members in parallel.
    ///
    /// Results preserve input order.
    pub async fn spawn_many(
        &self,
        specs: Vec<SpawnMemberSpec>,
    ) -> Result<Vec<Result<SpawnResult, MobSpawnManyFailure>>, MobError> {
        let results = futures::future::join_all(specs.into_iter().map(|spec| async move {
            let identity = spec.identity.clone();
            self.spawn_spec_internal_with_source(spec, SpawnSource::BatchItem)
                .await?;
            let entry = self.get_member(&identity).await?.ok_or_else(|| {
                MobError::Internal(format!(
                    "spawn succeeded but roster entry missing for '{identity}'"
                ))
            })?;
            Ok(SpawnResult {
                agent_identity: entry.agent_identity,
                agent_runtime_id: entry.agent_runtime_id,
                fence_token: entry.fence_token,
            })
        }))
        .await;
        self.classify_spawn_many_results(results).await
    }

    pub(super) async fn spawn_many_receipts_with_owner_context(
        &self,
        specs: Vec<SpawnMemberSpec>,
        owner_context: CanonicalOpsOwnerContext,
    ) -> Result<Vec<Result<MemberSpawnReceipt, MobSpawnManyFailure>>, MobError> {
        let results = futures::future::join_all(specs.into_iter().map(|spec| {
            self.spawn_spec_receipt_with_owner_context_and_source(
                spec,
                owner_context.clone(),
                SpawnSource::BatchItem,
            )
        }))
        .await;
        self.classify_spawn_many_results(results).await
    }

    pub(super) async fn spawn_many_receipts_with_generated_owner_context(
        &self,
        specs: Vec<SpawnMemberSpec>,
        owner_bridge_session_id: SessionId,
    ) -> Result<Vec<Result<MemberSpawnReceipt, MobSpawnManyFailure>>, MobError> {
        let owner_context = self
            .generated_ops_owner_context_from_runtime(
                owner_bridge_session_id,
                "mob spawn_many owner-bound operations",
            )
            .await?;
        self.spawn_many_receipts_with_owner_context(specs, owner_context)
            .await
    }

    /// Retire a member, archiving its session and removing trust.
    pub async fn retire(&self, identity: AgentIdentity) -> Result<(), MobError> {
        match self
            .execute_machine_command(MobMachineCommand::Retire {
                agent_identity: identity,
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
        let reply = match self
            .execute_machine_command(MobMachineCommand::Respawn {
                agent_identity: identity,
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
        // MobMachine owns the spawn-vs-retain decision: `EnsureMember` emits
        // `MemberSpawnRequired` (member absent) or `MemberRetainRequired`
        // (member already present). The handle mirrors the emitted effect.
        let effects = self
            .apply_machine_input_effects(mob_dsl::MobMachineInput::EnsureMember {
                agent_identity: mob_dsl::AgentIdentity::from_domain(&identity),
            })
            .await?;
        let mut spawn_required = false;
        let mut retain_required = false;
        for effect in effects {
            match effect {
                mob_dsl::MobMachineEffect::MemberSpawnRequired { .. } => spawn_required = true,
                mob_dsl::MobMachineEffect::MemberRetainRequired { .. } => retain_required = true,
                _ => {}
            }
        }
        match (spawn_required, retain_required) {
            (true, false) => {
                // `Box::pin` breaks the compiler-visible recursion:
                // handle_ensure_member -> spawn_spec -> execute_machine_command
                // -> (MobMachineCommand::Spawn arm, which never re-enters here).
                match Box::pin(self.spawn_spec(spec)).await {
                    Ok(spawn_result) => Ok(EnsureMemberOutcome::Spawned(spawn_result)),
                    Err(MobError::MemberAlreadyExists(existing_identity))
                        if existing_identity == identity =>
                    {
                        // A concurrent ensure may have advanced the exact
                        // identity from machine-owned pending admission to a
                        // committed member after this EnsureMember decision.
                        // Converge on that winner instead of surfacing a
                        // duplicate-call failure.
                        let existing = self.await_machine_member_projection(&identity).await?;
                        Ok(EnsureMemberOutcome::Existed(Box::new(existing)))
                    }
                    Err(error) => Err(error),
                }
            }
            (false, true) => {
                // Membership commits before the mechanical roster projection.
                // The machine's retain verdict is authoritative even when a
                // concurrent ensure lands in that short projection interval.
                let existing = self.await_machine_member_projection(&identity).await?;
                Ok(EnsureMemberOutcome::Existed(Box::new(existing)))
            }
            _ => Err(MobError::Internal(format!(
                "ensure_member: MobMachine emitted no decisive spawn/retain verdict for '{identity}' (spawn_required={spawn_required}, retain_required={retain_required})"
            ))),
        }
    }

    async fn await_machine_member_projection(
        &self,
        identity: &AgentIdentity,
    ) -> Result<MobMemberListEntry, MobError> {
        let started = Instant::now();
        let dsl_identity = mob_dsl::AgentIdentity::from_domain(identity);
        loop {
            let machine_state = self.machine_state_watch_rx.borrow().clone();
            let lifecycle = machine_state.member_lifecycle_for_identity(&dsl_identity);
            let pending = machine_state
                .pending_spawn_sessions
                .contains_key(&dsl_identity);

            if lifecycle.status == mob_dsl::MobMemberLifecycleStatus::Active
                && let Some(entry) = self
                    .list_members()
                    .await
                    .into_iter()
                    .find(|entry| entry.agent_identity == *identity)
            {
                return Ok(entry);
            }

            if !pending && lifecycle.status != mob_dsl::MobMemberLifecycleStatus::Active {
                return Err(MobError::MemberNotFound(identity.clone()));
            }
            if started.elapsed() >= DEFAULT_READY_WAIT_TIMEOUT {
                return Err(MobError::ReadyWaitTimedOut {
                    pending_member_ids: vec![identity.clone()],
                });
            }

            // Machine membership commits just before the mechanical roster
            // insert. A short yield covers that projection window without
            // letting roster presence decide admission.
            tokio::time::sleep(Duration::from_millis(10)).await;
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
    ) -> Result<ReconcileReport, MobError> {
        // MobMachine owns the membership decision: `ReconcileRunning` recomputes
        // `members_to_spawn`/`members_to_retire`/`desired_members` from its own
        // roster. The handle reads those machine-owned sets from the returned
        // snapshot and resolves the full SpawnMemberSpec material per identity
        // (the machine does not own spawn parameters).
        let machine_state = self
            .project_machine_input(mob_dsl::MobMachineInput::Reconcile {
                desired: desired
                    .iter()
                    .map(|spec| mob_dsl::AgentIdentity::from_domain(&spec.identity))
                    .collect(),
                retire_stale: options.retire_stale,
            })
            .await?;

        let mut report = ReconcileReport {
            desired: desired.iter().map(|spec| spec.identity.clone()).collect(),
            ..ReconcileReport::default()
        };

        let mut specs_by_identity: std::collections::BTreeMap<AgentIdentity, SpawnMemberSpec> =
            desired
                .into_iter()
                .map(|spec| (spec.identity.clone(), spec))
                .collect();

        for dsl_identity in &machine_state.members_to_spawn {
            let identity = AgentIdentity::from(dsl_identity.0.as_str());
            let Some(spec) = specs_by_identity.remove(&identity) else {
                report.failures.push(ReconcileFailure {
                    agent_identity: identity,
                    error: MobError::Internal(
                        "reconcile: MobMachine requested a spawn for an identity absent from the desired set"
                            .to_string(),
                    ),
                    stage: ReconcileStage::Spawn,
                });
                continue;
            };
            match Box::pin(self.spawn_spec(spec)).await {
                Ok(spawn_result) => report.spawned.push(spawn_result),
                Err(error) => report.failures.push(ReconcileFailure {
                    agent_identity: identity,
                    error,
                    stage: ReconcileStage::Spawn,
                }),
            }
        }

        // Desired members the machine did not flag for spawning are already
        // present and retained.
        for dsl_identity in &machine_state.desired_members {
            if machine_state.members_to_spawn.contains(dsl_identity) {
                continue;
            }
            report
                .retained
                .push(AgentIdentity::from(dsl_identity.0.as_str()));
        }

        for dsl_identity in &machine_state.members_to_retire {
            let identity = AgentIdentity::from(dsl_identity.0.as_str());
            match Box::pin(self.retire(identity.clone())).await {
                Ok(()) => report.retired.push(identity),
                Err(error) => report.failures.push(ReconcileFailure {
                    agent_identity: identity,
                    error,
                    stage: ReconcileStage::Retire,
                }),
            }
        }

        Ok(report)
    }

    /// Core `list_members_matching` worker invoked by
    /// `execute_machine_command`. Composition over
    /// machine-projected list surfaces with each constraint applied
    /// conjunctively. An empty filter matches every non-retiring member.
    async fn handle_list_members_matching(&self, filter: MemberFilter) -> Vec<MobMemberListEntry> {
        let members = if filter.status == Some(MobMemberStatus::Retiring) {
            Box::pin(self.list_members_including_retiring()).await
        } else {
            Box::pin(self.list_members()).await
        };
        members
            .into_iter()
            .filter(|entry| {
                if let Some(role) = &filter.role
                    && entry.role != *role
                {
                    return false;
                }
                if let Some(status) = filter.status
                    && entry.status != status
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
    ///
    /// Returns `Err(MobError)` when the underlying machine command itself
    /// fails (query/transport fault); a transport fault must never be
    /// laundered into an empty match.
    pub async fn list_members_matching(
        &self,
        filter: MemberFilter,
    ) -> Result<Vec<MobMemberListEntry>, MobError> {
        let result = self
            .execute_machine_command(MobMachineCommand::ListMembersMatching {
                filter: Box::new(filter),
            })
            .await?;
        Self::project_list_members_matching_result(result)
    }

    /// Map a `ListMembersMatching` command result into the typed member list.
    ///
    /// Split out so the unexpected-variant fault arm (a genuine command-layer
    /// fault, never an empty match) is testable without laundering it into an
    /// empty `Vec`.
    fn project_list_members_matching_result(
        result: MobMachineCommandResult,
    ) -> Result<Vec<MobMemberListEntry>, MobError> {
        match result {
            MobMachineCommandResult::ListMembers(entries) => Ok(entries),
            _ => Err(MobError::Internal(
                "unexpected command result variant".into(),
            )),
        }
    }

    /// Test-only probe that drives [`Self::list_members_matching`]'s result
    /// mapping with an explicit command result, proving a command-layer fault
    /// surfaces as `Err(MobError)` and is never collapsed to an empty `Vec`.
    #[cfg(test)]
    pub(crate) fn project_list_members_matching_result_for_test(
        result: MobMachineCommandResult,
    ) -> Result<Vec<MobMemberListEntry>, MobError> {
        Self::project_list_members_matching_result(result)
    }

    /// Rotate the persisted mob supervisor authority.
    ///
    /// # Scope: mob-wide
    ///
    /// The supervisor authority is a **single per-mob fact** persisted in
    /// [`SupervisorAuthorityRecord`](crate::store::SupervisorAuthorityRecord) keyed by
    /// `mob_id`. Rotation generates a fresh authority (new public peer id,
    /// incremented epoch), durably records one stable operation id plus every
    /// member's exact target spec, and submits a one-way
    /// [`BridgeSupervisorDelivery::SubmitSupervisorRotation`](meerkat_contracts::wire::supervisor_bridge::BridgeSupervisorDelivery)
    /// to **every** remote member binding currently on the roster. The caller
    /// observes each member-owned operation separately and advances local
    /// authority only after every member returns an exact completed receipt.
    ///
    /// There is no per-member scope here, and no scoping parameter is
    /// missing. Per-member [`BridgeBootstrapToken`](meerkat_contracts::wire::supervisor_bridge::BridgeBootstrapToken)s
    /// carried on `MemberRef::BackendPeer` are the **bootstrap proof** that
    /// authorizes a specific member's bridge to (re)establish under the
    /// current supervisor — they are not a separate supervisor identity.
    /// One supervisor, many bootstrap tokens.
    ///
    /// # Incomplete-rotation semantics
    ///
    /// A delivery failure, observation timeout, or temporarily unreachable
    /// member leaves the durable operation pending. It never cancels the
    /// operation, rolls a completed member back, or reconstructs trust for the
    /// old supervisor. Exact retries reuse the persisted operation id and
    /// per-member target specs; members that already fenced the old authority
    /// are observed as the next authority, so an old-signed resubmission may
    /// fail without making mutation status ambiguous. Terminal rejection and
    /// mismatched operation/target receipts fail closed as
    /// [`MobError::SupervisorRotationIncomplete`].
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
    /// `test_rotate_supervisor_timeout_keeps_durable_operation_and_retry_reuses_id`,
    /// and the focused supervisor-delivery tests.
    pub async fn rotate_supervisor(&self) -> Result<SupervisorRotationReport, MobError> {
        self.send_actor_command(|reply_tx| MobCommand::RotateSupervisor { reply_tx })
            .await?
    }

    /// Bind a member-host daemon to this mob (§7.2 step 2).
    ///
    /// Drives the machine-owned ceremony: `BeginHostBind` opens the bind
    /// window and emits the `RequestHostBind` handoff, the actor realizes it
    /// as a `BindHost` bridge command against the descriptor-derived host
    /// identity, and `CommitHostBind` records the accepted identity, endpoint,
    /// epoch, and capability record. A failed ceremony leaves the machine in
    /// the `Requested` bind phase; retrying `bind_host` is idempotent
    /// (DEC-P2-10) and `revoke_host` clears the window.
    pub async fn bind_host(&self, request: HostBindRequest) -> Result<HostBindReport, MobError> {
        self.send_actor_command(|reply_tx| MobCommand::BindHost {
            request: Box::new(request),
            reply_tx,
        })
        .await?
    }

    /// Revoke a bound (or bind-requested) member host. A bound host first
    /// receives the authenticated host-addressed revoke and must return its
    /// durable terminal receipt after disposing every materialized member;
    /// only then are the controlling machine facts and durable authority row
    /// cleared. Reply loss is retryable through the host's durable receipt.
    /// `host_id` is the canonical peer id from [`HostBindReport::host_id`].
    /// Returns the typed [`HostRevokeReport`] naming placements affected on
    /// the controlling side.
    pub async fn revoke_host(&self, host_id: &str) -> Result<HostRevokeReport, MobError> {
        let host_id = host_id.to_string();
        self.send_actor_command(|reply_tx| MobCommand::RevokeHost { host_id, reply_tx })
            .await?
    }

    /// Host roster projection for `mob/hosts` (phase 7, ADJ-P7-2): a
    /// watch-read over the machine's bind facts + capability maps. One row
    /// per TRACKED host (`host_bind_phase` key set): `Bound` rows carry the
    /// CommitHostBind facts (endpoint, epoch, capability record — projected
    /// through THE one [`HostCapabilityReport::to_wire`] conversion);
    /// `Requested` rows carry typed-absent ceremony facts (an open or
    /// failed bind window commits nothing — fabricating empties would
    /// launder ceremony state into committed facts).
    /// `materialized_member_count` counts `member_placement` entries
    /// targeting the host.
    ///
    /// Reachability is the observer-local projection fed by periodic
    /// `HostStatus` and by member-events pages only after their actor-side
    /// runtime-incarnation/route barrier. It never mutates membership and
    /// uses only local monotonic elapsed time.
    pub fn hosts(&self) -> Result<meerkat_contracts::wire::MobHostsResult, MobError> {
        let state = self.machine_state_watch_rx.borrow().clone();
        let mut hosts = Vec::with_capacity(state.host_bind_phase.len());
        for (host_id, phase) in &state.host_bind_phase {
            let placed_count = state
                .member_placement
                .values()
                .filter(|placed| *placed == host_id)
                .count() as u64;
            let row = match phase {
                mob_dsl::HostBindPhase::Requested => meerkat_contracts::wire::MobHostStatus {
                    host_id: meerkat_contracts::wire::WireHostRef(host_id.as_str().to_string()),
                    endpoint: None,
                    bind_phase: meerkat_contracts::wire::WireHostBindPhase::Requested,
                    authority_epoch: None,
                    capabilities: None,
                    control_reachability: None,
                    last_seen_ms: None,
                    freshness_reason: None,
                    materialized_member_count: placed_count,
                },
                mob_dsl::HostBindPhase::Bound => {
                    // A Bound host missing any committed fact is machine
                    // drift — fail closed, never a fabricated row (the
                    // `bound_host_rotation_targets` posture).
                    let missing_fact = |fact: &str| {
                        MobError::Internal(format!(
                            "bound host '{}' has no recorded {fact}",
                            host_id.as_str()
                        ))
                    };
                    let endpoint = state
                        .host_endpoints
                        .get(host_id)
                        .ok_or_else(|| missing_fact("endpoint"))?;
                    let epoch = state
                        .host_authority_epochs
                        .get(host_id)
                        .copied()
                        .ok_or_else(|| missing_fact("authority epoch"))?;
                    let capabilities =
                        HostCapabilityReport {
                            protocol_min: state
                                .host_protocol_min
                                .get(host_id)
                                .copied()
                                .ok_or_else(|| missing_fact("protocol_min capability"))?,
                            protocol_max: state
                                .host_protocol_max
                                .get(host_id)
                                .copied()
                                .ok_or_else(|| missing_fact("protocol_max capability"))?,
                            engine_version: state
                                .host_engine_versions
                                .get(host_id)
                                .cloned()
                                .ok_or_else(|| missing_fact("engine_version capability"))?,
                            durable_sessions: state
                                .host_durable_sessions
                                .get(host_id)
                                .copied()
                                .ok_or_else(|| missing_fact("durable_sessions capability"))?,
                            autonomous_members: state
                                .host_autonomous_members
                                .get(host_id)
                                .copied()
                                .ok_or_else(|| missing_fact("autonomous_members capability"))?,
                            hard_cancel_member: state
                                .host_hard_cancel_member
                                .get(host_id)
                                .copied()
                                .ok_or_else(|| missing_fact("hard_cancel_member capability"))?,
                            tracked_input_cancel: state
                                .host_tracked_input_cancel
                                .get(host_id)
                                .copied()
                                .ok_or_else(|| missing_fact("tracked_input_cancel capability"))?,
                            memory_store: state
                                .host_memory_store
                                .get(host_id)
                                .copied()
                                .ok_or_else(|| missing_fact("memory_store capability"))?,
                            mcp: state
                                .host_mcp
                                .get(host_id)
                                .copied()
                                .ok_or_else(|| missing_fact("mcp capability"))?,
                            resolvable_providers: state
                                .host_resolvable_providers
                                .get(host_id)
                                .cloned()
                                .ok_or_else(|| missing_fact("resolvable_providers capability"))?,
                            approval_forwarding: state
                                .host_approval_forwarding
                                .get(host_id)
                                .copied()
                                .ok_or_else(|| missing_fact("approval_forwarding capability"))?,
                            // DL5: presence IS the live capability; absence is
                            // the fact, not a missing-fact fault.
                            live_endpoint: state
                                .host_live_endpoints
                                .get(host_id)
                                .map(|url| url.0.clone()),
                        };
                    let observation = self.reachability_observations.host(host_id.as_str());
                    meerkat_contracts::wire::MobHostStatus {
                        host_id: meerkat_contracts::wire::WireHostRef(host_id.as_str().to_string()),
                        endpoint: Some(endpoint.0.clone()),
                        bind_phase: meerkat_contracts::wire::WireHostBindPhase::Bound,
                        authority_epoch: Some(epoch),
                        capabilities: Some(capabilities.to_wire()),
                        control_reachability: observation.as_ref().map(|view| view.reachability),
                        last_seen_ms: observation.as_ref().and_then(|view| view.last_seen_ms),
                        freshness_reason: observation.map(|view| view.freshness_reason),
                        materialized_member_count: placed_count,
                    }
                }
            };
            hosts.push(row);
        }
        Ok(meerkat_contracts::wire::MobHostsResult { hosts })
    }

    /// Record (full-replace) a principal's control-scope grant (§8).
    ///
    /// `caller` must hold `AdminGrants` (owner implicit) — the grant verbs
    /// are themselves scope-gated, and the gate runs in the actor arm BEFORE
    /// the machine input is built (§17.2). The explicit `caller` parameter
    /// is deliberately asymmetric with the ambient-owner handle surface:
    /// operator authority is injected, not ambient (gotcha #19), and this is
    /// the principal-injection seam the scope-matrix fixtures use.
    ///
    /// The write path never reads a clock: an already-past `expires_at_ms`
    /// is recorded verbatim and is inert at the enforcement seam (one expiry
    /// evaluator, at `require()` time).
    pub async fn grant_scopes(
        &self,
        caller: crate::control_policy::MobControlPrincipal,
        principal: meerkat_core::auth::PrincipalId,
        scopes: BTreeSet<mob_dsl::ControlScope>,
        expires_at_ms: Option<u64>,
    ) -> Result<crate::control_policy::OperatorGrant, MobError> {
        self.send_actor_command(|reply_tx| MobCommand::GrantScopes {
            caller,
            principal,
            scopes,
            expires_at_ms,
            reply_tx,
        })
        .await?
    }

    /// Revoke scopes (`None` = the entire grant). Returns `removed` =
    /// whether the principal's grant RECORD was removed entirely: an
    /// absent-grant no-op and a partial revoke return `false`, a full
    /// removal returns `true` (the `MobRevokeScopesResult.removed` wire
    /// semantics). Revoke is idempotent: revoking never-granted scopes is a
    /// no-op, not an error — the actor intersects the request with the
    /// recorded set and the machine revalidates the partition.
    /// `AdminGrants`-gated like [`Self::grant_scopes`].
    pub async fn revoke_scopes(
        &self,
        caller: crate::control_policy::MobControlPrincipal,
        principal: meerkat_core::auth::PrincipalId,
        scopes: Option<BTreeSet<mob_dsl::ControlScope>>,
    ) -> Result<bool, MobError> {
        self.send_actor_command(|reply_tx| MobCommand::RevokeScopes {
            caller,
            principal,
            scopes,
            reply_tx,
        })
        .await?
    }

    /// List raw grant records read from machine state (never from the
    /// durable records — the machine is the owner; records are recovery
    /// inputs). Expired grants appear with their verbatim `expires_at_ms`
    /// and NO expired flag: evaluating expiry requires a clock read, and the
    /// enforcement seam is the one place that reads the clock against
    /// grants. Consoles render staleness client-side. `AdminGrants`-gated
    /// like its siblings (§17.2 gates all three grant verbs).
    pub async fn grants(
        &self,
        caller: crate::control_policy::MobControlPrincipal,
    ) -> Result<Vec<crate::control_policy::OperatorGrant>, MobError> {
        self.send_actor_command(|reply_tx| MobCommand::Grants { caller, reply_tx })
            .await?
    }

    /// Wire a local member to either another local member or an external peer.
    pub async fn wire<T>(&self, local: AgentIdentity, target: T) -> Result<(), MobError>
    where
        T: Into<PeerTarget>,
    {
        match self
            .execute_machine_command(MobMachineCommand::Wire {
                local: local.clone(),
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

    /// Materialize many local-member wiring edges in one actor command.
    ///
    /// This is intended for initial topology reconciliation, where callers
    /// already have a graph snapshot. It only accepts local mob-member
    /// identities; external peer wiring stays on the single-edge path because
    /// those mutations carry descriptor/rollback semantics per peer.
    pub async fn wire_members_batch<I, A, B>(
        &self,
        edges: I,
    ) -> Result<MobWireMembersBatchReport, MobError>
    where
        I: IntoIterator<Item = (A, B)>,
        A: Into<AgentIdentity>,
        B: Into<AgentIdentity>,
    {
        let edges = edges
            .into_iter()
            .map(|(a, b)| (a.into(), b.into()))
            .collect();
        match self
            .execute_machine_command(MobMachineCommand::WireMembersBatch { edges })
            .await?
        {
            MobMachineCommandResult::WireMembersBatchReport(report) => Ok(report),
            _ => Err(MobError::Internal(
                "unexpected command result variant".into(),
            )),
        }
    }

    /// Route-install status projection (multi-host §10.4, ADJ-P4-9a):
    /// the machine's outstanding cross-host route-install obligations
    /// (`pending_route_installs`), projected into the wire shape. A wire
    /// operation whose remote installs did not all confirm returns Ok with
    /// its obligations retained here — `complete == false` names exactly
    /// what is still converging (fail closed, never fail quiet). Synchronous
    /// pre-unwire Removes never enter this install-only projection. Projection
    /// only: the drain reads machine state directly, never this view.
    pub async fn route_installs(
        &self,
    ) -> Result<meerkat_contracts::wire::MobRouteInstallsResult, MobError> {
        let state = self.query_machine_state().await?;
        let mut outstanding = Vec::with_capacity(state.pending_route_installs.len());
        for obligation in &state.pending_route_installs {
            if obligation.kind != crate::machines::mob_machine::RouteObligationKind::Install {
                return Err(MobError::Internal(format!(
                    "MobMachine invariant violation: pending route ledger contains non-Install obligation for host '{}'",
                    obligation.host.as_str()
                )));
            }
            outstanding.push(meerkat_contracts::wire::WireRouteInstallObligation {
                edge_a: obligation.edge.a.0.clone(),
                edge_b: obligation.edge.b.0.clone(),
                host: meerkat_contracts::wire::WireHostRef(obligation.host.as_str().to_string()),
            });
        }
        Ok(meerkat_contracts::wire::MobRouteInstallsResult {
            complete: outstanding.is_empty(),
            outstanding,
        })
    }

    /// Explicit route-install drain (ADJ-P4-9b): realizes every PENDING
    /// obligation in the machine ledger through the one canonical drain the
    /// authenticated periodic HostStatus, host-(re)bind, and revival triggers
    /// also run actor-side. The drive re-derives nothing — the obligation set
    /// is the in-flight truth, so an idle drive sends nothing (re-derivation
    /// from durable graph facts belongs to the rebind/recovery/revival
    /// triggers that know trust was invalidated). Returns the post-drain
    /// projection — Ok with a non-empty `outstanding` set names the
    /// obligations still pending (per-obligation failures never fail the
    /// drive).
    pub async fn drive_route_installs(
        &self,
    ) -> Result<meerkat_contracts::wire::MobRouteInstallsResult, MobError> {
        self.send_actor_command(|reply_tx| MobCommand::DriveRouteInstalls { reply_tx })
            .await??;
        self.route_installs().await
    }

    /// Send typed peer communication from one mob member to another.
    ///
    /// This uses the sender member's comms runtime and the mob's installed
    /// wiring/trust state. It is deliberately distinct from
    /// [`MemberHandle::send`], which submits anonymous external work.
    pub async fn send_peer_message(
        &self,
        from: AgentIdentity,
        to: AgentIdentity,
        content: impl Into<meerkat_core::types::ContentInput>,
        handling_mode: HandlingMode,
    ) -> Result<PeerMessageReceipt, MobError> {
        let receipt = self
            .send_actor_command(|reply_tx| MobCommand::SendPeerMessage {
                from: from.clone(),
                to: to.clone(),
                content: content.into(),
                handling_mode,
                reply_tx,
            })
            .await??;
        match receipt {
            SendReceipt::PeerMessageSent {
                envelope_id,
                delivery,
            } => Ok(PeerMessageReceipt {
                from,
                to,
                envelope_id,
                delivery,
                handling_mode,
            }),
            other => Err(MobError::Internal(format!(
                "unexpected peer-message receipt variant: {other:?}"
            ))),
        }
    }

    /// Install (or clear) the host-owned outbound content-taint declaration
    /// for a member.
    ///
    /// The HOST owns the "this member's session content is tainted" fact
    /// (e.g. its content-trust tracker); this call makes the member's comms
    /// runtime the authenticated carrier — the declaration is stamped inside
    /// the signed region of every outbound content-bearing peer envelope
    /// until changed, so receivers get the sender's own claim instead of
    /// reconstructing taint from host-side joins. `None` clears the
    /// declaration (envelopes carry no claim — never coalesced into clean).
    ///
    /// The declaration does not survive respawn/reset: a fresh runtime
    /// starts with no declaration, matching fresh-context taint semantics —
    /// re-declare when your tracker re-marks the new context. External-bound
    /// members receive the declaration over the supervisor bridge.
    pub async fn declare_member_outbound_taint(
        &self,
        identity: AgentIdentity,
        taint: Option<meerkat_core::comms::SenderContentTaint>,
    ) -> Result<(), MobError> {
        self.send_actor_command(|reply_tx| MobCommand::DeclareMemberOutboundTaint {
            identity: identity.clone(),
            taint,
            reply_tx,
        })
        .await?
    }

    /// Unwire a local member from either another local member or an external peer.
    pub async fn unwire<T>(&self, local: AgentIdentity, target: T) -> Result<(), MobError>
    where
        T: Into<PeerTarget>,
    {
        match self
            .execute_machine_command(MobMachineCommand::Unwire {
                local: local.clone(),
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

    pub(super) async fn external_turn_for_member(
        &self,
        agent_identity: AgentIdentity,
        message: meerkat_core::types::ContentInput,
        handling_mode: HandlingMode,
        turn_metadata: Option<RuntimeTurnMetadata>,
        observers: MemberTurnObservers,
    ) -> Result<(AgentRuntimeId, FenceToken, Option<SessionId>), MobError> {
        let domain_identity = AgentIdentity::from(agent_identity.as_str());
        let machine_state = self.machine_state_watch_rx.borrow().clone();
        let dsl_identity = mob_dsl::AgentIdentity::from_domain(&domain_identity);
        let Some((runtime_id, fence_token)) = machine_state
            .member_runtime_material_for_identity(&dsl_identity)
            .map(|material| material.to_domain_for_identity(&domain_identity))
        else {
            return Err(self
                .resolve_submit_work_missing_runtime_rejection(
                    &agent_identity,
                    WorkOrigin::External,
                    "external_turn_for_member",
                )
                .await);
        };
        let session_id =
            Self::machine_bridge_session_id_for_identity(&domain_identity, &machine_state);
        let cmd = Box::new(crate::mob_machine::SubmitWorkCommand {
            runtime_id: runtime_id.clone(),
            fence_token,
            work_ref: WorkRef::new(),
            spec: WorkSpec::new(message, WorkOrigin::External),
            handling_mode,
            turn_metadata,
            event_tx: observers.event_tx,
            completion_tx: observers.completion_tx,
            llm_identity_applied_tx: observers.llm_identity_applied_tx,
            ack_mode: crate::mob_machine::SubmitWorkAckMode::IngressAccepted,
        });
        self.execute_machine_command(MobMachineCommand::SubmitWork(cmd))
            .await?;
        Ok((runtime_id, fence_token, session_id))
    }

    pub(super) async fn internal_turn_for_member(
        &self,
        agent_identity: AgentIdentity,
        message: meerkat_core::types::ContentInput,
    ) -> Result<(AgentRuntimeId, FenceToken), MobError> {
        let (runtime_id, fence_token) = self
            .resolve_submit_work_runtime_binding(
                &agent_identity,
                WorkOrigin::Internal,
                "internal_turn_for_member",
            )
            .await?;
        let cmd = Box::new(crate::mob_machine::SubmitWorkCommand {
            runtime_id: runtime_id.clone(),
            fence_token,
            work_ref: WorkRef::new(),
            spec: WorkSpec::new(message, WorkOrigin::Internal),
            handling_mode: HandlingMode::Queue,
            turn_metadata: None,
            event_tx: None,
            completion_tx: None,
            llm_identity_applied_tx: None,
            ack_mode: crate::mob_machine::SubmitWorkAckMode::TurnCompleted,
        });
        self.execute_machine_command(MobMachineCommand::SubmitWork(cmd))
            .await?;
        Ok((runtime_id, fence_token))
    }

    async fn resolve_submit_work_runtime_binding(
        &self,
        agent_identity: &AgentIdentity,
        origin: WorkOrigin,
        context: &str,
    ) -> Result<(AgentRuntimeId, FenceToken), MobError> {
        let domain_identity = AgentIdentity::from(agent_identity.as_str());
        if let Some(binding) = self.machine_runtime_identity_fields_for_identity(&domain_identity) {
            return Ok(binding);
        }
        Err(self
            .resolve_submit_work_missing_runtime_rejection(agent_identity, origin, context)
            .await)
    }

    async fn resolve_submit_work_missing_runtime_rejection(
        &self,
        agent_identity: &AgentIdentity,
        origin: WorkOrigin,
        context: &str,
    ) -> MobError {
        let domain_identity = AgentIdentity::from(agent_identity.as_str());
        let declared_runtime_id = AgentRuntimeId::initial(domain_identity.clone());
        let declared_fence_token = FenceToken::new(0);
        let effects = match self
            .apply_machine_input_effects(mob_dsl::MobMachineInput::ResolveSubmitWorkRejection {
                agent_identity: mob_dsl::AgentIdentity::from_domain(&domain_identity),
                agent_runtime_id: mob_dsl::AgentRuntimeId::from_domain(&declared_runtime_id),
                fence_token: mob_dsl::FenceToken::from_domain(declared_fence_token),
                origin: mob_dsl::WorkOrigin::from(origin),
            })
            .await
        {
            Ok(effects) => effects,
            Err(error) => return error,
        };
        let Some(reason) = effects.into_iter().find_map(|effect| match effect {
            mob_dsl::MobMachineEffect::SubmitWorkRejected {
                agent_runtime_id,
                origin: effect_origin,
                reason,
                ..
            } if agent_runtime_id == mob_dsl::AgentRuntimeId::from_domain(&declared_runtime_id)
                && effect_origin == mob_dsl::WorkOrigin::from(origin) =>
            {
                Some(reason)
            }
            _ => None,
        }) else {
            return MobError::Internal(format!(
                "{context} requires MobMachine runtime binding for '{agent_identity}', and generated SubmitWork rejection emitted no result"
            ));
        };

        match reason {
            mob_dsl::SubmitWorkRejectReasonKind::MemberNotFound => {
                MobError::MemberNotFound(agent_identity.clone())
            }
            mob_dsl::SubmitWorkRejectReasonKind::NotExternallyAddressable => {
                MobError::NotExternallyAddressable(agent_identity.clone())
            }
            mob_dsl::SubmitWorkRejectReasonKind::StaleFenceToken => MobError::StaleFenceToken {
                runtime_id: declared_runtime_id,
                expected: declared_fence_token,
                actual: declared_fence_token,
            },
            mob_dsl::SubmitWorkRejectReasonKind::MobNotRunning => MobError::InvalidTransition {
                // Error rendering only: the machine already rejected; the
                // watch observation avoids a principal-gated QueryPhase
                // round-trip inside error construction.
                from: self.status_observation_snapshot(),
                to: MobState::Running,
            },
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
        self.submit_work_with_mode(runtime_id, fence_token, work_ref, spec, HandlingMode::Queue)
            .await
    }

    /// Submit a unit of work to a mob member with an explicit turn handling mode.
    ///
    /// This is the ingress-acknowledged work-lane counterpart to member send.
    /// The caller supplies the already-authorized runtime binding and fence
    /// token; the mob machine still owns work-origin legality and stale-fence
    /// rejection.
    pub async fn submit_work_with_mode(
        &self,
        runtime_id: AgentRuntimeId,
        fence_token: FenceToken,
        work_ref: WorkRef,
        spec: WorkSpec,
        handling_mode: HandlingMode,
    ) -> Result<WorkDeliveryReceipt, MobError> {
        let cmd = Box::new(crate::mob_machine::SubmitWorkCommand {
            runtime_id: runtime_id.clone(),
            fence_token,
            work_ref: work_ref.clone(),
            spec,
            handling_mode,
            turn_metadata: None,
            event_tx: None,
            completion_tx: None,
            llm_identity_applied_tx: None,
            ack_mode: crate::mob_machine::SubmitWorkAckMode::IngressAccepted,
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
    /// Per-unit cancellation has no backing work-tracking ledger, so this
    /// always fails closed with [`MobError::WorkCancellationUnsupported`]
    /// rather than returning a phantom success or a misleading
    /// `WorkNotFound`. Use [`MobHandle::cancel_all_work`] to cancel a
    /// member's in-flight work.
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
        let deadline = Instant::now() + DEFAULT_KICKOFF_WAIT_TIMEOUT;
        let mut retry_delay = Duration::from_millis(25);
        loop {
            match self.execute_machine_command(MobMachineCommand::Stop).await {
                Ok(MobMachineCommandResult::Unit) => return Ok(()),
                Ok(_) => {
                    return Err(MobError::Internal(
                        "unexpected command result variant".into(),
                    ));
                }
                Err(
                    error @ (MobError::PlacedCompletionCleanupPending { .. }
                    | MobError::PlacedKickoffCleanupPending { .. }
                    | MobError::AutonomousStopInterruptsPending { .. }),
                ) => {
                    if Instant::now() >= deadline {
                        return Err(error);
                    }
                    tokio::time::sleep(retry_delay).await;
                    retry_delay = (retry_delay * 2).min(Duration::from_millis(250));
                }
                Err(error) => return Err(error),
            }
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
    /// | event marker         | `MobCreated` + `MobReset` (new epoch, replayable)  | `MobDestroying` until successful storage clear              |
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
    pub(crate) async fn debug_lifecycle_notification_burst(
        &self,
        count: usize,
        message: impl Into<String>,
    ) -> Result<(), MobError> {
        match self
            .execute_machine_command(MobMachineCommand::LifecycleNotificationBurst {
                count,
                message: message.into(),
            })
            .await?
        {
            MobMachineCommandResult::LifecycleNotificationBurst => Ok(()),
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

    #[cfg(test)]
    pub(crate) async fn debug_stage_pending_spawn_for_retire(
        &self,
        agent_identity: AgentIdentity,
        pending_spawn_session_id: SessionId,
        operation_id: meerkat_core::ops::OperationId,
    ) -> Result<(), MobError> {
        self.send_actor_command(|reply_tx| MobCommand::StagePendingSpawnForRetireTest {
            agent_identity,
            pending_spawn_session_id,
            operation_id,
            reply_tx,
        })
        .await?
    }

    /// 0.7.2 L5 test seam: deliver a kickoff completion outcome through the
    /// REAL actor command path (`MobCommand::KickoffOutcomeResolved`), exactly
    /// as the spawned completion-waiter task does. Used by the
    /// teardown-interleaving tests to drive a deterministic "outcome arrives
    /// after retire/destroy" ordering without racing task aborts.
    #[cfg(all(test, feature = "runtime-adapter"))]
    pub(crate) async fn debug_inject_kickoff_outcome(
        &self,
        agent_identity: AgentIdentity,
        outcome: Result<
            meerkat_runtime::completion::CompletionOutcome,
            meerkat_runtime::completion::CompletionWaitError,
        >,
    ) -> Result<(), MobError> {
        self.send_actor_command(|ack_tx| MobCommand::KickoffOutcomeResolved {
            agent_identity,
            outcome,
            ack_tx,
        })
        .await
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
        let deadline = Instant::now() + DEFAULT_KICKOFF_WAIT_TIMEOUT;
        let mut retry_delay = Duration::from_millis(25);
        loop {
            match self
                .execute_machine_command(MobMachineCommand::Shutdown)
                .await
            {
                Ok(MobMachineCommandResult::Unit) => return Ok(()),
                Ok(_) => {
                    return Err(MobError::Internal(
                        "unexpected command result variant".into(),
                    ));
                }
                Err(
                    error @ (MobError::PlacedCompletionCleanupPending { .. }
                    | MobError::PlacedKickoffCleanupPending { .. }
                    | MobError::AutonomousStopInterruptsPending { .. }),
                ) => {
                    if Instant::now() >= deadline {
                        return Err(error);
                    }
                    tokio::time::sleep(retry_delay).await;
                    retry_delay = (retry_delay * 2).min(Duration::from_millis(250));
                }
                Err(error) => return Err(error),
            }
        }
    }

    /// Stop only volatile runtime tasks, preserving durable active-run and
    /// remote-turn custody exactly as a process crash would.
    ///
    /// This is intentionally hidden from the product command surface. It is
    /// used by persistence recovery harnesses that must not launder a crash
    /// into graceful `Canceled` run terminalization before rebuilding.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub async fn crash_stop_preserving_durable_work_for_test(&self) -> Result<(), MobError> {
        self.send_actor_command(
            |reply_tx| MobCommand::CrashStopPreservingDurableWorkForTest { reply_tx },
        )
        .await?
    }

    /// Force-cancel a member's in-flight turn via the user interrupt path.
    ///
    /// Unlike [`retire`](Self::retire), this does not archive the session or
    /// remove the member from the roster — it only cancels the current turn.
    pub async fn force_cancel_member(&self, identity: AgentIdentity) -> Result<(), MobError> {
        match self
            .execute_machine_command(MobMachineCommand::ForceCancel {
                agent_identity: identity.clone(),
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
        snapshot: &MobMemberSnapshot,
        pending_kickoff_member_ids: &BTreeSet<String>,
    ) -> bool {
        if entry.runtime_mode != crate::MobRuntimeMode::AutonomousHost {
            return true;
        }
        match snapshot.status {
            MobMemberStatus::Unknown => false,
            MobMemberStatus::Active => {
                !pending_kickoff_member_ids.contains(entry.agent_identity.as_str())
            }
            MobMemberStatus::Retiring | MobMemberStatus::Broken | MobMemberStatus::Completed => {
                true
            }
        }
    }

    fn ready_wait_is_satisfied(
        entry: &RosterEntry,
        machine_state: &mob_dsl::MobMachineState,
    ) -> bool {
        if entry.runtime_mode != crate::MobRuntimeMode::AutonomousHost {
            return true;
        }

        let dsl_identity = mob_dsl::AgentIdentity::from_domain(&entry.agent_identity);
        let lifecycle = machine_state.member_lifecycle_for_identity(&dsl_identity);
        match lifecycle.status {
            mob_dsl::MobMemberLifecycleStatus::Unknown => false,
            mob_dsl::MobMemberLifecycleStatus::Active => {
                if super::member_runtime_is_host_owned(machine_state, &entry.agent_identity) {
                    machine_state.placed_member_ready_for_identity(&dsl_identity)
                } else {
                    machine_state
                        .identity_to_runtime
                        .get(&dsl_identity)
                        .is_some_and(|runtime_id| {
                            machine_state
                                .member_startup_runtime_ready
                                .contains(runtime_id)
                                || machine_state.member_startup_ready.contains(runtime_id)
                        })
                }
            }
            mob_dsl::MobMemberLifecycleStatus::Retiring
            | mob_dsl::MobMemberLifecycleStatus::Broken
            | mob_dsl::MobMemberLifecycleStatus::Completed => true,
        }
    }

    async fn wait_for_kickoff_resolution(
        &self,
        target_ids: &[AgentIdentity],
        timeout: Option<Duration>,
    ) -> Result<(), MobError> {
        if target_ids.is_empty() {
            return Ok(());
        }

        let deadline = Instant::now() + timeout.unwrap_or(DEFAULT_KICKOFF_WAIT_TIMEOUT);
        loop {
            let kickoff_snapshot = self.startup_kickoff_snapshot().await?;
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
                let member_snapshot = self
                    .member_status(&AgentIdentity::from(id.as_str()))
                    .await?;
                if !Self::kickoff_wait_is_satisfied(
                    entry,
                    &member_snapshot,
                    &kickoff_snapshot.pending_kickoff_member_ids,
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
        target_ids: &[AgentIdentity],
        timeout: Option<Duration>,
    ) -> Result<(), MobError> {
        if target_ids.is_empty() {
            return Ok(());
        }

        let deadline = Instant::now() + timeout.unwrap_or(DEFAULT_READY_WAIT_TIMEOUT);
        let mut machine_state_rx = self.machine_state_watch_rx.clone();
        let mut observed_missing_bridge_sessions = BTreeSet::new();
        loop {
            let machine_state = machine_state_rx.borrow().clone();
            let entries = {
                let roster = self.roster.read().await;
                roster
                    .list_all()
                    .cloned()
                    .map(|entry| (entry.agent_identity.clone(), entry))
                    .collect::<HashMap<_, _>>()
            };

            let mut pending_member_ids = Vec::new();
            for id in target_ids {
                let Some(entry) = entries.get(id) else {
                    continue;
                };
                if !Self::ready_wait_is_satisfied(entry, &machine_state) {
                    pending_member_ids.push(id.clone());
                }
            }

            if pending_member_ids.is_empty() {
                return Ok(());
            }

            self.reconcile_missing_ready_wait_bridge_sessions(
                &entries,
                &machine_state,
                &pending_member_ids,
                &mut observed_missing_bridge_sessions,
            )
            .await?;

            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(MobError::ReadyWaitTimedOut { pending_member_ids });
            }
            let sleep_for = std::cmp::min(remaining, READY_WAIT_BRIDGE_SESSION_RECHECK_INTERVAL);

            tokio::select! {
                () = tokio::time::sleep(sleep_for) => {
                    if sleep_for == remaining {
                        return Err(MobError::ReadyWaitTimedOut { pending_member_ids });
                    }
                }
                changed = machine_state_rx.changed() => {
                    changed.map_err(|_| MobError::ActorCommandChannelClosed)?;
                }
            }
        }
    }

    async fn reconcile_missing_ready_wait_bridge_sessions(
        &self,
        entries: &HashMap<AgentIdentity, RosterEntry>,
        machine_state: &mob_dsl::MobMachineState,
        pending_member_ids: &[AgentIdentity],
        observed_missing_bridge_sessions: &mut BTreeSet<(AgentIdentity, String)>,
    ) -> Result<(), MobError> {
        let probes = pending_member_ids
            .iter()
            .filter_map(|identity| {
                let entry = entries.get(identity)?;
                if entry.runtime_mode != crate::MobRuntimeMode::AutonomousHost {
                    return None;
                }
                let dsl_identity = mob_dsl::AgentIdentity::from_domain(identity);
                let lifecycle = machine_state.member_lifecycle_for_identity(&dsl_identity);
                if lifecycle.status != mob_dsl::MobMemberLifecycleStatus::Active {
                    return None;
                }
                if super::member_runtime_is_host_owned(machine_state, identity) {
                    // The bound session belongs to the member host. Never probe
                    // it through the controlling host's local session service.
                    return None;
                }
                let bridge_session_id =
                    Self::machine_bridge_session_id_for_identity(identity, machine_state)?;
                Some((identity.clone(), bridge_session_id))
            })
            .collect::<Vec<_>>();

        for (identity, bridge_session_id) in probes {
            let bridge_session_key = bridge_session_id.to_string();
            if observed_missing_bridge_sessions
                .contains(&(identity.clone(), bridge_session_key.clone()))
            {
                continue;
            }
            match self.session_service.read(&bridge_session_id).await {
                Ok(_) => {}
                Err(SessionError::NotFound { .. }) => {
                    // Route the observation through the actor's binding-currency
                    // guard (the same path member_status uses) rather than firing
                    // RecoverMemberRestoreFailure raw: the read above is awaited, so
                    // a member can rebind to a fresh session in the gap. The actor
                    // re-checks the live member_session_binding before marking the
                    // member broken and drops the observation if it has gone stale.
                    match self
                        .command_tx
                        .try_send(super::scope_gate::RoutedMobCommand::internal(
                            MobCommand::RecordMissingMemberBridgeSession {
                                agent_identity: identity.clone(),
                                bridge_session_id: bridge_session_id.clone(),
                            },
                        )) {
                        Ok(()) => {
                            observed_missing_bridge_sessions.insert((identity, bridge_session_key));
                        }
                        Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                            tracing::debug!(
                                agent_identity = %identity,
                                bridge_session_id = %bridge_session_id,
                                "ready wait observed missing bridge session but actor command channel is full; will retry"
                            );
                        }
                        Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                            return Err(MobError::ActorCommandChannelClosed);
                        }
                    }
                }
                Err(_) => {}
            }
        }

        Ok(())
    }

    async fn wait_one_snapshot(
        &self,
        agent_identity: &AgentIdentity,
    ) -> Result<MobMemberSnapshot, MobError> {
        loop {
            let wait_class = self.classify_member_wait(agent_identity).await?;
            if wait_class == mob_dsl::MemberWaitClassificationKind::MissingRuntimeMaterial {
                return Err(MobError::Internal(format!(
                    "MobMachine runtime material is absent for member '{agent_identity}'"
                )));
            }
            let snapshot = self
                .member_status(&AgentIdentity::from(agent_identity.as_str()))
                .await?;
            if snapshot.is_final {
                return Ok(snapshot);
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }

    async fn classify_member_wait(
        &self,
        agent_identity: &AgentIdentity,
    ) -> Result<mob_dsl::MemberWaitClassificationKind, MobError> {
        let dsl_identity =
            mob_dsl::AgentIdentity::from_domain(&AgentIdentity::from(agent_identity.as_str()));
        let effects = self
            .apply_machine_input_effects(mob_dsl::MobMachineInput::ClassifyMemberWait {
                agent_identity: dsl_identity.clone(),
            })
            .await?;
        let (effect_identity, result) = effects
            .into_iter()
            .find_map(|effect| match effect {
                mob_dsl::MobMachineEffect::MemberWaitClassified {
                    agent_identity,
                    result,
                } => Some((agent_identity, result)),
                _ => None,
            })
            .ok_or_else(|| {
                MobError::Internal(
                    "MobMachine accepted member wait classification but emitted no result".into(),
                )
            })?;
        if effect_identity != dsl_identity {
            return Err(MobError::Internal(format!(
                "MobMachine member wait classification drift: input={dsl_identity:?}, effect={effect_identity:?}"
            )));
        }
        Ok(result)
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
            .project_retiring_member_status_from_machine_state(identity)
            .await
        {
            Some(snapshot) => snapshot,
            None => {
                self.send_actor_command(|reply_tx| MobCommand::ProjectMemberStatus {
                    agent_identity: identity.clone(),
                    reply_tx,
                })
                .await??
            }
        };
        snapshot.peer_connectivity = match tokio::time::timeout(
            Duration::from_secs(2),
            self.project_member_peer_connectivity(identity, &snapshot),
        )
        .await
        {
            Ok(connectivity) => Some(connectivity),
            Err(_) => {
                tracing::warn!(
                    agent_identity = %identity,
                    "mob member status peer-connectivity projection timed out"
                );
                // A timed-out probe is a transient unknown, NOT a structurally
                // absent binding: surface it as the explicit ProbeTimedOut
                // tri-state arm rather than collapsing it into None/NotApplicable.
                Some(meerkat_contracts::WirePeerConnectivity::ProbeTimedOut)
            }
        };
        snapshot.resolved_capabilities = self
            .project_resolved_capabilities(identity, &snapshot)
            .await;
        snapshot.external_member = self
            .project_external_member_observation(identity, &snapshot)
            .await;
        // Placement (phase 7, ADJ-P7-2): the machine's `member_placement`
        // fact, watch-read at THE status projection — never re-derived by
        // surface handlers.
        let (placement, durable_sessions) = {
            let dsl_identity = mob_dsl::AgentIdentity::from_domain(identity);
            let state = self.machine_state_watch_rx.borrow();
            let placement = state.member_placement.get(&dsl_identity).cloned();
            let durable_sessions = placement
                .as_ref()
                .and_then(|host| state.host_durable_sessions.get(host).copied());
            (placement, durable_sessions)
        };
        snapshot.placement = placement.as_ref().map(|host| host.as_str().to_string());
        if let Some(host) = placement {
            let control = self.reachability_observations.host(host.as_str());
            let comms = self.reachability_observations.member(identity);
            snapshot.control_reachability = control.as_ref().map(|view| view.reachability);
            snapshot.comms_reachability = comms.as_ref().map(|view| view.reachability);
            snapshot.last_seen_ms = comms
                .as_ref()
                .and_then(|view| view.last_seen_ms)
                .or_else(|| control.as_ref().and_then(|view| view.last_seen_ms));
            snapshot.freshness_reason = comms
                .as_ref()
                .map(|view| view.freshness_reason.clone())
                .or_else(|| control.as_ref().map(|view| view.freshness_reason.clone()));
            snapshot.lifecycle_capabilities = durable_sessions.map(|durable| {
                meerkat_contracts::wire::WireMemberLifecycleCapabilities {
                    transcript_edits: false,
                    revisions: false,
                    resume_after_restart: durable,
                }
            });
            snapshot.non_portable_disabled = Some(Vec::new());
        }
        Ok(snapshot)
    }

    /// Explicitly conclude the durable kickoff objective owned by `identity`.
    pub async fn conclude_objective(
        &self,
        identity: &AgentIdentity,
        objective_id: meerkat_core::interaction::ObjectiveId,
        outcome: impl Into<String>,
    ) -> Result<(), MobError> {
        self.execute_machine_command(MobMachineCommand::ConcludeObjective {
            agent_identity: identity.clone(),
            objective_id,
            outcome: outcome.into(),
        })
        .await
        .map(|_| ())
    }

    /// Bind a propagated objective to its lead principal before delegated
    /// member kickoff can observe the same correlation id.
    pub async fn bind_objective_owner(
        &self,
        owner_identity: AgentIdentity,
        objective_id: meerkat_core::interaction::ObjectiveId,
    ) -> Result<(), MobError> {
        self.send_actor_command(|reply_tx| MobCommand::BindObjectiveOwner {
            owner_identity,
            objective_id,
            reply_tx,
        })
        .await?
    }

    async fn project_member_peer_connectivity(
        &self,
        identity: &AgentIdentity,
        snapshot: &MobMemberSnapshot,
    ) -> meerkat_contracts::WirePeerConnectivity {
        let placed = {
            let state = self.machine_state_watch_rx.borrow();
            super::member_runtime_is_host_owned(&state, identity)
        };
        if placed {
            // Placed connectivity is projected from host/member reachability
            // observations below; its remote session id is never a license to
            // probe the controlling host's local session service.
            return meerkat_contracts::WirePeerConnectivity::NotApplicable;
        }
        // No bridge session backs this member: live peer connectivity is not a
        // resolvable fact, so the projection is NotApplicable rather than a
        // None that a consumer could mistake for "resolved, zero peers".
        let Some(bridge_session_id) = snapshot.current_bridge_session_id().cloned() else {
            return meerkat_contracts::WirePeerConnectivity::NotApplicable;
        };
        let (entry, roster_snapshot) = {
            let roster = self.roster.read().await;
            match roster.get(identity).cloned() {
                Some(entry) => (entry, roster.snapshot()),
                None => return meerkat_contracts::WirePeerConnectivity::NotApplicable,
            }
        };
        match self
            .resolve_peer_connectivity(&entry, &bridge_session_id, &roster_snapshot)
            .await
        {
            Some(snapshot) => meerkat_contracts::WirePeerConnectivity::Known {
                snapshot: peer_connectivity_snapshot_to_wire(snapshot),
            },
            None => meerkat_contracts::WirePeerConnectivity::NotApplicable,
        }
    }

    /// Project the current realtime attachment status for the given member
    /// snapshot by consulting the MeerkatMachine runtime adapter. Returns
    async fn project_resolved_capabilities(
        &self,
        identity: &AgentIdentity,
        snapshot: &MobMemberSnapshot,
    ) -> Option<meerkat_contracts::WireResolvedModelCapabilities> {
        #[cfg(feature = "runtime-adapter")]
        {
            use meerkat_runtime::service_ext::SessionServiceRuntimeExt as _;
            if super::member_runtime_is_host_owned(&self.machine_state_watch_rx.borrow(), identity)
            {
                return None;
            }
            let session_id = snapshot.current_bridge_session_id().cloned()?;
            let runtime = self.runtime_adapter.as_ref()?.as_ref();
            runtime
                .resolved_session_llm_capabilities(&session_id)
                .await
                .ok()
                .flatten()
                .map(|surface| surface.to_wire_resolved())
        }
        #[cfg(not(feature = "runtime-adapter"))]
        {
            let _ = snapshot;
            None
        }
    }

    async fn project_external_member_observation(
        &self,
        identity: &AgentIdentity,
        snapshot: &MobMemberSnapshot,
    ) -> Option<ExternalMemberObservationSnapshot> {
        let entry = {
            let roster = self.roster.read().await;
            roster.get(identity).cloned()
        }?;
        let MemberRef::BackendPeer { .. } = &entry.member_ref else {
            return None;
        };

        let owner = ExternalMemberOwnerRef {
            mob_id: self.definition.id.clone(),
            agent_identity: identity.clone(),
        };
        // Bridge-session presence AND the rebind capability are both
        // machine-owned facts. Read both from `MobMachineState`: bridge-session
        // binding from `member_session_bindings`, and the
        // `Available`/`Unavailable` rebind capability from
        // `external_member_rebind_capability` (recorded by
        // `SetExternalMemberRebindCapability` at spawn-mint / recovery /
        // peer-only-rebind). The shell no longer derives this from the roster
        // `MemberRef.bootstrap_token` presence.
        let machine_state = self.machine_state_watch_rx.borrow().clone();
        let bridge_session_present =
            Self::machine_bridge_session_id_for_identity(identity, &machine_state).is_some();
        let dsl_identity = mob_dsl::AgentIdentity::from_domain(identity);
        let rebind_available = matches!(
            machine_state
                .external_member_rebind_capability
                .get(&dsl_identity),
            Some(mob_dsl::ExternalMemberRebindCapability::Available)
        );
        let binding_mode = if bridge_session_present {
            ExternalMemberBindingMode::BridgeSessionBacked
        } else {
            ExternalMemberBindingMode::PeerOnly
        };
        let reachability = match snapshot.status {
            MobMemberStatus::Broken => ExternalMemberReachability::Unavailable {
                reason: snapshot
                    .error
                    .clone()
                    .unwrap_or_else(|| "external member restore failed".to_string()),
            },
            _ => ExternalMemberReachability::Unknown,
        };
        let rebind = match snapshot.status {
            MobMemberStatus::Broken => ExternalMemberRebindStatus::Failed {
                reason: snapshot
                    .error
                    .clone()
                    .unwrap_or_else(|| "external member restore failed".to_string()),
            },
            _ if bridge_session_present => ExternalMemberRebindStatus::NotRequired,
            _ if rebind_available => ExternalMemberRebindStatus::Available,
            _ => ExternalMemberRebindStatus::Unavailable {
                reason: "missing bootstrap_token for supervisor rebind".to_string(),
            },
        };

        Some(ExternalMemberObservationSnapshot {
            owner: owner.clone(),
            binding_mode,
            bridge_session_present,
            reachability,
            rebind,
            forwarding: ExternalMemberObservationSnapshot::forwarding(&owner),
        })
    }

    /// Wait until all current autonomous members resolve their initial kickoff.
    ///
    /// In 0.6 autonomous members no longer run a synthetic second kickoff turn,
    /// but their initial prompt still resolves asynchronously through the
    /// runtime-backed input path. This barrier is satisfied once each targeted
    /// autonomous member leaves `pending` / `starting` and reaches a terminal
    /// completion-handle phase. `callback_pending` is terminal for this barrier
    /// (and remains visible in the member snapshot for external fulfillment).
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
        self.wait_for_kickoff_resolution(ids, timeout).await?;

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
        self.wait_for_ready_resolution(ids, timeout).await?;

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
        self.wait_one_snapshot(identity).await
    }

    /// Wait for all specified members to reach terminal states.
    pub async fn wait_all(
        &self,
        identities: &[AgentIdentity],
    ) -> Result<Vec<MobMemberSnapshot>, MobError> {
        let futs = identities
            .iter()
            .map(|identity| self.wait_one_snapshot(identity))
            .collect::<Vec<_>>();
        let results = futures::future::join_all(futs).await;
        results.into_iter().collect()
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
        // This is one SendCommand-scoped composite.  Admit before profile or
        // roster inspection; status collection and retirement below are
        // internal completion mechanics of the admitted operation.
        self.admit_control_scope(mob_dsl::ControlScope::SendCommand)
            .await?;
        let profile_name = options
            .role_name
            .or_else(|| self.definition.profiles.keys().next().cloned())
            .ok_or_else(|| {
                MobError::Internal("no profile specified and definition has no profiles".into())
            })?;
        let task_text = task.into();
        let member_identity = identity.clone();
        let mut spec = SpawnMemberSpec::new(profile_name, identity.clone());
        spec.initial_message = Some(task_text.into());
        spec.runtime_mode = Some(
            options
                .runtime_mode
                .unwrap_or(crate::MobRuntimeMode::TurnDriven),
        );
        spec.backend = options.backend;
        spec.tool_access_policy = options.tool_access_policy;
        spec.auth_binding = options.auth_binding;
        spec.inherited_tool_filter = options.inherited_tool_filter;
        spec.override_profile = options.override_profile;
        spec.model_override = options.model_override;
        spec.objective_id = options.objective_id;
        spec.auto_wire_parent = true;

        self.spawn_spec_internal_with_source(spec, SpawnSource::HelperSpawn)
            .await?;
        let admitted = self.clone().with_command_authority(
            crate::control_policy::CommandAuthority::principal(
                crate::control_policy::MobControlPrincipal::Owner,
            ),
        );
        let helper_snapshot = admitted.member_status(&identity).await?;
        let (agent_runtime_id, fence_token) =
            helper_snapshot.require_runtime_identity_fields("spawn_helper result")?;
        let agent_identity = helper_snapshot.agent_identity().clone();
        let agent_runtime_id = agent_runtime_id.clone();
        admitted.retire(identity).await?;

        Ok(HelperResult {
            output: helper_snapshot.output_preview,
            tokens_used: helper_snapshot.tokens_used,
            agent_identity,
            agent_runtime_id,
            fence_token,
        })
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
        self.admit_control_scope(mob_dsl::ControlScope::SendCommand)
            .await?;
        let profile_name = options
            .role_name
            .or_else(|| self.definition.profiles.keys().next().cloned())
            .ok_or_else(|| {
                MobError::Internal("no profile specified and definition has no profiles".into())
            })?;
        let task_text = task.into();
        let member_identity = identity.clone();
        let source_member_id = source_identity.clone();
        let mut spec = SpawnMemberSpec::new(profile_name, identity.clone());
        spec.initial_message = Some(task_text.into());
        spec.runtime_mode = Some(
            options
                .runtime_mode
                .unwrap_or(crate::MobRuntimeMode::TurnDriven),
        );
        spec.backend = options.backend;
        spec.tool_access_policy = options.tool_access_policy;
        spec.auth_binding = options.auth_binding;
        spec.inherited_tool_filter = options.inherited_tool_filter;
        spec.override_profile = options.override_profile;
        spec.model_override = options.model_override;
        spec.objective_id = options.objective_id;
        spec.auto_wire_parent = true;
        spec.launch_mode = crate::launch::MemberLaunchMode::Fork {
            source_member_id,
            fork_context,
        };

        self.spawn_spec_internal_with_source(spec, SpawnSource::Fork)
            .await?;
        let admitted = self.clone().with_command_authority(
            crate::control_policy::CommandAuthority::principal(
                crate::control_policy::MobControlPrincipal::Owner,
            ),
        );
        let helper_snapshot = admitted.member_status(&identity).await?;
        let (agent_runtime_id, fence_token) =
            helper_snapshot.require_runtime_identity_fields("fork_helper result")?;
        let agent_identity = helper_snapshot.agent_identity().clone();
        let agent_runtime_id = agent_runtime_id.clone();
        admitted.retire(identity).await?;

        Ok(HelperResult {
            output: helper_snapshot.output_preview,
            tokens_used: helper_snapshot.tokens_used,
            agent_identity,
            agent_runtime_id,
            fence_token,
        })
    }

    pub(crate) async fn project_machine_input(
        &self,
        input: crate::machines::mob_machine::MobMachineInput,
    ) -> Result<crate::machines::mob_machine::MobMachineState, MobError> {
        self.send_actor_command(|reply_tx| MobCommand::ProjectMachineInput {
            input: Box::new(input),
            reply_tx,
        })
        .await?
    }

    pub(crate) async fn apply_machine_input_effects(
        &self,
        input: crate::machines::mob_machine::MobMachineInput,
    ) -> Result<Vec<crate::machines::mob_machine::MobMachineEffect>, MobError> {
        self.send_actor_command(|reply_tx| MobCommand::ApplyMachineInputEffects {
            input: Box::new(input),
            reply_tx,
        })
        .await?
    }

    /// Enter the actor command gate and return only after the handle's bound
    /// authority has been validated against the actor-owned machine state.
    /// Fenced remote member-operator execution uses this before projection-only
    /// operations; mutating commands revalidate again at their own mailbox turn.
    pub(crate) async fn validate_command_authority(&self) -> Result<(), MobError> {
        self.send_actor_command(|reply_tx| MobCommand::ValidateCommandAuthority { reply_tx })
            .await?
    }

    /// Enter the actor's serialized principal gate for a composite or
    /// projection-only surface before it performs any raw lookup/effect.
    pub async fn admit_control_scope(
        &self,
        required: mob_dsl::ControlScope,
    ) -> Result<(), MobError> {
        self.send_actor_command(|reply_tx| MobCommand::AdmitControlScope { required, reply_tx })
            .await?
    }

    /// Ask the actor to derive the exact current placed-residency set from its
    /// serialized machine state and reclaim only stale member-operator ledger
    /// rows. Invoked before every admitted durable begin; actor startup runs
    /// the same seam for crash/recovery convergence.
    pub(crate) async fn prune_stale_member_operator_requests(&self) -> Result<u64, MobError> {
        self.send_actor_command(|reply_tx| MobCommand::PruneStaleMemberOperatorRequests {
            reply_tx,
        })
        .await?
    }

    pub(crate) async fn reserve_remote_turn_obligation(
        &self,
        intent: crate::run::MobRunRemoteTurnIntent,
    ) -> Result<super::remote_flow_ticket::ReservedRemoteTurnIntent, MobError> {
        self.send_actor_command(|reply_tx| MobCommand::ReserveRemoteTurnObligation {
            intent: Box::new(intent),
            reply_tx,
        })
        .await?
    }

    pub(crate) async fn commit_remote_turn_receipt(
        &self,
        receipt: crate::run::MobRunRemoteTurnReceipt,
    ) -> Result<(), MobError> {
        self.send_actor_command(|reply_tx| MobCommand::CommitRemoteTurnReceipt {
            receipt: Box::new(receipt),
            reply_tx,
        })
        .await?
    }

    pub(crate) async fn close_remote_turn_after_tracked_cancel(
        &self,
        receipt: crate::run::MobRunRemoteTurnReceipt,
    ) -> Result<(), MobError> {
        self.send_actor_command(|reply_tx| MobCommand::CloseRemoteTurnAfterTrackedCancel {
            receipt: Box::new(receipt),
            reply_tx,
        })
        .await?
    }

    pub(crate) async fn ensure_remote_turn_record(
        &self,
        obligation: crate::event::RemoteTurnObligationEvent,
    ) -> Result<(), MobError> {
        self.send_actor_command(|reply_tx| MobCommand::EnsureRemoteTurnRecord {
            obligation: Box::new(obligation),
            reply_tx,
        })
        .await?
    }

    pub(crate) async fn converge_recovered_flow_run(
        &self,
        run_id: crate::ids::RunId,
    ) -> Result<(), MobError> {
        self.send_actor_command(|reply_tx| MobCommand::ConvergeRecoveredFlowRun {
            run_id,
            reply_tx,
        })
        .await?
    }

    pub(crate) async fn resolve_remote_turn_outcome(
        &self,
        obligation: crate::machines::mob_machine::RemoteTurnObligation,
        record: super::bridge_protocol::BridgeTurnOutcomeRecord,
    ) -> Result<(), MobError> {
        self.send_actor_command(|reply_tx| MobCommand::ResolveRemoteTurnOutcome {
            obligation: Box::new(obligation),
            record,
            reply_tx,
        })
        .await?
    }

    pub(crate) async fn acknowledge_remote_turn_outcome(
        &self,
        obligation: crate::machines::mob_machine::RemoteTurnObligation,
        ack: super::bridge_protocol::BridgeTurnOutcomeAck,
    ) -> Result<(), MobError> {
        self.send_actor_command(|reply_tx| MobCommand::AcknowledgeRemoteTurnOutcome {
            obligation: Box::new(obligation),
            ack,
            reply_tx,
        })
        .await?
    }

    pub(crate) async fn finalize_remote_turn_privacy_cleanup(
        &self,
        cleanup: super::remote_turn_reconciler::FinalizedRemoteTurnPrivacyCleanup,
    ) -> Result<(), MobError> {
        self.send_actor_command(|reply_tx| MobCommand::FinalizeRemoteTurnPrivacyCleanup {
            cleanup: Box::new(cleanup),
            reply_tx,
        })
        .await?
    }

    pub(crate) async fn request_placed_completion_cancellation(
        &self,
        obligation: crate::event::PlacedCompletionObligationEvent,
    ) -> Result<(), MobError> {
        self.send_actor_command(|reply_tx| MobCommand::RequestPlacedCompletionCancellation {
            obligation: Box::new(obligation),
            reply_tx,
        })
        .await?
    }

    pub(crate) async fn resolve_placed_completion_outcome(
        &self,
        obligation: crate::event::PlacedCompletionObligationEvent,
        record: super::bridge_protocol::BridgeTurnOutcomeRecord,
    ) -> Result<(), MobError> {
        self.send_actor_command(|reply_tx| MobCommand::ResolvePlacedCompletionOutcome {
            obligation: Box::new(obligation),
            record,
            reply_tx,
        })
        .await?
    }

    pub(crate) async fn close_placed_completion_outcome(
        &self,
        obligation: crate::event::PlacedCompletionObligationEvent,
        closure: crate::event::PlacedCompletionClosureEvent,
    ) -> Result<(), MobError> {
        self.send_actor_command(|reply_tx| MobCommand::ClosePlacedCompletionOutcome {
            obligation: Box::new(obligation),
            closure,
            reply_tx,
        })
        .await?
    }

    pub(crate) async fn acknowledge_placed_completion_outcome(
        &self,
        obligation: crate::event::PlacedCompletionObligationEvent,
        ack: super::bridge_protocol::BridgeTurnOutcomeAck,
    ) -> Result<(), MobError> {
        self.send_actor_command(|reply_tx| MobCommand::AcknowledgePlacedCompletionOutcome {
            obligation: Box::new(obligation),
            ack,
            reply_tx,
        })
        .await?
    }

    pub(crate) async fn resolve_placed_kickoff_outcome(
        &self,
        obligation: crate::event::PlacedKickoffObligationEvent,
        record: super::bridge_protocol::BridgeTurnOutcomeRecord,
    ) -> Result<(), MobError> {
        self.send_actor_command(|reply_tx| MobCommand::ResolvePlacedKickoffOutcome {
            obligation: Box::new(obligation),
            record,
            reply_tx,
        })
        .await?
    }

    pub(crate) async fn resolve_placed_kickoff_cancelled(
        &self,
        obligation: crate::event::PlacedKickoffObligationEvent,
    ) -> Result<(), MobError> {
        self.send_actor_command(|reply_tx| MobCommand::ResolvePlacedKickoffCancelled {
            obligation: Box::new(obligation),
            reply_tx,
        })
        .await?
    }

    pub(crate) async fn acknowledge_placed_kickoff_outcome(
        &self,
        obligation: crate::event::PlacedKickoffObligationEvent,
        ack: super::bridge_protocol::BridgeTurnOutcomeAck,
    ) -> Result<(), MobError> {
        self.send_actor_command(|reply_tx| MobCommand::AcknowledgePlacedKickoffOutcome {
            obligation: Box::new(obligation),
            ack,
            reply_tx,
        })
        .await?
    }

    pub(crate) async fn reject_placed_kickoff_before_admission(
        &self,
        obligation: crate::event::PlacedKickoffObligationEvent,
        error: String,
    ) -> Result<(), MobError> {
        self.send_actor_command(|reply_tx| MobCommand::RejectPlacedKickoffBeforeAdmission {
            obligation: Box::new(obligation),
            error,
            reply_tx,
        })
        .await?
    }

    /// Initialize an AdaptiveRun and mint the per-run driver capability.
    ///
    /// This is the only public minting path for [`AdaptiveDriverCapability`].
    pub async fn initialize_adaptive_run(
        &self,
        request: InitializeAdaptiveRunRequest,
    ) -> Result<AdaptiveDriverCapability, MobError> {
        let InitializeAdaptiveRunRequest {
            adaptive_run_id,
            limits,
        } = request;
        let dsl_run_id = mob_dsl::AdaptiveRunId(adaptive_run_id.clone());
        let effects = self
            .apply_machine_input_effects(mob_dsl::MobMachineInput::InitializeAdaptiveRun {
                adaptive_run_id: dsl_run_id.clone(),
                max_depth: limits.max_depth,
                max_total_decisions: limits.max_total_decisions,
                max_repair_attempts: limits.max_repair_attempts,
                max_layer_failures: limits.max_layer_failures,
                max_attempts_per_layer: limits.max_attempts_per_layer,
                max_members_per_layer: limits.max_members_per_layer,
                max_total_spawned_members: limits.max_total_spawned_members,
                max_active_members: limits.max_active_members,
                max_retained_layer_mobs: limits.max_retained_layer_mobs,
                max_aggregate_tokens: limits.max_aggregate_tokens,
                max_aggregate_tool_calls: limits.max_aggregate_tool_calls,
                allowed_model_classes: limits.allowed_model_classes,
                allowed_tool_classes: limits.allowed_tool_classes,
                allowed_skill_identities: limits.allowed_skill_identities,
                allowed_auth_binding_refs: limits.allowed_auth_binding_refs,
                deadline_ms: limits.deadline_ms,
            })
            .await?;
        let initialized = effects.into_iter().any(|effect| {
            matches!(
                effect,
                mob_dsl::MobMachineEffect::AdaptiveRunInitialized { adaptive_run_id }
                    if adaptive_run_id == dsl_run_id
            )
        });
        if !initialized {
            return Err(MobError::Internal(
                "MobMachine accepted InitializeAdaptiveRun but emitted no adaptive capability evidence"
                    .into(),
            ));
        }
        Ok(AdaptiveDriverCapability {
            adaptive_run_id,
            _private: (),
        })
    }

    pub async fn adaptive_run_snapshot(
        &self,
        capability: &AdaptiveDriverCapability,
    ) -> Result<AdaptiveRunSnapshot, MobError> {
        self.adaptive_run_snapshot_by_id(&capability.adaptive_run_id)
            .await
    }

    /// Read-only projection of adaptive kernel state for public status surfaces.
    ///
    /// Mutation of the adaptive kernel remains gated by
    /// `AdaptiveDriverCapability`; status surfaces need to project by the
    /// caller-visible AdaptiveRun id without minting a driver capability.
    pub async fn adaptive_run_snapshot_by_id(
        &self,
        adaptive_run_id: &str,
    ) -> Result<AdaptiveRunSnapshot, MobError> {
        let run_id = mob_dsl::AdaptiveRunId(adaptive_run_id.to_string());
        let state = self.query_machine_state().await?;
        let phase = state
            .adaptive_run_phase
            .get(&run_id)
            .copied()
            .map(Into::into);
        let stop_reason = state
            .adaptive_stop_reason
            .get(&run_id)
            .copied()
            .map(Into::into);
        let mut layers = BTreeMap::new();
        for (layer_id, phase) in &state.adaptive_layer_phase {
            if state.adaptive_layer_adaptive_run.get(layer_id) != Some(&run_id) {
                continue;
            }
            let layer_key = layer_id.0.clone();
            layers.insert(
                layer_key.clone(),
                AdaptiveLayerSnapshot {
                    layer_id: layer_key,
                    phase: (*phase).into(),
                    attempt: state
                        .adaptive_layer_attempt
                        .get(layer_id)
                        .copied()
                        .unwrap_or_default(),
                    child_run_id: state
                        .adaptive_layer_run_id
                        .get(layer_id)
                        .and_then(|run_id| run_id.0.parse::<RunId>().ok()),
                    result_digest: state.adaptive_layer_result_digest.get(layer_id).cloned(),
                    plan_digest: state.adaptive_layer_plan_digest.get(layer_id).cloned(),
                    child_mob_id: state
                        .adaptive_layer_child_mob_id
                        .get(layer_id)
                        .map(|mob_id| mob_id.0.clone()),
                },
            );
        }
        Ok(AdaptiveRunSnapshot {
            adaptive_run_id: adaptive_run_id.to_string(),
            phase,
            stop_reason,
            depth: state
                .adaptive_depth
                .get(&run_id)
                .copied()
                .unwrap_or_default(),
            total_decisions: state
                .adaptive_total_decisions
                .get(&run_id)
                .copied()
                .unwrap_or_default(),
            repair_attempts: state
                .adaptive_repair_attempts
                .get(&run_id)
                .copied()
                .unwrap_or_default(),
            layer_failures: state
                .adaptive_layer_failures
                .get(&run_id)
                .copied()
                .unwrap_or_default(),
            total_spawned_members: state
                .adaptive_total_spawned_members
                .get(&run_id)
                .copied()
                .unwrap_or_default(),
            active_members: state
                .adaptive_active_members
                .get(&run_id)
                .copied()
                .unwrap_or_default(),
            retained_layer_mobs: state
                .adaptive_retained_layer_mobs
                .get(&run_id)
                .copied()
                .unwrap_or_default(),
            aggregate_token_reserved: state
                .adaptive_aggregate_token_reserved
                .get(&run_id)
                .copied()
                .unwrap_or_default(),
            aggregate_token_actual: state
                .adaptive_aggregate_token_actual
                .get(&run_id)
                .copied()
                .unwrap_or_default(),
            aggregate_tool_call_reserved: state
                .adaptive_aggregate_tool_call_reserved
                .get(&run_id)
                .copied()
                .unwrap_or_default(),
            aggregate_tool_call_actual: state
                .adaptive_aggregate_tool_call_actual
                .get(&run_id)
                .copied()
                .unwrap_or_default(),
            missing_body_digest: state.adaptive_missing_body_digest.get(&run_id).cloned(),
            layers,
        })
    }

    pub async fn record_adaptive_planning_decision(
        &self,
        capability: &AdaptiveDriverCapability,
        decision_kind: AdaptivePlanningDecisionKind,
    ) -> Result<(), MobError> {
        self.apply_machine_input_effects(mob_dsl::MobMachineInput::RecordPlanningDecision {
            adaptive_run_id: mob_dsl::AdaptiveRunId(capability.adaptive_run_id.clone()),
            decision_kind: decision_kind.into(),
        })
        .await
        .map(|_| ())
    }

    pub async fn record_adaptive_plan_rejected(
        &self,
        capability: &AdaptiveDriverCapability,
        layer_id: impl Into<String>,
    ) -> Result<(), MobError> {
        self.apply_machine_input_effects(mob_dsl::MobMachineInput::RecordPlanRejected {
            adaptive_run_id: mob_dsl::AdaptiveRunId(capability.adaptive_run_id.clone()),
            layer_id: mob_dsl::AdaptiveLayerId(layer_id.into()),
        })
        .await
        .map(|_| ())
    }

    pub async fn resolve_adaptive_layer_admission(
        &self,
        capability: &AdaptiveDriverCapability,
        request: AdaptiveLayerAdmissionRequest,
    ) -> Result<AdaptiveLayerAdmission, MobError> {
        let dsl_run_id = mob_dsl::AdaptiveRunId(capability.adaptive_run_id.clone());
        let dsl_layer_id = mob_dsl::AdaptiveLayerId(request.layer_id);
        let effects = self
            .apply_machine_input_effects(mob_dsl::MobMachineInput::ResolveLayerAdmission {
                adaptive_run_id: dsl_run_id.clone(),
                layer_id: dsl_layer_id.clone(),
                attempt: request.attempt,
                plan_digest: request.plan_digest,
                child_mob_id: mob_dsl::MobId(request.child_mob_id),
                member_count: request.member_count,
                token_reservation: request.token_reservation,
                tool_call_reservation: request.tool_call_reservation,
                used_model_classes: request.used_model_classes,
                used_tool_classes: request.used_tool_classes,
                used_skill_identities: request.used_skill_identities,
                used_auth_binding_refs: request.used_auth_binding_refs,
                observed_at_ms: request.observed_at_ms,
            })
            .await?;
        effects
            .into_iter()
            .find_map(|effect| match effect {
                mob_dsl::MobMachineEffect::AdaptiveLayerAdmissionResolved {
                    adaptive_run_id,
                    layer_id,
                    admission,
                } if adaptive_run_id == dsl_run_id && layer_id == dsl_layer_id => {
                    Some(AdaptiveLayerAdmission::from(admission))
                }
                _ => None,
            })
            .ok_or_else(|| {
                MobError::Internal(
                    "MobMachine accepted ResolveLayerAdmission but emitted no adaptive admission verdict"
                        .into(),
                )
            })
    }

    pub async fn record_adaptive_layer_provisioned(
        &self,
        capability: &AdaptiveDriverCapability,
        attempt: AdaptiveLayerAttempt,
    ) -> Result<(), MobError> {
        self.apply_machine_input_effects(mob_dsl::MobMachineInput::RecordLayerProvisioned {
            adaptive_run_id: mob_dsl::AdaptiveRunId(capability.adaptive_run_id.clone()),
            layer_id: mob_dsl::AdaptiveLayerId(attempt.layer_id),
            attempt: attempt.attempt,
        })
        .await
        .map(|_| ())
    }

    pub async fn record_adaptive_layer_run_started(
        &self,
        capability: &AdaptiveDriverCapability,
        start: AdaptiveLayerRunStart,
    ) -> Result<(), MobError> {
        self.apply_machine_input_effects(mob_dsl::MobMachineInput::RecordLayerRunStarted {
            adaptive_run_id: mob_dsl::AdaptiveRunId(capability.adaptive_run_id.clone()),
            layer_id: mob_dsl::AdaptiveLayerId(start.layer_id),
            attempt: start.attempt,
            child_run_id: mob_dsl::RunId::from(start.child_run_id.to_string()),
        })
        .await
        .map(|_| ())
    }

    pub async fn ingest_adaptive_layer_terminal(
        &self,
        capability: &AdaptiveDriverCapability,
        attempt: AdaptiveLayerAttempt,
        child_run: &MobRun,
    ) -> Result<(), MobError> {
        if !crate::run::mob_machine_run_status_is_terminal(&child_run.run_id, &child_run.status)? {
            return Err(MobError::Internal(format!(
                "cannot ingest non-terminal adaptive layer run '{}'",
                child_run.run_id
            )));
        }
        let result_class = match crate::run::mob_machine_run_public_result_class(
            &child_run.run_id,
            &child_run.status,
        )? {
            crate::run::MobFlowRunPublicResultClass::Success => {
                mob_dsl::FlowRunPublicResultClassKind::Success
            }
            crate::run::MobFlowRunPublicResultClass::Error => {
                mob_dsl::FlowRunPublicResultClassKind::Failure
            }
        };
        let store_plan = adaptive_bundle_layer_terminal_store_plan()?;
        let adaptive_bundle::GeneratedRouteTarget::Input(route) = &store_plan.target else {
            return Err(MobError::Internal(format!(
                "adaptive bundle driver selected non-input route '{}'",
                store_plan.route_id()
            )));
        };
        if route.route_id
            != adaptive_bundle::route_layer_terminal_reaches_adaptive_kernel().route_id
            || route.instance_id != adaptive_bundle::producers::control_mob_instance_id()
            || route.variant != adaptive_bundle::inputs::ingest_layer_terminal()
        {
            return Err(MobError::Internal(format!(
                "adaptive bundle driver selected unexpected layer-terminal target '{}'",
                route.route_id
            )));
        }
        self.apply_machine_input_effects(mob_dsl::MobMachineInput::IngestLayerTerminal {
            adaptive_run_id: mob_dsl::AdaptiveRunId(capability.adaptive_run_id.clone()),
            layer_id: mob_dsl::AdaptiveLayerId(attempt.layer_id),
            attempt: attempt.attempt,
            result_class,
            actual_tokens: 0,
            actual_tool_calls: 0,
        })
        .await
        .map(|_| ())
    }

    pub async fn record_adaptive_layer_setup_fault(
        &self,
        capability: &AdaptiveDriverCapability,
        observation: AdaptiveLayerSetupFaultObservation,
    ) -> Result<(), MobError> {
        self.apply_machine_input_effects(mob_dsl::MobMachineInput::RecordLayerSetupFault {
            adaptive_run_id: mob_dsl::AdaptiveRunId(capability.adaptive_run_id.clone()),
            layer_id: mob_dsl::AdaptiveLayerId(observation.layer_id),
            attempt: observation.attempt,
            fault: observation.fault.into(),
            spawned_members: observation.spawned_members,
            requested_members: observation.requested_members,
        })
        .await
        .map(|_| ())
    }

    pub async fn record_adaptive_layer_interrupted(
        &self,
        capability: &AdaptiveDriverCapability,
        attempt: AdaptiveLayerAttempt,
    ) -> Result<(), MobError> {
        self.apply_machine_input_effects(mob_dsl::MobMachineInput::RecordLayerInterrupted {
            adaptive_run_id: mob_dsl::AdaptiveRunId(capability.adaptive_run_id.clone()),
            layer_id: mob_dsl::AdaptiveLayerId(attempt.layer_id),
            attempt: attempt.attempt,
        })
        .await
        .map(|_| ())
    }

    pub async fn record_adaptive_layer_result_validated(
        &self,
        capability: &AdaptiveDriverCapability,
        result: AdaptiveLayerResultDigest,
    ) -> Result<(), MobError> {
        self.apply_machine_input_effects(mob_dsl::MobMachineInput::RecordLayerResultValidated {
            adaptive_run_id: mob_dsl::AdaptiveRunId(capability.adaptive_run_id.clone()),
            layer_id: mob_dsl::AdaptiveLayerId(result.layer_id),
            attempt: result.attempt,
            result_digest: result.result_digest,
        })
        .await
        .map(|_| ())
    }

    pub async fn record_adaptive_layer_result_invalid(
        &self,
        capability: &AdaptiveDriverCapability,
        attempt: AdaptiveLayerAttempt,
    ) -> Result<(), MobError> {
        self.apply_machine_input_effects(mob_dsl::MobMachineInput::RecordLayerResultInvalid {
            adaptive_run_id: mob_dsl::AdaptiveRunId(capability.adaptive_run_id.clone()),
            layer_id: mob_dsl::AdaptiveLayerId(attempt.layer_id),
            attempt: attempt.attempt,
        })
        .await
        .map(|_| ())
    }

    pub async fn record_adaptive_layer_mob_destroyed(
        &self,
        capability: &AdaptiveDriverCapability,
        attempt: AdaptiveLayerAttempt,
    ) -> Result<(), MobError> {
        self.apply_machine_input_effects(mob_dsl::MobMachineInput::RecordLayerMobDestroyed {
            adaptive_run_id: mob_dsl::AdaptiveRunId(capability.adaptive_run_id.clone()),
            layer_id: mob_dsl::AdaptiveLayerId(attempt.layer_id),
            attempt: attempt.attempt,
        })
        .await
        .map(|_| ())
    }

    pub async fn record_adaptive_layer_mob_retained(
        &self,
        capability: &AdaptiveDriverCapability,
        retention: AdaptiveLayerRetention,
    ) -> Result<(), MobError> {
        self.apply_machine_input_effects(mob_dsl::MobMachineInput::RecordLayerMobRetained {
            adaptive_run_id: mob_dsl::AdaptiveRunId(capability.adaptive_run_id.clone()),
            layer_id: mob_dsl::AdaptiveLayerId(retention.layer_id),
            attempt: retention.attempt,
            disposition: retention.disposition.into(),
        })
        .await
        .map(|_| ())
    }

    pub async fn record_adaptive_cleanup_resolved(
        &self,
        capability: &AdaptiveDriverCapability,
    ) -> Result<(), MobError> {
        self.apply_machine_input_effects(mob_dsl::MobMachineInput::RecordCleanupResolved {
            adaptive_run_id: mob_dsl::AdaptiveRunId(capability.adaptive_run_id.clone()),
        })
        .await
        .map(|_| ())
    }

    pub async fn record_adaptive_body_evidence_missing(
        &self,
        capability: &AdaptiveDriverCapability,
        missing_digest: impl Into<String>,
    ) -> Result<(), MobError> {
        self.apply_machine_input_effects(mob_dsl::MobMachineInput::RecordBodyEvidenceMissing {
            adaptive_run_id: mob_dsl::AdaptiveRunId(capability.adaptive_run_id.clone()),
            missing_digest: missing_digest.into(),
        })
        .await
        .map(|_| ())
    }

    pub async fn resolve_adaptive_finish(
        &self,
        capability: &AdaptiveDriverCapability,
        final_result_digest: impl Into<String>,
    ) -> Result<(), MobError> {
        self.apply_machine_input_effects(mob_dsl::MobMachineInput::ResolveAdaptiveFinish {
            adaptive_run_id: mob_dsl::AdaptiveRunId(capability.adaptive_run_id.clone()),
            final_result_digest: final_result_digest.into(),
        })
        .await
        .map(|_| ())
    }

    pub async fn request_adaptive_cancel(
        &self,
        capability: &AdaptiveDriverCapability,
    ) -> Result<(), MobError> {
        self.apply_machine_input_effects(mob_dsl::MobMachineInput::RequestAdaptiveCancel {
            adaptive_run_id: mob_dsl::AdaptiveRunId(capability.adaptive_run_id.clone()),
        })
        .await
        .map(|_| ())
    }

    pub async fn record_adaptive_deadline_observed(
        &self,
        capability: &AdaptiveDriverCapability,
        observed_at_ms: u64,
    ) -> Result<(), MobError> {
        self.apply_machine_input_effects(mob_dsl::MobMachineInput::RecordDeadlineObserved {
            adaptive_run_id: mob_dsl::AdaptiveRunId(capability.adaptive_run_id.clone()),
            observed_at_ms,
        })
        .await
        .map(|_| ())
    }

    /// Resolve the composite spawn-member operator admission verdict.
    ///
    /// The tool surface extracts RAW, atomic observations — manage scope over
    /// the target mob, raw per-profile spawn-scope set membership, and the
    /// per-argument presence of every privileged spawn argument — and feeds
    /// them here WITHOUT pre-composing them. MobMachine, not the tool surface,
    /// owns the privileged-argument SET membership policy (OR-ing the presence
    /// facts) and the `manage_scope_present || profile_scope_contains`
    /// disjunction, composing the Allow/Deny verdict; the surface mirrors the
    /// returned verdict (`SpawnMemberAdmission::Denied` -> `access_denied`).
    /// Fails closed if the machine emits no verdict.
    pub async fn resolve_spawn_member_admission(
        &self,
        observations: SpawnMemberAdmissionObservations,
    ) -> Result<SpawnMemberAdmission, MobError> {
        let SpawnMemberAdmissionObservations {
            manage_scope_present,
            profile_scope_contains,
            resume_bridge_session_present,
            resume_session_present,
            backend_present,
            runtime_mode_present,
            launch_mode_present,
            tool_access_policy_present,
            tooling_present,
            auth_binding_present,
        } = observations;
        let effects = self
            .apply_machine_input_effects(mob_dsl::MobMachineInput::ResolveSpawnMemberAdmission {
                manage_scope_present,
                profile_scope_contains,
                privileged_resume_bridge_session_present: resume_bridge_session_present,
                privileged_resume_session_present: resume_session_present,
                privileged_backend_present: backend_present,
                privileged_runtime_mode_present: runtime_mode_present,
                privileged_launch_mode_present: launch_mode_present,
                privileged_tool_access_policy_present: tool_access_policy_present,
                privileged_tooling_present: tooling_present,
                privileged_auth_binding_present: auth_binding_present,
            })
            .await?;
        let admission = effects
            .into_iter()
            .find_map(|effect| match effect {
                mob_dsl::MobMachineEffect::SpawnMemberAdmissionResolved { admission } => {
                    Some(admission)
                }
                _ => None,
            })
            .ok_or_else(|| {
                MobError::Internal(
                    "MobMachine accepted spawn-member admission observations but emitted no verdict"
                        .into(),
                )
            })?;
        Ok(match admission {
            mob_dsl::MobSpawnMemberAdmissionKind::Allowed => SpawnMemberAdmission::Allowed,
            // Every other kind is a typed denial cause: the scope-policy
            // `Denied` plus the multi-host portability/placement causes the
            // BeginSpawnExec denial ladder emits (§15.4/§18.9). The tool-seam
            // verdict is binary, so all of them mirror to `Denied`.
            _ => SpawnMemberAdmission::Denied,
        })
    }

    /// Resolve the per-mob operator admission verdict for current-mob-scoped
    /// tools.
    ///
    /// The tool surface extracts a single raw observation — whether the
    /// operator holds manage scope over the current mob (a machine-owned
    /// operator-scope projection) — and feeds it here. MobMachine, not the
    /// tool surface, decides the Allow/Deny verdict; the surface mirrors the
    /// returned verdict (`CurrentMobAdmission::Denied` -> `access_denied`).
    /// Fails closed if the machine emits no verdict.
    pub async fn resolve_current_mob_admission(
        &self,
        can_manage_mob: bool,
    ) -> Result<CurrentMobAdmission, MobError> {
        let effects = self
            .apply_machine_input_effects(mob_dsl::MobMachineInput::ResolveCurrentMobAdmission {
                can_manage_mob,
            })
            .await?;
        let admission = effects
            .into_iter()
            .find_map(|effect| match effect {
                mob_dsl::MobMachineEffect::CurrentMobAdmissionResolved { admission } => {
                    Some(admission)
                }
                _ => None,
            })
            .ok_or_else(|| {
                MobError::Internal(
                    "MobMachine accepted current-mob admission observation but emitted no verdict"
                        .into(),
                )
            })?;
        Ok(match admission {
            mob_dsl::MobCurrentMobAdmissionKind::Allowed => CurrentMobAdmission::Allowed,
            mob_dsl::MobCurrentMobAdmissionKind::Denied => CurrentMobAdmission::Denied,
        })
    }

    /// Resolve the coarse spawn-tool admission verdict for the spawn-member
    /// tool surfaces (`spawn_member` / `spawn_many_members`).
    ///
    /// The tool surface extracts TWO raw, atomic observations — whether the
    /// operator can manage the current mob (`can_manage_mob`) and whether the
    /// operator's spawn-profile scope for the mob is non-empty
    /// (`spawn_profile_scope_present`, a machine-owned operator-scope
    /// set-non-empty projection) — and feeds BOTH here WITHOUT pre-composing
    /// them. MobMachine, not the tool surface, composes the disjunction and
    /// decides the Allow/Deny verdict; the surface mirrors the returned verdict
    /// (`SpawnToolAdmission::Denied` -> `access_denied`). This coarse gate
    /// uniquely covers the empty-specs `spawn_many_members` case (where zero
    /// per-member iterations fire no per-member admission), so it must be
    /// machine-routed rather than reduced in the shell. Fails closed if the
    /// machine emits no verdict.
    pub async fn resolve_spawn_tool_admission(
        &self,
        can_manage_mob: bool,
        spawn_profile_scope_present: bool,
    ) -> Result<SpawnToolAdmission, MobError> {
        let effects = self
            .apply_machine_input_effects(mob_dsl::MobMachineInput::ResolveSpawnToolAdmission {
                can_manage_mob,
                spawn_profile_scope_present,
            })
            .await?;
        let admission = effects
            .into_iter()
            .find_map(|effect| match effect {
                mob_dsl::MobMachineEffect::SpawnToolAdmissionResolved { admission } => {
                    Some(admission)
                }
                _ => None,
            })
            .ok_or_else(|| {
                MobError::Internal(
                    "MobMachine accepted spawn-tool admission observation but emitted no verdict"
                        .into(),
                )
            })?;
        Ok(match admission {
            mob_dsl::MobSpawnToolAdmissionKind::Allowed => SpawnToolAdmission::Allowed,
            mob_dsl::MobSpawnToolAdmissionKind::Denied => SpawnToolAdmission::Denied,
        })
    }

    /// Resolve the member-operator upcall admission verdict (multi-host §15
    /// R6, ADJ-14 wire mapping is the caller's).
    ///
    /// The responder extracts the RAW facts — the requesting identity, the
    /// signed requester generation/fence, ingress-authenticated sender peer
    /// id, and upcall request id — and feeds them here WITHOUT pre-composing.
    /// MobMachine, not the responder, guards the current identity binding,
    /// peer key, placement, and host liveness and composes the verdict; the
    /// responder mirrors it. All arms are pure self-loops callable in every
    /// mob phase. Fails closed if the machine emits no verdict.
    pub(crate) async fn resolve_member_operator_admission(
        &self,
        request: &MemberOperatorAdmissionRequest,
    ) -> Result<MemberOperatorAdmissionVerdict, MobError> {
        let key = &request.request_key;
        let dsl_identity = mob_dsl::AgentIdentity::from(key.agent_identity.as_str());
        let effects = self
            .apply_machine_input_effects(mob_dsl::MobMachineInput::ResolveMemberOperatorAdmission {
                agent_identity: dsl_identity.clone(),
                requester_generation: mob_dsl::Generation(key.generation),
                requester_fence_token: mob_dsl::FenceToken(key.fence_token),
                requester_host_id: mob_dsl::HostId(key.host_id.clone()),
                requester_host_binding_generation: key.host_binding_generation,
                requester_member_session_id: mob_dsl::SessionId(key.member_session_id.clone()),
                sender_peer_id: mob_dsl::PeerId(request.sender_peer_id.as_str()),
                request_id: key.request_id.clone(),
            })
            .await?;
        effects
            .into_iter()
            .find_map(|effect| match effect {
                mob_dsl::MobMachineEffect::MemberOperatorAdmitted {
                    agent_identity: effect_identity,
                    request_id: effect_request_id,
                } if effect_identity == dsl_identity && effect_request_id == key.request_id => {
                    Some(MemberOperatorAdmissionVerdict::Admitted)
                }
                mob_dsl::MobMachineEffect::MemberOperatorRejected {
                    agent_identity: effect_identity,
                    request_id: effect_request_id,
                    cause,
                } if effect_identity == dsl_identity && effect_request_id == key.request_id => {
                    Some(MemberOperatorAdmissionVerdict::Rejected(cause))
                }
                _ => None,
            })
            .ok_or_else(|| {
                MobError::Internal("ResolveMemberOperatorAdmission emitted no verdict".to_string())
            })
    }

    pub(super) async fn subscribe_authorized_agent_session_events(
        &self,
        agent_identity: &AgentIdentity,
        session_id: &SessionId,
    ) -> Result<EventStream, MobError> {
        crate::runtime::session_service::MobSessionService::subscribe_session_events(
            self.session_service.as_ref(),
            session_id,
        )
        .await
        .map_err(|error| {
            MobError::Internal(format!(
                "failed to subscribe to agent events for '{agent_identity}': {error}"
            ))
        })
    }

    pub(super) async fn authorized_mob_event_router_members(
        &self,
        runtime_ids: &BTreeSet<mob_dsl::AgentRuntimeId>,
    ) -> Vec<super::event_router::AuthorizedMobEventRouterMember> {
        let machine_state = self.machine_state_watch_rx.borrow().clone();
        let roster = self.roster.read().await;
        let mut members = Vec::new();
        for (dsl_identity, dsl_runtime_id) in &machine_state.identity_to_runtime {
            if !runtime_ids.contains(dsl_runtime_id) {
                continue;
            }
            // Placement decides the transport lane (ADJ-24): a placed
            // member's session binding names its REMOTE resident session —
            // it fans in through the pump-tap lane
            // (`authority.external_members`), never a local session stream.
            if machine_state.member_placement.contains_key(dsl_identity) {
                continue;
            }
            let Some(dsl_session_id) = machine_state.member_session_bindings.get(dsl_identity)
            else {
                continue;
            };
            let Some(dsl_fence_token) = machine_state.runtime_fence_tokens.get(dsl_runtime_id)
            else {
                continue;
            };
            let Some(runtime_id) = Self::domain_runtime_from_machine_binding(
                dsl_identity,
                dsl_runtime_id,
                &machine_state,
            ) else {
                continue;
            };
            let Ok(session_id) =
                Self::session_id_from_dsl(dsl_session_id, "mob event router bootstrap")
            else {
                continue;
            };
            let agent_identity = AgentIdentity::from(dsl_identity.0.as_str());
            let role = roster
                .entry(&agent_identity)
                .map(|entry| entry.role.clone())
                .unwrap_or_else(|| ProfileName::from(agent_identity.as_str()));
            members.push(super::event_router::AuthorizedMobEventRouterMember {
                agent_identity,
                runtime_id,
                fence_token: FenceToken::new(dsl_fence_token.0),
                session_id,
                role,
            });
        }
        members
    }

    pub(super) async fn authorize_mob_event_router_member_subscription(
        &self,
        agent_identity: &AgentIdentity,
        runtime_id: &AgentRuntimeId,
        fence_token: FenceToken,
        role: ProfileName,
    ) -> Result<super::event_router::AuthorizedMobEventRouterMember, MobError> {
        let dsl_identity = mob_dsl::AgentIdentity(agent_identity.to_string());
        let dsl_runtime_id = mob_dsl::AgentRuntimeId::from_domain(runtime_id);
        let dsl_fence_token = mob_dsl::FenceToken::from_domain(fence_token);
        let effects = self
            .apply_machine_input_effects(
                mob_dsl::MobMachineInput::AuthorizeMobEventRouterMemberSubscription {
                    agent_identity: dsl_identity.clone(),
                    agent_runtime_id: dsl_runtime_id.clone(),
                    fence_token: dsl_fence_token,
                },
            )
            .await?;
        for effect in effects {
            if let mob_dsl::MobMachineEffect::AuthorizeMobEventRouterMemberSubscription {
                agent_identity: effect_identity,
                agent_runtime_id: effect_runtime_id,
                fence_token: effect_fence_token,
                session_id,
            } = effect
                && effect_identity == dsl_identity
                && effect_runtime_id == dsl_runtime_id
                && effect_fence_token == dsl_fence_token
            {
                return Ok(super::event_router::AuthorizedMobEventRouterMember {
                    agent_identity: agent_identity.clone(),
                    runtime_id: runtime_id.clone(),
                    fence_token,
                    session_id: Self::session_id_from_dsl(
                        &session_id,
                        "mob event router member subscription",
                    )?,
                    role,
                });
            }
        }
        Err(MobError::Internal(
            "MobMachine did not emit mob event router member subscription authority".into(),
        ))
    }

    pub(super) async fn authorize_mob_event_router_member_removal(
        &self,
        agent_identity: &AgentIdentity,
    ) -> Result<bool, MobError> {
        let dsl_identity = mob_dsl::AgentIdentity(agent_identity.to_string());
        let effects = self
            .apply_machine_input_effects(
                mob_dsl::MobMachineInput::AuthorizeMobEventRouterMemberRemoval {
                    agent_identity: dsl_identity.clone(),
                },
            )
            .await?;
        Ok(effects.into_iter().any(|effect| {
            matches!(
                effect,
                mob_dsl::MobMachineEffect::AuthorizeMobEventRouterMemberRemoval {
                    agent_identity
                } if agent_identity == dsl_identity
            )
        }))
    }

    pub(super) async fn commit_flow_run_command(
        &self,
        run_id: &RunId,
        command: MobMachineFlowRunCommand,
        context: &'static str,
    ) -> Result<Option<Vec<flow_run::Effect>>, MobError> {
        self.send_actor_command(|reply_tx| MobCommand::CommitFlowRunCommand {
            run_id: run_id.clone(),
            command: Box::new(command),
            context,
            reply_tx,
        })
        .await?
    }

    pub(super) async fn commit_flow_terminalization(
        &self,
        run_id: RunId,
        flow_id: FlowId,
        target: TerminalizationTarget,
        command: MobMachineFlowRunCommand,
        context: &'static str,
    ) -> Result<TerminalizationOutcome, MobError> {
        self.send_actor_command(|reply_tx| MobCommand::CommitFlowTerminalization {
            run_id,
            flow_id,
            target,
            command: Box::new(command),
            context,
            reply_tx,
        })
        .await?
    }

    pub(super) async fn commit_flow_frame_store_plan(
        &self,
        run_id: &RunId,
        plan: FlowFrameLoopStorePlan,
    ) -> Result<bool, MobError> {
        self.send_actor_command(|reply_tx| MobCommand::CommitFlowFrameStorePlan {
            run_id: run_id.clone(),
            plan: Box::new(plan),
            reply_tx,
        })
        .await?
    }

    pub(crate) async fn preview_machine_input(
        &self,
        input: crate::machines::mob_machine::MobMachineInput,
    ) -> Result<crate::machines::mob_machine::MobMachineState, MobError> {
        self.send_actor_command(|reply_tx| MobCommand::PreviewMachineInput {
            input: Box::new(input),
            reply_tx,
        })
        .await?
    }

    pub(crate) async fn query_machine_state(
        &self,
    ) -> Result<crate::machines::mob_machine::MobMachineState, MobError> {
        match self
            .send_actor_command(|reply_tx| MobCommand::QueryMachineState { reply_tx })
            .await
        {
            Ok(state) => Ok(state),
            // Destroy is terminal: once the actor commits the `Destroy`
            // transition it exits its command loop and the command channel
            // closes. The actor publishes the final (Destroyed) machine state to
            // the watch channel before exiting, so a post-destroy query reads the
            // durable final projection from the watch instead of failing on the
            // closed command channel. Live callers (flows) always reach the actor
            // because the command channel is open while the actor runs.
            Err(MobError::ActorCommandChannelClosed | MobError::ActorReplyChannelClosed) => {
                Ok(self.machine_state_watch_rx.borrow().clone())
            }
            Err(error) => Err(error),
        }
    }

    /// Return whether `bridge_session_id` is the machine-owned owner session
    /// for this mob. This intentionally exposes the narrow ownership query,
    /// not the generated machine state itself, across crate boundaries.
    pub async fn owns_bridge_session(
        &self,
        bridge_session_id: &meerkat_core::types::SessionId,
    ) -> Result<bool, MobError> {
        Ok(self
            .query_machine_state()
            .await?
            .owner_bridge_session_id
            .as_ref()
            .is_some_and(|session_id| session_id.0 == bridge_session_id.to_string()))
    }

    #[cfg(test)]
    pub(crate) async fn authorize_member_trust_cleanup_for_test(
        &self,
        edge: crate::machines::mob_machine::WiringEdge,
    ) -> Result<
        crate::generated::protocol_mob_member_trust_unwiring::MobMemberTrustUnwiringObligation,
        MobError,
    > {
        self.send_actor_command(|reply_tx| MobCommand::AuthorizeMemberTrustCleanupForTest {
            edge,
            reply_tx,
        })
        .await?
    }
}

impl MemberHandle {
    /// Target member identity.
    pub fn identity(&self) -> AgentIdentity {
        AgentIdentity::from(self.agent_identity.as_str())
    }

    /// Install (or clear) the host-owned outbound content-taint declaration
    /// for this member. See [`MobHandle::declare_member_outbound_taint`].
    pub async fn declare_outbound_taint(
        &self,
        taint: Option<meerkat_core::comms::SenderContentTaint>,
    ) -> Result<(), MobError> {
        self.mob
            .declare_member_outbound_taint(self.identity(), taint)
            .await
    }

    /// Submit external work to this member through the canonical runtime path.
    pub async fn send(
        &self,
        content: impl Into<meerkat_core::types::ContentInput>,
        handling_mode: HandlingMode,
    ) -> Result<MemberDeliveryReceipt, MobError> {
        self.send_with_internal_turn_metadata(content, handling_mode, None, None, None)
            .await
    }

    /// Submit external work with explicit normalized render metadata.
    pub async fn send_with_render_metadata(
        &self,
        content: impl Into<meerkat_core::types::ContentInput>,
        handling_mode: HandlingMode,
        render_metadata: Option<RenderMetadata>,
    ) -> Result<MemberDeliveryReceipt, MobError> {
        let turn_metadata = render_metadata.map(|render_metadata| RuntimeTurnMetadata {
            render_metadata: Some(render_metadata),
            ..Default::default()
        });
        self.send_with_internal_turn_metadata(content, handling_mode, turn_metadata, None, None)
            .await
    }

    /// Start an external member turn with host-owned runtime options and an
    /// optional live event stream.
    ///
    /// The mob actor remains the admission authority: member generation,
    /// fence, session binding, transcript identity, and runtime dispatch are
    /// resolved through the same canonical SubmitWork path as [`Self::send`].
    /// Nonterminal events stream as they happen; terminal events and
    /// [`MemberTurnHandle::wait`] remain gated on committed runtime completion.
    pub async fn start_turn(
        &self,
        content: impl Into<meerkat_core::types::ContentInput>,
        handling_mode: HandlingMode,
        options: MemberTurnOptions,
        event_tx: Option<MemberTurnEventSender>,
    ) -> Result<MemberTurnHandle, MobError> {
        let turn_metadata = options.into_runtime_metadata(handling_mode);
        let (completion_tx, completion_rx) = tokio::sync::oneshot::channel();
        let (llm_identity_applied_tx, llm_identity_applied_rx) = tokio::sync::oneshot::channel();
        let (agent_runtime_id, fence_token, session_id) = self
            .mob
            .external_turn_for_member(
                self.agent_identity.clone(),
                content.into(),
                handling_mode,
                Some(turn_metadata),
                MemberTurnObservers {
                    event_tx,
                    completion_tx: Some(completion_tx),
                    llm_identity_applied_tx: Some(llm_identity_applied_tx),
                },
            )
            .await?;
        Ok(MemberTurnHandle {
            receipt: MemberDeliveryReceipt {
                identity: self.identity(),
                agent_runtime_id,
                fence_token,
                handling_mode,
            },
            session_id,
            completion_rx,
            llm_identity_applied_rx: Some(llm_identity_applied_rx),
        })
    }

    async fn send_with_internal_turn_metadata(
        &self,
        content: impl Into<meerkat_core::types::ContentInput>,
        handling_mode: HandlingMode,
        turn_metadata: Option<RuntimeTurnMetadata>,
        event_tx: Option<MemberTurnEventSender>,
        completion_tx: Option<tokio::sync::oneshot::Sender<Result<(), MobError>>>,
    ) -> Result<MemberDeliveryReceipt, MobError> {
        let (agent_runtime_id, fence_token, _session_id) = self
            .mob
            .external_turn_for_member(
                self.agent_identity.clone(),
                content.into(),
                handling_mode,
                turn_metadata,
                MemberTurnObservers {
                    event_tx,
                    completion_tx,
                    llm_identity_applied_tx: None,
                },
            )
            .await?;
        Ok(MemberDeliveryReceipt {
            identity: self.identity(),
            agent_runtime_id,
            fence_token,
            handling_mode,
        })
    }

    /// Send typed peer communication from this member to another mob member.
    ///
    /// Unlike [`Self::send`], this preserves sender/recipient attribution by
    /// routing through this member's comms runtime.
    pub async fn send_peer_message(
        &self,
        to: AgentIdentity,
        content: impl Into<meerkat_core::types::ContentInput>,
        handling_mode: HandlingMode,
    ) -> Result<PeerMessageReceipt, MobError> {
        self.mob
            .send_peer_message(self.identity(), to, content, handling_mode)
            .await
    }

    /// Submit internal work to this member without external addressability checks.
    ///
    /// Three operations that sometimes get mistaken for the same thing are
    /// actually three distinct slices of "deliver content to a member".
    /// [`MemberHandle::internal_turn`] (this) is Rust in-process direct write
    /// into the member's pending turn slot — no peer comms, no handling-mode
    /// selection. `mob/turn_start` (RPC) resolves the identity to the bridge
    /// session and delegates to the canonical `turn/start` handler with
    /// turn-level overrides. `mob/member_send` (RPC) is peer-delivery shape
    /// over comms with `HandlingMode` + `RenderMetadata`; it lands in the
    /// member's comms inbox, not as a new turn. The three surfaces share a
    /// name fragment but diverge on who authorizes the delivery, what the
    /// member's runtime does with it, and what the caller gets back. Keep
    /// them separate — collapsing them would erase real policy distinctions.
    pub async fn internal_turn(
        &self,
        content: impl Into<meerkat_core::types::ContentInput>,
    ) -> Result<MemberDeliveryReceipt, MobError> {
        let (agent_runtime_id, fence_token) = self
            .mob
            .internal_turn_for_member(self.agent_identity.clone(), content.into())
            .await?;
        Ok(MemberDeliveryReceipt {
            identity: self.identity(),
            agent_runtime_id,
            fence_token,
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

    /// Pre-addressed conclusion affordance for this member's kickoff objective.
    pub async fn conclude_objective(
        &self,
        objective_id: meerkat_core::interaction::ObjectiveId,
        outcome: impl Into<String>,
    ) -> Result<(), MobError> {
        self.mob
            .conclude_objective(&self.identity(), objective_id, outcome)
            .await
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

    // Dogma #282 gate: a command-layer fault must surface as `Err(MobError)`,
    // never be laundered into a "not found" (`Ok(None)`) or "empty match"
    // (`Ok(vec![])`) result. We drive each public method's result mapping with
    // a deliberately-wrong command result variant (the same shape a genuine
    // query/transport fault would produce) and assert the typed error path.
    #[test]
    fn get_member_unexpected_command_result_surfaces_error_not_absent() {
        // True absence is still distinguishable as `Ok(None)`.
        let absent =
            MobHandle::project_get_member_result_for_test(MobMachineCommandResult::GetMember(None))
                .expect("a `GetMember(None)` result is a genuine absent member, not a fault");
        assert!(
            absent.is_none(),
            "true absence must remain `Ok(None)`, not be reported as a fault"
        );

        // A command-layer fault (wrong result variant) must NOT collapse to
        // `Ok(None)`; it must surface as `Err(MobError)`.
        let faulted = MobHandle::project_get_member_result_for_test(MobMachineCommandResult::Unit);
        assert!(
            matches!(faulted, Err(MobError::Internal(_))),
            "a command-layer fault must surface as Err(MobError), never Ok(None): {faulted:?}"
        );
    }

    #[test]
    fn list_members_matching_unexpected_command_result_surfaces_error_not_empty() {
        // A genuinely empty match is still distinguishable as `Ok(vec![])`.
        let empty = MobHandle::project_list_members_matching_result_for_test(
            MobMachineCommandResult::ListMembers(Vec::new()),
        )
        .expect("an empty `ListMembers` result is a genuine empty match, not a fault");
        assert!(
            empty.is_empty(),
            "a genuine empty match must remain `Ok(vec![])`, not be reported as a fault"
        );

        // A command-layer fault (wrong result variant) must NOT collapse to an
        // empty `Vec`; it must surface as `Err(MobError)`.
        let faulted =
            MobHandle::project_list_members_matching_result_for_test(MobMachineCommandResult::Unit);
        assert!(
            matches!(faulted, Err(MobError::Internal(_))),
            "a command-layer fault must surface as Err(MobError), never Ok(empty): {faulted:?}"
        );
    }

    #[test]
    fn member_projection_types_omit_bridge_session_fields_in_serialized_output() {
        let sid = SessionId::new();

        let snapshot = MobMemberSnapshot {
            status: MobMemberStatus::Active,
            agent_identity: AgentIdentity::from("worker"),
            agent_runtime_id: Some(AgentRuntimeId::initial(AgentIdentity::from("worker"))),
            fence_token: Some(FenceToken::new(0)),
            output_preview: None,
            error: None,
            tokens_used: 0,
            is_final: false,
            current_session_id: None,
            current_bridge_session_id: None,
            peer_connectivity: None,
            kickoff: None,
            external_member: None,
            resolved_capabilities: None,
            progress: None,
            placement: None,
            control_reachability: None,
            comms_reachability: None,
            last_seen_ms: None,
            freshness_reason: None,
            lifecycle_capabilities: None,
            non_portable_disabled: None,
        }
        .with_current_bridge_session_id(Some(sid.clone()));
        let snapshot_value =
            serde_json::to_value(&snapshot).expect("snapshot should serialize to json");
        // 0.6 clean break: session fields are #[serde(skip)] and must not appear
        assert!(snapshot_value.get("current_bridge_session_id").is_none());
        // `agent_runtime_id` and `fence_token` are binding-era atoms marked
        // `pub(crate)` + `#[serde(skip)]` per the struct definition — they
        // are bridge-internal and must NOT leak into app-facing serialized
        // payloads. The public identity contract is `agent_identity()`
        assert!(snapshot_value.get("agent_runtime_id").is_none());
        assert!(snapshot_value.get("fence_token").is_none());
    }

    #[test]
    fn mob_member_snapshot_exposes_runtime_identity_only_by_accessor() {
        let runtime_id = AgentRuntimeId::new(AgentIdentity::from("worker"), Generation::new(3));
        let snapshot = MobMemberSnapshot {
            status: MobMemberStatus::Active,
            agent_identity: AgentIdentity::from("worker"),
            agent_runtime_id: Some(runtime_id.clone()),
            fence_token: Some(FenceToken::new(9)),
            output_preview: None,
            error: None,
            tokens_used: 0,
            is_final: false,
            current_session_id: None,
            current_bridge_session_id: None,
            peer_connectivity: None,
            kickoff: None,
            external_member: None,
            resolved_capabilities: None,
            progress: None,
            placement: None,
            control_reachability: None,
            comms_reachability: None,
            last_seen_ms: None,
            freshness_reason: None,
            lifecycle_capabilities: None,
            non_portable_disabled: None,
        };

        let snapshot_value =
            serde_json::to_value(&snapshot).expect("snapshot should serialize to json");
        assert!(snapshot_value.get("agent_runtime_id").is_none());
        assert!(snapshot_value.get("fence_token").is_none());

        let (projected_runtime_id, projected_fence_token) = snapshot
            .runtime_identity_fields()
            .expect("runtime identity fields should be present");
        assert_eq!(projected_runtime_id, &runtime_id);
        assert_eq!(projected_fence_token, FenceToken::new(9));
    }

    #[test]
    fn mob_member_snapshot_exposes_agent_identity_convenience() {
        // Regression for DELETE_ME C9: every consumer used to reach through
        // `snapshot.agent_runtime_id.identity`; the snapshot now exposes a
        // direct accessor even when runtime material is absent.
        let snapshot = MobMemberSnapshot {
            status: MobMemberStatus::Active,
            agent_identity: AgentIdentity::from("singer"),
            agent_runtime_id: None,
            fence_token: None,
            output_preview: None,
            error: None,
            tokens_used: 0,
            is_final: false,
            current_session_id: None,
            current_bridge_session_id: None,
            peer_connectivity: None,
            kickoff: None,
            external_member: None,
            resolved_capabilities: None,
            progress: None,
            placement: None,
            control_reachability: None,
            comms_reachability: None,
            last_seen_ms: None,
            freshness_reason: None,
            lifecycle_capabilities: None,
            non_portable_disabled: None,
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
            is_terminal: false,
            error: None,
            output_preview: None,
            tokens_used: 0,
            agent_identity: AgentIdentity::from("worker"),
            agent_runtime_id: Some(AgentRuntimeId::initial(AgentIdentity::from("worker"))),
            fence_token: Some(FenceToken::new(0)),
            current_bridge_session_id: Some(sid.clone()),
            peer_connectivity: None,
            kickoff: None,
            progress: None,
        }
        .to_snapshot();

        assert_eq!(snapshot.current_bridge_session_id(), Some(&sid));
        assert_eq!(snapshot.current_bridge_session_id, Some(sid));
    }

    #[test]
    fn member_list_projection_is_seeded_by_machine_identity() {
        let mut authority = mob_dsl::MobMachineAuthority::new();
        let identity = AgentIdentity::from("worker");
        let dsl_identity = mob_dsl::AgentIdentity::from_domain(&identity);
        let runtime_id = AgentRuntimeId::initial(identity.clone());
        let profile_digest = "worker-profile-digest".to_string();

        mob_dsl::MobMachineMutator::apply(
            &mut authority,
            mob_dsl::MobMachineInput::AuthorizeSpawnProfile {
                agent_identity: dsl_identity.clone(),
                profile_name: "worker-profile".to_string(),
                model: "test-model".to_string(),
                profile_material_digest: profile_digest.clone(),
                tool_config_digest: "tools".to_string(),
                skills_digest: "skills".to_string(),
                provider_params_digest: None,
                output_schema_digest: None,
                external_addressable: false,
                resolved_spec_digest: None,
            },
        )
        .expect("profile authority should admit");
        mob_dsl::MobMachineMutator::apply(
            &mut authority,
            mob_dsl::MobMachineInput::BeginSpawnExec {
                agent_identity: dsl_identity.clone(),
                agent_runtime_id: mob_dsl::AgentRuntimeId::from_domain(&runtime_id),
                fence_token: mob_dsl::FenceToken::from_domain(FenceToken::new(7)),
                generation: mob_dsl::Generation::from_domain(runtime_id.generation),
                profile_material_digest: profile_digest.clone(),
                external_addressable: false,
                runtime_mode: mob_dsl::SpawnPolicyRuntimeMode::TurnDriven,
                bridge_session_id: None,
                replacing: None,
                placement: None,
                workgraph_required: false,
                rust_bundles_present: false,
                per_spawn_external_tools_present: false,
                mob_default_external_tools_present: false,
                default_llm_client_override_present: false,
                host_surface_mcp_allowlist_present: false,
                inherited_tool_filter_present: false,
                shell_env_present: false,
                mcp_stdio_env_present: false,
                mcp_http_headers_present: false,
                memory_required: false,
                mcp_required: false,
                resume_session_id: None,
                placed_spawn_id: None,
                placed_provision_operation_id: None,
                placed_operation_owner_session_id: None,
                effective_profile_override_present: false,
                effective_model_override_present: false,
            },
        )
        .expect("begin spawn exec should admit");
        mob_dsl::MobMachineMutator::apply(
            &mut authority,
            mob_dsl::MobMachineInput::CommitSpawnMembership {
                agent_identity: dsl_identity.clone(),
                agent_runtime_id: mob_dsl::AgentRuntimeId::from_domain(&runtime_id),
                fence_token: mob_dsl::FenceToken::from_domain(FenceToken::new(7)),
                generation: mob_dsl::Generation::from_domain(runtime_id.generation),
                profile_material_digest: profile_digest,
                external_addressable: false,
                runtime_mode: mob_dsl::SpawnPolicyRuntimeMode::TurnDriven,
                bridge_session_id: None,
                replacing: None,
                member_peer_endpoint: None,
                spec_digest_echo: None,
                ack_engine_version: None,
                placed_spawn_id: None,
                provision_operation_id: None,
            },
        )
        .expect("commit spawn membership should admit");

        let projected = MobHandle::project_member_list_entry_from_machine_identity(
            &dsl_identity,
            None,
            authority.state(),
        )
        .expect("machine-owned identity should project without a roster row");

        assert_eq!(projected.agent_identity, identity);
        assert_eq!(projected.role, ProfileName::from("worker-profile"));
        assert_eq!(projected.runtime_mode, MobRuntimeMode::TurnDriven);
        assert_eq!(projected.status, MobMemberStatus::Active);
        assert!(projected.labels.is_empty());
    }

    /// Gate for dogma row #314: the external-member observation's
    /// `bridge_session_present` (which drives both `binding_mode` and the
    /// `NotRequired` rebind branch) must be read from the machine-owned
    /// `member_session_bindings` fact, not recomputed from the roster
    /// `MemberRef.session_id` compatibility mirror.
    ///
    /// `project_external_member_observation` now consults
    /// `machine_bridge_session_id_for_identity`, so this locks that the helper
    /// reports binding presence from machine authority: present when the
    /// machine bound a bridge session at spawn, absent (peer-only) otherwise.
    #[test]
    fn external_member_bridge_presence_reads_machine_session_binding_fact() {
        fn spawn_member(
            authority: &mut mob_dsl::MobMachineAuthority,
            identity: &AgentIdentity,
            bridge_session_id: Option<mob_dsl::SessionId>,
        ) {
            let dsl_identity = mob_dsl::AgentIdentity::from_domain(identity);
            let runtime_id = AgentRuntimeId::initial(identity.clone());
            let profile_digest = format!("{}-profile-digest", identity.as_str());
            mob_dsl::MobMachineMutator::apply(
                authority,
                mob_dsl::MobMachineInput::AuthorizeSpawnProfile {
                    agent_identity: dsl_identity.clone(),
                    profile_name: "worker-profile".to_string(),
                    model: "test-model".to_string(),
                    profile_material_digest: profile_digest.clone(),
                    tool_config_digest: "tools".to_string(),
                    skills_digest: "skills".to_string(),
                    provider_params_digest: None,
                    output_schema_digest: None,
                    external_addressable: true,
                    resolved_spec_digest: None,
                },
            )
            .expect("profile authority should admit");
            mob_dsl::MobMachineMutator::apply(
                authority,
                mob_dsl::MobMachineInput::BeginSpawnExec {
                    agent_identity: dsl_identity.clone(),
                    agent_runtime_id: mob_dsl::AgentRuntimeId::from_domain(&runtime_id),
                    fence_token: mob_dsl::FenceToken::from_domain(FenceToken::new(0)),
                    generation: mob_dsl::Generation::from_domain(runtime_id.generation),
                    profile_material_digest: profile_digest.clone(),
                    external_addressable: true,
                    runtime_mode: mob_dsl::SpawnPolicyRuntimeMode::TurnDriven,
                    bridge_session_id: bridge_session_id.clone(),
                    replacing: None,
                    placement: None,
                    workgraph_required: false,
                    rust_bundles_present: false,
                    per_spawn_external_tools_present: false,
                    mob_default_external_tools_present: false,
                    default_llm_client_override_present: false,
                    host_surface_mcp_allowlist_present: false,
                    inherited_tool_filter_present: false,
                    shell_env_present: false,
                    mcp_stdio_env_present: false,
                    mcp_http_headers_present: false,
                    memory_required: false,
                    mcp_required: false,
                    resume_session_id: None,
                    placed_spawn_id: None,
                    placed_provision_operation_id: None,
                    placed_operation_owner_session_id: None,
                    effective_profile_override_present: false,
                    effective_model_override_present: false,
                },
            )
            .expect("begin spawn exec should admit");
            mob_dsl::MobMachineMutator::apply(
                authority,
                mob_dsl::MobMachineInput::CommitSpawnMembership {
                    agent_identity: dsl_identity,
                    agent_runtime_id: mob_dsl::AgentRuntimeId::from_domain(&runtime_id),
                    fence_token: mob_dsl::FenceToken::from_domain(FenceToken::new(0)),
                    generation: mob_dsl::Generation::from_domain(runtime_id.generation),
                    profile_material_digest: profile_digest,
                    external_addressable: true,
                    runtime_mode: mob_dsl::SpawnPolicyRuntimeMode::TurnDriven,
                    bridge_session_id,
                    replacing: None,
                    member_peer_endpoint: None,
                    spec_digest_echo: None,
                    ack_engine_version: None,
                    placed_spawn_id: None,
                    provision_operation_id: None,
                },
            )
            .expect("commit spawn membership should admit");
        }

        let bridge_backed = AgentIdentity::from("w-bridge");
        let peer_only = AgentIdentity::from("w-peer");
        let bridge_session = SessionId::new();

        let mut authority = mob_dsl::MobMachineAuthority::new();
        spawn_member(
            &mut authority,
            &bridge_backed,
            Some(mob_dsl::SessionId::from_domain(&bridge_session)),
        );
        spawn_member(&mut authority, &peer_only, None);

        let state = authority.state();

        // Bridge-session-backed member: the machine fact owns the binding, so
        // presence is reported as the bound session (drives `NotRequired`).
        assert_eq!(
            MobHandle::machine_bridge_session_id_for_identity(&bridge_backed, state),
            Some(bridge_session),
            "bridge-session-backed member must read its binding from MobMachineState.member_session_bindings",
        );

        // Peer-only member: no machine session binding, so the observation must
        // treat the bridge session as absent regardless of any roster mirror.
        assert_eq!(
            MobHandle::machine_bridge_session_id_for_identity(&peer_only, state),
            None,
            "peer-only member must report no bridge session from machine authority",
        );
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
        // Public contract: `identity` is the only identity field that
        // surfaces in app-facing serialized output. The binding-era atoms
        // (`agent_runtime_id`, `previous_fence_token`, `fence_token`) are
        // `pub(crate)` + `#[serde(skip)]` on the struct definition — they
        // are bridge-internal and must not leak.
        assert_eq!(receipt_value["identity"], "worker");
        assert!(receipt_value.get("agent_runtime_id").is_none());
        assert!(receipt_value.get("previous_fence_token").is_none());
        assert!(receipt_value.get("fence_token").is_none());

        let delivery = MemberDeliveryReceipt {
            identity: AgentIdentity::from("worker"),
            agent_runtime_id: runtime_id,
            fence_token: FenceToken::new(8),
            handling_mode: HandlingMode::Queue,
        };
        let delivery_value =
            serde_json::to_value(&delivery).expect("delivery receipt should serialize to json");
        assert_eq!(delivery_value["identity"], "worker");
        assert!(delivery_value.get("agent_runtime_id").is_none());
        assert!(delivery_value.get("fence_token").is_none());
    }

    #[test]
    fn helper_result_omits_binding_era_atoms_in_serialized_output() {
        let runtime_id = AgentRuntimeId::new(AgentIdentity::from("worker"), Generation::new(2));
        let result = HelperResult {
            output: Some("done".to_string()),
            tokens_used: 7,
            agent_identity: AgentIdentity::from("worker"),
            agent_runtime_id: runtime_id.clone(),
            fence_token: FenceToken::new(9),
        };

        let value = serde_json::to_value(&result).expect("helper result should serialize to json");
        // Public contract: `agent_identity`, `output`, `tokens_used` surface
        // in app-facing output. The binding-era atoms (`agent_runtime_id`,
        // `fence_token`) are `pub(crate)` + `#[serde(skip)]` per the struct
        // definition — bridge-internal and must not leak. Session fields
        // were never present.
        assert_eq!(value["agent_identity"], "worker");
        assert_eq!(value["tokens_used"], 7);
        assert!(value.get("agent_runtime_id").is_none());
        assert!(value.get("fence_token").is_none());
        assert!(value.get("session_id").is_none());
        assert!(value.get("bridge_session_id").is_none());
    }

    #[test]
    fn status_watch_fallback_rejects_live_phase_after_actor_drop() {
        let err = MobHandle::status_from_closed_actor(
            MobError::ActorCommandChannelClosed,
            MobState::Running,
        )
        .expect_err("live phase watch must not become lifecycle truth after actor loss");
        assert!(
            err.to_string().contains("not terminal lifecycle authority"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn status_watch_fallback_accepts_actor_published_terminal_phase() {
        let status = MobHandle::status_from_closed_actor(
            MobError::ActorCommandChannelClosed,
            MobState::Destroyed,
        )
        .expect("destroyed phase watch is an actor-published terminal fallback");
        assert_eq!(status, MobState::Destroyed);
    }

    #[test]
    fn spawn_member_spec_resume_bridge_session_accessors_stay_additive() {
        let sid = SessionId::new();
        let spec =
            SpawnMemberSpec::new("worker", "worker-1").with_resume_bridge_session_id(sid.clone());

        assert_eq!(spec.launch_mode.resume_bridge_session_id(), Some(&sid));
        assert_eq!(spec.launch_mode.resume_bridge_session_id(), Some(&sid));
    }

    #[test]
    fn spawn_source_launch_mode_classification_is_surface_independent() {
        let sid = SessionId::new();
        let resume = crate::launch::MemberLaunchMode::Resume {
            bridge_session_id: sid,
        };
        let fork = crate::launch::MemberLaunchMode::Fork {
            source_member_id: AgentIdentity::from("lead-1"),
            fork_context: crate::launch::ForkContext::LastMessages { count: 1 },
        };

        assert_eq!(
            SpawnSource::for_launch_mode(SpawnSource::Consumer, &resume),
            SpawnSource::Resume
        );
        assert_eq!(
            SpawnSource::for_launch_mode(SpawnSource::AgentSpawnMember, &resume),
            SpawnSource::Resume
        );
        assert_eq!(
            SpawnSource::for_launch_mode(SpawnSource::BatchItem, &fork),
            SpawnSource::Fork
        );
        assert_eq!(
            SpawnSource::for_launch_mode(
                SpawnSource::AgentSpawnMember,
                &crate::launch::MemberLaunchMode::Fresh,
            ),
            SpawnSource::AgentSpawnMember
        );
    }

    fn ready_wait_test_entry(
        identity: &AgentIdentity,
        runtime_mode: MobRuntimeMode,
    ) -> RosterEntry {
        let agent_runtime_id = AgentRuntimeId::initial(identity.clone());
        RosterEntry {
            agent_identity: identity.clone(),
            generation: Generation::INITIAL,
            fence_token: FenceToken::new(0),
            agent_runtime_id,
            role: ProfileName::from("worker"),
            runtime_mode,
            wired_to: BTreeSet::new(),
            labels: BTreeMap::new(),
            kickoff: None,
            member_ref: MemberRef::from_bridge_session_id(SessionId::new()),
            peer_id: None,
            transport_public_key: None,
            external_peer_specs: BTreeMap::new(),
            effective_profile_override: None,
            effective_model_override: None,
        }
    }

    fn seed_ready_wait_machine(
        identity: &AgentIdentity,
        runtime_mode: mob_dsl::SpawnPolicyRuntimeMode,
    ) -> mob_dsl::MobMachineAuthority {
        let mut authority = mob_dsl::MobMachineAuthority::new();
        let dsl_identity = mob_dsl::AgentIdentity::from_domain(identity);
        let runtime_id = AgentRuntimeId::initial(identity.clone());
        let dsl_runtime_id = mob_dsl::AgentRuntimeId::from_domain(&runtime_id);
        let fence_token = mob_dsl::FenceToken::from_domain(FenceToken::new(0));
        let generation = mob_dsl::Generation::from_domain(runtime_id.generation);
        let profile_digest = "ready-wait-profile-digest".to_string();

        mob_dsl::MobMachineMutator::apply(
            &mut authority,
            mob_dsl::MobMachineInput::AuthorizeSpawnProfile {
                agent_identity: dsl_identity.clone(),
                profile_name: "worker-profile".to_string(),
                model: "test-model".to_string(),
                profile_material_digest: profile_digest.clone(),
                tool_config_digest: "tools".to_string(),
                skills_digest: "skills".to_string(),
                provider_params_digest: None,
                output_schema_digest: None,
                external_addressable: false,
                resolved_spec_digest: None,
            },
        )
        .expect("profile authority should admit");
        mob_dsl::MobMachineMutator::apply(
            &mut authority,
            mob_dsl::MobMachineInput::BeginSpawnExec {
                agent_identity: dsl_identity.clone(),
                agent_runtime_id: dsl_runtime_id.clone(),
                fence_token,
                generation,
                profile_material_digest: profile_digest.clone(),
                external_addressable: false,
                runtime_mode,
                bridge_session_id: Some(mob_dsl::SessionId("ready-session".to_string())),
                replacing: None,
                placement: None,
                workgraph_required: false,
                rust_bundles_present: false,
                per_spawn_external_tools_present: false,
                mob_default_external_tools_present: false,
                default_llm_client_override_present: false,
                host_surface_mcp_allowlist_present: false,
                inherited_tool_filter_present: false,
                shell_env_present: false,
                mcp_stdio_env_present: false,
                mcp_http_headers_present: false,
                memory_required: false,
                mcp_required: false,
                resume_session_id: None,
                placed_spawn_id: None,
                placed_provision_operation_id: None,
                placed_operation_owner_session_id: None,
                effective_profile_override_present: false,
                effective_model_override_present: false,
            },
        )
        .expect("begin spawn exec should admit");
        mob_dsl::MobMachineMutator::apply(
            &mut authority,
            mob_dsl::MobMachineInput::CommitSpawnMembership {
                agent_identity: dsl_identity,
                agent_runtime_id: dsl_runtime_id,
                fence_token,
                generation,
                profile_material_digest: profile_digest,
                external_addressable: false,
                runtime_mode,
                bridge_session_id: Some(mob_dsl::SessionId("ready-session".to_string())),
                replacing: None,
                member_peer_endpoint: None,
                spec_digest_echo: None,
                ack_engine_version: None,
                placed_spawn_id: None,
                provision_operation_id: None,
            },
        )
        .expect("commit spawn membership should admit");

        authority
    }

    #[test]
    fn default_ready_wait_timeout_is_bounded_for_agent_tool_callers() {
        assert_eq!(DEFAULT_READY_WAIT_TIMEOUT, Duration::from_secs(60));
    }

    #[test]
    fn ready_wait_requires_startup_ready_for_active_autonomous_members() {
        let identity = AgentIdentity::from("worker-ready-wait");
        let entry = ready_wait_test_entry(&identity, MobRuntimeMode::AutonomousHost);
        let mut authority =
            seed_ready_wait_machine(&identity, mob_dsl::SpawnPolicyRuntimeMode::AutonomousHost);

        assert!(
            !MobHandle::ready_wait_is_satisfied(&entry, authority.state()),
            "an active autonomous member should remain pending until startup readiness is observed"
        );

        let runtime_id = mob_dsl::AgentRuntimeId::from_domain(&entry.agent_runtime_id);
        authority
            .apply_signal(mob_dsl::MobMachineSignal::ObserveRuntimeReady {
                agent_runtime_id: runtime_id,
                fence_token: mob_dsl::FenceToken::from_domain(entry.fence_token),
            })
            .expect("runtime ready observation should admit for current runtime");

        assert!(
            MobHandle::ready_wait_is_satisfied(&entry, authority.state()),
            "ready wait should be satisfied from machine startup-ready state without actor polling"
        );
    }

    #[test]
    fn ready_wait_treats_turn_driven_members_as_ready_without_startup_marker() {
        let identity = AgentIdentity::from("worker-ready-td");
        let entry = ready_wait_test_entry(&identity, MobRuntimeMode::TurnDriven);
        let authority =
            seed_ready_wait_machine(&identity, mob_dsl::SpawnPolicyRuntimeMode::TurnDriven);

        assert!(
            MobHandle::ready_wait_is_satisfied(&entry, authority.state()),
            "turn-driven members do not require autonomous startup readiness"
        );
    }

    fn placed_ready_wait_state(identity: &AgentIdentity) -> mob_dsl::MobMachineState {
        let mut state = mob_dsl::MobMachineAuthority::new().state().clone();
        let dsl_identity = mob_dsl::AgentIdentity::from_domain(identity);
        let runtime_id =
            mob_dsl::AgentRuntimeId::from_domain(&AgentRuntimeId::initial(identity.clone()));
        let host = mob_dsl::HostId("placed-ready-host".to_string());
        let binding_generation = 7;

        state
            .member_placement
            .insert(dsl_identity.clone(), host.clone());
        state
            .host_bind_phase
            .insert(host.clone(), mob_dsl::HostBindPhase::Bound);
        state
            .host_binding_generations
            .insert(host, binding_generation);
        state
            .current_placed_spawn_host_binding_generations
            .insert(dsl_identity.clone(), binding_generation);
        state.current_placed_spawn_ids.insert(
            dsl_identity.clone(),
            mob_dsl::PlacedSpawnId("placed-ready-spawn".to_string()),
        );
        state
            .current_placed_spawn_provision_operation_ids
            .insert(dsl_identity.clone(), "placed-ready-operation".to_string());
        state
            .current_placed_spawn_operation_owner_session_ids
            .insert(
                dsl_identity.clone(),
                mob_dsl::SessionId("placed-ready-owner".to_string()),
            );
        state.member_session_bindings.insert(
            dsl_identity.clone(),
            mob_dsl::SessionId("placed-ready-session".to_string()),
        );
        state
            .member_peer_endpoints
            .insert(dsl_identity.clone(), mob_dsl::MemberPeerEndpoint::default());
        state
            .identity_runtime_generations
            .insert(dsl_identity.clone(), mob_dsl::Generation(1));
        state
            .identity_runtime_fence_tokens
            .insert(dsl_identity.clone(), mob_dsl::FenceToken(0));
        state
            .identity_to_runtime
            .insert(dsl_identity, runtime_id.clone());
        state.live_runtime_ids.insert(runtime_id);
        state
    }

    #[test]
    fn ready_wait_uses_committed_placement_instead_of_local_startup_marker() {
        let identity = AgentIdentity::from("placed-ready-worker");
        let entry = ready_wait_test_entry(&identity, MobRuntimeMode::AutonomousHost);
        let mut state = placed_ready_wait_state(&identity);
        let dsl_identity = mob_dsl::AgentIdentity::from_domain(&identity);

        assert!(
            state.member_startup_ready.is_empty() && state.member_startup_runtime_ready.is_empty(),
            "placed peer-only members intentionally have no local startup marker"
        );
        assert!(
            MobHandle::ready_wait_is_satisfied(&entry, &state),
            "the exact settled placed incarnation is ready"
        );

        state
            .host_binding_generations
            .insert(mob_dsl::HostId("placed-ready-host".to_string()), 8);
        assert!(
            !MobHandle::ready_wait_is_satisfied(&entry, &state),
            "a stale carrier generation must close placed readiness"
        );
        state
            .host_binding_generations
            .insert(mob_dsl::HostId("placed-ready-host".to_string()), 7);
        state
            .member_materialization_failures
            .insert(dsl_identity, "remote materialization unhealthy".to_string());
        assert!(
            !MobHandle::ready_wait_is_satisfied(&entry, &state),
            "materialization failure must close placed readiness"
        );
    }
}
