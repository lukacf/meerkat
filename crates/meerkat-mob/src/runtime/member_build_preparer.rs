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
//! the wired peers the mob's durable wiring records for the identity then
//! (none for an identity no edge names yet; the spawn wires its peers after
//! the build). A refusal there fails the spawn:
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

    /// The refusal of a composition of a store-owned identity's seat
    /// ([`MemberBuildPreparer::prepare_store_owned_seat`]) whose labels
    /// differ from the seat's: `answered` are the labels the host's build
    /// step gives the member of `seat`, the seat Core lowered. `None` when
    /// they are exactly the seat's.
    ///
    /// A store-owned member carries its row's labels and no others, and
    /// what decides on a member's labels (an access rule, an audience)
    /// reads its seat. So a label the host requires must be in the
    /// identity's row: a key the seat lacks, a key of the seat the answer
    /// lacks and a key the answer gives another value each refuse, for good
    /// ([`Self::Final`]), and the member is not built. The reason names the
    /// keys, never a value, and the remedy. Core refuses such an answer
    /// itself; a host calls this to refuse first, with the same reason, and
    /// to show it where its operators read it.
    pub fn store_owned_seat_labels(
        seat: &SpawnMemberSpec,
        answered: &std::collections::BTreeMap<String, String>,
    ) -> Option<Self> {
        let none = std::collections::BTreeMap::new();
        Self::seat_labels_differ(
            &seat.identity,
            seat.labels.as_ref().unwrap_or(&none),
            answered,
        )
    }

    /// [`Self::store_owned_seat_labels`] over the seat's labels themselves:
    /// `seated` are the labels of `identity`'s seat, `answered` the labels
    /// its host gives the build (its composition's answer, or the labels
    /// the build carries once its spawn customizer ran).
    pub(crate) fn seat_labels_differ(
        identity: &AgentIdentity,
        seated: &std::collections::BTreeMap<String, String>,
        answered: &std::collections::BTreeMap<String, String>,
    ) -> Option<Self> {
        let keys = |keys: Vec<&str>, how: &str| {
            (!keys.is_empty()).then(|| format!("{how} [{}]", keys.join(", ")))
        };
        let added = answered
            .keys()
            .filter(|key| !seated.contains_key(*key))
            .map(String::as_str)
            .collect();
        let dropped = seated
            .keys()
            .filter(|key| !answered.contains_key(*key))
            .map(String::as_str)
            .collect();
        let changed = seated
            .iter()
            .filter(|(key, value)| answered.get(*key).is_some_and(|other| other != *value))
            .map(|(key, _)| key.as_str())
            .collect();
        let differences = [
            keys(added, "added"),
            keys(dropped, "dropped"),
            keys(changed, "changed"),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        if differences.is_empty() {
            return None;
        }
        Some(Self::Final {
            reason: format!(
                "the labels the host answered for the seat of '{}' differ from the seat's \
                 (label keys {}). A store-owned identity's member carries its row's labels and \
                 no others: put a label the host requires in the identity's row, at its create \
                 or by an update, and seat its member again",
                identity,
                differences.join("; ")
            ),
        })
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
    /// Nothing else of the answer is taken. The seat's runtime mode,
    /// backend, binding, placement, first message, tool category overrides
    /// and wiring stay as Core lowered them. An answer that changed the
    /// seat's identity, role, launch mode or custody is refused: the build
    /// fails as a [`MemberBuildPreparationRefusal::Final`] refusal does.
    ///
    /// The seat's labels are its row's labels: Core lowers them from the
    /// row at a fresh spawn; at a wake or a restore they are the labels the
    /// member was last seated with, or the row's labels while the row
    /// records that the seat is owed a label restamp (an update committed
    /// while the member held work, say: `label_restamp_owed`), read from
    /// the row before this is called. A row the identity store cannot
    /// serve then ends the build for a retry as a store that cannot serve
    /// the member, and this is not called. The answer
    /// must leave them as given: what decides on a member's labels reads
    /// its seat. An answer whose labels are not exactly the seat's (a key
    /// added, a key dropped, a key given another value) is refused for
    /// good, and the member is not built
    /// ([`MemberBuildPreparationRefusal::store_owned_seat_labels`]: the
    /// reason names the label keys, never a value). Core makes the same
    /// comparison again on the labels the build carries once the host's
    /// spawn customizer ran, whether or not a preparer is configured, so a
    /// label a customizer sets on a store-owned member is refused too. A
    /// host that labels its members in its build step puts those labels in
    /// the identity's row, at its create or by an update.
    ///
    /// Where Core composes no build of the seat, this is not called and
    /// the labels are not compared: a spawn its host wrote itself under
    /// created custody (a placed or externally bound member, a held spawn,
    /// an owner-bound spawn); a live actor that is kept, which builds
    /// nothing; a reload or warm revival, which rebuilds the member with
    /// its seat's labels, the profile and tool overlay of its last build in
    /// the process and its session's own context and instructions; and an
    /// explicit resume of a stopped mob, which rebuilds each member from
    /// its seat as the host's spawn customizer makes it, labels included,
    /// and takes those labels into the rebuilt member without this
    /// comparison (a known limit).
    ///
    /// Core asks the preparer the mob was built with from the mob's start
    /// on, and waits for no composition of the host's own. A host that
    /// composes its preparer after the mob starts must therefore refuse a
    /// fresh store-owned seat, retryably, until it is composed, and must
    /// call `MobHandle::seat_owed_store_owned_identities` once it is: until
    /// that call or the mob's next start the identity stays owed its seat.
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
    /// [`Self::prepare_member_build`] would give: a retryable one
    /// ([`MemberBuildPreparationRefusal::Retryable`]) leaves the member
    /// Dormant (its session unborn if no build created it), and its next
    /// wake asks again once the retry is due; one for good
    /// ([`MemberBuildPreparationRefusal::Final`], as every label refusal
    /// is) records the member broken, and nothing asks again until it is
    /// respawned or reset, or the mob starts again.
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
/// Core lowered it.
///
/// The host composes a copy: a refusal, an answer that changed the seat's
/// identity, role, launch mode or custody, and an answer whose labels are
/// not exactly the seat's (each refused as
/// [`MemberBuildPreparationRefusal::Final`], naming what changed) leave
/// `seat` untouched. Both of Core's sites compose through here, so no
/// answer reaches a store-owned seat past these checks. The host's spawn
/// customizer runs after the composition: each site compares the labels the
/// build then carries with the seat's again
/// ([`MemberBuildPreparationRefusal::seat_labels_differ`]).
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
    // The seat's labels are its row's: an answer with other labels is
    // refused, never dropped, so the member is not seated without a label
    // its host requires.
    let unlabelled = std::collections::BTreeMap::new();
    if let Some(refusal) = MemberBuildPreparationRefusal::store_owned_seat_labels(
        seat,
        composed.labels.as_ref().unwrap_or(&unlabelled),
    ) {
        return Err(refusal);
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
