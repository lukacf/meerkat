//! Host preparation of one member's build (lead-approved Core seam).
//!
//! A member that is not materialized in its process is built on demand: by a
//! wake (a host delivery, a peer message, a kickoff, an operator request), or
//! by the boot restore of a member that holds work. The host may need to do
//! something for that member before it runs, typically publish the tools it
//! composes for the member's identity. Core asks the host's
//! [`MemberBuildPreparer`] once per build, off the actor, before the member's
//! provision, on every wake and every boot-set restore, so a member never runs
//! with a surface its host has not prepared. The spawn of a store-owned
//! identity's member (the kickoff spawn of its create, or the seat a start
//! owes it) is such a build too: it is asked before that spawn's provision,
//! about the session the spawn names, which no build has created yet, with
//! no wired peers (the spawn wires them). A refusal there fails the spawn:
//! the identity stays owed its seat, and no retry is recorded. With no
//! preparer configured, nothing changes.
//!
//! The preparer receives a read-only [`MobReadHandle`]: it is called inside a
//! wake and inside a boot restore before the mob's actor runs, so it must not
//! issue actor commands (a send that waits on a wake, `ensure_member_live`),
//! which could wait on the very wake that called it.

use std::collections::BTreeSet;
use std::sync::Arc;

use meerkat_core::types::SessionId;

use crate::ids::AgentIdentity;
use crate::runtime::MobReadHandle;
use crate::runtime::session_service::MobSessionService;

/// What the host is asked to prepare for one member's build.
#[derive(Clone)]
#[non_exhaustive]
pub struct MemberBuildPreparation {
    /// The member being built.
    pub agent_identity: AgentIdentity,
    /// The session the member is built on (its durable binding, or the
    /// successor its restore repointed it to).
    pub session_id: SessionId,
    /// A read-only observer of the mob. It issues no actor command. Its reads
    /// carry the authority the mob's launch minted (the owner's), on a wake
    /// as on a boot restore: never the agent lane of the member being built.
    pub handle: MobReadHandle,
    /// The member's local wired peers, from the mob's durable wiring (not
    /// from live comms, which a member being built does not have yet).
    pub wired_peers: BTreeSet<AgentIdentity>,
}

/// The host's answer when the build may proceed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum MemberBuildPrepared {
    /// Proceed; the member may run as soon as it is ready.
    Prepared,
    /// Proceed, but the member starts no run until the host releases it:
    /// Core stages the `ToolsNotPublished` run-start hold on the member's
    /// runtime registration, and the host releases it with
    /// [`MobHandle::release_member_run_starts`](crate::MobHandle::release_member_run_starts)
    /// once it published what the member needs.
    HoldUntilReleased,
}

/// The host's refusal to prepare a member's build now.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum MemberBuildPreparationRefusal {
    /// Not now: the member stays dormant and the caller that needed it gets
    /// a typed retryable refusal
    /// ([`crate::MemberWakeRefusal::BuildPreparationRetry`]). MobMachine
    /// records the retry, and every wake of the member is refused for it
    /// without asking the host again until the host retries it
    /// ([`crate::MobHandle::resolve_member_build_preparation`]) or the
    /// mob's retry timer does, at the wake retry backoff
    /// ([`crate::MobBuilder::with_member_wake_retry_backoff`]); the timer
    /// then wakes the member only for work it still owes, and asks the
    /// host again with that wake.
    Retryable { reason: String },
    /// Never as it stands: the member is recorded broken with `reason`, as a
    /// failed restore is.
    Final { reason: String },
}

impl MemberBuildPreparationRefusal {
    pub fn reason(&self) -> &str {
        match self {
            Self::Retryable { reason } | Self::Final { reason } => reason,
        }
    }
}

/// What Core reports once a member it built is live.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct MemberBuildReport {
    pub agent_identity: AgentIdentity,
    pub session_id: SessionId,
    /// The built session's visible tool names (its tool catalog entries the
    /// model sees, or its tool scope's visible names when the service
    /// reports no catalog), sorted and deduplicated.
    pub visible_tools: Vec<String>,
}

/// Host preparation of member builds. See the module documentation.
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
pub trait MemberBuildPreparer: Send + Sync {
    /// Prepare the build of `request.agent_identity`. Called once per build,
    /// off the actor, before the member's provision.
    async fn prepare_member_build(
        &self,
        request: MemberBuildPreparation,
    ) -> Result<MemberBuildPrepared, MemberBuildPreparationRefusal>;

    /// The member Core built is live, with this tool surface. Called once per
    /// successful local build, off the actor: a wake's or a boot restore's
    /// (which Core prepared through this preparer), and a spawn's once its
    /// activation committed (a fresh or embodied spawn, a held create once
    /// activated, a respawn), which Core does not prepare, except the spawn
    /// of a store-owned identity's member, which it does. A build that
    /// failed, or a wake that was refused or abandoned, is never reported.
    /// The default ignores it.
    async fn member_built(&self, report: MemberBuildReport) {
        let _ = report;
    }
}

/// The member's local wired peers from the machine's durable wiring edges.
pub(crate) fn member_build_wired_peers(
    state: &crate::machines::mob_machine::MobMachineState,
    agent_identity: &AgentIdentity,
) -> BTreeSet<AgentIdentity> {
    let local = crate::machines::mob_machine::AgentIdentity::from_domain(agent_identity);
    state
        .wiring_edges
        .iter()
        .filter_map(|edge| {
            if edge.a == local {
                Some(AgentIdentity::from(edge.b.0.as_str()))
            } else if edge.b == local {
                Some(AgentIdentity::from(edge.a.0.as_str()))
            } else {
                None
            }
        })
        .collect()
}

/// Ask the host's build preparer about the spawn of a store-owned
/// identity's member (the kickoff spawn of its create, or the seat a start
/// owes it) before its provision, off the actor, as a wake asks it. `Ok` is
/// whether a hold the host asked for was staged on the member's next
/// registration: the caller removes it again when the spawn fails. A
/// refusal, for now or for good, fails the spawn with the host's reason:
/// the identity stays owed its seat, and the mob's next start asks again.
/// No retry is recorded for it, since no member exists yet to refuse a wake
/// of.
#[cfg(feature = "runtime-adapter")]
pub(crate) async fn prepare_spawn_member_build(
    preparer: &dyn MemberBuildPreparer,
    provisioner: &dyn crate::runtime::provisioner::MobProvisioner,
    request: MemberBuildPreparation,
) -> Result<bool, crate::error::MobError> {
    let agent_identity = request.agent_identity.clone();
    let session_id = request.session_id.clone();
    match preparer.prepare_member_build(request).await {
        Ok(MemberBuildPrepared::Prepared) => Ok(false),
        Ok(MemberBuildPrepared::HoldUntilReleased) => {
            provisioner
                .stage_member_registration_run_start_hold_only(
                    &crate::event::MemberRef::from_bridge_session_id(session_id),
                    meerkat_runtime::RunStartHoldReason::ToolsNotPublished,
                )
                .await?;
            Ok(true)
        }
        Err(MemberBuildPreparationRefusal::Retryable { reason }) => {
            Err(crate::error::MobError::Internal(format!(
                "the host asked to retry the build preparation of '{agent_identity}': {reason}"
            )))
        }
        Err(MemberBuildPreparationRefusal::Final { reason }) => {
            Err(crate::error::MobError::Internal(format!(
                "the host refused to prepare the build of '{agent_identity}': {reason}"
            )))
        }
    }
}

/// Report a built member to the host's preparer, if one is configured: read
/// the built session's visible tool names and hand them over. A failed read
/// skips the report (traced); it never fails the build that already
/// succeeded. Run it off the actor.
pub(crate) async fn report_member_built(
    preparer: Option<Arc<dyn MemberBuildPreparer>>,
    session_service: Arc<dyn MobSessionService>,
    agent_identity: AgentIdentity,
    session_id: SessionId,
) {
    let Some(preparer) = preparer else {
        return;
    };
    let visible_tools = match session_service.tool_catalog_view(&session_id).await {
        Ok(Some(view)) => view
            .entries
            .into_iter()
            .filter(|entry| entry.visibility == meerkat_core::ToolCatalogVisibility::Visible)
            .map(|entry| entry.name.into_string())
            .collect::<BTreeSet<_>>(),
        Ok(None) => match session_service.tool_scope_snapshot(&session_id).await {
            Ok(Some(snapshot)) => snapshot
                .visible_names
                .into_iter()
                .map(meerkat_core::ToolName::into_string)
                .collect::<BTreeSet<_>>(),
            Ok(None) => BTreeSet::new(),
            Err(error) => {
                tracing::warn!(
                    agent_identity = %agent_identity,
                    session_id = %session_id,
                    %error,
                    "built member's tool scope could not be read; its build is not reported"
                );
                return;
            }
        },
        Err(error) => {
            tracing::warn!(
                agent_identity = %agent_identity,
                session_id = %session_id,
                %error,
                "built member's tool catalog could not be read; its build is not reported"
            );
            return;
        }
    };
    preparer
        .member_built(MemberBuildReport {
            agent_identity,
            session_id,
            visible_tools: visible_tools.into_iter().collect(),
        })
        .await;
}
