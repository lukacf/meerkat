//! Test fixture: a session bridge whose Core is the members fixture
//! ([`super::core_members`]). Creates and resumes bind there, retires retire
//! there, Core's member reads answer from there, and peer wiring is accepted
//! and recorded nowhere. Delivery is refused: a scenario that only needs Core
//! to build members uses this double.
//!
//! A test crate that uses it also declares `mod core_members` from
//! `support/core_members.rs`.

#![allow(dead_code)]

use async_trait::async_trait;
use meerkat_meta::identity_first::{
    AgentBuildDraft, AgentIdentity, AgentRuntimeId, BridgeDelivery, BridgeError, DurableAgentSpec,
    MemberRuntimeBinding, ResumeSessionOutcome, SessionBridge, SessionSnapshot,
};

use super::core_members::CoreMembers;

#[derive(Default)]
pub struct CoreBridge {
    pub core: CoreMembers,
}

#[async_trait]
impl SessionBridge for CoreBridge {
    async fn create_session(
        &self,
        identity: &AgentIdentity,
        spec: &DurableAgentSpec,
        _draft: &AgentBuildDraft,
    ) -> Result<MemberRuntimeBinding, BridgeError> {
        let session_id = meerkat_core::types::SessionId::new();
        let agent_runtime_id =
            self.core
                .bind_live(identity, &spec.profile, 0, Some(session_id.clone()));
        Ok(MemberRuntimeBinding {
            agent_runtime_id,
            session_id: Some(session_id),
        })
    }

    async fn resume_session(
        &self,
        identity: &AgentIdentity,
        spec: &DurableAgentSpec,
        _draft: &AgentBuildDraft,
        session_id: &meerkat_core::types::SessionId,
        _resume_from_role: Option<&meerkat_mob::ProfileName>,
    ) -> Result<ResumeSessionOutcome, BridgeError> {
        let agent_runtime_id =
            self.core
                .bind_live(identity, &spec.profile, 0, Some(session_id.clone()));
        Ok(ResumeSessionOutcome::Resumed {
            session_id: session_id.clone(),
            agent_runtime_id,
        })
    }

    async fn deliver_admitted(
        &self,
        runtime_id: &AgentRuntimeId,
        _delivery: BridgeDelivery,
    ) -> Result<meerkat_core::types::SessionId, BridgeError> {
        Err(BridgeError::Mob(format!(
            "this double delivers nothing ({runtime_id})"
        )))
    }

    async fn checkpoint_session(
        &self,
        _runtime_id: &AgentRuntimeId,
        _session_id: &meerkat_core::types::SessionId,
    ) -> Result<SessionSnapshot, BridgeError> {
        Ok(SessionSnapshot { data: Vec::new() })
    }

    async fn retire_member(&self, runtime_id: &AgentRuntimeId) -> Result<(), BridgeError> {
        self.core.retire(runtime_id);
        Ok(())
    }

    async fn wire_peer(&self, _a: &AgentRuntimeId, _b: &AgentRuntimeId) -> Result<(), BridgeError> {
        Ok(())
    }

    async fn wire_peers_batch(
        &self,
        _edges: &[(AgentRuntimeId, AgentRuntimeId)],
    ) -> Result<(), BridgeError> {
        Ok(())
    }

    async fn unwire_peer(
        &self,
        _a: &AgentRuntimeId,
        _b: &AgentRuntimeId,
    ) -> Result<(), BridgeError> {
        Ok(())
    }

    async fn current_member_wires(
        &self,
    ) -> Result<Vec<(AgentRuntimeId, AgentRuntimeId)>, BridgeError> {
        Ok(Vec::new())
    }

    async fn live_member(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<MemberRuntimeBinding>, BridgeError> {
        self.core.live_member(identity)
    }

    async fn live_member_scope(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<meerkat_mob::MemberDeliveryScope>, BridgeError> {
        self.core.live_member_scope(identity)
    }

    async fn ensure_member_live(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<MemberRuntimeBinding>, BridgeError> {
        self.core.live_member(identity)
    }

    async fn member_binding_history(
        &self,
    ) -> Result<meerkat_mob::MemberBindingHistory, BridgeError> {
        self.core.history()
    }

    async fn last_member_binding(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<meerkat_mob::MemberBinding>, BridgeError> {
        self.core.last_member_binding(identity)
    }

    async fn durable_identity_members(
        &self,
    ) -> Result<Vec<meerkat_mob::MemberBinding>, BridgeError> {
        self.core.durable_identity_members()
    }

    async fn identity_management(
        &self,
        _identity: &AgentIdentity,
    ) -> Result<meerkat_mob::MemberIdentityManagement, BridgeError> {
        Ok(meerkat_mob::MemberIdentityManagement::CommandDriven)
    }
}
