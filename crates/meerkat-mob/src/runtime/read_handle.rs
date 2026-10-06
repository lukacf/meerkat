//! Direct, read-only runtime observations available before actor activation.

use super::MobHandle;
use crate::{
    AgentIdentity, MemberCreationError, MemberCreationSnapshot, MobError, MobId, MobState,
};
use meerkat_core::SessionId;

/// Read-only view for host service binding and per-call authority checks.
///
/// Every method reads an existing projection or durable journal directly. This
/// type exposes no actor commands, so a preactivation callback cannot wait for
/// an actor that has not started. Observations remain live after activation.
#[derive(Clone)]
pub struct MobReadHandle {
    handle: MobHandle,
}

impl MobHandle {
    /// Restrict this handle to direct read-only runtime observations.
    pub fn read_handle(&self) -> MobReadHandle {
        MobReadHandle {
            handle: self.clone(),
        }
    }
}

impl MobReadHandle {
    pub fn mob_id(&self) -> &MobId {
        self.handle.mob_id()
    }

    /// Last actor-published phase, not authority to initiate a transition.
    pub fn status_observation_snapshot(&self) -> MobState {
        self.handle.status_observation_snapshot()
    }

    /// Current member binding projected from the existing machine and roster.
    pub async fn get_member(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<crate::roster::RosterEntry>, MobError> {
        self.handle.get_member(identity).await
    }

    /// Historical facts do not authenticate a currently executing caller.
    pub async fn member_creation_for_session(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<MemberCreationSnapshot>, MemberCreationError> {
        self.handle.member_creation_for_session(session_id).await
    }

    pub async fn member_creation_journal_cursor(&self) -> Result<u64, MemberCreationError> {
        self.handle.member_creation_journal_cursor().await
    }

    /// Read the generated machine's owner-session lifecycle observation.
    pub fn owner_bridge_session_lifecycle_authority(
        &self,
    ) -> Option<super::handle::OwnerBridgeSessionLifecycleAuthority> {
        self.handle.owner_bridge_session_lifecycle_authority()
    }
}
