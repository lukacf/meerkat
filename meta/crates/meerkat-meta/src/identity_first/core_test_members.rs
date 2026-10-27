//! Test fixture: the members Core holds, for test doubles of
//! [`SessionBridge`](super::bridge::SessionBridge) whose Core reads
//! (`live_member`, `live_member_scope`, `last_member_binding`,
//! `member_binding_history`, `durable_identity_members`) answer from what
//! the double's create, resume, reset and retire calls committed.

use std::collections::BTreeMap;
use std::sync::{Mutex, PoisonError};

use meerkat_core::types::SessionId;

use super::bridge::{BridgeError, MemberRuntimeBinding};
use super::types::{AgentIdentity, AgentRuntimeId};

#[derive(Clone)]
struct TestMember {
    runtime_id: AgentRuntimeId,
    session_id: Option<SessionId>,
    generation: u64,
    fence: u64,
    profile: meerkat_mob::ProfileName,
    live: bool,
}

#[derive(Default)]
pub(crate) struct CoreTestMembers {
    members: Mutex<BTreeMap<AgentIdentity, TestMember>>,
    /// Bindings a rebind ended: Core's history still names the sessions an
    /// identity's member was bound to before (who owned a session).
    ended: Mutex<Vec<(AgentIdentity, TestMember)>>,
}

impl CoreTestMembers {
    /// Core committed `binding` as `identity`'s live member. The
    /// generation is the runtime id's `rt:{identity}:{generation}` suffix.
    pub(crate) fn bind(
        &self,
        identity: &AgentIdentity,
        binding: &MemberRuntimeBinding,
        profile: &meerkat_mob::ProfileName,
    ) {
        let generation = generation_of(&binding.agent_runtime_id);
        let mut members = self.members.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(previous) = members.get(identity)
            && previous.session_id.is_some()
            && previous.session_id != binding.session_id
        {
            self.ended
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push((
                    identity.clone(),
                    TestMember {
                        live: false,
                        ..previous.clone()
                    },
                ));
        }
        let fence = members.get(identity).map_or(1, |member| member.fence + 1);
        members.insert(
            identity.clone(),
            TestMember {
                runtime_id: binding.agent_runtime_id.clone(),
                session_id: binding.session_id.clone(),
                generation,
                fence,
                profile: profile.clone(),
                live: true,
            },
        );
    }

    /// Core retired the member whose runtime id is `runtime_id`; its
    /// binding stays in the history as retired.
    pub(crate) fn retire(&self, runtime_id: &AgentRuntimeId) {
        let mut members = self.members.lock().unwrap_or_else(PoisonError::into_inner);
        for member in members.values_mut() {
            if member.runtime_id == *runtime_id {
                member.live = false;
            }
        }
    }

    pub(crate) fn live_member(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<MemberRuntimeBinding>, BridgeError> {
        Ok(self.live(identity).map(|member| MemberRuntimeBinding {
            agent_runtime_id: member.runtime_id,
            session_id: member.session_id,
        }))
    }

    pub(crate) fn live_member_scope(
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
                "identity": crate::member_comms_id::roster_member_id_for_identity(
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

    pub(crate) fn last_member_binding(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<meerkat_mob::MemberBinding>, BridgeError> {
        let members = self.members.lock().unwrap_or_else(PoisonError::into_inner);
        Ok(members
            .get(identity)
            .map(|member| member_binding(identity, member)))
    }

    pub(crate) fn history(&self) -> Result<meerkat_mob::MemberBindingHistory, BridgeError> {
        let members = self.members.lock().unwrap_or_else(PoisonError::into_inner);
        let bindings = members
            .iter()
            .map(|(identity, member)| member_binding(identity, member))
            .collect::<Vec<_>>();
        // Ended bindings first, so a session a live binding still names
        // resolves to that binding.
        let sessions = self
            .ended
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(|(identity, member)| member_binding(identity, member))
            .chain(bindings.iter().cloned())
            .collect::<Vec<_>>();
        Ok(meerkat_mob::MemberBindingHistory::for_test(
            bindings,
            sessions,
            [],
            members.len() as u64,
        ))
    }

    /// Core's live members spawned with a durable identity intent, in
    /// identity order: Core's own filter over [`Self::history`].
    pub(crate) fn durable_identity_members(
        &self,
    ) -> Result<Vec<meerkat_mob::MemberBinding>, BridgeError> {
        Ok(self
            .history()?
            .durable_identity_members()
            .cloned()
            .collect())
    }

    /// The session and generation of the incarnation `runtime_id` while it
    /// is its identity's live member; `None` once it is retired or replaced.
    pub(crate) fn live_incarnation(
        &self,
        runtime_id: &AgentRuntimeId,
    ) -> Option<(Option<SessionId>, u64)> {
        let members = self.members.lock().unwrap_or_else(PoisonError::into_inner);
        members
            .values()
            .find(|member| member.live && member.runtime_id == *runtime_id)
            .map(|member| (member.session_id.clone(), member.generation))
    }

    fn live(&self, identity: &AgentIdentity) -> Option<TestMember> {
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

fn member_binding(identity: &AgentIdentity, member: &TestMember) -> meerkat_mob::MemberBinding {
    meerkat_mob::MemberBinding::for_test(
        crate::member_comms_id::roster_member_id_for_identity(identity.as_str()),
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
    .with_labels(BTreeMap::from([(
        "agent_identity".to_owned(),
        identity.as_str().to_owned(),
    )]))
}
