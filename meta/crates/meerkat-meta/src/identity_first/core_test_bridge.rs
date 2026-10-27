//! Test fixture: a session bridge whose only answers are Core's member
//! reads, taken from a [`CoreTestMembers`].
//!
//! An identity's status, runtime id and session are Core's member
//! history. Tests whose subject is a view over that history (console and RPC
//! resolution, delivery routing, completion waits) bind the members they need
//! with [`bind_live`] instead of seeding a Meta continuity record. Every Core
//! write refuses, except a retire: Core retires the member, and the history
//! keeps its binding as retired. A bridge built with
//! [`CoreReadsBridge::with_registration_reload`] also answers Core's
//! registration reload, which leaves the member's binding as it is.

use meerkat_core::types::SessionId;

use super::IdentityRuntime;
use super::bridge::{
    BridgeDelivery, BridgeError, BridgeMemberReload, MemberRuntimeBinding, ResumeSessionOutcome,
    SessionBridge,
};
use super::core_test_members::CoreTestMembers;
use super::types::{
    AgentBuildDraft, AgentIdentity, AgentRuntimeId, DurableAgentSpec, MemberReloadDisposition,
    SessionSnapshot,
};

/// Core holds `identity`'s member live at `generation`, bound to `session`
/// (`None` for a peer-only member) under the role `profile`. Its runtime id
/// is `rt:{identity}:{generation}`.
pub(crate) fn bind_live(
    core: &CoreTestMembers,
    identity: &AgentIdentity,
    profile: &meerkat_mob::ProfileName,
    generation: u64,
    session: Option<SessionId>,
) -> Result<AgentRuntimeId, BridgeError> {
    let agent_runtime_id = AgentRuntimeId::parse(&format!("rt:{identity}:{generation}"))
        .map_err(|error| BridgeError::Mob(error.to_string()))?;
    core.bind(
        identity,
        &MemberRuntimeBinding {
            agent_runtime_id: agent_runtime_id.clone(),
            session_id: session,
        },
        profile,
    );
    Ok(agent_runtime_id)
}

/// Register `spec` with Core holding its member live at `generation` on
/// `session`: the shape a materialized identity has at rest.
pub(crate) async fn register_live(
    runtime: &IdentityRuntime,
    core: &CoreTestMembers,
    spec: DurableAgentSpec,
    generation: u64,
    session: SessionId,
) -> Result<AgentRuntimeId, BridgeError> {
    let runtime_id = bind_live(
        core,
        &spec.identity,
        &spec.profile,
        generation,
        Some(session),
    )?;
    runtime.register(spec).await;
    Ok(runtime_id)
}

/// A bridge that answers Core's member reads from [`Self::core`] and commits
/// nothing but retires.
#[derive(Default)]
pub(crate) struct CoreReadsBridge {
    pub(crate) core: CoreTestMembers,
    /// What Core's registration reload (`MobHandle::reload_member_registration`)
    /// answers for a live member. `None`: this bridge has no registration
    /// reload, so a reload retires the member and wakes it.
    registration_reload: Option<MemberReloadDisposition>,
}

impl CoreReadsBridge {
    /// A bridge whose registration reload answers `disposition` for the live
    /// member in place: the member keeps its session and its generation, as
    /// Core's reload does.
    pub(crate) fn with_registration_reload(disposition: MemberReloadDisposition) -> Self {
        Self {
            core: CoreTestMembers::default(),
            registration_reload: Some(disposition),
        }
    }
}

#[async_trait::async_trait]
impl SessionBridge for CoreReadsBridge {
    async fn create_session(
        &self,
        _identity: &AgentIdentity,
        _spec: &DurableAgentSpec,
        _draft: &AgentBuildDraft,
    ) -> Result<MemberRuntimeBinding, BridgeError> {
        Err(BridgeError::Unsupported {
            operation: "create_session",
        })
    }

    async fn resume_session(
        &self,
        _identity: &AgentIdentity,
        _spec: &DurableAgentSpec,
        _draft: &AgentBuildDraft,
        _session_id: &SessionId,
        _resume_from_role: Option<&meerkat_mob::ProfileName>,
    ) -> Result<ResumeSessionOutcome, BridgeError> {
        Err(BridgeError::Unsupported {
            operation: "resume_session",
        })
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

    async fn identity_management(
        &self,
        _identity: &AgentIdentity,
    ) -> Result<meerkat_mob::MemberIdentityManagement, BridgeError> {
        Ok(meerkat_mob::MemberIdentityManagement::CommandDriven)
    }

    async fn deliver_admitted(
        &self,
        _runtime_id: &AgentRuntimeId,
        _delivery: BridgeDelivery,
    ) -> Result<SessionId, BridgeError> {
        Err(BridgeError::Unsupported {
            operation: "deliver_admitted",
        })
    }

    async fn checkpoint_session(
        &self,
        _runtime_id: &AgentRuntimeId,
        _session_id: &SessionId,
    ) -> Result<SessionSnapshot, BridgeError> {
        Err(BridgeError::Unsupported {
            operation: "checkpoint_session",
        })
    }

    async fn retire_member(&self, runtime_id: &AgentRuntimeId) -> Result<(), BridgeError> {
        self.core.retire(runtime_id);
        Ok(())
    }

    async fn reload_member_registration(
        &self,
        runtime_id: &AgentRuntimeId,
    ) -> Result<Option<BridgeMemberReload>, BridgeError> {
        let Some(disposition) = self.registration_reload else {
            return Ok(None);
        };
        match self.core.live_incarnation(runtime_id) {
            Some((Some(session_id), generation)) => Ok(Some(BridgeMemberReload {
                disposition,
                session_id,
                registration_generation: generation,
            })),
            _ => Err(BridgeError::Mob(format!(
                "{runtime_id} is not a live member bound to a session"
            ))),
        }
    }
}
