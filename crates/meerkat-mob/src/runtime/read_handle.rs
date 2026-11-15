//! Direct, read-only runtime observations available before actor activation.

use super::MobHandle;
use crate::{
    AgentIdentity, MemberBinding, MemberBindingHistory, MemberCreationError,
    MemberCreationSnapshot, MemberIdentityManagement, MobError, MobId, MobState,
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
            handle: self.non_owning(),
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

    /// The mob's lifecycle as observed without asking its actor:
    /// [`MobHandle::lifecycle_observation`].
    ///
    /// Inside the pre-activation hook of a resume the observation's
    /// `owed_intent` is `None` for a Stop, Complete, Reset, retirement of
    /// every member or not yet admitted Destroy the mob's log leaves open:
    /// the resume applies it to the machine after the hook. The same read
    /// handle, kept past the hook, reports it once the resume has returned.
    pub fn lifecycle_observation(&self) -> super::MobLifecycleObservation {
        self.handle.lifecycle_observation()
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

    /// The member binding history, one fold of the journal and the
    /// identity intent rows: [`MobHandle::member_binding_history`], with the
    /// same authority.
    pub async fn member_binding_history(&self) -> Result<MemberBindingHistory, MobError> {
        self.handle.member_binding_history().await
    }

    /// The journal binding that last named a session, before or after any
    /// mob reset: [`MobHandle::member_binding_for_session`], with the same
    /// authority.
    pub async fn member_binding_for_session(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<MemberBinding>, MobError> {
        self.handle.member_binding_for_session(session_id).await
    }

    /// An identity's latest incarnation since the latest mob reset, live or
    /// retired: [`MobHandle::last_member_binding`], with the same authority.
    pub async fn last_member_binding(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<MemberBinding>, MobError> {
        self.handle.last_member_binding(identity).await
    }

    /// The live durable-identity members:
    /// [`MobHandle::durable_identity_members`], with the same authority.
    pub async fn durable_identity_members(&self) -> Result<Vec<MemberBinding>, MobError> {
        self.handle.durable_identity_members().await
    }

    /// Whether Core holds an identity intent row for an identity:
    /// [`MobHandle::identity_management`], with the same authority.
    pub async fn identity_management(
        &self,
        identity: &AgentIdentity,
    ) -> Result<MemberIdentityManagement, MobError> {
        self.handle.identity_management(identity).await
    }

    /// Read the generated machine's owner-session lifecycle observation.
    pub fn owner_bridge_session_lifecycle_authority(
        &self,
    ) -> Option<super::handle::OwnerBridgeSessionLifecycleAuthority> {
        self.handle.owner_bridge_session_lifecycle_authority()
    }
}
