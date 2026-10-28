//! Dormant seats: an identity becomes a member without being materialized.
//!
//! [`super::MobHandle::seat_dormant_members`] takes each chunk of specs
//! through three steps:
//! 1. The actor plans each spec, reading nothing durable beyond the profile
//!    resolution a spawn also makes there. The plan is a typed answer, the
//!    spawn of a fresh spec that owes a kickoff (such a member is
//!    embodied exactly as a spawn is), or what the seat needs.
//! 2. The handle prepares each seat off the actor: the session's archived
//!    state from the session owner's resume observation (no session is
//!    built; on a host store the observation loads the stored session's
//!    rows once per seat, while a session that does not exist yet reads
//!    nothing), and the endpoint from the session's stored comms key. A
//!    seat builds nothing: a fresh seat's session (under its name-based id)
//!    does not exist yet, and MobMachine records it unborn. The member's
//!    first wake creates it through the spawn provision path, with the
//!    runtime registration and the host's registration hook any member
//!    build has, and applies the build inputs the seat carried
//!    ([`SeatBuild`]).
//! 3. The actor commits the chunk in one step: on one prepared
//!    authority, every identity's `AuthorizeSpawnProfile`, `BeginSpawnExec`
//!    and `CommitDormantSeat` as one batch (when the machine refuses an
//!    identity, the batch is undone and the chunk applied again one identity
//!    at a time, so the refused one is answered alone), then one runtime
//!    obligation scan and the seated identities' `PlanBootMaterialization`
//!    as one more batch, then the edges the definition declares for the
//!    members it seats, as a spawn of each would be and, for an orchestrator
//!    member, to the members that auto-wire to it (their `WireMembers`
//!    inputs), then the `MemberSpawned` rows and the edges' `MembersWired`
//!    rows in one `append_batch` before the base-checked commit: a seat is
//!    never durable without its declared edges.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

use meerkat_core::comms::TrustedPeerDescriptor;
use meerkat_core::service::MobIncarnationId;
use meerkat_core::types::SessionId;
use meerkat_core::{AgentToolDispatcher, ToolCategoryOverride};
use serde::{Deserialize, Serialize};

use crate::error::{MobError, SessionResumeUnavailableReason};
use crate::ids::{AgentIdentity, Generation, MobId, ProfileName};
use crate::machines::mob_machine as mob_dsl;

/// Specs one seat step commits at most: one `append_batch` per chunk.
pub(crate) const DORMANT_SEAT_CHUNK: usize = 256;

/// The build inputs a seat's spec names that a spawn applies at its build.
/// A seat builds nothing, so it carries them to the member's first build,
/// which applies them once, as a spawn of the seat's spec would.
///
/// The seat's `MemberSpawned` row journals them (an internal field of the
/// stored row, absent from rows written before it), so a restart before the
/// first build keeps them. The born row clears them: after the first build
/// the member's session holds them, as any member's does. The spec's role,
/// labels, runtime mode, continuity and profile and model overrides (its
/// provider params included) are the seat's roster facts already.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct SeatBuildInputs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) context: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) additional_instructions: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) system_prompt_override: Option<super::handle::SpawnSystemPromptOverride>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) budget_limits: Option<meerkat_core::BudgetLimits>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) tool_access_policy: Option<meerkat_core::ops::ToolAccessPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) application_tool_policy: Option<meerkat_core::ApplicationToolPolicyBinding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) auth_binding: Option<meerkat_core::AuthBindingRef>,
    /// The spec's web search override (`ToolCategoryOverrides::web_search`):
    /// the one tool category the seat's profile override does not hold.
    #[serde(default, skip_serializing_if = "web_search_inherits")]
    pub(crate) web_search: ToolCategoryOverride,
}

fn web_search_inherits(web_search: &ToolCategoryOverride) -> bool {
    *web_search == ToolCategoryOverride::Inherit
}

impl SeatBuildInputs {
    /// Whether the seat carries none of these inputs.
    pub(crate) fn is_empty(&self) -> bool {
        self == &Self::default()
    }

    /// Overlay what `later` names: a later seat of the same never-born
    /// session sets the inputs its spec names and keeps the rest of the
    /// earlier seat's.
    pub(crate) fn overlay(&mut self, later: &Self) {
        if let Some(context) = &later.context {
            self.context = Some(context.clone());
        }
        if let Some(instructions) = &later.additional_instructions {
            self.additional_instructions = Some(instructions.clone());
        }
        if let Some(prompt) = &later.system_prompt_override {
            self.system_prompt_override = Some(prompt.clone());
        }
        if let Some(limits) = &later.budget_limits {
            self.budget_limits = Some(limits.clone());
        }
        if let Some(policy) = &later.tool_access_policy {
            self.tool_access_policy = Some(policy.clone());
        }
        if let Some(policy) = &later.application_tool_policy {
            self.application_tool_policy = Some(policy.clone());
        }
        if let Some(binding) = &later.auth_binding {
            self.auth_binding = Some(binding.clone());
        }
        if !web_search_inherits(&later.web_search) {
            self.web_search = later.web_search;
        }
    }
}

/// The process-local build inputs of a seat's spec: objects of the process
/// that seated the member, and its shell environment, which may hold
/// secrets. They are never journaled. That process keeps them for the
/// member's first build there; after a restart the spawn customizer
/// supplies them, as it does for any member's rebuild.
#[derive(Clone, Default)]
pub(crate) struct SeatLocalBuildInputs {
    pub(crate) external_tools: Option<Arc<dyn AgentToolDispatcher>>,
    pub(crate) compaction_curator_override: Option<Arc<dyn meerkat_core::CompactionCurator>>,
    pub(crate) tool_dispatch_admission: Option<Arc<dyn meerkat_core::ToolDispatchAdmission>>,
    pub(crate) shell_env: Option<HashMap<String, String>>,
}

impl SeatLocalBuildInputs {
    /// Whether the seat carries none of these inputs.
    pub(crate) fn is_empty(&self) -> bool {
        self.external_tools.is_none()
            && self.compaction_curator_override.is_none()
            && self.tool_dispatch_admission.is_none()
            && self.shell_env.is_none()
    }
}

impl std::fmt::Debug for SeatLocalBuildInputs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SeatLocalBuildInputs")
            .field("external_tools", &self.external_tools.is_some())
            .field(
                "compaction_curator_override",
                &self.compaction_curator_override.is_some(),
            )
            .field(
                "tool_dispatch_admission",
                &self.tool_dispatch_admission.is_some(),
            )
            .field("shell_env", &self.shell_env.is_some())
            .finish()
    }
}

/// What a seat carries to the first build of its unborn session: its spec's
/// build inputs ([`SeatBuildInputs`]) and, in the process that seated it, the
/// process-local ones ([`SeatLocalBuildInputs`]). The roster keeps it per
/// session until the session's born row ([`crate::roster::Roster::seat_build`]),
/// also across a retirement of its member before the first build, so a later
/// seat of the same never-born session builds from it. A rebuild of the
/// member sets it on its resume request before the spawn customizer is asked
/// ([`super::fork_build::rebuild_seat_resume_spec`]), as a spawn's caller
/// sets these inputs on its spec.
#[derive(Debug, Clone, Default)]
pub(crate) struct SeatBuild {
    pub(crate) inputs: SeatBuildInputs,
    pub(crate) local: SeatLocalBuildInputs,
}

impl SeatBuild {
    /// Take the inputs a seat carries out of `spec`.
    pub(in crate::runtime) fn take_from(spec: &mut super::handle::SpawnMemberSpec) -> Self {
        Self {
            inputs: SeatBuildInputs {
                context: spec.context.take(),
                additional_instructions: spec.additional_instructions.take(),
                system_prompt_override: spec.system_prompt_override.take(),
                budget_limits: spec.budget_limits.take(),
                tool_access_policy: spec.tool_access_policy.take(),
                application_tool_policy: spec.application_tool_policy.take(),
                auth_binding: spec.auth_binding.take(),
                web_search: spec.tool_category_overrides.web_search,
            },
            local: SeatLocalBuildInputs {
                external_tools: spec.external_tools.take(),
                compaction_curator_override: spec.compaction_curator_override.take(),
                tool_dispatch_admission: spec.tool_dispatch_admission.take(),
                shell_env: spec.shell_env.take(),
            },
        }
    }

    /// Set these inputs on `spec`, a rebuild's resume request, as a spawn's
    /// caller sets them on its spec. The spawn customizer is asked after, as
    /// it is at a spawn: it may append to the instructions, compose over the
    /// overlay, or refuse.
    pub(crate) fn apply_to(&self, spec: &mut super::handle::SpawnMemberSpec) {
        let inputs = &self.inputs;
        if let Some(context) = &inputs.context {
            spec.context = Some(context.clone());
        }
        if let Some(instructions) = &inputs.additional_instructions {
            spec.additional_instructions = Some(instructions.clone());
        }
        if let Some(prompt) = &inputs.system_prompt_override {
            spec.system_prompt_override = Some(prompt.clone());
        }
        if let Some(limits) = &inputs.budget_limits {
            spec.budget_limits = Some(limits.clone());
        }
        if let Some(policy) = &inputs.tool_access_policy {
            spec.tool_access_policy = Some(policy.clone());
        }
        if let Some(policy) = &inputs.application_tool_policy {
            spec.application_tool_policy = Some(policy.clone());
        }
        if let Some(binding) = &inputs.auth_binding {
            spec.auth_binding = Some(binding.clone());
        }
        if !web_search_inherits(&inputs.web_search) {
            spec.tool_category_overrides.web_search = inputs.web_search;
        }
        let local = &self.local;
        if let Some(tools) = &local.external_tools {
            spec.external_tools = Some(Arc::clone(tools));
        }
        if let Some(curator) = &local.compaction_curator_override {
            spec.compaction_curator_override = Some(Arc::clone(curator));
        }
        if let Some(admission) = &local.tool_dispatch_admission {
            spec.tool_dispatch_admission = Some(Arc::clone(admission));
        }
        if let Some(env) = &local.shell_env {
            spec.shell_env = Some(env.clone());
        }
    }
}

/// Seats a chunk prepares at once off the actor.
pub(crate) const DORMANT_SEAT_PREPARE_CONCURRENCY: usize = 16;

/// What [`super::MobHandle::seat_dormant_members`] did with one spec.
#[derive(Debug)]
#[non_exhaustive]
pub enum DormantSeatOutcome {
    /// Seated Dormant on `session_id`: a member, wireable and reachable, not
    /// materialized. Its first delivery wakes it (or MobMachine's boot
    /// verdict does at once, for a member with runtime work).
    Seated {
        agent_identity: AgentIdentity,
        session_id: SessionId,
    },
    /// Already seated on exactly this session (an earlier seat of the same
    /// spec): nothing was written.
    AlreadySeated {
        agent_identity: AgentIdentity,
        session_id: SessionId,
    },
    /// The spec's session is archived. Nothing was written: an archived
    /// session is never bound.
    SessionArchived {
        agent_identity: AgentIdentity,
        session_id: SessionId,
    },
    /// A fresh spec that owes a kickoff (an `AutonomousHost` member, or a
    /// member with an initial message) is not seated: it was spawned through
    /// the ordinary spawn path, live, and its kickoff runs once.
    Embodied {
        agent_identity: AgentIdentity,
        session_id: SessionId,
    },
    /// Refused for this spec alone: nothing of it was written.
    Refused {
        agent_identity: AgentIdentity,
        error: MobError,
    },
}

impl DormantSeatOutcome {
    /// The error a single seat's outcome answers its caller with, when the
    /// seat did not happen.
    pub(crate) fn into_unseated_error(self) -> MobError {
        match self {
            Self::Refused { error, .. } => error,
            Self::SessionArchived { session_id, .. } => MobError::SessionUnavailableForResume {
                session_id,
                reason: SessionResumeUnavailableReason::ArchivedNotRevivable,
                runtime_state: None,
                verdict: None,
            },
            Self::Seated { agent_identity, .. }
            | Self::AlreadySeated { agent_identity, .. }
            | Self::Embodied { agent_identity, .. } => MobError::Internal(format!(
                "'{agent_identity}' was seated, but its seat was answered as not seated"
            )),
        }
    }

    /// The identity this outcome answers for.
    pub fn agent_identity(&self) -> &AgentIdentity {
        match self {
            Self::Seated { agent_identity, .. }
            | Self::AlreadySeated { agent_identity, .. }
            | Self::SessionArchived { agent_identity, .. }
            | Self::Embodied { agent_identity, .. }
            | Self::Refused { agent_identity, .. } => agent_identity,
        }
    }
}

/// Where a seat takes its member's peer endpoint from: the comms transport
/// the host's members run on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MemberSeatEndpointSource {
    /// Inproc comms with session-scoped identities: a member's key lives
    /// under `<identity_root>/<session id>`, and its endpoint is the inproc
    /// address of its comms name, as its woken runtime reports it.
    SessionScopedInproc { identity_root: PathBuf },
    /// Network comms (Tcp or Uds): the key belongs to the realm, not to the
    /// session, so a seat records no endpoint. The member is seated with its
    /// endpoint owed (`EndpointPending`), and its boot wake registers it.
    Network,
}

/// The name-based id of a fresh seat's session: the same mob, mob
/// incarnation, identity and generation always name the same session, so a
/// seat re-driven after a crash finds the session its earlier attempt
/// created, and no ownerless session can exist.
pub(crate) fn fresh_seat_session_id(
    mob_id: &MobId,
    incarnation: MobIncarnationId,
    identity: &AgentIdentity,
    generation: Generation,
) -> SessionId {
    let incarnation = incarnation.to_string();
    let generation = generation.get().to_string();
    let mut name = b"meerkat.mob.fresh-seat.v1".to_vec();
    for component in [
        mob_id.as_str(),
        incarnation.as_str(),
        identity.as_str(),
        generation.as_str(),
    ] {
        let bytes = component.as_bytes();
        name.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
        name.extend_from_slice(bytes);
    }
    SessionId::from_uuid(uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, &name))
}

/// How a seat binds its session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::runtime) enum DormantSeatLaunch {
    /// A session under the fresh seat's name-based id, created by the
    /// member's first wake (unborn until then).
    Fresh,
    /// The session the spec names: an existing one, or one a seat named and
    /// no build created (its member was retired before its first wake),
    /// which is seated unborn again.
    Resume,
}

/// One spec of a seat chunk, as the actor planned it.
pub(in crate::runtime) enum DormantSeatPlanEntry {
    /// Answered at planning: nothing of it is written.
    Done(DormantSeatOutcome),
    /// Seated on exactly this session already: the commit step clears the
    /// identity's host preparation and plans it, writing nothing.
    AlreadySeated {
        agent_identity: AgentIdentity,
        session_id: SessionId,
    },
    /// A fresh spec that owes a kickoff: spawned through the ordinary spawn
    /// path, on its name-based session.
    Embody(Box<super::handle::SpawnMemberSpec>),
    /// A seat to prepare and commit.
    Seat(Box<DormantSeatCandidate>),
}

/// The plan of one seat chunk.
pub(in crate::runtime) struct DormantSeatPlan {
    pub(in crate::runtime) entries: Vec<DormantSeatPlanEntry>,
    pub(in crate::runtime) endpoint_source: Option<MemberSeatEndpointSource>,
}

/// What one seat's commit needs, as the actor planned it.
pub(in crate::runtime) struct DormantSeatCandidate {
    pub(in crate::runtime) agent_identity: AgentIdentity,
    pub(in crate::runtime) profile_name: ProfileName,
    pub(in crate::runtime) launch: DormantSeatLaunch,
    pub(in crate::runtime) session_id: SessionId,
    pub(in crate::runtime) generation: Generation,
    /// Whether a seat or a rotation named the seat's session unborn and no
    /// build has created it (MobMachine's `unborn_seat_sessions`, read at the
    /// plan): its member was retired before its first wake. Such a session
    /// is seated unborn again, whatever the store shows.
    pub(in crate::runtime) session_never_born: bool,
    /// Whether a journal row of this mob recorded the comms endpoint (the
    /// public key) of the seat's session, read at the plan: peers trusted
    /// that key, so its key pair must exist. A session no row recorded (one
    /// imported without its key pair) gets a fresh one, as its first build
    /// would.
    pub(in crate::runtime) session_endpoint_recorded: bool,
    /// Whether no build has created the seat's session: a fresh launch whose
    /// session the preparation found absent, or a session no build created
    /// ([`Self::session_never_born`]). Nothing is created at the seat;
    /// MobMachine records the member's session unborn, and the member's
    /// first wake creates it (or adopts the one an interrupted create left)
    /// through the spawn provision path, with the host's registration hook.
    /// Set by [`prepare_dormant_seat`].
    pub(in crate::runtime) session_unborn: bool,
    pub(in crate::runtime) runtime_mode: crate::MobRuntimeMode,
    pub(in crate::runtime) labels: BTreeMap<String, String>,
    pub(in crate::runtime) continuity_intent: super::SpawnContinuityIntent,
    /// The spec's creation origin: a host-root seat is a root creation.
    pub(in crate::runtime) creation_origin: crate::member_creation::MemberCreationOrigin,
    pub(in crate::runtime) effective_profile_override: Option<crate::profile::Profile>,
    pub(in crate::runtime) effective_model_override: Option<String>,
    /// The generated `AuthorizeSpawnProfile` input and the authorization it
    /// must answer with.
    pub(in crate::runtime) authorize_input: mob_dsl::MobMachineInput,
    pub(in crate::runtime) authorized:
        super::spawn_profile_authority::AuthorizedSpawnProfileMaterial,
    pub(in crate::runtime) observations: super::actor::SpawnExecObservations,
    /// Whether the spec owes a kickoff: a fresh AutonomousHost life (its one
    /// kickoff turn) or an initial message. A seat that owes one is
    /// embodied instead (never seated); a rotation records it as an owed
    /// kickoff that wakes the next incarnation once.
    pub(in crate::runtime) kickoff_owed: bool,
    /// The owed kickoff's prompt when the spec carries an initial message;
    /// `None` runs the role's fallback prompt, as a spawn does.
    pub(in crate::runtime) kickoff_prompt: Option<meerkat_core::types::ContentInput>,
    /// The objective the owed kickoff resolves under (the spec's, or the
    /// default a spawn uses).
    pub(in crate::runtime) kickoff_objective_id: meerkat_core::interaction::ObjectiveId,
    /// What the seat carries to its session's first build: the spec's build
    /// inputs, taken out of it at the plan. Journaled and kept only for an
    /// unborn session; a seat whose session exists binds what that session
    /// holds.
    pub(in crate::runtime) seat_build: SeatBuild,
}

impl DormantSeatCandidate {
    /// The build inputs the seat's `MemberSpawned` row journals: those the
    /// first build of its unborn session applies, when it names any.
    pub(in crate::runtime) fn unborn_seat_build_inputs(&self) -> Option<SeatBuildInputs> {
        (self.session_unborn && !self.seat_build.inputs.is_empty())
            .then(|| self.seat_build.inputs.clone())
    }
}

/// The plan of a rotation (`rotate_member`).
pub(in crate::runtime) enum MemberRotationPlan {
    /// The next incarnation's dormant seat.
    Seat {
        candidate: Box<DormantSeatCandidate>,
        /// Whether the identity is seated now: its current incarnation is
        /// then retired and the next one seated in the retirement's terminal
        /// step. Otherwise (its last retirement already completed) the
        /// rotation is one fresh seat.
        seated: bool,
        endpoint_source: Option<MemberSeatEndpointSource>,
    },
    /// An identity that is not seated, whose spec owes a kickoff: spawned
    /// through the ordinary spawn path on its name-based session, as a seat
    /// of that spec is.
    Embody(Box<super::handle::SpawnMemberSpec>),
}

/// One entry of a seat chunk's commit, in the chunk's order.
pub(in crate::runtime) enum DormantSeatCommitEntry {
    /// Seated on exactly this session already.
    AlreadySeated {
        agent_identity: AgentIdentity,
        session_id: SessionId,
    },
    /// Spawned live by the ordinary spawn path: the commit step clears its
    /// host preparation, which the spawn does not.
    Embodied {
        agent_identity: AgentIdentity,
        session_id: SessionId,
    },
    /// A prepared seat: its session exists, or is unborn (created by the
    /// member's first wake), and `endpoint` is its stored-key endpoint
    /// (`None` under network comms).
    Seat {
        candidate: Box<DormantSeatCandidate>,
        endpoint: Option<TrustedPeerDescriptor>,
    },
}

/// Prepare one seat off the actor: its session's archived state, whether a
/// fresh seat's session is unborn, and its endpoint. Nothing is built or
/// created. A spec this refuses is answered with its typed outcome, and
/// nothing of it is written.
pub(in crate::runtime) async fn prepare_dormant_seat(
    session_service: &dyn super::session_service::MobSessionService,
    endpoint_source: Option<&MemberSeatEndpointSource>,
    mob_id: &MobId,
    mut candidate: Box<DormantSeatCandidate>,
) -> Result<DormantSeatCommitEntry, Box<DormantSeatOutcome>> {
    let agent_identity = candidate.agent_identity.clone();
    let session_id = candidate.session_id.clone();
    let refused = |error: MobError| {
        Box::new(DormantSeatOutcome::Refused {
            agent_identity: agent_identity.clone(),
            error,
        })
    };
    let Some(endpoint_source) = endpoint_source else {
        return Err(refused(MobError::WiringError(format!(
            "seating '{agent_identity}' dormant needs the members' comms identity source \
             (MobBuilder::with_member_comms_identity_root or with_network_member_comms)"
        ))));
    };
    // The session owner's resume observation: authority, catalog lifecycle
    // and runtime lifecycle in one snapshot. No session is built. On a host
    // store the runtime store answers it by loading the stored session's
    // rows (the authority is one of its row entries): one load per seat of
    // an existing session, none for a session that does not exist yet.
    let lifecycle = session_service
        .observe_session_resume_authority(&session_id)
        .await
        .map_err(|error| refused(MobError::from(error)))?
        .lifecycle();
    candidate.session_unborn = match lifecycle {
        super::session_service::SessionResumeLifecycle::Archived { .. } => {
            return Err(Box::new(DormantSeatOutcome::SessionArchived {
                agent_identity: agent_identity.clone(),
                session_id,
            }));
        }
        super::session_service::SessionResumeLifecycle::ContradictoryDurableAuthority {
            runtime_state,
            ..
        } => {
            return Err(refused(MobError::SessionUnavailableForResume {
                session_id,
                reason: SessionResumeUnavailableReason::ContradictoryDurableAuthority,
                runtime_state: runtime_state.map(|state| state.to_string()),
                verdict: None,
            }));
        }
        // The session exists: a resume's, or one an earlier build created
        // under this fresh seat's name-based id, and its wake resumes it. One
        // a seat named and no build created (a create that ended before its
        // born row) is still unborn: the member's first wake adopts it.
        super::session_service::SessionResumeLifecycle::Active { .. } => {
            candidate.session_never_born
        }
        super::session_service::SessionResumeLifecycle::NoCurrentDurableAuthority => {
            match candidate.launch {
                // Nothing is created here: the member's first wake creates
                // the session, through the spawn provision path.
                DormantSeatLaunch::Fresh => true,
                // A session a seat named and no build created (its member
                // was retired before its first wake) is seated unborn again.
                DormantSeatLaunch::Resume if candidate.session_never_born => true,
                // Any other absent session was lost.
                DormantSeatLaunch::Resume => {
                    return Err(refused(MobError::SessionUnavailableForResume {
                        session_id,
                        reason: SessionResumeUnavailableReason::Absent,
                        runtime_state: None,
                        verdict: None,
                    }));
                }
            }
        }
    };
    let endpoint = match endpoint_source {
        MemberSeatEndpointSource::Network => None,
        MemberSeatEndpointSource::SessionScopedInproc { identity_root } => {
            // The member's first build loads this key; its wake checks that
            // the endpoint it built is the one recorded here. A key pair is
            // minted where none exists unless this mob recorded the session's
            // endpoint: a fresh seat, an unborn session and a built session
            // whose key no row recorded (an imported one) get the key pair
            // their first build would load.
            let (public_key, minted) = stored_member_comms_key(
                identity_root,
                &session_id,
                candidate.launch == DormantSeatLaunch::Fresh
                    || candidate.session_unborn
                    || !candidate.session_endpoint_recorded,
            )
            .await
            .map_err(&refused)?;
            // A resume seat names a session some build may already have
            // given a key pair. Minting one is right for a session imported
            // without its key pair, and the one silent re-key when the root
            // is not the one the session's builds use: make it visible.
            if minted && candidate.launch == DormantSeatLaunch::Resume {
                tracing::warn!(
                    mob_id = %mob_id,
                    agent_identity = %agent_identity,
                    %session_id,
                    identity_root = %identity_root.display(),
                    "a resume seat minted a comms key pair for a session that had none under the \
                     configured member comms identity root; if the session's builds keep its key \
                     pair under another root, the root is misconfigured and the member's peers \
                     will not know the new key"
                );
            }
            Some(
                inproc_member_endpoint(
                    mob_id,
                    &candidate.profile_name,
                    &agent_identity,
                    public_key,
                )
                .map_err(&refused)?,
            )
        }
    };
    Ok(DormantSeatCommitEntry::Seat {
        candidate,
        endpoint,
    })
}

/// The public key of a member session's stored comms identity, and whether
/// this read minted it: one local key-store read, no session load. With
/// `mint`, an identity that does not exist yet is generated where the
/// session's builds load it, as a build does, and one that exists is loaded
/// the same way (`load_or_generate`, which waits a moment for a key another
/// seat of the session is still writing). Without it the identity must
/// exist: this mob recorded the session's endpoint, peers trust that key,
/// and a missing one means the configured root is not the session service's
/// or the key was lost, so the seat is refused typed instead of minting a
/// key those peers would not know.
#[cfg(not(target_arch = "wasm32"))]
async fn stored_member_comms_key(
    identity_root: &std::path::Path,
    session_id: &SessionId,
    mint: bool,
) -> Result<([u8; 32], bool), MobError> {
    let dir = identity_root.join(session_id.to_string());
    let exists = tokio::fs::try_exists(dir.join("identity.key"))
        .await
        .map_err(|error| {
            MobError::WiringError(format!(
                "the comms identity of session '{session_id}' could not be read: {error}"
            ))
        })?;
    let loaded = if mint {
        meerkat_comms::Keypair::load_or_generate(&dir).await
    } else if exists {
        meerkat_comms::Keypair::load(&dir).await
    } else {
        return Err(MobError::WiringError(format!(
            "session '{session_id}' has no comms identity under the configured member \
             comms identity root"
        )));
    };
    let keypair = loaded.map_err(|error| {
        MobError::WiringError(format!(
            "the comms identity of session '{session_id}' could not be read: {error}"
        ))
    })?;
    Ok((*keypair.public_key().as_bytes(), !exists))
}

#[cfg(target_arch = "wasm32")]
async fn stored_member_comms_key(
    _identity_root: &std::path::Path,
    session_id: &SessionId,
    _mint: bool,
) -> Result<([u8; 32], bool), MobError> {
    Err(MobError::WiringError(format!(
        "a browser runtime keeps no durable comms identity for session '{session_id}'"
    )))
}

/// The inproc endpoint a member's woken runtime reports: its comms name, the
/// inproc address of that name, and its session's key.
pub(in crate::runtime) fn inproc_member_endpoint(
    mob_id: &MobId,
    role: &ProfileName,
    identity: &AgentIdentity,
    public_key: [u8; 32],
) -> Result<TrustedPeerDescriptor, MobError> {
    let comms_name =
        super::actor::render_member_comms_name(mob_id.as_str(), role.as_str(), identity.as_str())?;
    let peer_id = meerkat_comms::PubKey::new(public_key).to_peer_id();
    TrustedPeerDescriptor::unsigned_with_pubkey(
        comms_name.clone(),
        peer_id.to_string(),
        public_key,
        format!("inproc://{comms_name}"),
    )
    .map_err(|error| {
        MobError::WiringError(format!(
            "invalid dormant seat endpoint for '{identity}': {error}"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_seat_session_ids_are_name_based() {
        let mob = MobId::from("mob");
        let incarnation = MobIncarnationId::mint();
        let identity = AgentIdentity::from("worker");
        let first = fresh_seat_session_id(&mob, incarnation, &identity, Generation::new(3));
        assert_eq!(
            first,
            fresh_seat_session_id(&mob, incarnation, &identity, Generation::new(3)),
            "a re-driven seat derives the same session"
        );
        assert_ne!(
            first,
            fresh_seat_session_id(&mob, incarnation, &identity, Generation::new(4)),
            "the next generation is a new session"
        );
        assert_ne!(
            first,
            fresh_seat_session_id(
                &mob,
                incarnation,
                &AgentIdentity::from("worker2"),
                Generation::new(3)
            ),
        );
        assert_ne!(
            first,
            fresh_seat_session_id(
                &mob,
                MobIncarnationId::mint(),
                &identity,
                Generation::new(3)
            ),
            "a new mob incarnation never reuses a session"
        );
    }

    #[test]
    fn inproc_member_endpoint_is_the_woken_runtime_endpoint() {
        let key = meerkat_comms::Keypair::generate().public_key();
        let endpoint = inproc_member_endpoint(
            &MobId::from("mob"),
            &ProfileName::from("worker"),
            &AgentIdentity::from("w1"),
            *key.as_bytes(),
        )
        .expect("endpoint");
        assert_eq!(endpoint.peer_id, key.to_peer_id());
        assert_eq!(endpoint.pubkey, *key.as_bytes());
        assert_eq!(
            endpoint.address.to_string(),
            format!("inproc://{}", endpoint.name)
        );
        assert_eq!(
            mob_dsl::MemberPeerEndpoint::from(&endpoint).transport,
            mob_dsl::PeerTransportKind::Inproc
        );
    }
}
