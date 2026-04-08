use crate::roster::{MemberState, MobMemberKickoffSnapshot};
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CanonicalSessionObservation {
    Active,
    Inactive,
    Missing,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MobMemberTerminalClass {
    Running,
    TerminalFailure,
    TerminalUnknown,
    TerminalCompleted,
}

#[derive(Debug, Clone)]
pub(super) struct CanonicalMemberSnapshotMaterial {
    pub(super) member_present: bool,
    pub(super) status: CanonicalMemberStatus,
    pub(super) session_observation: CanonicalSessionObservation,
    pub(super) error: Option<String>,
    pub(super) output_preview: Option<String>,
    pub(super) tokens_used: u64,
    pub(super) current_session_id: Option<SessionId>,
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
        let is_final = MobMemberLifecycleAuthority::is_terminal(self);
        MobMemberSnapshot {
            status,
            output_preview: self.output_preview.clone(),
            error: self.error.clone(),
            tokens_used: self.tokens_used,
            is_final,
            current_session_id: self.current_session_id.clone(),
            peer_connectivity: self.peer_connectivity.clone(),
            kickoff: self.kickoff.clone(),
        }
    }

    pub(super) fn to_helper_result(&self) -> HelperResult {
        HelperResult {
            output: self.output_preview.clone(),
            tokens_used: self.tokens_used,
            session_id: self.current_session_id.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct MobMemberLifecycleInput {
    pub(super) member_present: bool,
    pub(super) roster_state: Option<MemberState>,
    pub(super) session_observation: CanonicalSessionObservation,
    pub(super) restore_failure: Option<String>,
    pub(super) output_preview: Option<String>,
    pub(super) tokens_used: u64,
    pub(super) current_session_id: Option<SessionId>,
    pub(super) peer_connectivity: Option<MobPeerConnectivitySnapshot>,
    pub(super) kickoff: Option<MobMemberKickoffSnapshot>,
}

pub(super) struct MobMemberLifecycleAuthority;

impl MobMemberLifecycleAuthority {
    pub(super) fn materialize(input: MobMemberLifecycleInput) -> CanonicalMemberSnapshotMaterial {
        if let Some(reason) = input.restore_failure {
            return CanonicalMemberSnapshotMaterial {
                member_present: input.member_present,
                status: if input.member_present {
                    CanonicalMemberStatus::Broken
                } else {
                    CanonicalMemberStatus::Unknown
                },
                session_observation: CanonicalSessionObservation::Missing,
                error: Some(reason),
                output_preview: None,
                tokens_used: 0,
                current_session_id: input.current_session_id,
                peer_connectivity: None,
                kickoff: input.kickoff,
            };
        }

        let status = match (
            input.member_present,
            input.roster_state,
            input.session_observation,
        ) {
            (false, _, _) => CanonicalMemberStatus::Unknown,
            (true, Some(MemberState::Retiring), _) => CanonicalMemberStatus::Retiring,
            (true, Some(MemberState::Active), CanonicalSessionObservation::Missing) => {
                CanonicalMemberStatus::Completed
            }
            (true, Some(MemberState::Active), _) => CanonicalMemberStatus::Active,
            (true, None, _) => CanonicalMemberStatus::Unknown,
        };

        CanonicalMemberSnapshotMaterial {
            member_present: input.member_present,
            status,
            session_observation: input.session_observation,
            error: None,
            output_preview: input.output_preview,
            tokens_used: input.tokens_used,
            current_session_id: input.current_session_id,
            peer_connectivity: input.peer_connectivity,
            kickoff: input.kickoff,
        }
    }

    pub(super) fn classify(material: &CanonicalMemberSnapshotMaterial) -> MobMemberTerminalClass {
        if !material.member_present {
            return MobMemberTerminalClass::TerminalUnknown;
        }
        match material.status {
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

    pub(super) fn is_terminal(material: &CanonicalMemberSnapshotMaterial) -> bool {
        matches!(
            Self::classify(material),
            MobMemberTerminalClass::TerminalFailure
                | MobMemberTerminalClass::TerminalUnknown
                | MobMemberTerminalClass::TerminalCompleted
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restore_failure_breaks_present_member() {
        let material = MobMemberLifecycleAuthority::materialize(MobMemberLifecycleInput {
            member_present: true,
            roster_state: Some(MemberState::Active),
            session_observation: CanonicalSessionObservation::Active,
            restore_failure: Some("restore mismatch".into()),
            output_preview: Some("ignored".into()),
            tokens_used: 12,
            current_session_id: None,
            peer_connectivity: None,
            kickoff: None,
        });
        assert_eq!(material.status, CanonicalMemberStatus::Broken);
        assert_eq!(material.error.as_deref(), Some("restore mismatch"));
    }

    #[test]
    fn missing_active_session_means_completed_not_broken() {
        let material = MobMemberLifecycleAuthority::materialize(MobMemberLifecycleInput {
            member_present: true,
            roster_state: Some(MemberState::Active),
            session_observation: CanonicalSessionObservation::Missing,
            restore_failure: None,
            output_preview: None,
            tokens_used: 0,
            current_session_id: None,
            peer_connectivity: None,
            kickoff: None,
        });
        assert_eq!(material.status, CanonicalMemberStatus::Completed);
        assert!(MobMemberLifecycleAuthority::is_terminal(&material));
    }

    #[test]
    fn retiring_member_is_non_terminal_even_if_session_missing() {
        let material = MobMemberLifecycleAuthority::materialize(MobMemberLifecycleInput {
            member_present: true,
            roster_state: Some(MemberState::Retiring),
            session_observation: CanonicalSessionObservation::Missing,
            restore_failure: None,
            output_preview: None,
            tokens_used: 0,
            current_session_id: None,
            peer_connectivity: None,
            kickoff: None,
        });
        assert_eq!(material.status, CanonicalMemberStatus::Retiring);
        assert!(!MobMemberLifecycleAuthority::is_terminal(&material));
    }
}
