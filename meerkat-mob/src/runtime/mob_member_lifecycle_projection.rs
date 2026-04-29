use crate::ids::{AgentRuntimeId, FenceToken};
use crate::machines::mob_machine as mob_dsl;
use crate::roster::MobMemberKickoffSnapshot;
use crate::runtime::handle::{
    HelperResult, MobMemberSnapshot, MobMemberStatus, MobPeerConnectivitySnapshot,
};
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
    pub(super) terminal_class: mob_dsl::MobMemberTerminalClass,
    pub(super) error: Option<String>,
    pub(super) output_preview: Option<String>,
    pub(super) tokens_used: u64,
    pub(super) agent_runtime_id: AgentRuntimeId,
    pub(super) fence_token: FenceToken,
    pub(super) current_bridge_session_id: Option<SessionId>,
    pub(super) peer_connectivity: Option<MobPeerConnectivitySnapshot>,
    pub(super) kickoff: Option<MobMemberKickoffSnapshot>,
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
        let is_final = MobMemberLifecycleProjection::is_terminal(self);
        MobMemberSnapshot {
            status,
            agent_runtime_id: self.agent_runtime_id.clone(),
            fence_token: self.fence_token,
            output_preview: self.output_preview.clone(),
            error: self.error.clone(),
            tokens_used: self.tokens_used,
            is_final,
            realtime_attachment_status: None,
            current_session_id: None,
            current_bridge_session_id: None,
            peer_connectivity: self.peer_connectivity.clone(),
            kickoff: self.kickoff.clone(),
            external_member: None,
        }
        .with_current_bridge_session_id(self.current_bridge_session_id.clone())
    }

    pub(super) fn to_helper_result(&self) -> HelperResult {
        HelperResult {
            output: self.output_preview.clone(),
            tokens_used: self.tokens_used,
            agent_identity: self.agent_runtime_id.identity.clone(),
            agent_runtime_id: self.agent_runtime_id.clone(),
            fence_token: self.fence_token,
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct MobMemberLifecycleInput {
    pub(super) member_present: bool,
    pub(super) machine_lifecycle: mob_dsl::MobMemberLifecycleMaterial,
    pub(super) restore_failure: Option<String>,
    pub(super) output_preview: Option<String>,
    pub(super) tokens_used: u64,
    pub(super) agent_runtime_id: AgentRuntimeId,
    pub(super) fence_token: FenceToken,
    pub(super) current_bridge_session_id: Option<SessionId>,
    pub(super) peer_connectivity: Option<MobPeerConnectivitySnapshot>,
    pub(super) kickoff: Option<MobMemberKickoffSnapshot>,
}

pub(super) struct MobMemberLifecycleProjection;

impl MobMemberLifecycleProjection {
    fn canonical_status(status: mob_dsl::MobMemberLifecycleStatus) -> CanonicalMemberStatus {
        match status {
            mob_dsl::MobMemberLifecycleStatus::Unknown => CanonicalMemberStatus::Unknown,
            mob_dsl::MobMemberLifecycleStatus::Active => CanonicalMemberStatus::Active,
            mob_dsl::MobMemberLifecycleStatus::Retiring => CanonicalMemberStatus::Retiring,
            mob_dsl::MobMemberLifecycleStatus::Completed => CanonicalMemberStatus::Completed,
        }
    }

    pub(super) fn materialize(input: MobMemberLifecycleInput) -> CanonicalMemberSnapshotMaterial {
        if let Some(reason) = input.restore_failure {
            return CanonicalMemberSnapshotMaterial {
                member_present: input.member_present,
                status: if input.member_present {
                    CanonicalMemberStatus::Broken
                } else {
                    CanonicalMemberStatus::Unknown
                },
                terminal_class: mob_dsl::MobMemberTerminalClass::TerminalFailure,
                error: Some(reason),
                output_preview: None,
                tokens_used: 0,
                agent_runtime_id: input.agent_runtime_id,
                fence_token: input.fence_token,
                current_bridge_session_id: input.current_bridge_session_id,
                peer_connectivity: None,
                kickoff: input.kickoff,
            };
        }

        CanonicalMemberSnapshotMaterial {
            member_present: input.member_present,
            status: Self::canonical_status(input.machine_lifecycle.status),
            terminal_class: input.machine_lifecycle.terminal_class,
            error: None,
            output_preview: input.output_preview,
            tokens_used: input.tokens_used,
            agent_runtime_id: input.agent_runtime_id,
            fence_token: input.fence_token,
            current_bridge_session_id: input.current_bridge_session_id,
            peer_connectivity: input.peer_connectivity,
            kickoff: input.kickoff,
        }
    }

    pub(super) fn is_terminal(material: &CanonicalMemberSnapshotMaterial) -> bool {
        matches!(
            material.terminal_class,
            mob_dsl::MobMemberTerminalClass::TerminalFailure
                | mob_dsl::MobMemberTerminalClass::TerminalUnknown
                | mob_dsl::MobMemberTerminalClass::TerminalCompleted
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::AgentIdentity;

    #[test]
    fn restore_failure_breaks_present_member() {
        let material = MobMemberLifecycleProjection::materialize(MobMemberLifecycleInput {
            member_present: true,
            machine_lifecycle: mob_dsl::MobMemberLifecycleMaterial {
                status: mob_dsl::MobMemberLifecycleStatus::Active,
                terminal_class: mob_dsl::MobMemberTerminalClass::Running,
            },
            restore_failure: Some("restore mismatch".into()),
            output_preview: Some("ignored".into()),
            tokens_used: 12,
            agent_runtime_id: AgentRuntimeId::initial(AgentIdentity::from("test")),
            fence_token: FenceToken::new(0),
            current_bridge_session_id: None,
            peer_connectivity: None,
            kickoff: None,
        });
        assert_eq!(material.status, CanonicalMemberStatus::Broken);
        assert_eq!(material.error.as_deref(), Some("restore mismatch"));
    }

    #[test]
    fn missing_active_session_means_completed_not_broken() {
        let material = MobMemberLifecycleProjection::materialize(MobMemberLifecycleInput {
            member_present: true,
            machine_lifecycle: mob_dsl::MobMemberLifecycleMaterial {
                status: mob_dsl::MobMemberLifecycleStatus::Completed,
                terminal_class: mob_dsl::MobMemberTerminalClass::TerminalCompleted,
            },
            restore_failure: None,
            output_preview: None,
            tokens_used: 0,
            agent_runtime_id: AgentRuntimeId::initial(AgentIdentity::from("test")),
            fence_token: FenceToken::new(0),
            current_bridge_session_id: None,
            peer_connectivity: None,
            kickoff: None,
        });
        assert_eq!(material.status, CanonicalMemberStatus::Completed);
        assert!(MobMemberLifecycleProjection::is_terminal(&material));
    }

    #[test]
    fn unknown_active_sessionless_member_stays_non_terminal() {
        let material = MobMemberLifecycleProjection::materialize(MobMemberLifecycleInput {
            member_present: true,
            machine_lifecycle: mob_dsl::MobMemberLifecycleMaterial {
                status: mob_dsl::MobMemberLifecycleStatus::Active,
                terminal_class: mob_dsl::MobMemberTerminalClass::Running,
            },
            restore_failure: None,
            output_preview: None,
            tokens_used: 0,
            agent_runtime_id: AgentRuntimeId::initial(AgentIdentity::from("test")),
            fence_token: FenceToken::new(0),
            current_bridge_session_id: None,
            peer_connectivity: None,
            kickoff: None,
        });
        assert_eq!(material.status, CanonicalMemberStatus::Active);
        assert!(!MobMemberLifecycleProjection::is_terminal(&material));
    }

    #[test]
    fn retiring_member_is_non_terminal_even_if_session_missing() {
        let material = MobMemberLifecycleProjection::materialize(MobMemberLifecycleInput {
            member_present: true,
            machine_lifecycle: mob_dsl::MobMemberLifecycleMaterial {
                status: mob_dsl::MobMemberLifecycleStatus::Retiring,
                terminal_class: mob_dsl::MobMemberTerminalClass::Running,
            },
            restore_failure: None,
            output_preview: None,
            tokens_used: 0,
            agent_runtime_id: AgentRuntimeId::initial(AgentIdentity::from("test")),
            fence_token: FenceToken::new(0),
            current_bridge_session_id: None,
            peer_connectivity: None,
            kickoff: None,
        });
        assert_eq!(material.status, CanonicalMemberStatus::Retiring);
        assert!(!MobMemberLifecycleProjection::is_terminal(&material));
    }
}
