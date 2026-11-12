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
//! A store-owned identity's member is seated from its row in the mob's
//! identity store, with no host step. So that it is built from the same host
//! inputs as a member its host spawns itself, Core asks the preparer to
//! compose its seat ([`MemberBuildPreparer::prepare_store_owned_seat`]) at
//! every build of it, before the host's spawn customizer and before the
//! build's preparation: before a fresh spawn under created custody is sent
//! (the kickoff spawn of its create, and the spawn a start or a rotation
//! owes it), and at every wake and boot restore of the member. Each build
//! takes the profile and model overrides of the answer and the overlays
//! that are objects of the host's process (the external tools, the
//! compaction curator and the tool dispatch admission), which no journal
//! holds; in a process that already built the member, the external tools
//! overlay it kept for the member stands, as for every member. The first
//! build of the member's session also takes the build inputs that author
//! it (the context, the instructions, the system prompt, and with them the
//! tool policies, the budget limits, the auth binding and the shell
//! environment). No build relies on an
//! earlier answer: the host is the one source at each, so a restart
//! changes nothing. The default composes nothing.
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
use crate::runtime::handle::SpawnMemberSpec;
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

    /// Compose the seat of a store-owned identity's member
    /// ([`crate::IdentityEntryOrigin::Created`]) for the build Core is about
    /// to perform. Core seats such a member from its row in the mob's
    /// identity store, with no host step; this is where its host sets the
    /// build inputs it sets on the spec of a member it spawns itself.
    /// `seat` is the request Core lowered: the row's profile, labels,
    /// context and instructions for a first build, the member's own
    /// recorded request for a later one.
    ///
    /// Called at every build of such a member, off the actor, before the
    /// host's spawn customizer and before [`Self::prepare_member_build`]:
    /// the kickoff spawn of its create, the spawn a start or a rotation
    /// owes it, and every wake and boot restore. A seat Core holds Dormant
    /// asks nothing until it is built. What a build takes from the answer:
    ///
    /// - at every build, the profile and model overrides
    ///   ([`SpawnMemberSpec::override_profile`],
    ///   [`SpawnMemberSpec::model_override`]), which carry the model and
    ///   the provider parameters. They replace the overrides the member's
    ///   request held from an earlier build, so the answer is the member's
    ///   overrides for that build, and no answer builds it on its profile
    ///   as the definition gives it. On a rebuild of a durable session they
    ///   apply as any member's overrides do: the session's own model and
    ///   provider parameters stand unless the profile's resume mask names
    ///   them;
    /// - at every build too, the overlays that are objects of the host's
    ///   process: the external tools, the compaction curator and the tool
    ///   dispatch admission. No journal holds them and a process keeps
    ///   them only for a member it built, so a build after a restart has
    ///   them only from this answer. In a process that already built the
    ///   member, the external tools overlay it kept for the member stands,
    ///   as for every member;
    /// - at the first build of the member's session only, the build inputs
    ///   that author it: the context, the additional instructions and the
    ///   system prompt override, and with them the tool access and
    ///   application tool policies, the budget limits, the auth binding
    ///   and the shell environment (launch authority of that first build:
    ///   no member's rebuild on its durable session takes one). A later
    ///   build keeps what its session was first built with: instructions
    ///   are never rewritten after the fact.
    ///
    /// Nothing else of the answer is taken. The seat's labels, runtime
    /// mode, backend, binding, placement, first message, tool category
    /// overrides and wiring stay as Core lowered them. An answer that
    /// changed the seat's identity, role, launch mode or custody is
    /// refused: the build fails as a
    /// [`MemberBuildPreparationRefusal::Final`] refusal does.
    ///
    /// The host answers from the identity's row and its own state, the same
    /// for every build: no build relies on an earlier answer, so a restart
    /// changes nothing. It may be called more than once for one seat: a
    /// first build that fails after the host answered is asked again.
    /// `handle` is the read-only view [`MemberBuildPreparation`] carries,
    /// with the same rule: it issues no actor command.
    ///
    /// A refusal fails the build before anything is built. At a spawn the
    /// identity stays owed its seat, and the mob's next start asks again. At
    /// a wake or a boot restore it is the refusal
    /// [`Self::prepare_member_build`] would give: the member stays Dormant
    /// (its session unborn if no build created it), and its next wake asks
    /// again once the retry is due
    /// ([`MemberBuildPreparationRefusal::Retryable`]), or the member is
    /// recorded broken ([`MemberBuildPreparationRefusal::Final`]).
    ///
    /// The default changes nothing: the member is built from its row alone.
    async fn prepare_store_owned_seat(
        &self,
        handle: MobReadHandle,
        seat: &mut SpawnMemberSpec,
    ) -> Result<(), MemberBuildPreparationRefusal> {
        let _ = (handle, seat);
        Ok(())
    }

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

/// The fact of a store-owned seat its host's composition changed that Core
/// refuses a change of (its identity, role, launch mode or custody); `None`
/// when the composition changed none.
fn refused_seat_change(seat: &SpawnMemberSpec, composed: &SpawnMemberSpec) -> Option<&'static str> {
    use crate::launch::MemberLaunchMode;
    let same_launch = match (&seat.launch_mode, &composed.launch_mode) {
        (MemberLaunchMode::Fresh, MemberLaunchMode::Fresh) => true,
        (
            MemberLaunchMode::Resume {
                bridge_session_id: seated,
                ..
            },
            MemberLaunchMode::Resume {
                bridge_session_id: answered,
                ..
            },
        ) => seated == answered,
        _ => false,
    };
    if composed.identity != seat.identity {
        Some("identity")
    } else if composed.role_name != seat.role_name {
        Some("role")
    } else if !same_launch {
        Some("launch mode")
    } else if composed.created_custody != seat.created_custody
        || composed.entry_custody != seat.entry_custody
    {
        Some("custody")
    } else {
        None
    }
}

/// Ask the host's build preparer to compose `seat`, the request Core
/// lowered for a build of a store-owned identity's member
/// ([`MemberBuildPreparer::prepare_store_owned_seat`]), and set on it what
/// that build takes from the answer: the profile and model overrides and
/// the overlays that are objects of the host's process (the external
/// tools, the compaction curator and the tool dispatch admission) at every
/// build, and at the first build of the member's session (`first_build`)
/// the build inputs that author it. Everything else of `seat` stays as
/// Core lowered it, its labels included.
///
/// The host composes a copy: a refusal, and an answer that changed the
/// seat's identity, role, launch mode or custody (refused as
/// [`MemberBuildPreparationRefusal::Final`], naming what changed), leave
/// `seat` untouched.
pub(crate) async fn compose_store_owned_seat(
    preparer: &dyn MemberBuildPreparer,
    handle: MobReadHandle,
    seat: &mut SpawnMemberSpec,
    first_build: bool,
) -> Result<(), MemberBuildPreparationRefusal> {
    let mut composed = seat.clone();
    preparer
        .prepare_store_owned_seat(handle, &mut composed)
        .await?;
    if let Some(changed) = refused_seat_change(seat, &composed) {
        return Err(MemberBuildPreparationRefusal::Final {
            reason: format!(
                "the host's composition of the seat of '{}' changed its {changed}",
                seat.identity
            ),
        });
    }
    seat.override_profile = composed.override_profile;
    seat.model_override = composed.model_override;
    // The seat's profile override reads as a spawn and a Dormant seat
    // record theirs: with the model override applied to it, so the member's
    // roster entry holds the same override whichever path built it.
    if let (Some(profile), Some(model)) = (seat.override_profile.as_mut(), &seat.model_override) {
        profile.model.clone_from(model);
    }
    // The overlays that are objects of the host's process: no journal holds
    // them, and a process keeps them only for a member it built. Every
    // build takes them from the host again, so a build after a restart has
    // them as the first build had.
    seat.external_tools = composed.external_tools;
    seat.compaction_curator_override = composed.compaction_curator_override;
    seat.tool_dispatch_admission = composed.tool_dispatch_admission;
    if first_build {
        seat.context = composed.context;
        seat.additional_instructions = composed.additional_instructions;
        seat.system_prompt_override = composed.system_prompt_override;
        seat.tool_access_policy = composed.tool_access_policy;
        seat.application_tool_policy = composed.application_tool_policy;
        seat.budget_limits = composed.budget_limits;
        // The shell environment is launch authority of the session's first
        // build: no member's rebuild on its durable session takes one.
        seat.shell_env = composed.shell_env;
        seat.auth_binding = composed.auth_binding;
    }
    Ok(())
}

/// The error the spawn of `agent_identity`'s member fails with when its
/// host refused, for now or for good, to compose its seat or to prepare its
/// build: the host's reason, named for which of the two refusals it is.
pub(crate) fn spawn_refusal_error(
    agent_identity: &AgentIdentity,
    refusal: MemberBuildPreparationRefusal,
) -> crate::error::MobError {
    match refusal {
        MemberBuildPreparationRefusal::Retryable { reason } => {
            crate::error::MobError::Internal(format!(
                "the host asked to retry the build preparation of '{agent_identity}': {reason}"
            ))
        }
        MemberBuildPreparationRefusal::Final { reason } => crate::error::MobError::Internal(
            format!("the host refused to prepare the build of '{agent_identity}': {reason}"),
        ),
    }
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
        Err(refusal) => Err(spawn_refusal_error(&agent_identity, refusal)),
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
