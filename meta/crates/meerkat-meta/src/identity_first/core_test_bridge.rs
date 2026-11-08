//! Test fixture: a session bridge whose only answers are Core's member
//! reads, taken from a [`CoreTestMembers`].
//!
//! An identity's status, runtime id and session are Core's member
//! history. Tests whose subject is a view over that history (console and RPC
//! resolution, delivery routing, completion waits) bind the members they need
//! with [`bind_live`] instead of seeding a Meta continuity record. Every Core
//! write refuses, except a retire and a release: a retire ends the member,
//! and the history keeps its binding as retired; a release (Meta's retire
//! verb) leaves the member seated as it is. A bridge built with
//! [`CoreReadsBridge::with_registration_reload`] also answers Core's
//! registration reload, which leaves the member's binding as it is, one
//! built with [`CoreReadsBridge::with_host_human_input`] takes local host
//! human input on the live member's session, and one built with
//! [`CoreReadsBridge::with_deliveries`] admits a delivery to a live member on
//! its current session, as Core's delivery does.
//! [`CoreReadsBridge::fail_history_reads`] places a fault on chosen reads of
//! Core's member history, [`CoreReadsBridge::refuse_release_of`] has Core
//! refuse a member's release, retryably, as Core does while the member is
//! being woken or has work in flight, and
//! [`CoreReadsBridge::refuse_retire_of`] has Core fail a member's
//! retirement, as a store fault does. Its declared apply holds each
//! identity it declares declared from then on, keeps whatever Core holds of
//! it, and seats nothing; a complete roster unlists each identity held
//! declared that it does not list, through a retire journaled as unlisted,
//! as Core's seal journals it ([`DeclaredSet`]).
//! A double that seats members answers Core's declared apply through its
//! own seat, rotation, revival and restamp ([`apply_through_seats`]). Its member
//! lifecycle journal reads
//! answer once its members keep the journal
//! ([`CoreTestMembers::keep_journal`]);
//! [`CoreReadsBridge::refuse_journal_pages`],
//! [`CoreReadsBridge::refuse_binding_reads`] and
//! [`CoreReadsBridge::refuse_standing_reads`] have reads answer
//! `Unsupported` as a bridge without them does.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};

use meerkat_core::types::SessionId;

use super::IdentityRuntime;
use super::bridge::{
    BridgeDelivery, BridgeError, BridgeMemberReload, CoreIdentityEntry, DeclaredIdentityAnswer,
    DeclaredIdentityBuild, DeclaredRosterAnswer, DeclaredRosterRequest, DormantSeatRequest,
    DormantSeatResult, HostHumanInputError, MemberLifecycleJournalHead, MemberLifecycleJournalPage,
    MemberRuntimeBinding, MemberStanding, ResumeRejectionKind, ResumeSessionOutcome, SessionBridge,
    UnlistedIdentityAnswer,
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

/// Core rotates `identity`'s member onto `session` (`None` for a peer-only
/// member) at `generation` under the role `profile`, as Core's rotation does
/// ([`CoreTestMembers::rotate`]): the live incarnation's retirement and the
/// next one's seat name each other, and an identity no member holds is
/// seated as a fresh life after its last incarnation. Its runtime id is
/// `rt:{identity}:{generation}`.
pub(crate) fn rotate_live(
    core: &CoreTestMembers,
    identity: &AgentIdentity,
    profile: &meerkat_mob::ProfileName,
    generation: u64,
    session: Option<SessionId>,
) -> Result<AgentRuntimeId, BridgeError> {
    let agent_runtime_id = AgentRuntimeId::parse(&format!("rt:{identity}:{generation}"))
        .map_err(|error| BridgeError::Mob(error.to_string()))?;
    core.rotate(
        identity,
        &MemberRuntimeBinding {
            agent_runtime_id: agent_runtime_id.clone(),
            session_id: session,
        },
        profile,
    );
    Ok(agent_runtime_id)
}

/// A host's legacy respawn of `identity`'s live member onto `session` at
/// `generation` under the role `profile`, as Core journals it
/// ([`CoreTestMembers::respawn`]): a plain retirement, then a seat that names
/// the incarnation it retired. Its runtime id is `rt:{identity}:{generation}`.
pub(crate) fn respawn_live(
    core: &CoreTestMembers,
    identity: &AgentIdentity,
    profile: &meerkat_mob::ProfileName,
    generation: u64,
    session: Option<SessionId>,
) -> Result<AgentRuntimeId, BridgeError> {
    let agent_runtime_id = AgentRuntimeId::parse(&format!("rt:{identity}:{generation}"))
        .map_err(|error| BridgeError::Mob(error.to_string()))?;
    core.respawn(
        identity,
        &MemberRuntimeBinding {
            agent_runtime_id: agent_runtime_id.clone(),
            session_id: session,
        },
        profile,
    );
    Ok(agent_runtime_id)
}

/// The second step of [`respawn_live`], once Core retired `identity`'s live
/// incarnation in the first ([`CoreTestMembers::respawn_seat`]): the seat
/// onto `session` at `generation` under the role `profile`, naming the
/// incarnation Core retired. Its runtime id is `rt:{identity}:{generation}`.
pub(crate) fn respawn_seat_live(
    core: &CoreTestMembers,
    identity: &AgentIdentity,
    profile: &meerkat_mob::ProfileName,
    generation: u64,
    session: Option<SessionId>,
) -> Result<AgentRuntimeId, BridgeError> {
    let agent_runtime_id = AgentRuntimeId::parse(&format!("rt:{identity}:{generation}"))
        .map_err(|error| BridgeError::Mob(error.to_string()))?;
    core.respawn_seat(
        identity,
        &MemberRuntimeBinding {
            agent_runtime_id: agent_runtime_id.clone(),
            session_id: session,
        },
        profile,
    );
    Ok(agent_runtime_id)
}

/// [`respawn_live`] with no retirement of the live incarnation journaled
/// before the seat ([`CoreTestMembers::respawn_unretired`]): a journal that
/// breaks Core's rule.
pub(crate) fn respawn_live_unretired(
    core: &CoreTestMembers,
    identity: &AgentIdentity,
    profile: &meerkat_mob::ProfileName,
    generation: u64,
    session: Option<SessionId>,
) -> Result<AgentRuntimeId, BridgeError> {
    let agent_runtime_id = AgentRuntimeId::parse(&format!("rt:{identity}:{generation}"))
        .map_err(|error| BridgeError::Mob(error.to_string()))?;
    core.respawn_unretired(
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
    // Core runs the member in the spec's runtime mode override, or its
    // default mode (the double holds no profile table).
    if let Some(mode) = spec.runtime_mode_override {
        core.set_runtime_mode(&spec.identity, mode);
    }
    runtime
        .register(spec)
        .await
        .map_err(|error| BridgeError::Mob(format!("the identity registers: {error}")))?;
    Ok(runtime_id)
}

/// The identities a double's Core holds declared: each one a declared apply
/// declared, until a complete roster that does not list it unlists it
/// ([`Self::apply`]).
#[derive(Default)]
pub(crate) struct DeclaredSet(Mutex<BTreeSet<AgentIdentity>>);

impl DeclaredSet {
    /// Whether Core holds `identity` declared: an identity entry.
    pub(crate) fn contains(&self, identity: &AgentIdentity) -> bool {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(identity)
    }

    /// Hold `roster`'s identities declared and, for a complete roster,
    /// unlist each identity held declared that it does not list, as Core's
    /// seal does: `bridge` retires its live member
    /// ([`SessionBridge::retire_member`]; a failed retirement answers
    /// `Failed` and keeps the identity declared, so the next complete
    /// roster retires it again), or the declaration is removed when no
    /// member holds it.
    pub(crate) async fn apply(
        &self,
        bridge: &dyn SessionBridge,
        roster: &DeclaredRosterRequest,
    ) -> Result<BTreeMap<AgentIdentity, UnlistedIdentityAnswer>, BridgeError> {
        let listed: BTreeSet<AgentIdentity> = roster
            .identities
            .iter()
            .map(|declared| declared.spec.identity.clone())
            .collect();
        let unlisted: Vec<AgentIdentity> = {
            let mut declared = self.0.lock().unwrap_or_else(PoisonError::into_inner);
            declared.extend(listed.iter().cloned());
            if roster.complete {
                declared.difference(&listed).cloned().collect()
            } else {
                Vec::new()
            }
        };
        let mut ended = BTreeMap::new();
        for identity in unlisted {
            let outcome = match bridge.live_member(&identity).await? {
                Some(live) => match bridge.retire_member(&live.agent_runtime_id).await {
                    Ok(()) => UnlistedIdentityAnswer::Retired,
                    Err(error) => UnlistedIdentityAnswer::Failed { error },
                },
                None => UnlistedIdentityAnswer::Removed,
            };
            ended.insert(identity, outcome);
        }
        let mut declared = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        for (identity, outcome) in &ended {
            if !matches!(outcome, UnlistedIdentityAnswer::Failed { .. }) {
                declared.remove(identity);
            }
        }
        Ok(ended)
    }
}

/// A double's seat of an identity, Dormant, onto the archived session it
/// names ([`apply_through_seats`]): it builds nothing.
pub(crate) type ArchivedSeat<'a> = dyn Fn(&AgentIdentity, &DurableAgentSpec, &SessionId) -> Result<(), BridgeError>
    + Send
    + Sync
    + 'a;

/// A double's restamp of a declared role migration
/// ([`apply_through_seats`]), once the member was released: Core's rotation
/// of the identity's member onto its own session under the role the spec
/// names, its successor seated Dormant. It builds nothing (the next wake
/// builds the member) and answers the successor's generation.
pub(crate) type DormantRestamp<'a> = dyn Fn(&AgentIdentity, &DurableAgentSpec, &SessionId) -> Result<u64, BridgeError>
    + Send
    + Sync
    + 'a;

/// Core's declared apply (`MobHandle::apply_declared_roster`) as a double
/// that seats members emulates it, through the double's own steps:
/// - an identity whose member Core holds keeps it, and a first-boot
///   `Resume` request of the session the member is bound to is carried out
///   with it; a member the roster moves to another role is rotated onto a
///   fresh session when its sessions live in memory only
///   ([`SessionBridge::session_outlives_member`] answers `false`), is
///   restamped onto its own session when the activation declared that
///   migration, in Core's order: released first
///   ([`SessionBridge::release_member`]), then its successor seated Dormant
///   under the new role with nothing built (`restamp`, the double's own
///   rotation; a double with no restamp answers it `Failed`), and is held
///   under its previous role otherwise, its materialization released;
/// - a first-boot `Fresh` request starts the identity's fresh life
///   ([`SessionBridge::rotate_member`]);
/// - an identity Core holds no member of awaits its held creation when its
///   host holds it, is hosted on its first use when it is placed or bound
///   to an external runtime, and is otherwise seated Dormant, all of them in
///   one seat call ([`SessionBridge::seat_dormant_members`]): on its
///   first-boot request's session, else on its last binding's session, else
///   on a fresh one. A seat the double answers archived is revived as Core's
///   declared apply revives it, Dormant on that session with nothing built
///   (`revive`, the double's own seat onto an archived session); a double
///   with no revival answers it `Failed`;
/// - a declaration of facts only whose verdict seats or starts a member
///   answers `BuildRequired` and takes no step.
///
/// Nothing is unlisted ([`DeclaredSet::apply`] does that). Each identity's
/// answer is its own: a refused seat, rotation or revival is that
/// identity's `Failed`.
pub(crate) async fn apply_through_seats(
    bridge: &dyn SessionBridge,
    roster: &DeclaredRosterRequest,
    revive: Option<&ArchivedSeat<'_>>,
    restamp: Option<&DormantRestamp<'_>>,
) -> Result<DeclaredRosterAnswer, BridgeError> {
    let mut answer = DeclaredRosterAnswer::default();
    let mut seats = Vec::new();
    for declared in &roster.identities {
        let spec = &declared.spec;
        let identity = &spec.identity;
        let request = roster.first_boot_requests.get(identity);
        let draft = match &declared.build {
            DeclaredIdentityBuild::Composed(draft) => Some(draft),
            DeclaredIdentityBuild::FactsOnly => None,
        };
        let live = bridge.live_member(identity).await?;
        let outcome = match (live, request) {
            (_, Some(meerkat_mob::FirstBootRequest::Fresh)) => match draft {
                None => DeclaredIdentityAnswer::BuildRequired,
                Some(draft) => match bridge.rotate_member(identity, spec, draft).await {
                    Ok(session_id) => {
                        answer
                            .first_boot_requests_carried_out
                            .insert(identity.clone());
                        DeclaredIdentityAnswer::FreshLife { session_id }
                    }
                    Err(error) => DeclaredIdentityAnswer::Failed { error },
                },
            },
            (Some(live), request) => {
                let role = bridge
                    .last_member_binding(identity)
                    .await
                    .ok()
                    .flatten()
                    .map(|binding| binding.role().clone());
                match role {
                    // The roster moves the member to another role: Core
                    // restamps it onto its own session when this activation
                    // declared that migration, and holds it under its
                    // previous role otherwise; either way the previous
                    // role's materialization goes.
                    Some(role) if role != spec.profile => {
                        // A session that does not outlive its member has
                        // nothing to carry to the new role: Core rotates the
                        // member onto a fresh session, declared or not.
                        let memory_only = match live.session_id.as_ref() {
                            Some(session) => {
                                matches!(bridge.session_outlives_member(session).await, Ok(false))
                            }
                            None => false,
                        };
                        if memory_only {
                            match draft {
                                None => DeclaredIdentityAnswer::BuildRequired,
                                Some(draft) => {
                                    match bridge.rotate_member(identity, spec, draft).await {
                                        Ok(session_id) => {
                                            DeclaredIdentityAnswer::RoleRotated { session_id }
                                        }
                                        Err(error) => DeclaredIdentityAnswer::Failed { error },
                                    }
                                }
                            }
                        } else if bridge.declared_role_migration(identity).as_ref() != Some(&role) {
                            let _ = bridge.release_member(identity).await;
                            DeclaredIdentityAnswer::RoleMigrationHeld {
                                from: role,
                                to: spec.profile.clone(),
                            }
                        } else {
                            match (draft, live.session_id.as_ref()) {
                                (None, _) => DeclaredIdentityAnswer::BuildRequired,
                                (Some(_), None) => DeclaredIdentityAnswer::Failed {
                                    error: BridgeError::Mob(format!(
                                        "{identity}'s member is bound to no session to migrate on"
                                    )),
                                },
                                (Some(_), Some(session)) => {
                                    // Core's order: the release first (its
                                    // idle check), then the successor seated
                                    // on the kept session under the new role,
                                    // Dormant: nothing is built until the
                                    // wake.
                                    match bridge.release_member(identity).await {
                                        Err(error) => DeclaredIdentityAnswer::Failed { error },
                                        Ok(()) => match restamp
                                            .map(|restamp| restamp(identity, spec, session))
                                        {
                                            Some(Ok(generation)) => {
                                                DeclaredIdentityAnswer::RoleMigrated { generation }
                                            }
                                            Some(Err(error)) => {
                                                DeclaredIdentityAnswer::Failed { error }
                                            }
                                            None => DeclaredIdentityAnswer::Failed {
                                                error: BridgeError::Unsupported {
                                                    operation: "restamp a member onto its own \
                                                                session",
                                                },
                                            },
                                        },
                                    }
                                }
                            }
                        }
                    }
                    _ => {
                        if let Some(meerkat_mob::FirstBootRequest::Resume { session_id }) = request
                            && live.session_id.as_ref() == Some(session_id)
                        {
                            answer
                                .first_boot_requests_carried_out
                                .insert(identity.clone());
                        }
                        DeclaredIdentityAnswer::Kept
                    }
                }
            }
            (None, _) if declared.held_creation => DeclaredIdentityAnswer::AwaitingHeldCreation,
            (None, _) if spec.placement.is_some() || spec.binding.is_some() => {
                DeclaredIdentityAnswer::HostedOnFirstUse
            }
            (None, request) => match draft {
                None => DeclaredIdentityAnswer::BuildRequired,
                Some(draft) => {
                    let session = match request {
                        Some(meerkat_mob::FirstBootRequest::Resume { session_id }) => {
                            Some(session_id.clone())
                        }
                        _ => bridge
                            .last_member_binding(identity)
                            .await
                            .ok()
                            .flatten()
                            .and_then(|binding| binding.session_id().cloned()),
                    };
                    seats.push(DormantSeatRequest {
                        identity: identity.clone(),
                        spec: spec.clone(),
                        draft: draft.clone(),
                        session,
                        resume_from_role: None,
                    });
                    continue;
                }
            },
        };
        answer.identities.insert(identity.clone(), outcome);
    }
    if seats.is_empty() {
        return Ok(answer);
    }
    let requested: BTreeMap<AgentIdentity, DormantSeatRequest> = seats
        .iter()
        .map(|request| (request.identity.clone(), request.clone()))
        .collect();
    for (identity, result) in bridge.seat_dormant_members(seats).await? {
        let Some(request) = requested.get(&identity) else {
            continue;
        };
        let first_boot = matches!(
            roster.first_boot_requests.get(&identity),
            Some(meerkat_mob::FirstBootRequest::Resume { .. })
        );
        let outcome = match result {
            DormantSeatResult::Seated { session_id } => DeclaredIdentityAnswer::Seated {
                session_id,
                history_unavailable: None,
            },
            DormantSeatResult::AlreadySeated { .. } => DeclaredIdentityAnswer::Kept,
            DormantSeatResult::SessionArchived { session_id } => {
                match revive.map(|revive| revive(&identity, &request.spec, &session_id)) {
                    Some(Ok(())) => DeclaredIdentityAnswer::Seated {
                        session_id,
                        history_unavailable: None,
                    },
                    Some(Err(error)) => DeclaredIdentityAnswer::Failed { error },
                    None => DeclaredIdentityAnswer::Failed {
                        error: BridgeError::Unsupported {
                            operation: "revive an archived session",
                        },
                    },
                }
            }
            DormantSeatResult::Embodied { session_id } => DeclaredIdentityAnswer::Embodied {
                session_id,
                history_unavailable: None,
            },
            DormantSeatResult::Refused(error) => DeclaredIdentityAnswer::Failed { error },
        };
        if first_boot && outcome.bound_session().is_some() {
            answer
                .first_boot_requests_carried_out
                .insert(identity.clone());
        }
        answer.identities.insert(identity, outcome);
    }
    Ok(answer)
}

/// A bridge that answers Core's member reads from [`Self::core`] and commits
/// nothing but retires and releases.
#[derive(Default)]
pub(crate) struct CoreReadsBridge {
    pub(crate) core: CoreTestMembers,
    /// What Core's registration reload (`MobHandle::reload_member_registration`)
    /// answers for a live member. `None`: this bridge has no registration
    /// reload, so a reload retires the member and wakes it.
    registration_reload: Option<MemberReloadDisposition>,
    /// Set by [`Self::fail_history_reads`]. `None`: every read of Core's
    /// member history answers.
    history_fault: Mutex<Option<HistoryReadFault>>,
    /// Set by [`Self::with_host_human_input`]: this bridge takes local host
    /// human input. Otherwise it refuses it, as the trait default does.
    host_human_input: bool,
    /// The host human inputs this bridge took.
    host_human_inputs: AtomicUsize,
    /// Set by [`Self::with_deliveries`]: this bridge admits a delivery to a
    /// live member. Otherwise it refuses every delivery.
    delivers: bool,
    /// The incarnation each delivery this bridge admitted went to, in order.
    delivered: Mutex<Vec<AgentRuntimeId>>,
    /// The identities whose release Core refuses, retryably
    /// ([`Self::refuse_release_of`]). Empty: every release goes ahead.
    release_refusals: Mutex<BTreeSet<AgentIdentity>>,
    /// Each release this bridge carried out, in order.
    releases: Mutex<Vec<AgentIdentity>>,
    /// The identities whose member's retirement Core fails
    /// ([`Self::refuse_retire_of`]). Empty: every retirement goes ahead.
    retire_refusals: Mutex<BTreeSet<AgentIdentity>>,
    /// Set by [`Self::refuse_journal_pages`].
    journal_pages_refused: AtomicBool,
    /// Set by [`Self::refuse_binding_reads`].
    binding_reads_refused: AtomicBool,
    /// Set by [`Self::refuse_standing_reads`].
    standing_reads_refused: AtomicBool,
    /// Set by [`Self::refuse_entry_reads`].
    entry_reads_refused: AtomicBool,
    /// Set by [`Self::fail_entry_reads`]. `None`: every read of Core's entry
    /// of an identity answers.
    entry_fault: Mutex<Option<HistoryReadFault>>,
    /// The identities whose wake Core refuses, with the refusal's kind
    /// ([`Self::refuse_wakes_of`]).
    wake_refusals: Mutex<BTreeMap<AgentIdentity, ResumeRejectionKind>>,
    /// The identities Core holds declared.
    declared: DeclaredSet,
    /// The identities Core holds as created entries, whose records the mob's
    /// identity store owns ([`Self::hold_created`]).
    created: Mutex<BTreeSet<AgentIdentity>>,
    /// The names Core holds as removed identities ([`Self::hold_removed`]).
    removed: Mutex<BTreeSet<AgentIdentity>>,
    /// The names Core reserves as unreadable ([`Self::hold_unreadable`]).
    unreadable: Mutex<BTreeSet<AgentIdentity>>,
    /// Set while the declared apply unlists ([`DeclaredSet::apply`]): each
    /// retirement it starts is journaled for that cause, as Core's seal
    /// journals it.
    sealing: AtomicBool,
}

/// The reads of Core's member history that still answer, then the reads
/// the fault fails.
struct HistoryReadFault {
    answered: u64,
    failed: u64,
}

impl CoreReadsBridge {
    /// A bridge whose registration reload answers `disposition` for the live
    /// member in place: the member keeps its session and its generation, as
    /// Core's reload does.
    pub(crate) fn with_registration_reload(disposition: MemberReloadDisposition) -> Self {
        Self {
            registration_reload: Some(disposition),
            ..Self::default()
        }
    }

    /// A bridge that also takes local host human input on the live member's
    /// session, as Core's host-human admission does, and counts each input.
    pub(crate) fn with_host_human_input() -> Self {
        Self {
            host_human_input: true,
            ..Self::default()
        }
    }

    /// The host human inputs this bridge took.
    pub(crate) fn host_human_inputs(&self) -> usize {
        self.host_human_inputs.load(Ordering::SeqCst)
    }

    /// A bridge that also admits a delivery to a live member on its current
    /// session, as Core's delivery does, and records the incarnation each
    /// delivery went to. A delivery to an incarnation that is not its
    /// identity's live member is refused.
    pub(crate) fn with_deliveries() -> Self {
        Self {
            delivers: true,
            ..Self::default()
        }
    }

    /// The incarnation each delivery this bridge admitted went to, in order.
    pub(crate) fn delivered(&self) -> Vec<AgentRuntimeId> {
        self.delivered
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// From now on, the next `answered` reads of Core's member history
    /// answer, the `failed` reads after them fail as a store fault does, and
    /// later reads answer again: deterministic, so a test places a transient
    /// fault, or a lasting one (`u64::MAX`), on exactly the read under test.
    pub(crate) fn fail_history_reads(&self, answered: u64, failed: u64) {
        *self
            .history_fault
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(HistoryReadFault { answered, failed });
    }

    /// From now on, Core refuses the release of `identity`'s member,
    /// retryably (Core's `LifecycleOperationPending`, which the bridge
    /// answers as [`BridgeError::MemberReleaseRefused`]), as it does while
    /// the member is being woken or has work in flight. Nothing changes for
    /// the member. [`Self::allow_release_of`] ends the refusal.
    pub(crate) fn refuse_release_of(&self, identity: &AgentIdentity) {
        self.release_refusals
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(identity.clone());
    }

    /// Core releases `identity`'s member again.
    pub(crate) fn allow_release_of(&self, identity: &AgentIdentity) {
        self.release_refusals
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(identity);
    }

    /// From now on, Core fails the retirement of `identity`'s member, as a
    /// store fault does: the member stays live, and nothing changes for it.
    pub(crate) fn refuse_retire_of(&self, identity: &AgentIdentity) {
        self.retire_refusals
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(identity.clone());
    }

    /// Each release this bridge carried out, in order. A refused release is
    /// not one.
    pub(crate) fn releases(&self) -> Vec<AgentIdentity> {
        self.releases
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// From now on the pages of Core's member lifecycle journal answer
    /// `Unsupported`, while its head still reads: a bridge that implements
    /// one of the two journal reads only.
    pub(crate) fn refuse_journal_pages(&self) {
        self.journal_pages_refused.store(true, Ordering::SeqCst);
    }

    /// From now on Core's member history and an identity's last binding
    /// answer `Unsupported`: a bridge without those reads.
    pub(crate) fn refuse_binding_reads(&self) {
        self.binding_reads_refused.store(true, Ordering::SeqCst);
    }

    /// From now on Core's standing of an identity's member answers
    /// `Unsupported`: a bridge without that read.
    pub(crate) fn refuse_standing_reads(&self) {
        self.standing_reads_refused.store(true, Ordering::SeqCst);
    }

    /// From now on Core's entry of an identity answers `Unsupported`: a
    /// bridge without that read.
    pub(crate) fn refuse_entry_reads(&self) {
        self.entry_reads_refused.store(true, Ordering::SeqCst);
    }

    /// From now on Core refuses each wake of `identity`'s live member with a
    /// resume refusal of `kind`, naming the member's session, as the session
    /// bridge answers Core's refusal; until [`Self::allow_wakes_of`].
    pub(crate) fn refuse_wakes_of(&self, identity: &AgentIdentity, kind: ResumeRejectionKind) {
        self.wake_refusals
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(identity.clone(), kind);
    }

    /// Core wakes `identity`'s member again.
    pub(crate) fn allow_wakes_of(&self, identity: &AgentIdentity) {
        self.wake_refusals
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(identity);
    }

    /// Core holds `identity` declared, an identity entry, as an earlier
    /// declared apply of the mob left it (another process's roster pass).
    pub(crate) fn hold_declared(&self, identity: &AgentIdentity) {
        self.declared
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(identity.clone());
    }

    /// Core holds `identity` as a created entry: an identity created at
    /// runtime, whose record the mob's identity store owns, so no roster
    /// pass declares it.
    pub(crate) fn hold_created(&self, identity: &AgentIdentity) {
        self.created
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(identity.clone());
    }

    /// Core holds `identity` as a removed identity: its name is never an
    /// entry or a member again. Core's read names no member of it.
    pub(crate) fn hold_removed(&self, identity: &AgentIdentity) {
        self.removed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(identity.clone());
    }

    /// Core reserves `identity`'s name as one whose record it cannot read.
    /// Core's read names no member of it.
    pub(crate) fn hold_unreadable(&self, identity: &AgentIdentity) {
        self.unreadable
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(identity.clone());
    }

    /// From now on, the next `answered` reads of Core's entry of an identity
    /// answer, the `failed` reads after them fail as a read Core could not
    /// serve does, and later reads answer again (as
    /// [`Self::fail_history_reads`] for the member history).
    pub(crate) fn fail_entry_reads(&self, answered: u64, failed: u64) {
        *self
            .entry_fault
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(HistoryReadFault { answered, failed });
    }

    /// One read of Core's entry of an identity against the fault, if any.
    fn take_entry_read(&self) -> Result<(), BridgeError> {
        let mut guard = self
            .entry_fault
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let Some(fault) = guard.as_mut() else {
            return Ok(());
        };
        if fault.answered > 0 {
            fault.answered -= 1;
            return Ok(());
        }
        if fault.failed > 0 {
            fault.failed -= 1;
            return Err(BridgeError::Mob(
                "Core's entry of the identity cannot be read (test fault)".to_string(),
            ));
        }
        Ok(())
    }

    /// One read of Core's member history against the fault, if any.
    fn take_history_read(&self) -> Result<(), BridgeError> {
        let mut guard = self
            .history_fault
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let Some(fault) = guard.as_mut() else {
            return Ok(());
        };
        if fault.answered > 0 {
            fault.answered -= 1;
            return Ok(());
        }
        if fault.failed > 0 {
            fault.failed -= 1;
            return Err(BridgeError::Mob(
                "Core's member history cannot be read (test fault)".to_string(),
            ));
        }
        Ok(())
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
        let refusal = self
            .wake_refusals
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(identity)
            .copied();
        if let Some(kind) = refusal
            && let Some(live) = self.core.live_member(identity)?
            && let Some(session_id) = live.session_id
        {
            return Err(BridgeError::ResumeRejected {
                kind,
                session_id,
                detail: format!("Core refused the wake of {identity} (test refusal)"),
            });
        }
        self.core.live_member(identity)
    }

    async fn member_binding_history(
        &self,
    ) -> Result<meerkat_mob::MemberBindingHistory, BridgeError> {
        if self.binding_reads_refused.load(Ordering::SeqCst) {
            return Err(BridgeError::Unsupported {
                operation: "member_binding_history",
            });
        }
        self.take_history_read()?;
        self.core.history()
    }

    async fn last_member_binding(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<meerkat_mob::MemberBinding>, BridgeError> {
        if self.binding_reads_refused.load(Ordering::SeqCst) {
            return Err(BridgeError::Unsupported {
                operation: "last_member_binding",
            });
        }
        self.core.last_member_binding(identity)
    }

    async fn member_lifecycle_journal_head(
        &self,
    ) -> Result<MemberLifecycleJournalHead, BridgeError> {
        self.core.journal_head()
    }

    async fn member_lifecycle_journal_page(
        &self,
        after: u64,
        limit: usize,
    ) -> Result<MemberLifecycleJournalPage, BridgeError> {
        if self.journal_pages_refused.load(Ordering::SeqCst) {
            return Err(BridgeError::Unsupported {
                operation: "member_lifecycle_journal_page",
            });
        }
        self.core.journal_page(after, limit)
    }

    async fn member_standing(
        &self,
        identity: &AgentIdentity,
    ) -> Result<MemberStanding, BridgeError> {
        if self.standing_reads_refused.load(Ordering::SeqCst) {
            return Err(BridgeError::Unsupported {
                operation: "member_standing",
            });
        }
        self.core.member_standing(identity)
    }

    async fn identity_entry(
        &self,
        identity: &AgentIdentity,
    ) -> Result<CoreIdentityEntry, BridgeError> {
        if self.entry_reads_refused.load(Ordering::SeqCst) {
            return Err(BridgeError::Unsupported {
                operation: "identity_entry",
            });
        }
        self.take_entry_read()?;
        let held = |names: &Mutex<BTreeSet<AgentIdentity>>| {
            names
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .contains(identity)
        };
        if held(&self.removed) {
            return Ok(CoreIdentityEntry::removed());
        }
        if held(&self.unreadable) {
            return Ok(CoreIdentityEntry::reserved());
        }
        let created = self
            .created
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(identity);
        if created {
            return Ok(self
                .core
                .identity_entry(identity, true)
                .with_origin(meerkat_mob::IdentityEntryOrigin::Created));
        }
        Ok(self
            .core
            .identity_entry(identity, self.declared.contains(identity)))
    }

    async fn identity_management(
        &self,
        _identity: &AgentIdentity,
    ) -> Result<meerkat_mob::MemberIdentityManagement, BridgeError> {
        Ok(meerkat_mob::MemberIdentityManagement::CommandDriven)
    }

    async fn deliver_admitted(
        &self,
        runtime_id: &AgentRuntimeId,
        _delivery: BridgeDelivery,
    ) -> Result<SessionId, BridgeError> {
        if !self.delivers {
            return Err(BridgeError::Unsupported {
                operation: "deliver_admitted",
            });
        }
        // Core admits the input on the live member's current session.
        match self.core.live_incarnation(runtime_id) {
            Some((Some(session_id), _)) => {
                self.delivered
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(runtime_id.clone());
                Ok(session_id)
            }
            _ => Err(BridgeError::Mob(format!(
                "{runtime_id} is not a live member bound to a session"
            ))),
        }
    }

    async fn deliver_host_human_input(
        &self,
        runtime_id: &AgentRuntimeId,
        expected_session: &SessionId,
        _delivery: BridgeDelivery,
    ) -> Result<SessionId, BridgeError> {
        if !self.host_human_input {
            // The trait default's refusal, unchanged.
            return Err(BridgeError::HostHumanInput(
                HostHumanInputError::Unsupported(
                    "this session bridge has no local host-human input seam".to_string(),
                ),
            ));
        }
        // Core admits it on the live member's current session, or refuses.
        match self.core.live_incarnation(runtime_id) {
            Some((Some(session_id), _)) if session_id == *expected_session => {
                self.host_human_inputs.fetch_add(1, Ordering::SeqCst);
                Ok(session_id)
            }
            _ => Err(BridgeError::HostHumanInput(
                HostHumanInputError::BindingChanged,
            )),
        }
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

    /// Core retires the live incarnation `runtime_id` names, journaled as
    /// unlisted while the declared apply unlists. The retirement of a member
    /// [`Self::refuse_retire_of`] names fails, and changes nothing.
    async fn retire_member(&self, runtime_id: &AgentRuntimeId) -> Result<(), BridgeError> {
        let refused = self
            .retire_refusals
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        for identity in &refused {
            if self
                .core
                .live_member(identity)?
                .is_some_and(|live| live.agent_runtime_id == *runtime_id)
            {
                return Err(BridgeError::Mob(format!(
                    "Core could not retire {identity}'s member (test fault)"
                )));
            }
        }
        if self.sealing.load(Ordering::SeqCst) {
            self.core.start_unlisting(runtime_id);
        }
        self.core.retire(runtime_id);
        Ok(())
    }

    /// Core releases the member's materialization: it stays seated, on the
    /// same session and generation, so its binding is left as it is. A
    /// release [`Self::refuse_release_of`] names is refused, retryably, and
    /// changes nothing.
    async fn release_member(&self, identity: &AgentIdentity) -> Result<(), BridgeError> {
        if self
            .release_refusals
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(identity)
        {
            return Err(BridgeError::MemberReleaseRefused {
                identity: crate::member_comms_id::roster_member_id_for_identity(identity.as_str()),
                detail: format!(
                    "a lifecycle operation on {identity} is in progress: it is being woken or \
                     has work in flight"
                ),
            });
        }
        self.releases
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(identity.clone());
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

    /// Core's declared apply, as far as this bridge writes: each declared
    /// identity keeps whatever Core holds of it
    /// ([`DeclaredIdentityAnswer::Kept`]); this bridge seats nothing. A
    /// complete roster unlists each identity held declared that it does not
    /// list ([`DeclaredSet::apply`]), each retirement journaled for that
    /// cause, as Core's seal journals it: a retirement
    /// [`Self::refuse_retire_of`] names answers `Failed`.
    async fn apply_declared_roster(
        &self,
        roster: DeclaredRosterRequest,
    ) -> Result<DeclaredRosterAnswer, BridgeError> {
        let mut answer = DeclaredRosterAnswer::kept_for(&roster);
        // An identity that owes its host a held creation, and that Core
        // holds no member of, awaits it, as Core's verdict answers.
        for declared in &roster.identities {
            let identity = &declared.spec.identity;
            if declared.held_creation && self.core.live_member(identity)?.is_none() {
                answer.identities.insert(
                    identity.clone(),
                    DeclaredIdentityAnswer::AwaitingHeldCreation,
                );
            }
        }
        self.sealing.store(true, Ordering::SeqCst);
        let unlisted = self.declared.apply(self, &roster).await;
        self.sealing.store(false, Ordering::SeqCst);
        answer.unlisted = unlisted?;
        Ok(answer)
    }
}
