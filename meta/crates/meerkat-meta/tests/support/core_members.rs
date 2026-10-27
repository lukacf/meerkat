//! Test fixture: the members Core holds, for integration-test doubles of
//! `SessionBridge` whose Core reads (`live_member`, `live_member_scope`,
//! `ensure_member_live`, `last_member_binding`, `member_binding_history`,
//! `durable_identity_members`) answer from what the double's creates, resumes
//! and retires committed, or from what a test bound.
//!
//! An identity's status, runtime id and session are Core's member history:
//! a double that answers none of these reads leaves every identity Dormant
//! and every delivery without a member.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::sync::{Mutex, PoisonError};

use meerkat_core::types::SessionId;
use meerkat_meta::identity_first::{
    AgentIdentity, AgentRuntimeId, BridgeError, MemberRuntimeBinding,
};

#[derive(Clone)]
struct Member {
    runtime_id: AgentRuntimeId,
    session_id: Option<SessionId>,
    generation: u64,
    fence: u64,
    profile: meerkat_mob::ProfileName,
    live: bool,
}

#[derive(Default)]
pub struct CoreMembers {
    members: Mutex<BTreeMap<AgentIdentity, Member>>,
}

impl CoreMembers {
    /// Core committed `binding` as `identity`'s live member. The generation
    /// is the runtime id's `rt:{identity}:{generation}` suffix (0 without
    /// one).
    pub fn bind(
        &self,
        identity: &AgentIdentity,
        binding: &MemberRuntimeBinding,
        profile: &meerkat_mob::ProfileName,
    ) {
        let generation = generation_of(&binding.agent_runtime_id);
        let mut members = self.members.lock().unwrap_or_else(PoisonError::into_inner);
        let fence = members.get(identity).map_or(1, |member| member.fence + 1);
        members.insert(
            identity.clone(),
            Member {
                runtime_id: binding.agent_runtime_id.clone(),
                session_id: binding.session_id.clone(),
                generation,
                fence,
                profile: profile.clone(),
                live: true,
            },
        );
    }

    /// Core holds `identity`'s member live at `generation`, bound to
    /// `session` (`None` for a peer-only member). Returns its runtime id,
    /// `rt:{identity}:{generation}`.
    pub fn bind_live(
        &self,
        identity: &AgentIdentity,
        profile: &meerkat_mob::ProfileName,
        generation: u64,
        session: Option<SessionId>,
    ) -> AgentRuntimeId {
        let agent_runtime_id = AgentRuntimeId::parse(&format!("rt:{identity}:{generation}"))
            .expect("a test identity forms a valid runtime id");
        self.bind(
            identity,
            &MemberRuntimeBinding {
                agent_runtime_id: agent_runtime_id.clone(),
                session_id: session,
            },
            profile,
        );
        agent_runtime_id
    }

    /// [`Self::bind_live`] with Core's fence for the member set to `fence`
    /// (a completion cursor's epoch is that fence).
    pub fn bind_live_at_fence(
        &self,
        identity: &AgentIdentity,
        profile: &meerkat_mob::ProfileName,
        generation: u64,
        session: Option<SessionId>,
        fence: u64,
    ) -> AgentRuntimeId {
        let runtime_id = self.bind_live(identity, profile, generation, session);
        let mut members = self.members.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(member) = members.get_mut(identity) {
            member.fence = fence;
        }
        runtime_id
    }

    /// Core retired the member whose runtime id is `runtime_id`; its binding
    /// stays in the history as retired.
    pub fn retire(&self, runtime_id: &AgentRuntimeId) {
        let mut members = self.members.lock().unwrap_or_else(PoisonError::into_inner);
        for member in members.values_mut() {
            if member.runtime_id == *runtime_id {
                member.live = false;
            }
        }
    }

    pub fn live_member(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<MemberRuntimeBinding>, BridgeError> {
        Ok(self.live(identity).map(|member| MemberRuntimeBinding {
            agent_runtime_id: member.runtime_id,
            session_id: member.session_id,
        }))
    }

    pub fn live_member_scope(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<meerkat_mob::MemberDeliveryScope>, BridgeError> {
        let Some(member) = self.live(identity) else {
            return Ok(None);
        };
        let Some(session_id) = member.session_id else {
            return Err(BridgeError::Mob(format!(
                "{identity} is a member bound to no session"
            )));
        };
        meerkat_mob::MemberDeliveryScope::from_json_value(serde_json::json!({
            "version": 1,
            "runtime_id": {
                "identity": meerkat_meta::member_comms_id::roster_member_id_for_identity(
                    identity.as_str()
                )
                .to_string(),
                "generation": member.generation,
            },
            "fence_token": member.fence,
            "session_id": session_id.to_string(),
        }))
        .map(Some)
        .map_err(|error| BridgeError::Mob(error.to_string()))
    }

    pub fn last_member_binding(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<meerkat_mob::MemberBinding>, BridgeError> {
        let members = self.members.lock().unwrap_or_else(PoisonError::into_inner);
        Ok(members
            .get(identity)
            .map(|member| member_binding(identity, member)))
    }

    /// Core moved `identity`'s live member onto `session` and kept its
    /// runtime id (a repair's fresh spawn on the same incarnation).
    pub fn move_live_session(&self, identity: &AgentIdentity, session: SessionId) {
        let mut members = self.members.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(member) = members.get_mut(identity).filter(|member| member.live) {
            member.session_id = Some(session);
        }
    }

    pub fn history(&self) -> Result<meerkat_mob::MemberBindingHistory, BridgeError> {
        let members = self.members.lock().unwrap_or_else(PoisonError::into_inner);
        let bindings = members
            .iter()
            .map(|(identity, member)| member_binding(identity, member))
            .collect::<Vec<_>>();
        Ok(meerkat_mob::MemberBindingHistory::for_test(
            bindings.clone(),
            bindings,
            [],
            members.len() as u64,
        ))
    }

    /// Core's live members spawned with a durable identity intent, in
    /// identity order: Core's own filter over [`Self::history`].
    pub fn durable_identity_members(&self) -> Result<Vec<meerkat_mob::MemberBinding>, BridgeError> {
        Ok(self
            .history()?
            .durable_identity_members()
            .cloned()
            .collect())
    }

    fn live(&self, identity: &AgentIdentity) -> Option<Member> {
        let members = self.members.lock().unwrap_or_else(PoisonError::into_inner);
        members.get(identity).filter(|member| member.live).cloned()
    }
}

fn generation_of(runtime_id: &AgentRuntimeId) -> u64 {
    runtime_id
        .as_str()
        .rsplit(':')
        .next()
        .and_then(|generation| generation.parse().ok())
        .unwrap_or(0)
}

fn member_binding(identity: &AgentIdentity, member: &Member) -> meerkat_mob::MemberBinding {
    meerkat_mob::MemberBinding::for_test(
        meerkat_meta::member_comms_id::roster_member_id_for_identity(identity.as_str()),
        meerkat_mob::Generation::new(member.generation),
        member.session_id.clone(),
        if member.live {
            meerkat_mob::MemberBindingState::Live
        } else {
            meerkat_mob::MemberBindingState::Retired
        },
        member.profile.clone(),
        meerkat_mob::SpawnContinuityIntent::DurableIdentity {
            continuity_key: identity.as_str().to_string(),
        },
        meerkat_mob::MemberIdentityManagement::CommandDriven,
    )
    // Every identity-first spawn labels its member with the identity.
    .with_labels(std::collections::BTreeMap::from([(
        "agent_identity".to_owned(),
        identity.as_str().to_owned(),
    )]))
}
