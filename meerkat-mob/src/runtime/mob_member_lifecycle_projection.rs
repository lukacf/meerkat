use crate::ids::{AgentIdentity, AgentRuntimeId, FenceToken};
use crate::machines::mob_machine as mob_dsl;
use crate::roster::{MobMemberKickoffPhase, MobMemberKickoffSnapshot};
use crate::runtime::handle::MemberProgressSnapshot;
use crate::runtime::handle::{HelperResult, MobMemberSnapshot, MobMemberStatus};
use meerkat_core::time_compat::UNIX_EPOCH;
use meerkat_core::types::SessionId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CanonicalMemberStatus {
    Unknown,
    Active,
    Retiring,
    Broken,
    Completed,
}

#[derive(Debug, Clone)]
pub(super) struct CanonicalMemberSnapshotMaterial {
    pub(super) member_present: bool,
    pub(super) status: CanonicalMemberStatus,
    pub(super) is_terminal: bool,
    pub(super) error: Option<String>,
    pub(super) output_preview: Option<String>,
    pub(super) tokens_used: u64,
    pub(super) agent_identity: AgentIdentity,
    pub(super) agent_runtime_id: Option<AgentRuntimeId>,
    pub(super) fence_token: Option<FenceToken>,
    pub(super) current_bridge_session_id: Option<SessionId>,
    pub(super) peer_connectivity: Option<meerkat_contracts::WirePeerConnectivity>,
    pub(super) kickoff: Option<MobMemberKickoffSnapshot>,
    pub(super) progress: Option<MemberProgressSnapshot>,
}

impl CanonicalMemberSnapshotMaterial {
    pub(super) fn to_snapshot(&self) -> MobMemberSnapshot {
        let status = match self.status {
            CanonicalMemberStatus::Unknown => MobMemberStatus::Unknown,
            CanonicalMemberStatus::Active => MobMemberStatus::Active,
            CanonicalMemberStatus::Retiring => MobMemberStatus::Retiring,
            CanonicalMemberStatus::Broken => MobMemberStatus::Broken,
            CanonicalMemberStatus::Completed => MobMemberStatus::Completed,
        };
        MobMemberSnapshot {
            status,
            agent_identity: self.agent_identity.clone(),
            agent_runtime_id: self.agent_runtime_id.clone(),
            fence_token: self.fence_token,
            output_preview: self.output_preview.clone(),
            error: self.error.clone(),
            tokens_used: self.tokens_used,
            is_final: self.is_terminal,
            current_session_id: None,
            current_bridge_session_id: None,
            peer_connectivity: self.peer_connectivity.clone(),
            kickoff: self.kickoff.clone(),
            external_member: None,
            resolved_capabilities: None,
            progress: self.progress.clone(),
            placement: None,
            control_reachability: None,
            comms_reachability: None,
            last_seen_ms: None,
            freshness_reason: None,
            lifecycle_capabilities: None,
            non_portable_disabled: None,
        }
        .with_current_bridge_session_id(self.current_bridge_session_id.clone())
    }

    pub(super) fn to_helper_result(&self) -> Option<HelperResult> {
        let agent_runtime_id = self.agent_runtime_id.clone()?;
        let fence_token = self.fence_token?;
        Some(HelperResult {
            output: self.output_preview.clone(),
            tokens_used: self.tokens_used,
            agent_identity: self.agent_identity.clone(),
            agent_runtime_id,
            fence_token,
        })
    }
}

#[derive(Debug, Clone)]
pub(super) struct MobMemberLifecycleInput {
    pub(super) member_present: bool,
    pub(super) machine_lifecycle: mob_dsl::MobMemberLifecycleMaterial,
    pub(super) output_preview: Option<String>,
    pub(super) tokens_used: u64,
    pub(super) agent_identity: AgentIdentity,
    pub(super) agent_runtime_id: Option<AgentRuntimeId>,
    pub(super) fence_token: Option<FenceToken>,
    pub(super) current_bridge_session_id: Option<SessionId>,
    pub(super) peer_connectivity: Option<meerkat_contracts::WirePeerConnectivity>,
    pub(super) kickoff: Option<MobMemberKickoffSnapshot>,
    pub(super) progress: Option<MemberProgressSnapshot>,
}

pub(super) struct MobMemberLifecycleProjection;

fn kickoff_phase_from_machine(phase: mob_dsl::KickoffPhase) -> MobMemberKickoffPhase {
    match phase {
        mob_dsl::KickoffPhase::Pending => MobMemberKickoffPhase::Pending,
        mob_dsl::KickoffPhase::Starting => MobMemberKickoffPhase::Starting,
        mob_dsl::KickoffPhase::Started => MobMemberKickoffPhase::Started,
        mob_dsl::KickoffPhase::CallbackPending => MobMemberKickoffPhase::CallbackPending,
        mob_dsl::KickoffPhase::Failed => MobMemberKickoffPhase::Failed,
        mob_dsl::KickoffPhase::Cancelled => MobMemberKickoffPhase::Cancelled,
    }
}

pub(super) fn kickoff_snapshot_from_machine_state(
    member_id: &str,
    machine_state: &mob_dsl::MobMachineState,
    timestamp_hint: Option<&MobMemberKickoffSnapshot>,
) -> Option<MobMemberKickoffSnapshot> {
    let material = machine_state.kickoff_material_for_member_id(member_id)?;
    let phase = kickoff_phase_from_machine(material.phase);
    let updated_at = timestamp_hint
        .filter(|snapshot| snapshot.phase == phase && snapshot.error == material.error)
        .map(|snapshot| snapshot.updated_at)
        .unwrap_or(UNIX_EPOCH);
    Some(MobMemberKickoffSnapshot {
        objective_id: machine_state
            .member_kickoff_objective_ids
            .get(&mob_dsl::AgentIdentity(member_id.to_string()))
            .and_then(|id| uuid::Uuid::parse_str(id.as_str()).ok())
            .map(meerkat_core::interaction::ObjectiveId),
        phase,
        error: material.error,
        updated_at,
    })
}

impl MobMemberLifecycleProjection {
    fn canonical_status(status: mob_dsl::MobMemberLifecycleStatus) -> CanonicalMemberStatus {
        match status {
            mob_dsl::MobMemberLifecycleStatus::Unknown => CanonicalMemberStatus::Unknown,
            mob_dsl::MobMemberLifecycleStatus::Active => CanonicalMemberStatus::Active,
            mob_dsl::MobMemberLifecycleStatus::Retiring => CanonicalMemberStatus::Retiring,
            mob_dsl::MobMemberLifecycleStatus::Broken => CanonicalMemberStatus::Broken,
            mob_dsl::MobMemberLifecycleStatus::Completed => CanonicalMemberStatus::Completed,
        }
    }

    pub(super) fn is_active_machine_lifecycle(
        lifecycle: &mob_dsl::MobMemberLifecycleMaterial,
    ) -> bool {
        lifecycle.status == mob_dsl::MobMemberLifecycleStatus::Active
    }

    pub(super) fn materialize(input: MobMemberLifecycleInput) -> CanonicalMemberSnapshotMaterial {
        CanonicalMemberSnapshotMaterial {
            member_present: input.member_present,
            status: Self::canonical_status(input.machine_lifecycle.status),
            is_terminal: input.machine_lifecycle.is_terminal(),
            error: input.machine_lifecycle.error,
            output_preview: input.output_preview,
            tokens_used: input.tokens_used,
            agent_identity: input.agent_identity,
            agent_runtime_id: input.agent_runtime_id,
            fence_token: input.fence_token,
            current_bridge_session_id: input.current_bridge_session_id,
            peer_connectivity: input.peer_connectivity,
            kickoff: input.kickoff,
            progress: input.progress,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::AgentIdentity;

    #[test]
    fn machine_restore_failure_breaks_present_member() {
        let material = MobMemberLifecycleProjection::materialize(MobMemberLifecycleInput {
            member_present: true,
            machine_lifecycle: mob_dsl::MobMemberLifecycleMaterial {
                status: mob_dsl::MobMemberLifecycleStatus::Broken,
                terminal_class: mob_dsl::MobMemberTerminalClass::TerminalFailure,
                error: Some("restore mismatch".into()),
            },
            output_preview: Some("ignored".into()),
            tokens_used: 12,
            agent_identity: AgentIdentity::from("test"),
            agent_runtime_id: Some(AgentRuntimeId::initial(AgentIdentity::from("test"))),
            fence_token: Some(FenceToken::new(0)),
            current_bridge_session_id: None,
            peer_connectivity: None,
            kickoff: None,
            progress: None,
        });
        assert_eq!(material.status, CanonicalMemberStatus::Broken);
        assert!(material.is_terminal);
        assert_eq!(material.error.as_deref(), Some("restore mismatch"));
    }

    #[test]
    fn kickoff_projection_uses_machine_state_not_roster_projection() {
        let roster_projection = MobMemberKickoffSnapshot {
            objective_id: None,
            phase: MobMemberKickoffPhase::Started,
            error: None,
            updated_at: UNIX_EPOCH + std::time::Duration::from_secs(42),
        };
        let mut authority = mob_dsl::MobMachineAuthority::new();

        assert!(
            kickoff_snapshot_from_machine_state(
                "worker",
                authority.state(),
                Some(&roster_projection),
            )
            .is_none(),
            "roster-only kickoff projection must not surface without machine truth"
        );

        authority
            .apply_signal(mob_dsl::MobMachineSignal::RecoverMemberKickoff {
                member_id: mob_dsl::AgentIdentity::from("worker"),
                phase: mob_dsl::KickoffPhase::Pending,
                error: None,
            })
            .expect("machine recovery should accept kickoff state");
        let projected = kickoff_snapshot_from_machine_state(
            "worker",
            authority.state(),
            Some(&roster_projection),
        )
        .expect("machine kickoff state should project");
        assert_eq!(projected.phase, MobMemberKickoffPhase::Pending);
        assert_eq!(projected.error, None);
        assert_eq!(projected.updated_at, UNIX_EPOCH);
    }

    #[test]
    fn missing_active_session_means_completed_not_broken() {
        let material = MobMemberLifecycleProjection::materialize(MobMemberLifecycleInput {
            member_present: true,
            machine_lifecycle: mob_dsl::MobMemberLifecycleMaterial {
                status: mob_dsl::MobMemberLifecycleStatus::Completed,
                terminal_class: mob_dsl::MobMemberTerminalClass::TerminalCompleted,
                error: None,
            },
            output_preview: None,
            tokens_used: 0,
            agent_identity: AgentIdentity::from("test"),
            agent_runtime_id: Some(AgentRuntimeId::initial(AgentIdentity::from("test"))),
            fence_token: Some(FenceToken::new(0)),
            current_bridge_session_id: None,
            peer_connectivity: None,
            kickoff: None,
            progress: None,
        });
        assert_eq!(material.status, CanonicalMemberStatus::Completed);
        assert!(material.is_terminal);
    }

    #[test]
    fn unknown_active_sessionless_member_stays_non_terminal() {
        let material = MobMemberLifecycleProjection::materialize(MobMemberLifecycleInput {
            member_present: true,
            machine_lifecycle: mob_dsl::MobMemberLifecycleMaterial {
                status: mob_dsl::MobMemberLifecycleStatus::Active,
                terminal_class: mob_dsl::MobMemberTerminalClass::Running,
                error: None,
            },
            output_preview: None,
            tokens_used: 0,
            agent_identity: AgentIdentity::from("test"),
            agent_runtime_id: Some(AgentRuntimeId::initial(AgentIdentity::from("test"))),
            fence_token: Some(FenceToken::new(0)),
            current_bridge_session_id: None,
            peer_connectivity: None,
            kickoff: None,
            progress: None,
        });
        assert_eq!(material.status, CanonicalMemberStatus::Active);
        assert!(!material.is_terminal);
    }

    #[test]
    fn retiring_member_is_non_terminal_even_if_session_missing() {
        let material = MobMemberLifecycleProjection::materialize(MobMemberLifecycleInput {
            member_present: true,
            machine_lifecycle: mob_dsl::MobMemberLifecycleMaterial {
                status: mob_dsl::MobMemberLifecycleStatus::Retiring,
                terminal_class: mob_dsl::MobMemberTerminalClass::Running,
                error: None,
            },
            output_preview: None,
            tokens_used: 0,
            agent_identity: AgentIdentity::from("test"),
            agent_runtime_id: Some(AgentRuntimeId::initial(AgentIdentity::from("test"))),
            fence_token: Some(FenceToken::new(0)),
            current_bridge_session_id: None,
            peer_connectivity: None,
            kickoff: None,
            progress: None,
        });
        assert_eq!(material.status, CanonicalMemberStatus::Retiring);
        assert!(!material.is_terminal);
    }
}
