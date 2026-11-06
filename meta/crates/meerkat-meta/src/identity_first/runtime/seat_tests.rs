//! Rows for the seats a roster's declaration in Core's mob makes and the
//! first-boot carry-out, driven through the roster pass
//! ([`IdentityRuntime::declare_roster_pass`]), the doors (a delivery, a
//! materialization, a warm, which declare an identity alone first) and the
//! lifecycle verbs: every roster identity Core does not hold is seated
//! Dormant by one declaration of the roster, with nothing materialized; a
//! pending first-boot row is carried out by its identity's declaration; a
//! rotation a crash interrupted is Core's to finish; and the lifecycle
//! projection tells each seat and retirement once. The double answers
//! Core's declared apply through its own seat, rotation, revival and
//! restamp ([`apply_through_seats`]), and unlists as Core's seal does
//! ([`DeclaredSet`]).

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use meerkat_core::types::HandlingMode;
use tokio::sync::oneshot;

use super::*;
use crate::identity_first::bridge::{
    BridgeDelivery, BridgeError, DormantSeatRequest, DormantSeatResult, MemberLifecycleJournalHead,
    MemberLifecycleJournalPage, MemberRuntimeBinding, MemberStanding, ResumeRejectionKind,
    ResumeSessionOutcome, SessionBridge,
};
use crate::identity_first::core_test_bridge::{
    CoreReadsBridge, DeclaredSet, apply_through_seats, bind_live, register_live, respawn_live,
    respawn_live_unretired, respawn_seat_live, rotate_live,
};
use crate::identity_first::core_test_members::CoreTestMembers;
use crate::identity_first::first_boot_manifest::FirstBootManifest;
use crate::identity_first::local_lease::LocalLeaseProvider;
use crate::identity_first::local_store::LocalContinuityStore;
use crate::identity_first::runtime::RetiredMemberObserver;
use crate::identity_first::{
    AgentAddressability, AgentBuildContext, AgentMemoryConfig, AgentMemoryRuntimeInjector,
    AgentRuntimeId, DurabilityPolicy, IdentityFirstRuntimeContext, IdentityRuntimeConfig,
    MutableRosterProvider, RetiredIdentityMember, RosterProvider, SessionSnapshot,
};
use crate::memory::distiller::{
    CompactionDiscardSource, DiscardEntry, DistillerClientHandle, DistillerConfig, DistillerEngine,
    DistillerError, DistillerProfile, TranscriptSlice, TranscriptSource,
};
use crate::memory::steward::{
    StewardClientHandle, StewardConfig, StewardEngine, StewardError, StewardProfile,
};

fn lock<T>(mutex: &StdMutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn identity(name: &str) -> AgentIdentity {
    AgentIdentity::parse(name).expect("valid identity")
}

fn spec(identity: &AgentIdentity) -> DurableAgentSpec {
    DurableAgentSpec {
        identity: identity.clone(),
        profile: meerkat_mob::ProfileName::from("personal"),
        addressability: AgentAddressability::Addressable,
        display_name: None,
        labels: std::collections::BTreeMap::new(),
        context: None,
        additional_instructions: Vec::new(),
        initial_message: None,
        runtime_mode_override: None,
        backend: None,
        binding: None,
        placement: None,
    }
}

/// One seat call to Core: the identities and requested sessions it carried.
type SeatCall = Vec<(AgentIdentity, Option<SessionId>)>;

/// A Core that seats, rotates and releases members over a
/// [`CoreTestMembers`] history, and counts every other Core step a seat
/// must never take: a create, a resume, a wake. It keeps Core's residency
/// too, and counts each member build it makes: a revival, and a wake or a
/// delivery that finds its member not materialized.
///
/// It answers as Core does where a row depends on it: an identity's first
/// generation is zero; a seat of an identity Core holds is `AlreadySeated`
/// on the session it is bound to and refused on any other; a placed spec's
/// create is denied while its host is not bound.
#[derive(Default)]
struct SeatCore {
    core: CoreTestMembers,
    /// Core refuses this identity's seat, typed.
    refused: Option<AgentIdentity>,
    /// Core holds no durable copy of these sessions: a seat that names one
    /// is refused as Core refuses it (absent).
    unheld: Vec<SessionId>,
    /// Core's read of its broken members fails.
    broken_read_fails: bool,
    /// Core admits a delivery to a live member onto the member's session
    /// and records it; otherwise it delivers nothing.
    admits_deliveries: bool,
    /// The state directory whose first-boot manifest each rotation reads, to
    /// record whether the manifest still listed the identity's row.
    manifest_dir: Option<std::path::PathBuf>,
    /// Each seat call, as the identities and requested sessions it carried.
    seat_calls: StdMutex<Vec<SeatCall>>,
    /// The session each fresh seat bound, by identity. Core derives a fresh
    /// seat's session from the identity and its generation, so a repeated
    /// fresh seat names the session the first one bound.
    fresh_seats: StdMutex<BTreeMap<AgentIdentity, SessionId>>,
    rotations: StdMutex<Vec<AgentIdentity>>,
    /// Each rotation, with whether the manifest in `manifest_dir` still
    /// listed the identity's first-boot row when Core was asked.
    rotations_listed: StdMutex<Vec<(AgentIdentity, bool)>>,
    releases: StdMutex<Vec<AgentIdentity>>,
    /// The member each admitted delivery went to.
    delivered: StdMutex<Vec<AgentRuntimeId>>,
    history_reads: AtomicUsize,
    creates: AtomicUsize,
    resumes: AtomicUsize,
    /// Every restamp of a declared role migration Core's declared apply made
    /// (`apply_declared_roster`): the successor seated Dormant, nothing built.
    restamps: AtomicUsize,
    wakes: AtomicUsize,
    /// Core keeps these sessions archived: a seat that names one, for an
    /// identity Core does not hold, answers `SessionArchived` and writes
    /// nothing.
    archived: Vec<SessionId>,
    /// Core's resume, and a declared role migration's restamp, bind the
    /// identity on the named session at its next generation instead of
    /// refusing it.
    resumes_onto_session: bool,
    /// The role migration this activation declares, by identity.
    migrations: BTreeMap<AgentIdentity, meerkat_mob::ProfileName>,
    /// The identities Core keeps host-pending: a delivery to one is refused
    /// until the declaration that carries its first-boot request out, a
    /// revival of its archived session, or its rotation, ends the mark.
    host_pending: StdMutex<BTreeSet<AgentIdentity>>,
    /// Core's member history read fails.
    history_read_fails: std::sync::atomic::AtomicBool,
    /// Core refuses each rotation before any of it starts.
    refuses_rotation: bool,
    /// Holds the next seat call inside Core: it signals that the call
    /// reached Core, then waits for the release.
    seat_gate: StdMutex<Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>>,
    /// Core refuses each wake with this verdict.
    wake_refusal: StdMutex<Option<meerkat_mob::MemberWakeRefusal>>,
    /// Core answers each wake with this typed resume refusal on this
    /// session, as the bridge reads Core's Broken verdict on a member it
    /// holds.
    wake_rejection: StdMutex<Option<(ResumeRejectionKind, SessionId)>>,
    /// Core's session service keeps sessions in memory only: no session
    /// outlives its member. Otherwise the double cannot say.
    sessions_in_memory: bool,
    /// Core's rotation fails with this error after it started: Core
    /// journaled the start of the live incarnation's retirement, and ended
    /// neither the retirement nor the rotation.
    rotation_failure: StdMutex<Option<BridgeError>>,
    /// The bridge answers the next rotation with this error before it asks
    /// Core for anything (a spec it could not lower, say): nothing reaches
    /// Core.
    refuse_before_core: StdMutex<Option<BridgeError>>,
    /// Core carries the next rotation out, and the bridge then answers it
    /// with this error: a wrapper that ran Core's rotation and answered an
    /// error after it.
    answers_after_rotating: StdMutex<Option<BridgeError>>,
    /// Core compensates the next rotations' seats: the seat is journaled,
    /// then the spawn's membership step fails and Core closes the
    /// incarnation with a retirement that records no facts.
    compensate_next_rotations: AtomicUsize,
    /// Core starts the next retires and fails each before its end: its
    /// start is journaled, and the member stays seated, retiring.
    retire_starts_then_fails: AtomicUsize,
    /// The launch's member auth binding refuses these identities' member
    /// builds wherever Core builds one outside a wake, as the bridge reads
    /// the gateway's build check off Core's typed refusal: the live spawn
    /// of a fresh seat that owes a kickoff, and of the rotation of an
    /// identity Core does not hold that owes one (each refused before
    /// anything is seated), and a resume ([`SessionBridge::resume_session`]).
    /// A seat (an archived session's revival in Core's declared apply
    /// included), a declared role migration's restamp or a rotation that
    /// owes no kickoff builds nothing, so it is not refused here.
    route_refused: Vec<AgentIdentity>,
    /// Every rotation asked of Core, refused or carried out.
    rotation_calls: AtomicUsize,
    /// The sessions Core holds unborn: a seat named them and no wake has
    /// created them (`MemberBindingHistory::session_unborn`).
    unborn: StdMutex<BTreeSet<SessionId>>,
    /// The next reads of the head of Core's member lifecycle journal fail,
    /// as a store fault does.
    journal_head_failures: AtomicUsize,
    /// Core refuses the next rotations before any of them starts.
    refuse_next_rotations: AtomicUsize,
    /// Core refuses the next retires before any of them starts.
    refuse_next_retires: AtomicUsize,
    /// Holds the next retire inside Core once Core retired the member: it
    /// signals that, then waits for the release.
    retire_gate: StdMutex<Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>>,
    /// Every read of one identity's binding (`last_member_binding`). Core
    /// answers one by folding its whole member history, so each costs what
    /// a read of the history costs.
    binding_reads: AtomicUsize,
    /// The pages of Core's member lifecycle journal read after a cursor
    /// below this one fail, as a store fault does; zero fails none.
    journal_pages_fail_before: AtomicU64,
    /// Holds the next rotation inside Core before any of it starts: it
    /// signals that, then waits for the release.
    rotate_gate: StdMutex<Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>>,
    /// Holds the bridge's answer to the next rotation once Core carried the
    /// rotation out: it signals that, then waits for the release.
    rotated_gate: StdMutex<Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>>,
    /// The incarnations Core holds materialized in this process, as Core's
    /// residency does: a revival (a resume) builds its member, a release or a
    /// retirement drops it, and a wake or a delivery that finds its seated
    /// member not materialized builds it.
    materialized: StdMutex<BTreeSet<AgentRuntimeId>>,
    /// Every member build Core made: each revival, and each wake or delivery
    /// that found its member not materialized.
    builds: AtomicUsize,
    /// The identities Core holds declared.
    declared: DeclaredSet,
    /// Every declared apply asked of Core (`apply_declared_roster`): one per
    /// declaration of a roster or of a door's one identity.
    apply_calls: AtomicUsize,
    /// Set while Core's seal of a complete roster retires the entries it no
    /// longer lists: each retirement starts for that cause.
    sealing: AtomicBool,
}

/// Takes one from `counter` when it is above zero: whether it was.
fn take_one(counter: &AtomicUsize) -> bool {
    counter
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
            left.checked_sub(1)
        })
        .is_ok()
}

/// Whether a fresh life of `spec` owes a kickoff (an initial message, or an
/// autonomous host's first turn), which Core spawns live instead of seating
/// it Dormant.
fn owes_kickoff(spec: &DurableAgentSpec) -> bool {
    spec.initial_message.is_some()
        || matches!(
            spec.runtime_mode_override,
            Some(meerkat_mob::MobRuntimeMode::AutonomousHost)
        )
}

/// The bridge's answer for `identity`'s member build at `step` that the
/// launch's member auth binding refuses.
fn route_refusal(identity: &AgentIdentity, step: &str) -> BridgeError {
    BridgeError::MemberRouteRefused {
        kind: "member_auth_binding_uncarried",
        detail: format!(
            "{step} of {identity}: session error: member_auth_binding_uncarried: this member's \
             build enables web search where the provider's own search does not serve it"
        ),
    }
}

impl SeatCore {
    fn session_of(&self, identity: &AgentIdentity) -> Option<SessionId> {
        self.core
            .last_member_binding(identity)
            .expect("Core's history reads")
            .and_then(|binding| binding.session_id().cloned())
    }

    fn generation_of(&self, identity: &AgentIdentity) -> Option<u64> {
        self.core
            .last_member_binding(identity)
            .expect("Core's history reads")
            .map(|binding| binding.generation().get())
    }

    /// The generation Core seats `identity` at next: zero for an identity
    /// Core never held, then one past its last binding.
    fn next_generation(&self, identity: &AgentIdentity) -> Result<u64, BridgeError> {
        Ok(self
            .core
            .last_member_binding(identity)?
            .map_or(0, |binding| binding.generation().get() + 1))
    }

    /// Core materializes the incarnation `runtime_id` for a wake or a
    /// delivery: it builds the member when it is not materialized yet.
    fn materialize(&self, runtime_id: &AgentRuntimeId) {
        if lock(&self.materialized).insert(runtime_id.clone()) {
            self.builds.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// The incarnation `runtime_id` is no longer materialized here: Core
    /// released or retired it.
    fn dematerialize(&self, runtime_id: &AgentRuntimeId) {
        lock(&self.materialized).remove(runtime_id);
    }
}

#[async_trait::async_trait]
impl SessionBridge for SeatCore {
    async fn create_session(
        &self,
        identity: &AgentIdentity,
        spec: &DurableAgentSpec,
        _draft: &AgentBuildDraft,
    ) -> Result<MemberRuntimeBinding, BridgeError> {
        self.creates.fetch_add(1, Ordering::SeqCst);
        // A placed spec's create reaches Core's spawn ladder, which denies a
        // host this mob has not bound before anything is built.
        if spec.placement.is_some() {
            return Err(BridgeError::Mob(format!(
                "spawn of {identity}: placement denied: {HOST_NOT_BOUND}"
            )));
        }
        Err(BridgeError::Mob(format!(
            "{identity}: a seat creates no member"
        )))
    }

    async fn resume_session(
        &self,
        identity: &AgentIdentity,
        spec: &DurableAgentSpec,
        _draft: &AgentBuildDraft,
        session_id: &SessionId,
        _resume_from_role: Option<&meerkat_mob::ProfileName>,
    ) -> Result<ResumeSessionOutcome, BridgeError> {
        self.resumes.fetch_add(1, Ordering::SeqCst);
        if self.route_refused.contains(identity) {
            return Err(route_refusal(identity, "resume"));
        }
        // Core holds no durable copy of the session: Core refuses the resume
        // typed (absent), and binds nothing in its place.
        if self.unheld.contains(session_id) {
            return Err(BridgeError::ResumeRejected {
                kind: ResumeRejectionKind::Unavailable {
                    reason: meerkat_mob::error::SessionResumeUnavailableReason::Absent,
                },
                session_id: session_id.clone(),
                detail: "Core holds no durable copy of the session".to_string(),
            });
        }
        if !self.resumes_onto_session {
            return Err(BridgeError::Mob(format!(
                "{identity}: a seat resumes no session"
            )));
        }
        // Core's revival or restamp: the identity is bound on the session at
        // its next generation. A spawn ends no host preparation, and Core
        // refuses the plain retirement of an occupant while its host
        // preparation is pending: the retirement never reaches its terminal.
        let live = self.core.live_member(identity)?;
        if let Some(live) = live.as_ref() {
            if lock(&self.host_pending).contains(identity) {
                return Err(BridgeError::Mob(format!(
                    "{identity}: Core refuses retiring the occupant while its host preparation \
                     is pending"
                )));
            }
            self.dematerialize(&live.agent_runtime_id);
        }
        let generation = self.next_generation(identity)?;
        // The restamp of a live member on its own session is Core's rotation
        // onto the same session, whose retirement and seat name each other.
        // A resume of a live member onto another session retires the
        // occupant and then spawns, two rows that name no rotation; so is the
        // revival of one no member holds a spawn.
        let restamp = live
            .as_ref()
            .is_some_and(|live| live.session_id.as_ref() == Some(session_id));
        if !restamp && let Some(live) = live.as_ref() {
            self.core.retire(&live.agent_runtime_id);
        }
        let agent_runtime_id = if restamp {
            rotate_live(
                &self.core,
                identity,
                &spec.profile,
                generation,
                Some(session_id.clone()),
            )?
        } else {
            bind_live(
                &self.core,
                identity,
                &spec.profile,
                generation,
                Some(session_id.clone()),
            )?
        };
        // A resume is a spawn: Core builds the member, live.
        self.materialize(&agent_runtime_id);
        Ok(ResumeSessionOutcome::Resumed {
            session_id: session_id.clone(),
            agent_runtime_id,
        })
    }

    fn declared_role_migration(
        &self,
        identity: &AgentIdentity,
    ) -> Option<meerkat_mob::ProfileName> {
        self.migrations.get(identity).cloned()
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
        self.wakes.fetch_add(1, Ordering::SeqCst);
        if let Some(refusal) = *lock(&self.wake_refusal) {
            return Err(BridgeError::MemberWakeRefused {
                identity: member_id(identity),
                refusal,
                detail: format!("Core refused the wake: {}", refusal.as_str()),
            });
        }
        if let Some((kind, session_id)) = lock(&self.wake_rejection).clone() {
            return Err(BridgeError::ResumeRejected {
                kind,
                session_id,
                detail: "Core holds the member Broken".to_string(),
            });
        }
        let live = self.core.live_member(identity)?;
        if let Some(live) = live.as_ref() {
            self.materialize(&live.agent_runtime_id);
        }
        Ok(live)
    }

    async fn member_binding_history(
        &self,
    ) -> Result<meerkat_mob::MemberBindingHistory, BridgeError> {
        self.history_reads.fetch_add(1, Ordering::SeqCst);
        if self.history_read_fails.load(Ordering::SeqCst) {
            return Err(BridgeError::Mob(
                "Core's member history did not read".to_string(),
            ));
        }
        let unborn = lock(&self.unborn).iter().cloned().collect::<Vec<_>>();
        Ok(self.core.history()?.with_unborn_sessions(unborn))
    }

    async fn last_member_binding(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<meerkat_mob::MemberBinding>, BridgeError> {
        self.binding_reads.fetch_add(1, Ordering::SeqCst);
        self.core.last_member_binding(identity)
    }

    async fn identity_management(
        &self,
        _identity: &AgentIdentity,
    ) -> Result<meerkat_mob::MemberIdentityManagement, BridgeError> {
        Ok(meerkat_mob::MemberIdentityManagement::CommandDriven)
    }

    async fn durable_identity_members(
        &self,
    ) -> Result<Vec<meerkat_mob::MemberBinding>, BridgeError> {
        self.core.durable_identity_members()
    }

    async fn broken_members(
        &self,
    ) -> Result<std::collections::BTreeMap<meerkat_mob::AgentIdentity, String>, BridgeError> {
        if self.broken_read_fails {
            return Err(BridgeError::Mob(
                "Core's member list did not read".to_string(),
            ));
        }
        Ok(std::collections::BTreeMap::new())
    }

    async fn seat_dormant_members(
        &self,
        requests: Vec<DormantSeatRequest>,
    ) -> Result<Vec<(AgentIdentity, DormantSeatResult)>, BridgeError> {
        let gate = lock(&self.seat_gate).take();
        if let Some((reached, release)) = gate {
            let _ = reached.send(());
            let _ = release.await;
        }
        let mut call = Vec::with_capacity(requests.len());
        let mut results = Vec::with_capacity(requests.len());
        for request in requests {
            call.push((request.identity.clone(), request.session.clone()));
            // Core spawns a fresh seat that owes a kickoff live, and the
            // gateway's build check refuses that spawn.
            if self.route_refused.contains(&request.identity)
                && request.session.is_none()
                && owes_kickoff(&request.spec)
            {
                results.push((
                    request.identity.clone(),
                    DormantSeatResult::Refused(route_refusal(&request.identity, "seat")),
                ));
                continue;
            }
            if self.refused.as_ref() == Some(&request.identity) {
                results.push((
                    request.identity.clone(),
                    DormantSeatResult::Refused(BridgeError::Mob(format!(
                        "Core refused the seat of {}",
                        request.identity
                    ))),
                ));
                continue;
            }
            if let Some(session_id) = request
                .session
                .as_ref()
                .filter(|session| self.unheld.contains(session))
            {
                results.push((
                    request.identity.clone(),
                    DormantSeatResult::Refused(BridgeError::ResumeRejected {
                        kind: ResumeRejectionKind::Unavailable {
                            reason: meerkat_mob::error::SessionResumeUnavailableReason::Absent,
                        },
                        session_id: session_id.clone(),
                        detail: format!("Core holds no durable copy of session {session_id}"),
                    }),
                ));
                continue;
            }
            if let Some(session_id) = request
                .session
                .as_ref()
                .filter(|session| self.archived.contains(session))
                && self.core.live_member(&request.identity)?.is_none()
            {
                results.push((
                    request.identity.clone(),
                    DormantSeatResult::SessionArchived {
                        session_id: session_id.clone(),
                    },
                ));
                continue;
            }
            if let Some(live) = self.core.live_member(&request.identity)? {
                // Core holds the identity seated: a seat of the session it
                // is bound to is `AlreadySeated`, and any other seat is
                // refused (Core's `MemberAlreadyExists`), its binding left
                // as it is.
                let seated_on = match &request.session {
                    Some(session) => Some(session.clone()),
                    None => lock(&self.fresh_seats).get(&request.identity).cloned(),
                };
                let result = match (live.session_id, seated_on) {
                    (Some(bound), Some(seated_on)) if bound == seated_on => {
                        lock(&self.host_pending).remove(&request.identity);
                        DormantSeatResult::AlreadySeated { session_id: bound }
                    }
                    _ => DormantSeatResult::Refused(BridgeError::Mob(format!(
                        "member '{}' already exists: Core holds it seated on another session",
                        request.identity
                    ))),
                };
                results.push((request.identity.clone(), result));
                continue;
            }
            let session_id = request.session.clone().unwrap_or_else(SessionId::new);
            if request.session.is_none() {
                lock(&self.fresh_seats).insert(request.identity.clone(), session_id.clone());
            }
            let generation = self.next_generation(&request.identity)?;
            bind_live(
                &self.core,
                &request.identity,
                &request.spec.profile,
                generation,
                Some(session_id.clone()),
            )?;
            lock(&self.host_pending).remove(&request.identity);
            results.push((
                request.identity.clone(),
                DormantSeatResult::Seated { session_id },
            ));
        }
        lock(&self.seat_calls).push(call);
        Ok(results)
    }

    async fn rotate_member(
        &self,
        identity: &AgentIdentity,
        spec: &DurableAgentSpec,
        _draft: &AgentBuildDraft,
    ) -> Result<SessionId, BridgeError> {
        self.rotation_calls.fetch_add(1, Ordering::SeqCst);
        if let Some(refusal) = lock(&self.refuse_before_core).take() {
            return Err(refusal);
        }
        let gate = lock(&self.rotate_gate).take();
        if let Some((reached, release)) = gate {
            let _ = reached.send(());
            let _ = release.await;
        }
        // The rotation of an identity Core does not hold that owes a
        // kickoff is Core's live spawn of its fresh life, which the
        // gateway's build check refuses.
        if self.route_refused.contains(identity)
            && self.core.live_member(identity)?.is_none()
            && owes_kickoff(spec)
        {
            return Err(route_refusal(identity, "rotate_member"));
        }
        if self.refuses_rotation {
            return Err(BridgeError::RotationRefused {
                identity: crate::member_comms_id::roster_member_id_for_identity(identity.as_str()),
                detail: "the mob is not running".to_string(),
            });
        }
        if take_one(&self.refuse_next_rotations) {
            return Err(BridgeError::RotationRefused {
                identity: crate::member_comms_id::roster_member_id_for_identity(identity.as_str()),
                detail: "the member's other lifecycle operation has not ended".to_string(),
            });
        }
        if let Some(failure) = lock(&self.rotation_failure).take() {
            if let Some(live) = self.core.live_member(identity)? {
                self.core.start_retirement(
                    &live.agent_runtime_id,
                    Some(meerkat_mob::RotationTarget::FreshSession),
                );
            }
            return Err(failure);
        }
        let listed = self.manifest_dir.as_deref().is_some_and(|dir| {
            FirstBootManifest::read(dir)
                .ok()
                .flatten()
                .is_some_and(|manifest| {
                    manifest.rows().iter().any(|row| row.identity() == identity)
                })
        });
        lock(&self.rotations).push(identity.clone());
        lock(&self.rotations_listed).push((identity.clone(), listed));
        if let Some(live) = self.core.live_member(identity)? {
            self.dematerialize(&live.agent_runtime_id);
        }
        let generation = self.next_generation(identity)?;
        let session_id = SessionId::new();
        // One step retires the live incarnation and seats the next one, and
        // its journal rows name each other, as Core's rotation's do.
        rotate_live(
            &self.core,
            identity,
            &spec.profile,
            generation,
            Some(session_id.clone()),
        )?;
        lock(&self.host_pending).remove(identity);
        if take_one(&self.compensate_next_rotations) {
            self.core.compensate(identity);
            return Err(BridgeError::Mob(format!(
                "{identity}: the fresh life's membership step failed, and Core compensated its \
                 spawn"
            )));
        }
        if let Some(error) = lock(&self.answers_after_rotating).take() {
            return Err(error);
        }
        let gate = lock(&self.rotated_gate).take();
        if let Some((rotated, release)) = gate {
            let _ = rotated.send(());
            let _ = release.await;
        }
        Ok(session_id)
    }

    async fn release_member(&self, identity: &AgentIdentity) -> Result<(), BridgeError> {
        lock(&self.releases).push(identity.clone());
        // The member stays seated at its generation, not materialized.
        if let Some(live) = self.core.live_member(identity)? {
            self.dematerialize(&live.agent_runtime_id);
        }
        Ok(())
    }

    async fn session_outlives_member(&self, _session_id: &SessionId) -> Result<bool, BridgeError> {
        if self.sessions_in_memory {
            return Ok(false);
        }
        Err(BridgeError::Unsupported {
            operation: "session_outlives_member",
        })
    }

    async fn deliver_admitted(
        &self,
        runtime_id: &AgentRuntimeId,
        _delivery: BridgeDelivery,
    ) -> Result<SessionId, BridgeError> {
        if !self.admits_deliveries {
            return Err(BridgeError::Mob(format!(
                "{runtime_id}: a seat delivers nothing"
            )));
        }
        let pending = lock(&self.host_pending).iter().any(|identity| {
            self.core
                .live_member(identity)
                .ok()
                .flatten()
                .is_some_and(|live| &live.agent_runtime_id == runtime_id)
        });
        if pending {
            return Err(BridgeError::Mob(format!(
                "{runtime_id}: Core refuses the wake while the host preparation is pending"
            )));
        }
        let session = self
            .core
            .live_incarnation(runtime_id)
            .and_then(|(session, _)| session)
            .ok_or_else(|| BridgeError::Mob(format!("{runtime_id} is no live member")))?;
        // Core's dispatch materializes a member that is not live before it
        // delivers.
        self.materialize(runtime_id);
        lock(&self.delivered).push(runtime_id.clone());
        Ok(session)
    }

    async fn checkpoint_session(
        &self,
        runtime_id: &AgentRuntimeId,
        _session_id: &SessionId,
    ) -> Result<SessionSnapshot, BridgeError> {
        Err(BridgeError::Mob(format!(
            "{runtime_id}: a seat checkpoints nothing"
        )))
    }

    async fn retire_member(&self, runtime_id: &AgentRuntimeId) -> Result<(), BridgeError> {
        if take_one(&self.refuse_next_retires) {
            return Err(BridgeError::Mob(format!(
                "{runtime_id}: Core refused the retire before any of it started"
            )));
        }
        // Core's seal starts the retirement of an entry the complete roster
        // no longer lists for that cause.
        if self.sealing.load(Ordering::SeqCst) {
            self.core.start_unlisting(runtime_id);
        }
        if take_one(&self.retire_starts_then_fails) {
            self.core.start_retirement(runtime_id, None);
            return Err(BridgeError::Mob(format!(
                "{runtime_id}: Core started the retirement and failed before its end"
            )));
        }
        self.core.retire(runtime_id);
        self.dematerialize(runtime_id);
        let gate = lock(&self.retire_gate).take();
        if let Some((reached, release)) = gate {
            let _ = reached.send(());
            let _ = release.await;
        }
        Ok(())
    }

    async fn member_lifecycle_journal_head(
        &self,
    ) -> Result<MemberLifecycleJournalHead, BridgeError> {
        if take_one(&self.journal_head_failures) {
            return Err(BridgeError::Mob(
                "the head of Core's member lifecycle journal did not read".to_string(),
            ));
        }
        self.core.journal_head()
    }

    fn mob_incarnation(&self) -> Option<String> {
        Some(crate::identity_first::core_test_members::JOURNAL_INCARNATION.to_string())
    }

    async fn member_standing(
        &self,
        identity: &AgentIdentity,
    ) -> Result<MemberStanding, BridgeError> {
        self.core.member_standing(identity)
    }

    async fn member_lifecycle_journal_page(
        &self,
        after: u64,
        limit: usize,
    ) -> Result<MemberLifecycleJournalPage, BridgeError> {
        if after < self.journal_pages_fail_before.load(Ordering::SeqCst) {
            return Err(BridgeError::Mob(format!(
                "Core's member lifecycle journal after {after} did not read"
            )));
        }
        self.core.journal_page(after, limit)
    }

    /// Core's declared apply through this double's own seat, rotation and
    /// role restamp ([`apply_through_seats`]). Core's declaration step
    /// carries out the first-boot `Resume` request of an identity it holds
    /// seated, ending its host preparation before the identity's verdict is
    /// realized (a role restamp's retirement of the occupant included). Core
    /// revives an archived session as a Dormant seat at the identity's next
    /// generation, which ends its host preparation and builds nothing, and
    /// unlists as its seal does ([`DeclaredSet`]), each retirement it starts
    /// journaled for that cause.
    async fn apply_declared_roster(
        &self,
        roster: crate::identity_first::DeclaredRosterRequest,
    ) -> Result<crate::identity_first::DeclaredRosterAnswer, crate::identity_first::BridgeError>
    {
        self.apply_calls.fetch_add(1, Ordering::SeqCst);
        let mut carried_out = BTreeSet::new();
        for (identity, request) in &roster.first_boot_requests {
            if matches!(request, meerkat_mob::FirstBootRequest::Resume { .. })
                && self.core.live_member(identity)?.is_some()
            {
                lock(&self.host_pending).remove(identity);
                carried_out.insert(identity.clone());
            }
        }
        let revive = |identity: &AgentIdentity, spec: &DurableAgentSpec, session: &SessionId| {
            let generation = self.next_generation(identity)?;
            bind_live(
                &self.core,
                identity,
                &spec.profile,
                generation,
                Some(session.clone()),
            )?;
            lock(&self.host_pending).remove(identity);
            Ok(())
        };
        // A declared role migration, once the member was released: Core's
        // rotation onto its own session under the new role, the successor
        // seated Dormant with nothing built (the next wake builds it). Core
        // refuses the occupant's retirement while its host preparation is
        // pending.
        let restamp = |identity: &AgentIdentity, spec: &DurableAgentSpec, session: &SessionId| {
            self.restamps.fetch_add(1, Ordering::SeqCst);
            if !self.resumes_onto_session {
                return Err(BridgeError::Mob(format!(
                    "{identity}: a seat restamps no member"
                )));
            }
            if lock(&self.host_pending).contains(identity) {
                return Err(BridgeError::Mob(format!(
                    "{identity}: Core refuses retiring the occupant while its host preparation \
                     is pending"
                )));
            }
            let generation = self.next_generation(identity)?;
            rotate_live(
                &self.core,
                identity,
                &spec.profile,
                generation,
                Some(session.clone()),
            )?;
            Ok(generation)
        };
        let mut answer = apply_through_seats(self, &roster, Some(&revive), Some(&restamp)).await?;
        answer.first_boot_requests_carried_out.extend(carried_out);
        // Core ends an identity's host preparation in the step that carries
        // its first-boot request out.
        for identity in &answer.first_boot_requests_carried_out {
            lock(&self.host_pending).remove(identity);
        }
        self.sealing.store(true, Ordering::SeqCst);
        let unlisted = self.declared.apply(self, &roster).await;
        self.sealing.store(false, Ordering::SeqCst);
        answer.unlisted = unlisted?;
        Ok(answer)
    }
}

/// The roster pass's declaration of `specs`, a partial roster, as a
/// bootstrap or a refresh makes it ([`IdentityRuntime::declare_roster_pass`]):
/// each listed identity is registered whatever Core answers, and one hosted
/// on its first use is left for it. Each identity's restore outcome.
async fn roster_pass(
    runtime: &IdentityRuntime,
    specs: &[DurableAgentSpec],
    customizer: Option<&dyn AgentCustomizer>,
) -> BTreeMap<AgentIdentity, crate::identity_first::orchestrator::RestoreOutcome> {
    runtime
        .declare_roster_pass(
            specs,
            false,
            true,
            customizer,
            HostedIdentities::LeaveForFirstUse,
        )
        .await
        .outcomes
}

/// The typed refusal a roster pass answered for an identity, if it refused.
fn pass_refusal(
    outcome: &crate::identity_first::orchestrator::RestoreOutcome,
) -> Option<&IdentityRuntimeError> {
    match outcome {
        crate::identity_first::orchestrator::RestoreOutcome::Refused(error) => Some(error.as_ref()),
        _ => None,
    }
}

/// The member binding a roster pass answered for an identity: `Created` on
/// a session the declaration started, `Resumed` on one Core held, with
/// whether it was created. `None` for a dormant or refused outcome.
fn pass_binding(
    outcome: &crate::identity_first::orchestrator::RestoreOutcome,
) -> Option<(bool, &MemberRuntimeBinding)> {
    match outcome {
        crate::identity_first::orchestrator::RestoreOutcome::Created { binding, .. } => {
            Some((true, binding))
        }
        crate::identity_first::orchestrator::RestoreOutcome::Resumed { binding, .. } => {
            Some((false, binding))
        }
        _ => None,
    }
}

/// A runtime whose Core is `core`, over the first-boot manifest in
/// `state_dir` when there is one.
fn runtime_over(state_dir: Option<&std::path::Path>, core: &Arc<SeatCore>) -> Arc<IdentityRuntime> {
    let runtime = Arc::new(IdentityRuntime::new(IdentityRuntimeConfig {
        continuity_store: Arc::new(LocalContinuityStore::in_memory().expect("store")),
        lease_provider: Arc::new(LocalLeaseProvider::new()),
        runtime_instance_id: "seat-rows".to_string(),
        has_runtime_store: true,
        durability_policy: DurabilityPolicy::SyncWriteThrough,
        bridge: Some(core.clone()),
        default_timeout: None,
    }));
    runtime.set_first_boot_state_dir(state_dir.map(std::path::Path::to_path_buf));
    runtime
}

async fn register_all(runtime: &IdentityRuntime, specs: &[DurableAgentSpec]) {
    for spec in specs {
        runtime
            .register(spec.clone())
            .await
            .expect("the identity registers");
    }
}

/// How Core denies the create of a member placed on a host this mob has not
/// bound.
const HOST_NOT_BOUND: &str = "host is not bound in this mob";

/// `count` first-boot `Resume` rows, each on a session of its own, in
/// identity order.
fn resume_rows(count: usize) -> Vec<FirstBootRow> {
    (0..count)
        .map(|index| FirstBootRow::Resume {
            identity: identity(&format!("personal:row-{index:02}")),
            session: SessionId::new(),
        })
        .collect()
}

/// The specs of `rows`' identities, in order.
fn row_specs(rows: &[FirstBootRow]) -> Vec<DurableAgentSpec> {
    rows.iter().map(|row| spec(row.identity())).collect()
}

/// The rows the manifest in `state_dir` lists, `None` once it is gone.
fn manifest_rows(state_dir: &std::path::Path) -> Option<Vec<FirstBootRow>> {
    FirstBootManifest::read(state_dir)
        .expect("the manifest reads")
        .map(FirstBootManifest::into_rows)
}

/// The identities whose rows the bootstrap status lists as pending.
fn pending(runtime: &IdentityRuntime) -> Vec<AgentIdentity> {
    runtime
        .identity_bootstrap_status()
        .first_boot_pending
        .into_iter()
        .map(|row| row.identity)
        .collect()
}

/// A hook that holds a declaration right before its manifest rewrite: it
/// says when the declaration got there, then waits for `released` (forever,
/// when that never resolves).
fn hold_at_rewrite(
    runtime: &IdentityRuntime,
    released: impl std::future::Future<Output = ()> + Send + 'static,
) -> oneshot::Receiver<()> {
    let (at_rewrite, reached_rewrite) = oneshot::channel::<()>();
    runtime.set_first_boot_batch_hook(Box::new(move || {
        Box::pin(async move {
            let _ = at_rewrite.send(());
            released.await;
        })
    }));
    reached_rewrite
}

/// A roster pass declares its roster together, never an identity at a time:
/// one declared apply of the whole roster's facts, and, since Core's
/// verdict seats each identity, one more of the whole roster with the builds
/// Core asked for. Core seats every identity it does not hold Dormant, in
/// one step, and the pass reads Core's member history a fixed number of
/// times, two at most, however many identities the roster lists: no read
/// per identity. Nothing is created, resumed or woken: each identity is a
/// member bound to a fresh session Core minted, and a delivery materializes
/// it.
#[tokio::test]
async fn x3_r15_a_compile_seats_every_unheld_identity_in_one_core_call_and_wakes_none() {
    let core = Arc::new(SeatCore::default());
    let runtime = runtime_over(None, &core);
    let specs = ["personal:alice", "personal:bob", "personal:carol"]
        .map(|name| spec(&identity(name)))
        .to_vec();

    let outcomes = roster_pass(&runtime, &specs, None).await;

    for spec in &specs {
        let (created, binding) = pass_binding(&outcomes[&spec.identity]).unwrap_or_else(|| {
            panic!(
                "{} is seated: {:?}",
                spec.identity, outcomes[&spec.identity]
            )
        });
        assert!(created, "{}: a fresh seat", spec.identity);
        assert_eq!(binding.session_id, core.session_of(&spec.identity));
    }
    assert_eq!(
        core.apply_calls.load(Ordering::SeqCst),
        2,
        "the whole roster's facts, then the whole roster with its builds"
    );
    let calls = lock(&core.seat_calls).clone();
    assert_eq!(calls.len(), 1, "Core seats the whole roster in one step");
    assert_eq!(
        calls[0],
        specs
            .iter()
            .map(|spec| (spec.identity.clone(), None))
            .collect::<Vec<_>>()
    );
    let history_reads = core.history_reads.load(Ordering::SeqCst);
    assert!(
        history_reads <= 2,
        "two reads of Core's member history at most for the whole pass of three \
         identities: {history_reads}"
    );
    assert_eq!(core.creates.load(Ordering::SeqCst), 0);
    assert_eq!(core.resumes.load(Ordering::SeqCst), 0);
    assert_eq!(core.wakes.load(Ordering::SeqCst), 0, "a seat wakes nothing");
}

/// A roster pass over identities Core already holds seats nothing again:
/// Core keeps every member, nothing is seated, rotated or woken, and the
/// pass reads Core's member history two times at most, as a pass that
/// seats does: no read per identity.
#[tokio::test]
async fn r15_a_compile_over_seated_identities_seats_nothing_again() {
    let core = Arc::new(SeatCore::default());
    let runtime = runtime_over(None, &core);
    let specs = ["personal:alice", "personal:bob"]
        .map(|name| spec(&identity(name)))
        .to_vec();
    roster_pass(&runtime, &specs, None).await;
    let history_reads = core.history_reads.load(Ordering::SeqCst);

    let outcomes = roster_pass(&runtime, &specs, None).await;

    assert!(
        outcomes
            .values()
            .all(|outcome| pass_binding(outcome).is_some_and(|(created, _)| !created)),
        "Core keeps every member: {outcomes:?}"
    );
    assert_eq!(lock(&core.seat_calls).len(), 1, "no second seat");
    assert!(lock(&core.rotations).is_empty(), "nothing is rotated");
    assert_eq!(core.wakes.load(Ordering::SeqCst), 0, "nothing is woken");
    let pass_reads = core.history_reads.load(Ordering::SeqCst) - history_reads;
    assert!(
        pass_reads <= 2,
        "two reads of Core's member history at most for the pass: {pass_reads}"
    );
}

/// The first-boot carry-out: each pending `Resume` row is its identity's
/// seat on the row's session, all of them in the roster's one declaration,
/// and the manifest is rewritten once for the whole pass (here removed, every
/// row being done). Nothing is resumed or woken.
#[tokio::test]
async fn first_boot_resume_rows_are_seats_on_their_sessions_and_the_manifest_is_rewritten_once() {
    let rows = (0..6)
        .map(|index| FirstBootRow::Resume {
            identity: identity(&format!("personal:row-{index:02}")),
            session: SessionId::new(),
        })
        .collect::<Vec<_>>();
    let state_dir = tempfile::tempdir().expect("state dir");
    FirstBootManifest::write_new(state_dir.path(), rows.clone()).expect("the manifest writes");
    let core = Arc::new(SeatCore::default());
    let runtime = runtime_over(Some(state_dir.path()), &core);
    let specs = rows
        .iter()
        .map(|row| spec(row.identity()))
        .collect::<Vec<_>>();

    let outcomes = roster_pass(&runtime, &specs, None).await;

    for row in &rows {
        assert!(
            pass_refusal(&outcomes[row.identity()]).is_none(),
            "{}: {:?}",
            row.identity(),
            outcomes[row.identity()]
        );
        assert_eq!(
            core.session_of(row.identity()).as_ref(),
            Some(row.session()),
            "{} is seated on its row's session",
            row.identity()
        );
    }
    assert_eq!(lock(&core.seat_calls).len(), 1);
    assert_eq!(core.resumes.load(Ordering::SeqCst), 0);
    assert_eq!(core.wakes.load(Ordering::SeqCst), 0);
    assert_eq!(
        FirstBootManifest::read(state_dir.path()).expect("reads"),
        None,
        "every row is done"
    );
    assert_eq!(runtime.first_boot_rewrites.load(Ordering::SeqCst), 1);
    assert!(
        runtime
            .identity_bootstrap_status()
            .first_boot_pending
            .is_empty()
    );
}

/// A row whose seat Core refuses fails alone: its identity's outcome is the
/// typed refusal, its row is the only one the one rewritten manifest still
/// holds and the status still lists, and every other row is carried out.
#[tokio::test]
async fn a_refused_first_boot_seat_fails_alone_and_stays_in_the_rewritten_manifest() {
    let rows = (0..4)
        .map(|index| FirstBootRow::Resume {
            identity: identity(&format!("personal:row-{index:02}")),
            session: SessionId::new(),
        })
        .collect::<Vec<_>>();
    let refused = rows[2].identity().clone();
    let state_dir = tempfile::tempdir().expect("state dir");
    FirstBootManifest::write_new(state_dir.path(), rows.clone()).expect("the manifest writes");
    let core = Arc::new(SeatCore {
        refused: Some(refused.clone()),
        ..SeatCore::default()
    });
    let runtime = runtime_over(Some(state_dir.path()), &core);
    let specs = rows
        .iter()
        .map(|row| spec(row.identity()))
        .collect::<Vec<_>>();

    let outcomes = roster_pass(&runtime, &specs, None).await;

    assert!(
        pass_refusal(&outcomes[&refused]).is_some(),
        "the refused row fails: {:?}",
        outcomes[&refused]
    );
    assert_eq!(
        core.session_of(&refused),
        None,
        "the refused row binds nothing"
    );
    for row in rows.iter().filter(|row| row.identity() != &refused) {
        assert!(
            pass_refusal(&outcomes[row.identity()]).is_none(),
            "{}",
            row.identity()
        );
        assert_eq!(
            core.session_of(row.identity()).as_ref(),
            Some(row.session())
        );
    }
    let left = FirstBootManifest::read(state_dir.path())
        .expect("reads")
        .expect("the refused row keeps the manifest")
        .into_rows();
    assert_eq!(
        left.iter().map(FirstBootRow::identity).collect::<Vec<_>>(),
        vec![&refused]
    );
    assert_eq!(runtime.first_boot_rewrites.load(Ordering::SeqCst), 1);
    assert_eq!(
        runtime
            .identity_bootstrap_status()
            .first_boot_pending
            .into_iter()
            .map(|row| row.identity)
            .collect::<Vec<_>>(),
        vec![refused]
    );
}

/// A host customizer that refuses every build until it is let build.
#[derive(Default)]
struct RefusesBuilds(std::sync::atomic::AtomicBool);

#[async_trait::async_trait]
impl AgentCustomizer for RefusesBuilds {
    async fn customize_build(
        &self,
        _context: &AgentBuildContext,
        _spec: &DurableAgentSpec,
        _draft: &mut AgentBuildDraft,
    ) -> Result<(), crate::identity_first::CustomizerError> {
        if self.0.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(crate::identity_first::CustomizerError::BuildFailed(
                "the host refuses the build".to_string(),
            ))
        }
    }

    async fn after_create(
        &self,
        _identity: &AgentIdentity,
        _session_id: &SessionId,
        _context: &crate::mob_handle_runtime::SessionCreatedContext,
    ) -> Result<(), crate::identity_first::CustomizerError> {
        Ok(())
    }
}

/// A first-boot row leaves the manifest only when Core names its request
/// carried out. Core answers the first round of a compile `BuildRequired` for
/// the row's identity, keeping the request and carrying nothing out; when the
/// host's customizer then refuses the build, the identity fails, nothing is
/// seated, and the row stays. The next compile, whose build the customizer
/// composes, seats the identity on the row's session and carries the row out.
///
/// RED: settle the row of every identity Core answers, whatever it answered:
/// the first compile's `BuildRequired` consumes the row.
#[tokio::test]
async fn a_first_boot_row_is_consumed_only_when_core_carried_it_out() {
    let alice = identity("personal:alice");
    let row_session = SessionId::new();
    let state_dir = tempfile::tempdir().expect("state dir");
    FirstBootManifest::write_new(
        state_dir.path(),
        vec![FirstBootRow::Resume {
            identity: alice.clone(),
            session: row_session.clone(),
        }],
    )
    .expect("the manifest writes");
    let core = Arc::new(SeatCore::default());
    let runtime = runtime_over(Some(state_dir.path()), &core);
    let customizer = RefusesBuilds::default();

    let refused = roster_pass(&runtime, &[spec(&alice)], Some(&customizer)).await;
    assert!(
        pass_refusal(&refused[&alice]).is_some(),
        "the refused build fails the identity: {:?}",
        refused[&alice]
    );
    assert_eq!(core.session_of(&alice), None, "nothing is seated");
    let left = FirstBootManifest::read(state_dir.path())
        .expect("reads")
        .expect("Core carried nothing out, so the row stays")
        .into_rows();
    assert_eq!(
        left.iter().map(FirstBootRow::identity).collect::<Vec<_>>(),
        vec![&alice]
    );

    customizer.0.store(true, Ordering::SeqCst);
    let seated = roster_pass(&runtime, &[spec(&alice)], Some(&customizer)).await;
    assert!(
        pass_refusal(&seated[&alice]).is_none(),
        "{:?}",
        seated[&alice]
    );
    assert_eq!(
        core.session_of(&alice).as_ref(),
        Some(&row_session),
        "the identity is seated on its row's session"
    );
    assert_eq!(
        FirstBootManifest::read(state_dir.path()).expect("reads"),
        None,
        "the row Core carried out leaves the manifest"
    );
}

/// The status window: between a row's carry-out and the
/// rewrite of the manifest that drops it, the bootstrap status still lists
/// the row as pending. No reader sees "no pending row" while the row is
/// only settled in memory; Core's own host-preparation mark of the identity
/// ended in the seat's journaled step.
#[tokio::test]
async fn a_carried_out_row_reads_pending_until_the_manifest_rewrite_lands() {
    let alice = identity("personal:alice");
    let row = FirstBootRow::Resume {
        identity: alice.clone(),
        session: SessionId::new(),
    };
    let state_dir = tempfile::tempdir().expect("state dir");
    FirstBootManifest::write_new(state_dir.path(), vec![row]).expect("the manifest writes");
    let core = Arc::new(SeatCore::default());
    let runtime = runtime_over(Some(state_dir.path()), &core);
    let specs = vec![spec(&alice)];
    runtime
        .load_first_boot_manifest()
        .await
        .expect("the manifest reads");

    let observed = Arc::new(StdMutex::new(None));
    let hook_runtime = Arc::clone(&runtime);
    let hook_observed = Arc::clone(&observed);
    runtime.set_first_boot_batch_hook(Box::new(move || {
        Box::pin(async move {
            let pending = hook_runtime
                .identity_bootstrap_status()
                .first_boot_pending
                .into_iter()
                .map(|row| row.identity)
                .collect::<Vec<_>>();
            *lock(&hook_observed) = Some(pending);
        })
    }));

    let outcomes = roster_pass(&runtime, &specs, None).await;

    assert!(
        pass_refusal(&outcomes[&alice]).is_none(),
        "{:?}",
        outcomes[&alice]
    );
    assert_eq!(
        lock(&observed).clone(),
        Some(vec![alice.clone()]),
        "before the rewrite lands the row still reads pending"
    );
    assert!(
        runtime
            .identity_bootstrap_status()
            .first_boot_pending
            .is_empty(),
        "after it lands, none does"
    );
}

/// A host customizer that, while it builds `builder`'s member, sends to
/// `target` and records how the send ended.
struct SendsWhileBuilding {
    runtime: std::sync::OnceLock<std::sync::Weak<IdentityRuntime>>,
    builder: AgentIdentity,
    target: AgentIdentity,
    sent: StdMutex<Option<Result<(), String>>>,
}

#[async_trait::async_trait]
impl AgentCustomizer for SendsWhileBuilding {
    async fn customize_build(
        &self,
        context: &AgentBuildContext,
        _spec: &DurableAgentSpec,
        _draft: &mut AgentBuildDraft,
    ) -> Result<(), crate::identity_first::CustomizerError> {
        if context.identity != self.builder {
            return Ok(());
        }
        let runtime = self
            .runtime
            .get()
            .and_then(std::sync::Weak::upgrade)
            .expect("the runtime outlives its builds");
        let sent = runtime
            .send(
                &self.target,
                &meerkat_core::ContentInput::Text("a message from a build".to_string()),
            )
            .await;
        *lock(&self.sent) = Some(sent.map(|_| ()).map_err(|error| error.to_string()));
        Ok(())
    }
}

/// A host customizer that, while it builds one identity's member, sends to
/// another identity of the same roster pass holds no pass up. The pass
/// composes the builds Core asks for with no lifecycle lock held: the send
/// declares its own target alone (carrying out the target's row) and is
/// delivered, and the pass completes. Each identity is seated once, on its
/// row's session, and the manifest ends with every row done.
#[tokio::test]
async fn a_build_that_sends_to_another_identity_of_the_step_does_not_hold_the_step() {
    let rows = resume_rows(2);
    let state_dir = tempfile::tempdir().expect("state dir");
    FirstBootManifest::write_new(state_dir.path(), rows.clone()).expect("the manifest writes");
    let core = Arc::new(SeatCore {
        admits_deliveries: true,
        ..SeatCore::default()
    });
    let runtime = runtime_over(Some(state_dir.path()), &core);
    let customizer = Arc::new(SendsWhileBuilding {
        runtime: std::sync::OnceLock::new(),
        builder: rows[0].identity().clone(),
        target: rows[1].identity().clone(),
        sent: StdMutex::new(None),
    });
    assert!(customizer.runtime.set(Arc::downgrade(&runtime)).is_ok());
    runtime.set_agent_customizer(Some(customizer.clone())).await;
    let specs = row_specs(&rows);

    let outcomes =
        tokio::time::timeout(Duration::from_secs(30), roster_pass(&runtime, &specs, None))
            .await
            .expect("the pass completes: a send from a build into the pass waits for no pass");

    for row in &rows {
        assert!(
            pass_refusal(&outcomes[row.identity()]).is_none(),
            "{}: {:?}",
            row.identity(),
            outcomes[row.identity()]
        );
        assert_eq!(
            (
                core.session_of(row.identity()).as_ref(),
                core.generation_of(row.identity())
            ),
            (Some(row.session()), Some(0)),
            "{} is seated once, on its row's session",
            row.identity()
        );
    }
    assert_eq!(manifest_rows(state_dir.path()), None, "every row is done");
    assert_eq!(
        lock(&customizer.sent).take(),
        Some(Ok(())),
        "the build's send returned, delivered"
    );
    assert_eq!(
        lock(&core.delivered).len(),
        1,
        "Core admitted the build's send"
    );
}

/// A host customizer that holds every build until `parties` builds wait.
struct BuildsMeet {
    barrier: tokio::sync::Barrier,
}

#[async_trait::async_trait]
impl AgentCustomizer for BuildsMeet {
    async fn customize_build(
        &self,
        _context: &AgentBuildContext,
        _spec: &DurableAgentSpec,
        _draft: &mut AgentBuildDraft,
    ) -> Result<(), crate::identity_first::CustomizerError> {
        self.barrier.wait().await;
        Ok(())
    }
}

/// Two roster passes that planned the same identity Core does not hold
/// before either declared it with its build (both builds wait until both
/// passes asked for them) seat it once. Core seats it for the first
/// declaration and keeps it for the second, so both passes answer it bound,
/// the second on the session Core holds, and Core holds one binding.
#[tokio::test]
async fn two_passes_planned_together_seat_an_identity_once() {
    let alice = identity("personal:alice");
    let core = Arc::new(SeatCore::default());
    let runtime = runtime_over(None, &core);
    runtime
        .set_agent_customizer(Some(Arc::new(BuildsMeet {
            barrier: tokio::sync::Barrier::new(2),
        })))
        .await;
    let specs = vec![spec(&alice)];

    let (first, second) = tokio::time::timeout(Duration::from_secs(30), async {
        tokio::join!(
            roster_pass(&runtime, &specs, None),
            roster_pass(&runtime, &specs, None)
        )
    })
    .await
    .expect("both passes complete");

    let outcomes = [&first[&alice], &second[&alice]];
    assert!(
        outcomes
            .iter()
            .all(|outcome| pass_binding(outcome).is_some()),
        "both passes answer the identity bound: {outcomes:?}"
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| pass_binding(outcome).is_some_and(|(created, _)| !created))
            .count(),
        1,
        "the later pass finds the identity seated: {outcomes:?}"
    );
    assert_eq!(lock(&core.seat_calls).len(), 1, "Core seats it once");
    assert_eq!(core.generation_of(&alice), Some(0), "Core bound it once");
    assert!(core.session_of(&alice).is_some());
}

/// While a roster pass waits for its one manifest rewrite (here it is held
/// right before it), the identities whose rows it carried out are free: a
/// reset of one goes ahead at once and writes the manifest down first, so
/// Core rotates the identity only once its row is gone, and a stop then
/// leaves no row to read contradicted. A reset of the identity whose seat
/// Core refused is refused as pending: its row stays. Released, the pass
/// finds the manifest already written down to the refused row.
#[tokio::test]
async fn a_reset_while_the_pass_waits_for_its_rewrite_writes_the_row_out_first() {
    let rows = resume_rows(3);
    let reset = rows[0].identity().clone();
    let refused = rows[2].identity().clone();
    let state_dir = tempfile::tempdir().expect("state dir");
    FirstBootManifest::write_new(state_dir.path(), rows.clone()).expect("the manifest writes");
    let core = Arc::new(SeatCore {
        refused: Some(refused.clone()),
        manifest_dir: Some(state_dir.path().to_path_buf()),
        ..SeatCore::default()
    });
    let runtime = runtime_over(Some(state_dir.path()), &core);
    let specs = row_specs(&rows);
    let (release, released) = oneshot::channel::<()>();
    let reached_rewrite = hold_at_rewrite(&runtime, async move {
        let _ = released.await;
    });

    let window = async {
        reached_rewrite.await.expect("the pass reaches its rewrite");
        let fresh = tokio::time::timeout(Duration::from_secs(30), runtime.reset(&reset))
            .await
            .expect("the reset waits for no pass")
            .expect("the reset goes ahead");
        let refusal = runtime.reset(&refused).await;
        let listed = manifest_rows(state_dir.path());
        assert!(release.send(()).is_ok(), "the pass waits to be released");
        (fresh, refusal, listed)
    };
    let (outcomes, (fresh, refusal, listed)) =
        tokio::join!(roster_pass(&runtime, &specs, None), window);

    assert!(
        pass_refusal(&outcomes[&reset]).is_none(),
        "{:?}",
        outcomes[&reset]
    );
    assert!(pass_refusal(&outcomes[rows[1].identity()]).is_none());
    assert!(
        pass_refusal(&outcomes[&refused]).is_some(),
        "Core refused this seat"
    );
    assert_eq!(
        lock(&core.rotations_listed).clone(),
        vec![(reset.clone(), false)],
        "Core is asked to rotate {reset} only once the manifest no longer lists its row"
    );
    assert_ne!(
        fresh.session_id(),
        Some(rows[0].session()),
        "the reset gives {reset} a fresh session"
    );
    assert!(
        matches!(
            &refusal,
            Err(IdentityRuntimeError::FirstBootRowPending { identity, operation: "reset", .. })
                if *identity == refused
        ),
        "a reset of the identity whose seat Core refused is refused as pending: {refusal:?}"
    );
    assert_eq!(
        listed,
        Some(rows[2..].to_vec()),
        "before the step's own rewrite, the reset wrote the carried-out rows out of the \
         manifest; the refused row stays"
    );
    assert_eq!(manifest_rows(state_dir.path()), Some(rows[2..].to_vec()));
    assert_eq!(pending(&runtime), vec![refused]);
}

/// A roster pass dropped after Core seated its rows and before its one
/// manifest rewrite (the request that carried the roster compile went away)
/// leaves those rows settled in this process and still listed in the
/// manifest. A reset of such an identity writes the manifest first, so Core
/// rotates the identity only once its row is gone. The next boot after a
/// stop reads the rest against Core's member history: the rows no pass
/// carried out are seated once, no identity is seated twice, the reset
/// identity keeps its fresh life (no row of it is left to read
/// contradicted), and the manifest is removed.
#[tokio::test]
async fn a_pass_dropped_before_its_rewrite_leaves_no_row_a_reset_could_contradict() {
    let rows = resume_rows(6);
    let specs = row_specs(&rows);
    let reset = rows[0].identity().clone();
    let state_dir = tempfile::tempdir().expect("state dir");
    FirstBootManifest::write_new(state_dir.path(), rows.clone()).expect("the manifest writes");
    let core = Arc::new(SeatCore {
        manifest_dir: Some(state_dir.path().to_path_buf()),
        ..SeatCore::default()
    });
    {
        let runtime = runtime_over(Some(state_dir.path()), &core);
        // The pass seats the first three rows' identities and is dropped
        // at its rewrite; no pass carries the other three rows out.
        let reached_rewrite = hold_at_rewrite(&runtime, std::future::pending::<()>());
        tokio::select! {
            _ = roster_pass(&runtime, &specs[..3], None) => {
                panic!("the pass is held at its rewrite");
            }
            reached = reached_rewrite => reached.expect("the pass reaches its rewrite"),
        }
        for row in &rows[..3] {
            assert_eq!(
                core.session_of(row.identity()).as_ref(),
                Some(row.session()),
                "the dropped pass seated {}",
                row.identity()
            );
        }
        assert_eq!(
            runtime.first_boot_rewrites.load(Ordering::SeqCst),
            0,
            "the dropped pass never rewrote the manifest"
        );
        assert_eq!(
            manifest_rows(state_dir.path()),
            Some(rows.clone()),
            "the manifest still lists every row"
        );

        let fresh = runtime
            .reset(&reset)
            .await
            .expect("the reset of a carried-out identity goes ahead");
        assert_ne!(
            fresh.session_id(),
            Some(rows[0].session()),
            "the reset gives {reset} a fresh session"
        );
        assert_eq!(
            lock(&core.rotations_listed).clone(),
            vec![(reset.clone(), false)],
            "Core is asked to rotate {reset} only once the manifest no longer lists its row"
        );
        assert_eq!(
            manifest_rows(state_dir.path()),
            Some(rows[3..].to_vec()),
            "the reset wrote the carried-out rows out first; the rows no pass carried out stay"
        );
    }

    // The process stops; the next boot reads the manifest again.
    let runtime = runtime_over(Some(state_dir.path()), &core);
    let outcomes = roster_pass(&runtime, &specs, None).await;

    for (identity, outcome) in &outcomes {
        assert!(
            pass_refusal(outcome).is_none(),
            "no row reads contradicted: {identity}: {outcome:?}"
        );
    }
    let calls = lock(&core.seat_calls).clone();
    assert_eq!(calls.len(), 2, "one seat call per boot");
    assert_eq!(
        calls[1],
        rows[3..]
            .iter()
            .map(|row| (row.identity().clone(), Some(row.session().clone())))
            .collect::<Vec<_>>(),
        "the next boot seats exactly the rows no step carried out"
    );
    for row in &rows[1..] {
        assert_eq!(
            (
                core.session_of(row.identity()).as_ref(),
                core.generation_of(row.identity())
            ),
            (Some(row.session()), Some(0)),
            "{} is seated once, on its row's session",
            row.identity()
        );
    }
    assert_ne!(
        core.session_of(&reset).as_ref(),
        Some(rows[0].session()),
        "{reset} keeps its fresh life"
    );
    assert_eq!(core.generation_of(&reset), Some(1));
    assert_eq!(
        lock(&core.rotations).clone(),
        vec![reset],
        "the reset's rotation is the only one"
    );
    assert_eq!(manifest_rows(state_dir.path()), None, "every row is done");
    assert!(pending(&runtime).is_empty());
}

/// The stop of a boot whose roster pass seated its rows but had not
/// rewritten the manifest yet (here the pass is held right before its
/// rewrite, then dropped with its runtime) leaves every row in the manifest.
/// The next boot reads each row against Core's member history: a seated row
/// reads done, and Core keeps its member (it binds nothing again); a row
/// whose seat Core refused is tried again; a row whose session Core holds no
/// durable copy of is refused by Core, typed; and a row Core's history
/// contradicts is refused before any Core step, with Core's binding left as
/// it is. The next boot's one rewrite keeps exactly those three rows.
#[tokio::test]
async fn a_boot_that_stops_before_its_pass_rewrite_settles_the_seated_rows_next_boot() {
    let rows = resume_rows(6);
    let specs = row_specs(&rows);
    let failing = rows[3].identity().clone();
    let unheld = rows[4].identity().clone();
    let contradicted = rows[5].identity().clone();
    let state_dir = tempfile::tempdir().expect("state dir");
    FirstBootManifest::write_new(state_dir.path(), rows.clone()).expect("the manifest writes");
    let core = Arc::new(SeatCore {
        refused: Some(failing.clone()),
        unheld: vec![rows[4].session().clone()],
        ..SeatCore::default()
    });
    // Core's history binds the last row's identity to another session.
    let elsewhere = SessionId::new();
    bind_live(
        &core.core,
        &contradicted,
        &meerkat_mob::ProfileName::from("personal"),
        0,
        Some(elsewhere.clone()),
    )
    .expect("bound");
    {
        let runtime = runtime_over(Some(state_dir.path()), &core);
        let reached_rewrite = hold_at_rewrite(&runtime, std::future::pending::<()>());
        tokio::select! {
            _ = roster_pass(&runtime, &specs, None) => {
                panic!("the pass is held at its rewrite");
            }
            reached = reached_rewrite => reached.expect("the pass reaches its rewrite"),
        }
        assert_eq!(
            runtime.first_boot_rewrites.load(Ordering::SeqCst),
            0,
            "the boot stops before the pass's rewrite"
        );
    }
    assert_eq!(
        manifest_rows(state_dir.path()),
        Some(rows.clone()),
        "the manifest still lists every row"
    );
    for row in &rows[..3] {
        assert_eq!(
            core.session_of(row.identity()).as_ref(),
            Some(row.session()),
            "the first boot seated {}",
            row.identity()
        );
    }

    // The next boot.
    let runtime = runtime_over(Some(state_dir.path()), &core);
    let outcomes = roster_pass(&runtime, &specs, None).await;

    for row in &rows[..3] {
        let (created, _) = pass_binding(&outcomes[row.identity()]).unwrap_or_else(|| {
            panic!(
                "a seated row reads done: {}: {:?}",
                row.identity(),
                outcomes[row.identity()]
            )
        });
        assert!(!created, "{} is not seated again", row.identity());
        assert_eq!(
            (
                core.session_of(row.identity()).as_ref(),
                core.generation_of(row.identity())
            ),
            (Some(row.session()), Some(0)),
            "{} stays on its row's session, bound once",
            row.identity()
        );
    }
    assert!(
        pass_refusal(&outcomes[&failing]).is_some(),
        "the row whose seat Core refused fails again"
    );
    assert!(
        matches!(
            pass_refusal(&outcomes[&unheld]),
            Some(IdentityRuntimeError::FirstBootRowRefused {
                core_session: None,
                ..
            })
        ),
        "a row whose session Core holds no durable copy of is refused: {:?}",
        outcomes[&unheld]
    );
    assert!(
        matches!(
            pass_refusal(&outcomes[&contradicted]),
            Some(IdentityRuntimeError::FirstBootRowRefused {
                core_session: Some(session),
                ..
            }) if *session == elsewhere
        ),
        "a row Core's history contradicts is refused: {:?}",
        outcomes[&contradicted]
    );
    assert_eq!(
        core.session_of(&contradicted),
        Some(elsewhere),
        "Core's binding of the contradicted identity stays"
    );
    let calls = lock(&core.seat_calls).clone();
    assert_eq!(calls.len(), 2, "one seat call per boot");
    assert!(
        calls
            .iter()
            .flatten()
            .all(|(identity, _)| identity != &contradicted),
        "Core is never asked to seat the contradicted row"
    );
    assert_eq!(
        calls[1]
            .iter()
            .filter(|(identity, _)| identity == &failing)
            .count(),
        1,
        "the refused row is tried at each boot"
    );
    assert_eq!(
        manifest_rows(state_dir.path()),
        Some(rows[3..].to_vec()),
        "the next boot's rewrite keeps exactly the refused, unheld and contradicted rows"
    );
    assert_eq!(pending(&runtime), vec![failing, unheld, contradicted]);
    assert_eq!(
        runtime.first_boot_rewrites.load(Ordering::SeqCst),
        1,
        "the next boot rewrites the manifest once"
    );
}

/// A pending first-boot `Fresh` row of an identity Core retired is carried
/// out by its declaration: Core starts the identity's fresh life on a fresh
/// session (its rotation of an identity no member holds, the fresh seat of
/// its name), and nothing is created or resumed through the host. The row
/// leaves the manifest.
#[tokio::test]
async fn r64_a_first_boot_fresh_row_is_a_fresh_life_on_a_fresh_session() {
    let alice = identity("personal:alice");
    let retired_session = SessionId::new();
    let core = Arc::new(SeatCore::default());
    let retired = bind_live(
        &core.core,
        &alice,
        &meerkat_mob::ProfileName::from("personal"),
        1,
        Some(retired_session.clone()),
    )
    .expect("bound");
    core.core.retire(&retired);
    let state_dir = tempfile::tempdir().expect("state dir");
    FirstBootManifest::write_new(
        state_dir.path(),
        vec![FirstBootRow::Fresh {
            identity: alice.clone(),
            retired_session: retired_session.clone(),
        }],
    )
    .expect("the manifest writes");
    let runtime = runtime_over(Some(state_dir.path()), &core);
    let specs = vec![spec(&alice)];

    let outcomes = roster_pass(&runtime, &specs, None).await;

    let (created, binding) = pass_binding(&outcomes[&alice])
        .unwrap_or_else(|| panic!("alice starts her fresh life: {:?}", outcomes[&alice]));
    assert!(created, "a fresh session");
    let session = core.session_of(&alice).expect("bound");
    assert_eq!(binding.session_id.as_ref(), Some(&session));
    assert_ne!(session, retired_session);
    assert_eq!(
        lock(&core.rotations).clone(),
        vec![alice.clone()],
        "Core's fresh life of the identity no member holds"
    );
    assert_eq!(core.creates.load(Ordering::SeqCst), 0, "no create");
    assert_eq!(core.resumes.load(Ordering::SeqCst), 0, "no resume");
    assert_eq!(
        FirstBootManifest::read(state_dir.path()).expect("reads"),
        None
    );
}

/// A rotation (`reset`, `delete_identity`) a crash interrupted is Core's to
/// finish, from its own record: the compile declares the identity Core holds
/// and asks Core for no rotation, and Meta's record of the rotation stays
/// written, for a rollback.
#[tokio::test]
async fn r114_an_interrupted_rotation_is_core_s_to_finish_and_no_compile_drives_it() {
    let alice = identity("personal:alice");
    let core = Arc::new(SeatCore::default());
    let old_session = SessionId::new();
    bind_live(
        &core.core,
        &alice,
        &meerkat_mob::ProfileName::from("personal"),
        1,
        Some(old_session.clone()),
    )
    .expect("bound");
    let runtime = runtime_over(None, &core);
    let specs = vec![spec(&alice)];
    roster_pass(&runtime, &specs, None).await;
    runtime
        .record_rotation_intent(&alice, "reset", Some(1))
        .await
        .expect("the rotation is recorded");
    let rotations_before = core.rotation_calls.load(Ordering::SeqCst);

    let outcomes = roster_pass(&runtime, &specs, None).await;

    let (created, _) =
        pass_binding(&outcomes[&alice]).unwrap_or_else(|| panic!("kept: {:?}", outcomes[&alice]));
    assert!(!created, "Core keeps the member it holds");
    assert_eq!(
        core.rotation_calls.load(Ordering::SeqCst),
        rotations_before,
        "the compile asks Core for no rotation"
    );
    assert_eq!(core.generation_of(&alice), Some(1));
    assert_eq!(core.session_of(&alice), Some(old_session));
    assert!(
        runtime
            .pending_rotation_intents()
            .await
            .expect("the records read")
            .contains_key(&alice),
        "Meta's record of the rotation stays written"
    );
}

/// Meta's retire releases the member: Core keeps it
/// seated on the same session at the same generation, so its saved delivery
/// scopes stay valid, and nothing is retired.
#[tokio::test]
async fn r66_retire_releases_the_member_and_keeps_its_session_and_generation() {
    let alice = identity("personal:alice");
    let core = Arc::new(SeatCore::default());
    let session = SessionId::new();
    let runtime = runtime_over(None, &core);
    crate::identity_first::core_test_bridge::register_live(
        &runtime,
        &core.core,
        spec(&alice),
        1,
        session.clone(),
    )
    .await
    .expect("registered live");

    runtime.retire(&alice).await.expect("retired");

    assert_eq!(lock(&core.releases).clone(), vec![alice.clone()]);
    assert!(
        core.core.live_member(&alice).expect("reads").is_some(),
        "still a member"
    );
    assert_eq!(core.session_of(&alice), Some(session));
    assert_eq!(core.generation_of(&alice), Some(1), "no generation advance");
}

/// A retire is a release, and Core refuses a release, retryably, while the
/// member is being woken or has work in flight. Both retire verbs answer the
/// typed retryable refusal naming the member's session, and nothing is
/// interrupted: Core still holds the member live on the same session at the
/// same generation, and nothing was released. Each goes ahead once Core
/// releases the member, the scoped one with the same scope.
#[tokio::test]
async fn a_retire_core_refuses_while_the_member_is_busy_is_typed_retryable_and_interrupts_nothing()
{
    let alice = identity("personal:alice");
    let core = Arc::new(CoreReadsBridge::default());
    let runtime = Arc::new(IdentityRuntime::new(IdentityRuntimeConfig {
        continuity_store: Arc::new(LocalContinuityStore::in_memory().expect("store")),
        lease_provider: Arc::new(LocalLeaseProvider::new()),
        runtime_instance_id: "seat-rows".to_string(),
        has_runtime_store: true,
        durability_policy: DurabilityPolicy::SyncWriteThrough,
        bridge: Some(core.clone()),
        default_timeout: None,
    }));
    let session = SessionId::new();
    register_live(&runtime, &core.core, spec(&alice), 1, session.clone())
        .await
        .expect("registered live");
    let scope = runtime
        .capture_delivery_scope(&alice)
        .await
        .expect("the live scope");
    core.refuse_release_of(&alice);

    let retired = runtime.retire(&alice).await;
    let scoped = runtime.retire_at_scope_tracked(&alice, &scope).await;

    match &retired {
        Err(IdentityRuntimeError::IdentityWakeRetryable {
            identity,
            session_id,
            ..
        }) => {
            assert_eq!(identity, &alice);
            assert_eq!(session_id.as_ref(), Some(&session));
        }
        other => panic!("expected the typed retryable refusal, got {other:?}"),
    }
    match &scoped {
        Err(crate::identity_first::ScopedRetireError::Retire(
            IdentityRuntimeError::IdentityWakeRetryable {
                identity,
                session_id,
                ..
            },
        )) => {
            assert_eq!(identity, &alice);
            assert_eq!(session_id.as_ref(), Some(&session));
        }
        other => panic!("expected the scoped retire's typed retryable refusal, got {other:?}"),
    }
    assert!(core.releases().is_empty(), "nothing was released");
    let live = core
        .core
        .live_member(&alice)
        .expect("reads")
        .expect("still live");
    assert_eq!(live.session_id, Some(session));
    assert_eq!(
        core.core
            .last_member_binding(&alice)
            .expect("reads")
            .map(|binding| binding.generation().get()),
        Some(1),
        "no generation advance"
    );

    // Once Core releases the member, the same retires go ahead.
    core.allow_release_of(&alice);
    runtime
        .retire(&alice)
        .await
        .expect("the retried retire goes ahead");
    runtime
        .retire_at_scope_tracked(&alice, &scope)
        .await
        .expect("the retried scoped retire goes ahead: the release kept the incarnation");
    assert_eq!(core.releases(), vec![alice.clone(), alice]);
}

/// Source guard: the identity runtime keeps no wake policy of
/// its own. No production path materializes a member's peers, keeps a
/// materialization backoff, holds run starts to keep a member dormant, or
/// decides a wake from a profile's `external_addressable` flag: Core decides
/// every materialization.
///
/// Each file is read whole, as code: comments and literals are blanked, and
/// every `#[cfg(test)]` module is dropped, so a test module anywhere in a
/// file hides nothing after it, and a default configuration a string
/// spells out is no wake decision. The files are the identity runtime, the
/// bridge, the build preparation, the customizer tools, and the runtime's
/// composition: the mob handle runtime, the unified runtime and the RPC
/// gateway.
#[test]
fn r68_f9_the_identity_runtime_keeps_no_wake_policy_of_its_own() {
    let sources = [
        ("identity_first/runtime.rs", include_str!("../runtime.rs")),
        (
            "identity_first/runtime/declare.rs",
            include_str!("declare.rs"),
        ),
        (
            "identity_first/runtime/records.rs",
            include_str!("records.rs"),
        ),
        (
            "identity_first/runtime/build_preparation.rs",
            include_str!("build_preparation.rs"),
        ),
        (
            "identity_first/orchestrator.rs",
            include_str!("../orchestrator.rs"),
        ),
        ("identity_first/bridge.rs", include_str!("../bridge.rs")),
        (
            "identity_first/build_preparer.rs",
            include_str!("../build_preparer.rs"),
        ),
        (
            "identity_first/customizer_tools.rs",
            include_str!("../customizer_tools.rs"),
        ),
        (
            "mob_handle_runtime.rs",
            include_str!("../../mob_handle_runtime.rs"),
        ),
        ("rpc_gateway.rs", include_str!("../../rpc_gateway.rs")),
        (
            "unified_runtime/bootstrap_completion.rs",
            include_str!("../../unified_runtime/bootstrap_completion.rs"),
        ),
        (
            "unified_runtime/builder.rs",
            include_str!("../../unified_runtime/builder.rs"),
        ),
        (
            "unified_runtime/console_events.rs",
            include_str!("../../unified_runtime/console_events.rs"),
        ),
        (
            "unified_runtime/continuity_mirror.rs",
            include_str!("../../unified_runtime/continuity_mirror.rs"),
        ),
        (
            "unified_runtime/continuity_mirror_bigquery.rs",
            include_str!("../../unified_runtime/continuity_mirror_bigquery.rs"),
        ),
        (
            "unified_runtime/cross_mob.rs",
            include_str!("../../unified_runtime/cross_mob.rs"),
        ),
        (
            "unified_runtime/edge_reconcile.rs",
            include_str!("../../unified_runtime/edge_reconcile.rs"),
        ),
        (
            "unified_runtime/edge_types.rs",
            include_str!("../../unified_runtime/edge_types.rs"),
        ),
        (
            "unified_runtime/event_log.rs",
            include_str!("../../unified_runtime/event_log.rs"),
        ),
        (
            "unified_runtime/http.rs",
            include_str!("../../unified_runtime/http.rs"),
        ),
        (
            "unified_runtime/implicit_delegate_retirement.rs",
            include_str!("../../unified_runtime/implicit_delegate_retirement.rs"),
        ),
        (
            "unified_runtime/in_flight_drain.rs",
            include_str!("../../unified_runtime/in_flight_drain.rs"),
        ),
        (
            "unified_runtime/lifecycle.rs",
            include_str!("../../unified_runtime/lifecycle.rs"),
        ),
        (
            "unified_runtime/live_compose.rs",
            include_str!("../../unified_runtime/live_compose.rs"),
        ),
        (
            "unified_runtime/mob_events.rs",
            include_str!("../../unified_runtime/mob_events.rs"),
        ),
        (
            "unified_runtime/mob_ops.rs",
            include_str!("../../unified_runtime/mob_ops.rs"),
        ),
        (
            "unified_runtime/mob_stop.rs",
            include_str!("../../unified_runtime/mob_stop.rs"),
        ),
        (
            "unified_runtime/mod.rs",
            include_str!("../../unified_runtime/mod.rs"),
        ),
        (
            "unified_runtime/module_ops.rs",
            include_str!("../../unified_runtime/module_ops.rs"),
        ),
        (
            "unified_runtime/types.rs",
            include_str!("../../unified_runtime/types.rs"),
        ),
    ];
    for (file, source) in sources {
        let production = production_code(source);
        assert!(
            !production.trim().is_empty(),
            "{file}: no production code was read"
        );
        for needle in WAKE_POLICY_NEEDLES {
            assert!(
                !production.contains(needle),
                "{file} names `{needle}`: Meta must not keep a wake policy of its own"
            );
        }
    }
}

/// The guard above reads what it should: code outside test modules, and
/// nothing else. A needle in a comment, in a string, or in a `#[cfg(test)]`
/// module (inline, with further attributes, or out of line) is no
/// production code; a needle after such a module is; and braces inside
/// literals end nothing.
#[test]
fn the_wake_policy_guard_reads_production_code_only() {
    let source = r##"
fn before() {
    let braces = r#"}{"#;
    let brace = '{';
    let text = "} materialization_backoff {";
    let label: &'static str = "}";
}
// hydrate_peer_topology in a comment is no code.
/* a block comment /* nested */ materialize_reachable_peers */
#[cfg(test)]
#[path = "elsewhere_tests.rs"]
mod elsewhere_tests;
#[cfg(test)]
#[allow(clippy::panic)]
pub(crate) mod tests {
    fn helper() {
        let brace = "}";
        let raw = r#"{"#;
        hold_restored_member_run_starts();
    }
}
fn after() {
    external_addressable();
}
"##;
    let production = production_code(source);

    assert!(production.contains("fn before()"));
    assert!(
        production.contains("external_addressable"),
        "code after a test module is production code: {production}"
    );
    for absent in [
        "materialization_backoff",
        "hydrate_peer_topology",
        "materialize_reachable_peers",
        "hold_restored_member_run_starts",
        "elsewhere_tests",
    ] {
        assert!(
            !production.contains(absent),
            "`{absent}` is no production code: {production}"
        );
    }
}

/// The names a wake policy of Meta's own would use.
const WAKE_POLICY_NEEDLES: [&str; 5] = [
    "materialize_reachable_peers",
    "hydrate_peer_topology",
    "materialization_backoff",
    "hold_restored_member_run_starts",
    "external_addressable",
];

/// The production code of `source`, as code: comments and literals blanked
/// to spaces, and every `#[cfg(test)]` module removed, inline or out of line.
fn production_code(source: &str) -> String {
    const TEST_GATE: &[u8] = b"#[cfg(test)]";
    let code = code_only(source);
    let mut kept = Vec::with_capacity(code.len());
    let mut copied = 0;
    let mut at = 0;
    while let Some(start) = find_from(&code, TEST_GATE, at) {
        match test_module_end(&code, start + TEST_GATE.len()) {
            Some(end) => {
                kept.extend_from_slice(&code[copied..start]);
                copied = end;
                at = end;
            }
            None => at = start + 1,
        }
    }
    kept.extend_from_slice(&code[copied..]);
    String::from_utf8_lossy(&kept).into_owned()
}

/// `source` with every comment and every string, raw string and character
/// literal blanked to spaces (newlines kept), byte for byte: what is left is
/// the code, at the same offsets.
fn code_only(source: &str) -> Vec<u8> {
    let bytes = source.as_bytes();
    let mut code = bytes.to_vec();
    let mut at = 0;
    while at < bytes.len() {
        let end = match bytes[at] {
            b'/' if bytes.get(at + 1) == Some(&b'/') => {
                find_from(bytes, b"\n", at).unwrap_or(bytes.len())
            }
            b'/' if bytes.get(at + 1) == Some(&b'*') => block_comment_end(bytes, at),
            b'"' => string_end(bytes, at),
            b'r' | b'b' => match raw_string_end(bytes, at) {
                Some(end) => end,
                None => {
                    at += 1;
                    continue;
                }
            },
            b'\'' => match char_literal_end(source, at) {
                Some(end) => end,
                None => {
                    at += 1;
                    continue;
                }
            },
            _ => {
                at += 1;
                continue;
            }
        };
        for byte in &mut code[at..end] {
            if *byte != b'\n' {
                *byte = b' ';
            }
        }
        at = end;
    }
    code
}

/// The first position of `needle` in `haystack` at or after `from`.
fn find_from(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    haystack
        .get(from..)?
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|position| from + position)
}

/// Where the block comment opening at `from` ends: block comments nest.
fn block_comment_end(bytes: &[u8], from: usize) -> usize {
    let mut depth = 0_usize;
    let mut at = from;
    while at < bytes.len() {
        if bytes[at..].starts_with(b"/*") {
            depth += 1;
            at += 2;
        } else if bytes[at..].starts_with(b"*/") {
            depth = depth.saturating_sub(1);
            at += 2;
            if depth == 0 {
                return at;
            }
        } else {
            at += 1;
        }
    }
    bytes.len()
}

/// Where the string literal opening at `at` ends: after its closing quote.
fn string_end(bytes: &[u8], at: usize) -> usize {
    let mut end = at + 1;
    while let Some(byte) = bytes.get(end) {
        match byte {
            b'\\' => end += 2,
            b'"' => return end + 1,
            _ => end += 1,
        }
    }
    bytes.len()
}

/// Where the raw string literal (`r"..."`, `r#"..."#`, `br"..."`) opening at
/// `at` ends, or `None` when none opens there: a raw identifier such as
/// `r#type`, or an identifier that ends in `r` or `b`.
fn raw_string_end(bytes: &[u8], at: usize) -> Option<usize> {
    if at
        .checked_sub(1)
        .and_then(|before| bytes.get(before))
        .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
    {
        return None;
    }
    let mut quote = at;
    if bytes.get(quote) == Some(&b'b') {
        quote += 1;
    }
    if bytes.get(quote) != Some(&b'r') {
        return None;
    }
    quote += 1;
    let hashes = bytes
        .get(quote..)?
        .iter()
        .take_while(|byte| **byte == b'#')
        .count();
    quote += hashes;
    if bytes.get(quote) != Some(&b'"') {
        return None;
    }
    let mut closing = vec![b'"'];
    closing.extend(std::iter::repeat_n(b'#', hashes));
    Some(find_from(bytes, &closing, quote + 1).map_or(bytes.len(), |close| close + closing.len()))
}

/// Where the character literal opening at `at` ends, or `None` when the
/// quote opens a lifetime or a label.
fn char_literal_end(source: &str, at: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    if bytes.get(at + 1) == Some(&b'\\') {
        // An escape ('\'', '\n', '\u{..}'): the literal ends at the first
        // quote after the escaped character.
        return find_from(bytes, b"'", at + 3).map(|close| close + 1);
    }
    let width = source.get(at + 1..)?.chars().next()?.len_utf8();
    (bytes.get(at + 1 + width) == Some(&b'\'')).then_some(at + 2 + width)
}

/// Where the item a `#[cfg(test)]` that ends at `from` gates ends, when
/// that item is a module: after its `;` (out of line) or after the brace
/// that closes its body. `None` when the gated item is no module.
fn test_module_end(code: &[u8], from: usize) -> Option<usize> {
    let mut at = skip_whitespace(code, from);
    // Further attributes of the same item.
    while code.get(at) == Some(&b'#') {
        at = skip_whitespace(code, matching_close(code, at + 1, b'[', b']')?);
    }
    // Its visibility.
    if code.get(at..)?.starts_with(b"pub") {
        at = skip_whitespace(code, at + 3);
        if code.get(at) == Some(&b'(') {
            at = skip_whitespace(code, matching_close(code, at, b'(', b')')?);
        }
    }
    if !code.get(at..)?.starts_with(b"mod")
        || !code.get(at + 3).is_some_and(u8::is_ascii_whitespace)
    {
        return None;
    }
    at += 3;
    // The module's name, then its `;` or its body.
    while code.get(at).is_some_and(|byte| {
        byte.is_ascii_whitespace() || byte.is_ascii_alphanumeric() || *byte == b'_'
    }) {
        at += 1;
    }
    match code.get(at)? {
        b';' => Some(at + 1),
        b'{' => matching_close(code, at, b'{', b'}'),
        _ => None,
    }
}

/// The position just after the `close` that matches the `open` at `at`.
fn matching_close(code: &[u8], at: usize, open: u8, close: u8) -> Option<usize> {
    if code.get(at) != Some(&open) {
        return None;
    }
    let mut depth = 0_usize;
    for (offset, byte) in code.get(at..)?.iter().enumerate() {
        if *byte == open {
            depth += 1;
        } else if *byte == close {
            depth -= 1;
            if depth == 0 {
                return Some(at + offset + 1);
            }
        }
    }
    None
}

/// The first position at or after `at` that is no whitespace.
fn skip_whitespace(code: &[u8], at: usize) -> usize {
    code.get(at..).map_or(at, |rest| {
        at + rest
            .iter()
            .take_while(|byte| byte.is_ascii_whitespace())
            .count()
    })
}

/// When Core's broken members cannot be read, an identity's status is
/// a typed refusal, never a guess: a member that may be Broken never reads
/// `active`.
#[tokio::test]
async fn r67_an_unreadable_broken_member_list_never_reads_active() {
    let alice = identity("personal:alice");
    let core = Arc::new(SeatCore {
        broken_read_fails: true,
        ..SeatCore::default()
    });
    let runtime = runtime_over(None, &core);
    crate::identity_first::core_test_bridge::register_live(
        &runtime,
        &core.core,
        spec(&alice),
        1,
        SessionId::new(),
    )
    .await
    .expect("registered live");

    match runtime.status(&alice).await {
        Err(IdentityRuntimeError::IdentityHistoryUnavailable { detail, .. }) => {
            assert!(detail.contains("broken members"), "{detail}");
        }
        other => panic!("expected a typed history refusal, got {other:?}"),
    }
}

/// The cross-mob door: an internal-only identity refuses a send through its
/// identity door exactly as it refuses an external `meta/send_message`
/// (`NotAddressable`), before any Core step, so a cross-mob send or inject
/// that names it delivers nothing.
#[tokio::test]
async fn an_internal_only_identity_refuses_a_cross_mob_send_as_an_external_send() {
    let alice = identity("personal:alice");
    let core = Arc::new(SeatCore::default());
    let runtime = runtime_over(None, &core);
    let mut internal = spec(&alice);
    internal.addressability = AgentAddressability::InternalOnly;
    crate::identity_first::core_test_bridge::register_live(
        &runtime,
        &core.core,
        internal,
        1,
        SessionId::new(),
    )
    .await
    .expect("registered live");
    let content = meerkat_core::ContentInput::Text("hello".to_string());

    let cross_mob = runtime
        .send_through_identity_door(&alice, alice.as_str(), &content)
        .await;
    let external = runtime
        .send_with_mode_and_interaction_tracked(&alice, &content, HandlingMode::Queue, None)
        .await;

    assert!(
        matches!(cross_mob, Err(IdentityRuntimeError::NotAddressable(_))),
        "{cross_mob:?}"
    );
    assert!(
        matches!(external, Err(IdentityRuntimeError::NotAddressable(_))),
        "{external:?}"
    );
    assert_eq!(core.wakes.load(Ordering::SeqCst), 0);
    assert!(lock(&core.seat_calls).is_empty(), "nothing reached Core");
}

/// A spec placed on a host no gateway has bound.
fn placed(identity: &AgentIdentity) -> DurableAgentSpec {
    DurableAgentSpec {
        placement: Some(meerkat_contracts::WireHostRef(
            "remote-member-host".to_string(),
        )),
        ..spec(identity)
    }
}

/// Placed and externally bound identities under the default pass
/// (`eager_materialize`): a placed identity Core does not hold has no
/// Dormant phase, so the pass creates it through its host, as 0.9.5's
/// default did, and a host that refuses it refuses it at the pass, typed.
/// The other identities are seated as ever.
#[tokio::test]
async fn the_default_pass_creates_a_placed_identity_through_its_host() {
    let core = Arc::new(SeatCore::default());
    let runtime = runtime_over(None, &core);
    let placed_identity = identity("personal:placed");
    let local = identity("personal:local");
    let specs = vec![placed(&placed_identity), spec(&local)];

    let result = crate::identity_first::orchestrator::restore_flow(&runtime, &specs, None, None)
        .await
        .expect("the pass runs");

    assert_eq!(
        core.creates.load(Ordering::SeqCst),
        1,
        "the pass creates the placed identity through its host"
    );
    assert!(
        matches!(
            result.outcomes.get(&placed_identity),
            Some(crate::identity_first::orchestrator::RestoreOutcome::Refused(error))
                if error.to_string().contains(HOST_NOT_BOUND)
        ),
        "Core's denial of the unbound host is the placed identity's own refusal: {:?}",
        result.outcomes.get(&placed_identity)
    );
    let seated = lock(&core.seat_calls)
        .iter()
        .flatten()
        .map(|(identity, _)| identity.clone())
        .collect::<Vec<_>>();
    assert_eq!(seated, vec![local], "the local identity is seated");
}

/// Placed and externally bound identities under the explicit lazy modes: as
/// in 0.9.5, the pass leaves a placed identity Core does not hold for its
/// first use. Nothing is created at the pass, the identity reads dormant
/// (never broken while its host is not bound), and its first use creates it
/// through its host. The other identities are seated as under the default.
#[tokio::test]
async fn a_lazy_pass_leaves_a_placed_identity_for_its_first_use() {
    let core = Arc::new(SeatCore::default());
    let runtime = runtime_over(None, &core);
    let placed_identity = identity("personal:placed");
    let local = identity("personal:local");
    let specs = vec![placed(&placed_identity), spec(&local)];

    let result = crate::identity_first::orchestrator::lazy_register_flow(&runtime, &specs, None)
        .await
        .expect("the pass runs");

    assert_eq!(
        core.creates.load(Ordering::SeqCst),
        0,
        "the lazy pass creates nothing through a host"
    );
    assert!(
        matches!(
            result.outcomes.get(&placed_identity),
            Some(crate::identity_first::orchestrator::RestoreOutcome::Dormant { .. })
        ),
        "the placed identity waits for its first use: {:?}",
        result.outcomes.get(&placed_identity)
    );
    let seated = lock(&core.seat_calls)
        .iter()
        .flatten()
        .map(|(identity, _)| identity.clone())
        .collect::<Vec<_>>();
    assert_eq!(seated, vec![local], "the local identity is seated");
    assert_eq!(
        runtime
            .status(&placed_identity)
            .await
            .expect("the placed identity's status")
            .state,
        IdentityLifecycleState::Dormant,
        "the placed identity reads dormant, not broken"
    );

    // Its first use creates it through its host, which this mob has not
    // bound: Core denies the create.
    let first_use = runtime.materialize(&placed_identity).await;
    assert!(
        first_use
            .as_ref()
            .is_err_and(|error| error.to_string().contains(HOST_NOT_BOUND)),
        "Core's denial of the unbound host is the first use's refusal: {:?}",
        first_use.err()
    );
    assert_eq!(
        core.creates.load(Ordering::SeqCst),
        1,
        "the first use creates the placed identity through its host"
    );
}

/// A placed identity Core holds no member of, with a pending first-boot
/// `Resume` row: Core's declared apply answers it hosted on its first use and
/// leaves the row to the host's own creation, which carries it out. The
/// default pass's create resumes the row's session (its history kept) and
/// settles the row, and so does the first use after a lazy pass. Nothing is
/// created fresh in its place.
///
/// RED on the old tree: the hosted create made a fresh member (here Core
/// denies it, `host is not bound in this mob`), ignored the row's session,
/// and left the row pending, so the next compile read it contradicted.
#[tokio::test]
async fn a_hosted_create_carries_out_the_identitys_first_boot_resume_row() {
    for first_use in [false, true] {
        let placed_identity = identity("personal:placed");
        let session = SessionId::new();
        let state_dir = tempfile::tempdir().expect("state dir");
        FirstBootManifest::write_new(
            state_dir.path(),
            vec![FirstBootRow::Resume {
                identity: placed_identity.clone(),
                session: session.clone(),
            }],
        )
        .expect("the manifest writes");
        let core = Arc::new(SeatCore {
            resumes_onto_session: true,
            ..SeatCore::default()
        });
        let runtime = runtime_over(Some(state_dir.path()), &core);
        let specs = vec![placed(&placed_identity)];

        let created = if first_use {
            let result =
                crate::identity_first::orchestrator::lazy_register_flow(&runtime, &specs, None)
                    .await
                    .expect("the lazy pass runs");
            assert!(
                matches!(
                    result.outcomes.get(&placed_identity),
                    Some(crate::identity_first::orchestrator::RestoreOutcome::Dormant { .. })
                ),
                "the lazy pass leaves the placed identity for its first use: {:?}",
                result.outcomes.get(&placed_identity)
            );
            runtime
                .materialize(&placed_identity)
                .await
                .map(|_| ())
                .map_err(|error| error.to_string())
        } else {
            let result =
                crate::identity_first::orchestrator::restore_flow(&runtime, &specs, None, None)
                    .await
                    .expect("the pass runs");
            match result.outcomes.get(&placed_identity) {
                Some(crate::identity_first::orchestrator::RestoreOutcome::Created {
                    binding,
                    ..
                }) => {
                    assert_eq!(binding.session_id.as_ref(), Some(&session));
                    Ok(())
                }
                other => Err(format!("{other:?}")),
            }
        };

        let path = if first_use {
            "first use"
        } else {
            "default pass"
        };
        assert_eq!(
            created,
            Ok(()),
            "{path}: the hosted create resumes the row's session"
        );
        assert_eq!(
            (
                core.creates.load(Ordering::SeqCst),
                core.resumes.load(Ordering::SeqCst)
            ),
            (0, 1),
            "{path}: one resume, and no fresh create"
        );
        assert_eq!(
            core.session_of(&placed_identity).as_ref(),
            Some(&session),
            "{path}: Core binds the identity on the row's session"
        );
        assert_eq!(
            manifest_rows(state_dir.path()),
            None,
            "{path}: the row is settled and the manifest goes"
        );
        assert!(pending(&runtime).is_empty(), "{path}: no row is pending");
    }
}

/// A placed identity whose pending first-boot `Resume` row names a session
/// Core holds no durable copy of: the hosted create is the row's typed
/// refusal, nothing is created in its place, and the row stays pending.
///
/// RED on the old tree: the hosted create made a fresh member over the row
/// (here Core denies it, `host is not bound in this mob`).
#[tokio::test]
async fn a_hosted_create_over_a_first_boot_row_core_holds_no_copy_of_is_refused() {
    let placed_identity = identity("personal:placed");
    let session = SessionId::new();
    let state_dir = tempfile::tempdir().expect("state dir");
    FirstBootManifest::write_new(
        state_dir.path(),
        vec![FirstBootRow::Resume {
            identity: placed_identity.clone(),
            session: session.clone(),
        }],
    )
    .expect("the manifest writes");
    let core = Arc::new(SeatCore {
        resumes_onto_session: true,
        unheld: vec![session.clone()],
        ..SeatCore::default()
    });
    let runtime = runtime_over(Some(state_dir.path()), &core);
    let specs = vec![placed(&placed_identity)];

    let result = crate::identity_first::orchestrator::restore_flow(&runtime, &specs, None, None)
        .await
        .expect("the pass runs");

    match result.outcomes.get(&placed_identity) {
        Some(crate::identity_first::orchestrator::RestoreOutcome::Refused(error)) => assert!(
            matches!(
                error.as_ref(),
                IdentityRuntimeError::FirstBootRowRefused {
                    identity,
                    kind: crate::identity_first::first_boot_manifest::FirstBootRowKind::Resume,
                    manifest_session,
                    core_session: None,
                    ..
                } if *identity == placed_identity && *manifest_session == session
            ),
            "{error:?}"
        ),
        other => panic!("the row's typed refusal is the identity's outcome: {other:?}"),
    }
    assert_eq!(
        core.creates.load(Ordering::SeqCst),
        0,
        "nothing is created in the row's place"
    );
    assert_eq!(
        core.session_of(&placed_identity),
        None,
        "Core binds nothing"
    );
    assert_eq!(
        pending(&runtime),
        vec![placed_identity],
        "the row stays pending"
    );
}

/// The member id Meta hands the mob as host-pending for `identity`.
fn member_id(identity: &AgentIdentity) -> meerkat_mob::AgentIdentity {
    crate::member_comms_id::roster_member_id_for_identity(identity.as_str())
}

fn text(content: &str) -> meerkat_core::ContentInput {
    meerkat_core::ContentInput::Text(content.to_string())
}

/// A first-boot `Resume` row whose session Core keeps archived (a directory
/// an earlier release wrote, whose members it retired): Core's declaration
/// revives the identity on that session, Dormant, with nothing built, and
/// that seat ends the host preparation Meta asked Core for at start. The row
/// is done, and the identity's next delivery goes through.
#[tokio::test]
async fn an_archived_first_boot_session_revived_dormant_ends_its_host_preparation() {
    let alice = identity("personal:alice");
    let session = SessionId::new();
    let state_dir = tempfile::tempdir().expect("state dir");
    FirstBootManifest::write_new(
        state_dir.path(),
        vec![FirstBootRow::Resume {
            identity: alice.clone(),
            session: session.clone(),
        }],
    )
    .expect("the manifest writes");
    let core = Arc::new(SeatCore {
        archived: vec![session.clone()],
        resumes_onto_session: true,
        admits_deliveries: true,
        ..SeatCore::default()
    });
    lock(&core.host_pending).insert(alice.clone());
    let runtime = runtime_over(Some(state_dir.path()), &core);
    runtime.seed_host_pending([member_id(&alice)]);

    let outcomes = roster_pass(&runtime, &[spec(&alice)], None).await;

    assert!(
        pass_refusal(&outcomes[&alice]).is_none(),
        "{:?}",
        outcomes[&alice]
    );
    assert_eq!(
        core.session_of(&alice),
        Some(session.clone()),
        "the identity is seated on its own session"
    );
    assert_eq!(
        core.builds.load(Ordering::SeqCst),
        0,
        "the revival builds nothing"
    );
    assert!(
        lock(&core.materialized).is_empty(),
        "the revival is Dormant"
    );
    assert!(
        !lock(&core.host_pending).contains(&alice),
        "the declaration ended the host preparation Meta asked for"
    );
    assert!(
        manifest_rows(state_dir.path()).is_none(),
        "the row is done and the manifest gone"
    );
    runtime
        .send(&alice, &text("hello"))
        .await
        .expect("the identity's next delivery goes through");
    assert_eq!(lock(&core.delivered).len(), 1, "one delivery");
}

/// A runtime whose Core retired `identity`'s built member at generation 0
/// and keeps its session archived, as Core does once it retired a built
/// member: the identity's next declaration revives it on that session.
/// Answers Core, the runtime and the session.
async fn retired_on_archived_session(
    identity: &AgentIdentity,
) -> (Arc<SeatCore>, Arc<IdentityRuntime>, SessionId) {
    let session = SessionId::new();
    let core = Arc::new(SeatCore {
        archived: vec![session.clone()],
        resumes_onto_session: true,
        admits_deliveries: true,
        ..SeatCore::default()
    });
    let runtime = runtime_over(None, &core);
    let runtime_id = register_live(&runtime, &core.core, spec(identity), 0, session.clone())
        .await
        .expect("registered live");
    core.core.retire(&runtime_id);
    (core, runtime, session)
}

/// An explicit materialization (a warm materializes the same way) of an
/// identity whose built member Core retired: the materialization's
/// declaration has Core revive the identity on its archived session, Dormant
/// and unbuilt, and Core's wake then builds it. The member is built once,
/// by the wake, and nothing is released.
#[tokio::test]
async fn a_materialization_over_an_archived_session_builds_the_member_once() {
    let alice = identity("personal:alice");
    let (core, runtime, session) = retired_on_archived_session(&alice).await;

    let scope = runtime
        .materialize(&alice)
        .await
        .expect("the materialization succeeds")
        .expect("a session-backed member has a delivery scope");

    assert_eq!(scope.session_id(), &session, "the member keeps its session");
    assert_eq!(
        scope.generation(),
        meerkat_mob::Generation::new(1),
        "at the next generation"
    );
    assert_eq!(
        core.resumes.load(Ordering::SeqCst),
        0,
        "Meta resumes nothing: Core's declaration revives the identity"
    );
    assert!(lock(&core.releases).is_empty(), "nothing is released");
    assert_eq!(
        core.wakes.load(Ordering::SeqCst),
        1,
        "Core's wake is asked once"
    );
    assert_eq!(
        core.builds.load(Ordering::SeqCst),
        1,
        "the member is built once, by the wake"
    );
}

/// A respawn of a built member: Core retires it and keeps its session
/// archived, the respawn's declaration has Core revive the identity on that
/// session, and the respawn's wake builds it. The respawn builds the member
/// once and releases nothing.
#[tokio::test]
async fn a_respawn_of_a_built_member_builds_it_once() {
    let alice = identity("personal:alice");
    let session = SessionId::new();
    let core = Arc::new(SeatCore {
        archived: vec![session.clone()],
        resumes_onto_session: true,
        ..SeatCore::default()
    });
    let runtime = runtime_over(None, &core);
    register_live(&runtime, &core.core, spec(&alice), 0, session.clone())
        .await
        .expect("registered live");
    runtime
        .materialize(&alice)
        .await
        .expect("the member is built");
    let built = core.builds.load(Ordering::SeqCst);
    assert_eq!(built, 1, "the member's first build");

    let scope = runtime
        .respawn(&alice)
        .await
        .expect("the respawn succeeds")
        .expect("a session-backed member has a delivery scope");

    assert_eq!(
        scope.session_id(),
        &session,
        "the respawn keeps the session"
    );
    assert_eq!(
        scope.generation(),
        meerkat_mob::Generation::new(1),
        "at the next generation"
    );
    assert_eq!(
        core.resumes.load(Ordering::SeqCst),
        0,
        "Meta resumes nothing: Core's declaration revives the identity"
    );
    assert!(lock(&core.releases).is_empty(), "nothing is released");
    assert_eq!(
        core.builds.load(Ordering::SeqCst) - built,
        1,
        "the respawn builds the member once, by its wake"
    );
}

/// A respawn of a built member tells the agent customizer `after_create`
/// once, from the lifecycle projection, with the session the respawn keeps:
/// the archived session its declaration has Core revive the identity on,
/// never a fresh one. Counted from the projection's settle before the
/// respawn, the customizer hears of one seat only, the respawn's. The build
/// half of the same respawn is `a_respawn_of_a_built_member_builds_it_once`.
#[tokio::test]
async fn a_respawn_of_a_built_member_tells_after_create_once_of_the_session_it_keeps() {
    let alice = identity("personal:alice");
    let session = SessionId::new();
    let core = Arc::new(SeatCore {
        archived: vec![session.clone()],
        resumes_onto_session: true,
        ..SeatCore::default()
    });
    core.core.keep_journal();
    let runtime = runtime_over(None, &core);
    // As the identity context that serves a runtime binds it.
    runtime.bind_lifecycle_projection(None);
    let customizer = ToldSeats::answering();
    runtime
        .set_agent_customizer(Some(Arc::clone(&customizer) as Arc<dyn AgentCustomizer>))
        .await;
    register_live(&runtime, &core.core, spec(&alice), 0, session.clone())
        .await
        .expect("registered live");
    runtime
        .materialize(&alice)
        .await
        .expect("the member is built");
    runtime.settle_lifecycle_projection().await;
    let told_before = customizer.told().len();

    let scope = runtime
        .respawn(&alice)
        .await
        .expect("the respawn succeeds")
        .expect("a session-backed member has a delivery scope");
    runtime.settle_lifecycle_projection().await;

    assert_eq!(
        scope.session_id(),
        &session,
        "the respawn keeps the session"
    );
    assert_eq!(
        scope.generation(),
        meerkat_mob::Generation::new(1),
        "at the next generation"
    );
    assert_eq!(
        core.resumes.load(Ordering::SeqCst),
        0,
        "the respawn's declaration has Core revive the identity on its archived session: \
         Meta resumes nothing"
    );
    let told = customizer.told();
    assert_eq!(
        told[told_before..],
        [(alice, session)],
        "the respawn tells after_create once, of the session it keeps: {told:?}"
    );
}

/// A delivery to an identity whose built member Core retired: the
/// declaration before its admission has Core revive the identity on its
/// archived session, and Core's dispatch builds the member for the delivery.
/// The member is built once, and nothing is released.
#[tokio::test]
async fn a_delivery_over_an_archived_session_builds_the_member_once() {
    let alice = identity("personal:alice");
    let (core, runtime, _session) = retired_on_archived_session(&alice).await;

    runtime
        .send(&alice, &text("hello"))
        .await
        .expect("the delivery goes through");

    assert_eq!(
        core.resumes.load(Ordering::SeqCst),
        0,
        "Meta resumes nothing: Core's declaration revives the identity"
    );
    assert!(lock(&core.releases).is_empty(), "nothing is released");
    assert_eq!(
        lock(&core.delivered).as_slice(),
        [AgentRuntimeId::parse("rt:personal:alice:1").expect("a runtime id")],
        "the delivery goes to the revived member"
    );
    assert_eq!(
        core.builds.load(Ordering::SeqCst),
        1,
        "the member is built once, for the delivery"
    );
}

/// The roster pass's declaration of an identity whose built member Core
/// retired has Core revive it on its archived session, Dormant: the pass
/// makes nothing live, so the identity is a Dormant member on its own
/// session, built at its first delivery.
#[tokio::test]
async fn the_roster_pass_revives_an_archived_session_dormant() {
    let alice = identity("personal:alice");
    let (core, runtime, session) = retired_on_archived_session(&alice).await;

    let outcomes = roster_pass(&runtime, &[spec(&alice)], None).await;

    let (_, binding) = pass_binding(&outcomes[&alice])
        .unwrap_or_else(|| panic!("alice is seated: {:?}", outcomes[&alice]));
    assert_eq!(
        binding.session_id,
        Some(session),
        "seated on its own session"
    );
    assert_eq!(
        core.resumes.load(Ordering::SeqCst),
        0,
        "Meta resumes nothing: Core's declaration revives the identity"
    );
    assert!(
        lock(&core.materialized).is_empty(),
        "the member is not materialized after the pass"
    );
    assert_eq!(core.builds.load(Ordering::SeqCst), 0, "nothing is built");
    assert_eq!(
        core.wakes.load(Ordering::SeqCst),
        0,
        "the pass wakes nothing"
    );
}

/// A declared role migration is the identity's declaration's: the
/// registration that declares the new role has Core release the member and
/// then seat its successor on its own session under that role, Dormant, so
/// the registration builds nothing; the materialization after it has Core's
/// wake build the member under the new role, once.
#[tokio::test]
async fn a_declared_migration_then_a_materialization_builds_the_member_once() {
    let alice = identity("personal:alice");
    let previous = meerkat_mob::ProfileName::from("previous");
    let session = SessionId::new();
    let core = Arc::new(SeatCore {
        resumes_onto_session: true,
        migrations: BTreeMap::from([(alice.clone(), previous.clone())]),
        ..SeatCore::default()
    });
    let runtime = runtime_over(None, &core);
    // Core holds alice on her session under her previous role; the roster
    // names her new one.
    bind_live(&core.core, &alice, &previous, 0, Some(session.clone())).expect("Core holds alice");
    runtime
        .register(spec(&alice))
        .await
        .expect("the identity registers");
    assert_eq!(core.restamps.load(Ordering::SeqCst), 1, "one restamp");
    assert_eq!(
        core.resumes.load(Ordering::SeqCst),
        0,
        "Meta resumes nothing: Core's declaration restamps the member"
    );
    assert_eq!(
        lock(&core.releases).as_slice(),
        std::slice::from_ref(&alice),
        "the declaration releases the member before its restamp"
    );
    assert_eq!(
        core.builds.load(Ordering::SeqCst),
        0,
        "the registration builds nothing: the successor is seated Dormant"
    );
    assert!(
        lock(&core.materialized).is_empty(),
        "nothing is materialized before the wake"
    );

    let scope = runtime
        .materialize(&alice)
        .await
        .expect("the materialization succeeds")
        .expect("a session-backed member has a delivery scope");

    assert_eq!(scope.session_id(), &session, "the member keeps its session");
    assert_eq!(
        core.core
            .last_member_binding(&alice)
            .expect("Core's history reads")
            .map(|binding| binding.role().clone()),
        Some(meerkat_mob::ProfileName::from("personal")),
        "the member runs the new role"
    );
    assert_eq!(
        core.restamps.load(Ordering::SeqCst),
        1,
        "the materialization restamps nothing again"
    );
    assert_eq!(
        core.builds.load(Ordering::SeqCst),
        1,
        "the materialization's wake builds the member once"
    );
}

/// A declared role migration of an identity whose first-boot row Core
/// already shows done: the pass declares the row's `Resume` request again
/// with the migration, and Core carries the request out (it ends the host
/// preparation Meta asked for at start, which would refuse the restamp's
/// retirement of the member under its previous role), releases the member
/// and seats its successor on its own session under the new role, Dormant.
/// The identity's next delivery goes through.
#[tokio::test]
async fn a_declared_migration_over_a_done_first_boot_row_ends_its_host_preparation() {
    let alice = identity("personal:alice");
    let previous = meerkat_mob::ProfileName::from("previous");
    let session = SessionId::new();
    let state_dir = tempfile::tempdir().expect("state dir");
    FirstBootManifest::write_new(
        state_dir.path(),
        vec![FirstBootRow::Resume {
            identity: alice.clone(),
            session: session.clone(),
        }],
    )
    .expect("the manifest writes");
    let core = Arc::new(SeatCore {
        resumes_onto_session: true,
        admits_deliveries: true,
        migrations: BTreeMap::from([(alice.clone(), previous.clone())]),
        ..SeatCore::default()
    });
    // Core holds alice on the row's session under her previous role: the
    // row is done.
    bind_live(&core.core, &alice, &previous, 0, Some(session.clone())).expect("Core holds alice");
    lock(&core.host_pending).insert(alice.clone());
    let runtime = runtime_over(Some(state_dir.path()), &core);
    runtime.seed_host_pending([member_id(&alice)]);

    let outcomes = roster_pass(&runtime, &[spec(&alice)], None).await;

    assert!(
        pass_refusal(&outcomes[&alice]).is_none(),
        "{:?}",
        outcomes[&alice]
    );
    assert_eq!(
        core.session_of(&alice),
        Some(session.clone()),
        "the migration keeps the identity's session"
    );
    assert_eq!(
        core.core
            .last_member_binding(&alice)
            .expect("Core's history reads")
            .map(|binding| binding.role().clone()),
        Some(meerkat_mob::ProfileName::from("personal")),
        "the member runs the new role"
    );
    assert!(
        !lock(&core.host_pending).contains(&alice),
        "the declaration ended the host preparation Meta asked for"
    );
    runtime
        .send(&alice, &text("hello"))
        .await
        .expect("the identity's next delivery goes through");
}

/// A reset Core refuses before any of it starts (here: the mob is not
/// running) answers the typed retryable refusal; Core's journal holds no
/// fact of it, so it takes its rotation record back, and no later roster
/// compile or restart resets the identity. A record an earlier reset left
/// is put back as it was.
#[tokio::test]
async fn a_reset_core_refuses_before_it_starts_leaves_no_rotation_to_carry_out() {
    let alice = identity("personal:alice");
    let bob = identity("personal:bob");
    let core = Arc::new(SeatCore {
        refuses_rotation: true,
        ..SeatCore::default()
    });
    core.core.keep_journal();
    let runtime = runtime_over(None, &core);
    for who in [&alice, &bob] {
        register_live(&runtime, &core.core, spec(who), 0, SessionId::new())
            .await
            .expect("registered live");
    }
    // An earlier reset of bob left its record.
    runtime
        .record_rotation_intent(&bob, "reset", Some(0))
        .await
        .expect("record bob's earlier rotation");
    let earlier = runtime
        .pending_rotation_intents()
        .await
        .expect("the records read")
        .remove(&bob)
        .expect("bob's earlier record");

    for who in [&alice, &bob] {
        let refused = runtime.reset(who).await;
        assert!(
            matches!(
                refused,
                Err(IdentityRuntimeError::IdentityWakeRetryable { .. })
            ),
            "{who}: {refused:?}"
        );
    }

    let records = runtime
        .pending_rotation_intents()
        .await
        .expect("the records read");
    assert!(
        !records.contains_key(&alice),
        "the refused reset left no record: {records:?}"
    );
    assert_eq!(
        records.get(&bob),
        Some(&earlier),
        "bob's earlier record is put back as it was"
    );
    // The next compile resets nobody: alice stays as she was.
    let outcomes = roster_pass(&runtime, &[spec(&alice)], None).await;
    assert!(
        pass_refusal(&outcomes[&alice]).is_none(),
        "{:?}",
        outcomes[&alice]
    );
    assert!(
        lock(&core.rotations).is_empty(),
        "Core was asked to rotate nobody"
    );
    assert_eq!(
        core.generation_of(&alice),
        Some(0),
        "alice keeps her generation"
    );
}

/// A roster pass that moves a seated identity to another profile keeps it a
/// member: with no migration declared, Core holds it typed under its
/// previous role and releases its live member, never retiring it. Its
/// deliveries are refused typed and Core's binding stays as it was.
#[tokio::test]
async fn a_reprofiled_identity_without_a_declared_migration_stays_seated_and_held() {
    let alice = identity("personal:alice");
    let session = SessionId::new();
    let core = Arc::new(SeatCore {
        admits_deliveries: true,
        ..SeatCore::default()
    });
    let runtime = runtime_over(None, &core);
    let mut registered = spec(&alice);
    registered.profile = meerkat_mob::ProfileName::from("previous");
    register_live(&runtime, &core.core, registered, 0, session.clone())
        .await
        .expect("registered live");

    let outcome = roster_pass(&runtime, &[spec(&alice)], None)
        .await
        .remove(&alice)
        .expect("the pass answers alice");
    assert!(
        matches!(
            pass_refusal(&outcome),
            Some(IdentityRuntimeError::WakeHeld {
                hold: WakeHold::RoleMigrationNotApplied,
                ..
            })
        ),
        "{outcome:?}"
    );
    assert_eq!(
        lock(&core.releases).as_slice(),
        std::slice::from_ref(&alice),
        "the live member is released"
    );
    assert_eq!(
        core.generation_of(&alice),
        Some(0),
        "never retired: the binding keeps its generation"
    );
    let refused = runtime.send(&alice, &text("hello")).await;
    assert!(
        matches!(
            refused,
            Err(IdentityRuntimeError::WakeHeld {
                hold: WakeHold::RoleMigrationNotApplied,
                ..
            })
        ),
        "{refused:?}"
    );
    assert!(
        lock(&core.delivered).is_empty(),
        "nothing is delivered under the previous role"
    );
    assert_eq!(
        core.session_of(&alice),
        Some(session),
        "the identity keeps its session"
    );
}

/// The same move with the migration declared: Core releases the member and
/// seats its successor on its own session under the new role, Dormant, and
/// its next delivery goes through.
#[tokio::test]
async fn a_reprofiled_identity_with_a_declared_migration_moves_onto_its_own_session() {
    let alice = identity("personal:alice");
    let previous = meerkat_mob::ProfileName::from("previous");
    let session = SessionId::new();
    let core = Arc::new(SeatCore {
        resumes_onto_session: true,
        admits_deliveries: true,
        migrations: BTreeMap::from([(alice.clone(), previous.clone())]),
        ..SeatCore::default()
    });
    let runtime = runtime_over(None, &core);
    let mut registered = spec(&alice);
    registered.profile = previous;
    register_live(&runtime, &core.core, registered, 0, session.clone())
        .await
        .expect("registered live");

    let outcome = roster_pass(&runtime, &[spec(&alice)], None)
        .await
        .remove(&alice)
        .expect("the pass answers alice");

    assert!(pass_refusal(&outcome).is_none(), "{outcome:?}");
    assert_eq!(
        core.session_of(&alice),
        Some(session),
        "the member keeps its session"
    );
    assert_eq!(
        core.core
            .last_member_binding(&alice)
            .expect("Core's history reads")
            .map(|binding| binding.role().clone()),
        Some(meerkat_mob::ProfileName::from("personal")),
        "the member runs the new role"
    );
    assert!(
        lock(&core.materialized).is_empty(),
        "the restamp seats the successor Dormant: the member is not materialized after the pass"
    );
    runtime
        .send(&alice, &text("hello"))
        .await
        .expect("the moved identity's delivery goes through");
}

/// The bootstrap pass's snapshot of one seated identity, `Active` and
/// ready, bound to the runtime's identity entry.
async fn ready_bootstrap_entry(runtime: &IdentityRuntime, identity: &AgentIdentity) {
    let generation =
        runtime.begin_identity_bootstrap_pending(super::IdentityBootstrapMode::EagerMaterialize);
    {
        let mut entries = runtime.entries.write().await;
        let entry = entries
            .get_mut(identity)
            .expect("the entry is bound to the pass");
        entry.spec = spec(identity);
        entry.bootstrap_generation = generation;
    }
    runtime.modify_bootstrap_status(Some(generation), |snapshot| {
        snapshot.identities.insert(
            identity.clone(),
            super::IdentityBootstrapEntry {
                state: super::IdentityBootstrapState::Active,
                error: None,
                restore: None,
            },
        );
        snapshot.complete = true;
        snapshot.refresh_aggregates();
    });
}

/// An explicit materialization or warm that Core refuses with a refusal
/// that clears on its own puts the identity's bootstrap entry back exactly
/// as it was: active, not warming, and the bootstrap still ready. A refusal
/// that does not clear on its own marks it broken.
#[tokio::test]
async fn a_warm_core_refuses_retryably_puts_the_bootstrap_entry_back_as_it_was() {
    let alice = identity("personal:alice");
    let core = Arc::new(SeatCore::default());
    *lock(&core.wake_refusal) = Some(meerkat_mob::MemberWakeRefusal::WakeTimedOut);
    let runtime = runtime_over(None, &core);
    register_live(&runtime, &core.core, spec(&alice), 0, SessionId::new())
        .await
        .expect("registered live");
    ready_bootstrap_entry(&runtime, &alice).await;
    assert!(
        runtime.identity_bootstrap_status().ready,
        "ready before the warm"
    );

    let refused = runtime.materialize(&alice).await;
    assert!(
        matches!(
            refused,
            Err(IdentityRuntimeError::IdentityWakeRetryable { .. })
        ),
        "{refused:?}"
    );
    let refused = runtime.warm_tracked(&alice, None).await;
    assert!(
        matches!(
            refused,
            Err(IdentityRuntimeError::IdentityWakeRetryable { .. })
        ),
        "{refused:?}"
    );
    assert_eq!(core.wakes.load(Ordering::SeqCst), 2, "Core was asked twice");
    let status = runtime.identity_bootstrap_status();
    assert_eq!(
        status.identities.get(&alice).map(|entry| entry.state),
        Some(super::IdentityBootstrapState::Active),
        "the entry is back as it was: {status:?}"
    );
    assert_eq!(status.counts.warming, 0, "{status:?}");
    assert!(status.ready, "the bootstrap stays ready: {status:?}");

    // A refusal that does not clear on its own marks the entry broken.
    *lock(&core.wake_refusal) = Some(meerkat_mob::MemberWakeRefusal::Retiring);
    let refused = runtime.materialize(&alice).await;
    assert!(
        matches!(refused, Err(IdentityRuntimeError::AdmissionFailed { .. })),
        "{refused:?}"
    );
    let status = runtime.identity_bootstrap_status();
    assert_eq!(
        status.identities.get(&alice).map(|entry| entry.state),
        Some(super::IdentityBootstrapState::Broken),
        "{status:?}"
    );
    assert!(!status.ready, "{status:?}");
}

/// A materialization or warm of a member Core already holds, which Core
/// answers with its Broken verdict carrying a durable resume hold, answers
/// the held refusal on the session Core names, and the identity records it.
/// The door's declaration names no session for a member Core already holds
/// (Core keeps it), so the session is read off Core's refusal; a refusal
/// with no session would read as retryable.
#[tokio::test]
async fn a_held_broken_verdict_on_a_seated_member_answers_the_held_refusal() {
    use meerkat_core::service::DurableResumeHold;

    let alice = identity("personal:alice");
    let session = SessionId::new();
    let core = Arc::new(SeatCore::default());
    let runtime = runtime_over(None, &core);
    register_live(&runtime, &core.core, spec(&alice), 0, session.clone())
        .await
        .expect("registered live");
    *lock(&core.wake_rejection) = Some((
        ResumeRejectionKind::Held {
            hold: DurableResumeHold::TailHeldForRecovery,
        },
        session.clone(),
    ));

    let held = runtime.materialize(&alice).await;
    match &held {
        Err(IdentityRuntimeError::WakeHeld {
            session_id,
            hold: WakeHold::DurableResume(DurableResumeHold::TailHeldForRecovery),
            ..
        }) => assert_eq!(session_id, &session, "{held:?}"),
        other => panic!("the materialization answers the held refusal: {other:?}"),
    }
    let held = runtime.warm_tracked(&alice, None).await;
    assert!(
        matches!(
            held,
            Err(IdentityRuntimeError::WakeHeld {
                hold: WakeHold::DurableResume(DurableResumeHold::TailHeldForRecovery),
                ..
            })
        ),
        "the warm answers the held refusal: {held:?}"
    );
    let status = runtime.status(&alice).await.expect("identity status");
    assert_eq!(
        status
            .wake_refusal
            .as_ref()
            .map(|refusal| (refusal.remedy, refusal.session_id.clone())),
        Some((
            crate::identity_first::types::WakeRefusalRemedy::OperatorReview,
            session.clone()
        )),
        "the hold is recorded on Core's session: {status:?}"
    );
}

/// A role move on a session that lives in memory only rotates only the
/// member Core decides it for. The pass's first declaration has Core ask for
/// the move's build, which the pass composes with no lifecycle lock held; a
/// reset that moves the identity meanwhile (here, onto another session under
/// the new role) leaves the move nothing to do: Core decides again on the
/// declaration that carries the build and keeps the member where the reset
/// put it, with no release and no second rotation.
#[tokio::test]
async fn an_in_memory_role_move_skips_a_member_moved_while_its_draft_was_built() {
    let alice = identity("personal:alice");
    let planned = SessionId::new();
    let moved = SessionId::new();
    let core = Arc::new(SeatCore {
        sessions_in_memory: true,
        ..SeatCore::default()
    });
    let runtime = runtime_over(None, &core);
    register_live(&runtime, &core.core, spec(&alice), 0, planned.clone())
        .await
        .expect("registered live");
    runtime
        .set_agent_customizer(Some(Arc::new(MovesWhileBuilding {
            core: Arc::clone(&core),
            identity: alice.clone(),
            session: moved.clone(),
        })))
        .await;
    let mut reviewer = spec(&alice);
    reviewer.profile = meerkat_mob::ProfileName::from("reviewer");

    let outcomes = roster_pass(&runtime, std::slice::from_ref(&reviewer), None).await;

    let (created, _) = pass_binding(&outcomes[&alice]).unwrap_or_else(|| {
        panic!(
            "the stale move is skipped, not refused: {:?}",
            outcomes[&alice]
        )
    });
    assert!(!created, "nothing was rotated or seated");
    assert!(lock(&core.releases).is_empty(), "no release was asked");
    assert_eq!(
        core.rotation_calls.load(Ordering::SeqCst),
        0,
        "no second rotation"
    );
    assert_eq!(
        core.core
            .live_member(&alice)
            .expect("Core's members read")
            .and_then(|live| live.session_id),
        Some(moved),
        "Core keeps the identity where the reset moved it"
    );
}

/// A host customizer that, while it builds `identity`'s draft, moves the
/// identity in Core onto `session` at the next generation, as a reset that
/// ran meanwhile would.
struct MovesWhileBuilding {
    core: Arc<SeatCore>,
    identity: AgentIdentity,
    session: SessionId,
}

#[async_trait::async_trait]
impl AgentCustomizer for MovesWhileBuilding {
    async fn customize_build(
        &self,
        context: &AgentBuildContext,
        spec: &DurableAgentSpec,
        _draft: &mut AgentBuildDraft,
    ) -> Result<(), crate::identity_first::CustomizerError> {
        if context.identity != self.identity {
            return Ok(());
        }
        if let Some(live) = self
            .core
            .core
            .live_member(&self.identity)
            .expect("Core's members read")
        {
            self.core.core.retire(&live.agent_runtime_id);
        }
        bind_live(
            &self.core.core,
            &self.identity,
            &spec.profile,
            1,
            Some(self.session.clone()),
        )
        .expect("Core moves the identity");
        Ok(())
    }
}

/// A seeded identity Core already holds on its first-boot row's session (the
/// row is done, and Core's mark still stands) has its host preparation ended
/// by its declaration: the compile declares the row's `Resume` request again,
/// Core keeps the member and carries the request out in the same step, and
/// the row leaves the manifest. Nothing is seated or rotated.
#[tokio::test]
async fn a_seeded_identity_core_already_holds_has_its_host_preparation_ended() {
    let alice = identity("personal:alice");
    let session = SessionId::new();
    let state_dir = tempfile::tempdir().expect("state dir");
    FirstBootManifest::write_new(
        state_dir.path(),
        vec![FirstBootRow::Resume {
            identity: alice.clone(),
            session: session.clone(),
        }],
    )
    .expect("the manifest writes");
    let core = Arc::new(SeatCore {
        admits_deliveries: true,
        ..SeatCore::default()
    });
    bind_live(
        &core.core,
        &alice,
        &meerkat_mob::ProfileName::from("personal"),
        0,
        Some(session.clone()),
    )
    .expect("Core holds alice");
    lock(&core.host_pending).insert(alice.clone());
    let runtime = runtime_over(Some(state_dir.path()), &core);
    runtime.seed_host_pending([member_id(&alice)]);

    let outcomes = roster_pass(&runtime, &[spec(&alice)], None).await;

    assert!(
        pass_refusal(&outcomes[&alice]).is_none(),
        "{:?}",
        outcomes[&alice]
    );
    assert!(lock(&core.seat_calls).is_empty(), "nothing is seated");
    assert!(
        !lock(&core.host_pending).contains(&alice),
        "the declaration ended the host preparation Meta asked for"
    );
    assert_eq!(core.generation_of(&alice), Some(0), "nothing was rotated");
    assert_eq!(
        FirstBootManifest::read(state_dir.path()).expect("reads"),
        None,
        "the carried-out row leaves the manifest"
    );
}

/// A delivery to an identity whose first-boot row is still owed declares the
/// identity first, with the row's `Resume` request: Core keeps the member it
/// holds on the row's session and carries the request out, which ends its
/// host preparation, so the delivery goes through. Nothing is seated.
#[tokio::test]
async fn a_delivery_to_a_seeded_identity_ends_its_host_preparation_first() {
    let alice = identity("personal:alice");
    let session = SessionId::new();
    let core = Arc::new(SeatCore {
        admits_deliveries: true,
        ..SeatCore::default()
    });
    let runtime = runtime_over(None, &core);
    register_live(&runtime, &core.core, spec(&alice), 0, session.clone())
        .await
        .expect("registered live");
    // The start found the identity's first-boot row, and asked Core to hold
    // the identity for the row's preparation.
    let state_dir = tempfile::tempdir().expect("state dir");
    FirstBootManifest::write_new(
        state_dir.path(),
        vec![FirstBootRow::Resume {
            identity: alice.clone(),
            session: session.clone(),
        }],
    )
    .expect("the manifest writes");
    runtime.set_first_boot_state_dir(Some(state_dir.path().to_path_buf()));
    lock(&core.host_pending).insert(alice.clone());
    runtime.seed_host_pending([member_id(&alice)]);

    runtime
        .send(&alice, &text("hello"))
        .await
        .expect("the delivery goes through");

    assert!(
        !lock(&core.host_pending).contains(&alice),
        "the delivery's declaration ended the mark"
    );
    assert!(lock(&core.seat_calls).is_empty(), "nothing is seated");
    assert_eq!(lock(&core.delivered).len(), 1, "one delivery");
    assert_eq!(
        FirstBootManifest::read(state_dir.path()).expect("reads"),
        None,
        "the carried-out row leaves the manifest"
    );
}

/// The members Meta hands the mob as host-pending before its build name
/// only listed identities Meta seats: a hosted identity (placed, or bound to
/// an external runtime) is never named, since Core seats no such member and
/// no declaration could end its mark.
#[tokio::test]
async fn a_hosted_identity_is_never_handed_to_the_mob_as_host_pending() {
    let local = identity("personal:local");
    let hosted = identity("personal:hosted");
    let state_dir = tempfile::tempdir().expect("state dir");
    FirstBootManifest::write_new(
        state_dir.path(),
        vec![
            FirstBootRow::Resume {
                identity: local.clone(),
                session: SessionId::new(),
            },
            FirstBootRow::Resume {
                identity: hosted.clone(),
                session: SessionId::new(),
            },
        ],
    )
    .expect("the manifest writes");
    let location = crate::identity_first::first_boot_manifest::FirstBootLocation::StateDir(
        state_dir.path().to_path_buf(),
    );

    let pending =
        super::host_pending_members_before_build(Some(&location), &[spec(&local), placed(&hosted)])
            .await;

    assert_eq!(pending, vec![member_id(&local)]);
}

/// A reset whose rotation fails after it may have started keeps its
/// rotation record and answers that the rotation may have started: Core
/// finishes the rotation it started from its own record, so the next compile
/// asks Core for no rotation, and the record stays written.
#[tokio::test]
async fn a_reset_that_may_have_started_keeps_its_record_and_no_compile_rotates_it() {
    let alice = identity("personal:alice");
    let core = Arc::new(SeatCore::default());
    *lock(&core.rotation_failure) = Some(BridgeError::Mob(
        "the retirement ran past its bound".to_string(),
    ));
    let runtime = runtime_over(None, &core);
    register_live(&runtime, &core.core, spec(&alice), 0, SessionId::new())
        .await
        .expect("registered live");

    let failed = runtime.reset(&alice).await;
    let Err(IdentityRuntimeError::Internal(detail)) = &failed else {
        panic!("the reset answers that its rotation may have started: {failed:?}");
    };
    assert!(detail.contains("may have started"), "{detail}");
    assert!(
        runtime
            .pending_rotation_intents()
            .await
            .expect("the records read")
            .contains_key(&alice),
        "the record of a rotation that may have started stays"
    );
    let asked = core.rotation_calls.load(Ordering::SeqCst);

    let _ = roster_pass(&runtime, &[spec(&alice)], None).await;

    assert_eq!(
        core.rotation_calls.load(Ordering::SeqCst),
        asked,
        "the compile asks Core for no rotation"
    );
    assert!(lock(&core.rotations).is_empty(), "nothing was rotated");
    assert!(
        runtime
            .pending_rotation_intents()
            .await
            .expect("the records read")
            .contains_key(&alice),
        "the record stays written"
    );
}

/// A reset whose fresh life spec cannot be lowered is refused before Core
/// is asked: Core's journal holds no fact of it, so its record is taken
/// back, and no later compile resets the identity.
#[tokio::test]
async fn a_reset_whose_spec_cannot_be_lowered_leaves_no_rotation_to_carry_out() {
    let alice = identity("personal:alice");
    let core = Arc::new(SeatCore::default());
    *lock(&core.refuse_before_core) = Some(BridgeError::InvalidInput(
        "lowering the fresh life spec failed".to_string(),
    ));
    core.core.keep_journal();
    let runtime = runtime_over(None, &core);
    register_live(&runtime, &core.core, spec(&alice), 0, SessionId::new())
        .await
        .expect("registered live");

    let refused = runtime.reset(&alice).await;
    assert!(
        matches!(refused, Err(IdentityRuntimeError::Internal(_))),
        "{refused:?}"
    );
    assert!(
        runtime
            .pending_rotation_intents()
            .await
            .expect("the records read")
            .is_empty(),
        "the record of a rotation that never started is taken back"
    );
    let outcomes = roster_pass(&runtime, &[spec(&alice)], None).await;
    assert!(
        pass_refusal(&outcomes[&alice]).is_none(),
        "{:?}",
        outcomes[&alice]
    );
    assert!(
        lock(&core.rotations).is_empty(),
        "Core was asked to rotate nobody"
    );
    assert_eq!(core.generation_of(&alice), Some(0));
}

/// `spec(identity)` owing a kickoff (an initial message): Core spawns its
/// fresh life live instead of seating it Dormant.
fn kickoff_spec(identity: &AgentIdentity) -> DurableAgentSpec {
    DurableAgentSpec {
        initial_message: Some(meerkat_core::ContentInput::Text(
            "Start with your kickoff.".to_string(),
        )),
        ..spec(identity)
    }
}

/// A fresh seat that owes a kickoff is Core's live spawn, not a Dormant
/// seat, and the gateway's build check judges that spawn at the roster pass.
/// Refused for its member route (a build off the launch's member auth
/// binding), it is final and typed: the pass and every delivery answer
/// `HostRejectedBuild`, which does not clear on its own, each with one seat
/// asked of Core. Nothing retries the seat in a loop, and nothing is seated
/// or woken.
#[tokio::test]
async fn a_member_route_refusal_of_a_kickoff_seat_is_final_and_typed() {
    let alice = identity("personal:alice");
    let core = Arc::new(SeatCore {
        route_refused: vec![alice.clone()],
        admits_deliveries: true,
        ..SeatCore::default()
    });
    let runtime = runtime_over(None, &core);

    let outcomes = roster_pass(&runtime, &[kickoff_spec(&alice)], None).await;
    let refused = pass_refusal(&outcomes[&alice]).expect("the pass's seat is refused");
    assert!(
        matches!(refused, IdentityRuntimeError::HostRejectedBuild { .. }),
        "{refused:?}"
    );
    assert!(!refused.clears_on_its_own(), "{refused:?}");

    for attempt in 1..=2 {
        let delivered = runtime.send(&alice, &text("hello")).await;
        match delivered {
            Err(refused @ IdentityRuntimeError::HostRejectedBuild { .. }) => {
                assert!(
                    !refused.clears_on_its_own(),
                    "delivery {attempt}: {refused:?}"
                );
            }
            other => panic!("delivery {attempt} answers the final refusal: {other:?}"),
        }
    }
    assert_eq!(
        lock(&core.seat_calls).len(),
        3,
        "one seat per declaration (the pass and each delivery), never retried"
    );
    assert!(core.session_of(&alice).is_none(), "nothing was seated");
    assert_eq!(core.wakes.load(Ordering::SeqCst), 0, "nothing was woken");
    assert!(lock(&core.delivered).is_empty(), "nothing was delivered");
}

/// A reset of an identity Core does not hold that owes a kickoff is Core's
/// live spawn of its fresh life. Refused for its member route, it is final
/// and leaves nothing to carry out: none of the rotation started (Core's
/// journal holds no fact of it), so its record is taken back, a restart
/// hands the mob no host-pending identity for it, and the next pass rotates
/// nothing (its seat, the same live spawn, is refused final again).
#[tokio::test]
async fn a_reset_refused_for_the_member_route_leaves_no_rotation_to_carry_out() {
    let alice = identity("personal:alice");
    let core = Arc::new(SeatCore {
        route_refused: vec![alice.clone()],
        ..SeatCore::default()
    });
    core.core.keep_journal();
    let runtime = runtime_over(None, &core);
    let records = Arc::new(crate::runtime::InMemoryMetadataStore::new());
    runtime.set_identity_records(records.clone(), "route-refused".to_string());
    // The pass registers alice; Core refuses her kickoff seat.
    let registered = roster_pass(&runtime, &[kickoff_spec(&alice)], None).await;
    assert!(
        matches!(
            pass_refusal(&registered[&alice]),
            Some(IdentityRuntimeError::HostRejectedBuild { .. })
        ),
        "{:?}",
        registered[&alice]
    );

    match runtime.reset(&alice).await {
        Err(refused @ IdentityRuntimeError::HostRejectedBuild { .. }) => {
            assert!(!refused.clears_on_its_own(), "{refused:?}");
        }
        other => panic!("the reset answers the final refusal: {other:?}"),
    }

    assert!(
        runtime
            .pending_rotation_intents()
            .await
            .expect("the records read")
            .is_empty(),
        "the refused rotation left no record"
    );
    let host_pending =
        super::host_pending_members_before_build(None, &[kickoff_spec(&alice)]).await;
    assert!(
        host_pending.is_empty(),
        "a restart hands the mob no rotation to carry out: {host_pending:?}"
    );

    let outcomes = roster_pass(&runtime, &[kickoff_spec(&alice)], None).await;
    assert!(
        matches!(
            pass_refusal(&outcomes[&alice]),
            Some(IdentityRuntimeError::HostRejectedBuild { .. })
        ),
        "the pass's seat is the same refused spawn: {:?}",
        outcomes[&alice]
    );
    assert_eq!(
        core.rotation_calls.load(Ordering::SeqCst),
        1,
        "the reset asked one rotation, and the passes none"
    );
    assert!(lock(&core.rotations).is_empty(), "nothing was rotated");
    assert!(core.session_of(&alice).is_none(), "nothing was seated");
}

/// A pass whose read of Core's member history fails reports the failure as
/// its own error and is never ready: it does not read every identity as
/// dormant.
#[tokio::test]
async fn a_failed_member_history_read_is_the_passs_own_error() {
    let alice = identity("personal:alice");
    let core = Arc::new(SeatCore::default());
    let runtime = runtime_over(None, &core);
    register_live(&runtime, &core.core, spec(&alice), 0, SessionId::new())
        .await
        .expect("registered live");
    let mode = super::IdentityBootstrapMode::EagerMaterialize;
    let generation = runtime.begin_identity_bootstrap_pending(mode.clone());
    core.history_read_fails.store(true, Ordering::SeqCst);
    let result = crate::identity_first::orchestrator::RestoreFlowResult {
        outcomes: BTreeMap::new(),
        managed_edges: Vec::new(),
        unlisted: BTreeMap::new(),
    };

    runtime
        .install_identity_bootstrap(generation, mode, &[spec(&alice)], &result)
        .await;

    let status = runtime.identity_bootstrap_status();
    assert!(
        status
            .error
            .as_deref()
            .is_some_and(|error| error.contains("member history")),
        "{status:?}"
    );
    assert!(!status.ready, "{status:?}");
}

/// Shutdown joins a delivery whose declaration is inside Core's seat call
/// (Core retired the identity's member, so the delivery's door declares it
/// again first): the join waits until Core answers.
#[tokio::test]
async fn shutdown_waits_for_a_door_declaration_inside_cores_seat_call() {
    let alice = identity("personal:alice");
    let (reached_tx, reached_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let core = Arc::new(SeatCore {
        admits_deliveries: true,
        ..SeatCore::default()
    });
    let runtime = runtime_over(None, &core);
    let live = register_live(&runtime, &core.core, spec(&alice), 0, SessionId::new())
        .await
        .expect("registered live");
    core.core.retire(&live);
    *lock(&core.seat_gate) = Some((reached_tx, release_rx));

    let sender = Arc::clone(&runtime);
    let target = alice.clone();
    let send = tokio::spawn(async move {
        sender
            .send_admission_tracked(&target, None, &text("hello"), HandlingMode::Queue, None)
            .await
    });
    tokio::time::timeout(Duration::from_secs(10), reached_rx)
        .await
        .expect("the door's declaration reaches Core's seat call")
        .expect("the gate signals");

    runtime.close_foreground_operations();
    let joiner = Arc::clone(&runtime);
    let mut join = tokio::spawn(async move { joiner.join_foreground_operations().await });
    assert!(
        tokio::time::timeout(Duration::from_millis(300), &mut join)
            .await
            .is_err(),
        "shutdown waits while the seat call is in Core"
    );

    release_tx.send(()).expect("release the seat call");
    tokio::time::timeout(Duration::from_secs(10), join)
        .await
        .expect("shutdown joins once Core answers")
        .expect("the join task");
    let _ = send.await.expect("the send task");
    assert!(
        core.session_of(&alice).is_some(),
        "Core's seat committed before the join returned"
    );
}

/// Install a retirement hook on `runtime` that records each incarnation it
/// hears of, and answers at once; the record.
fn recording_hook(runtime: &IdentityRuntime) -> Arc<StdMutex<Vec<RetiredIdentityMember>>> {
    let seen = Arc::new(StdMutex::new(Vec::new()));
    let recorded = Arc::clone(&seen);
    let observer: RetiredMemberObserver = Arc::new(move |retired| {
        lock(&recorded).push(retired);
        Box::pin(async { Ok(()) })
    });
    runtime.set_retired_member_observer(Some(observer));
    seen
}

/// A complete roster that no longer lists an identity has Core unlist it:
/// Core's seal retires its member, the identity leaves the registry, and the
/// host's retirement hook hears of that retirement once, from Core's
/// journal through the lifecycle projection. The next complete roster
/// unlists nothing, and the hook hears nothing more.
#[tokio::test]
async fn a_seal_unlisting_reaches_the_retirement_hook_once() {
    let alice = identity("personal:alice");
    let bob = identity("personal:bob");
    let core = Arc::new(SeatCore::default());
    core.core.keep_journal();
    let runtime = runtime_over(None, &core);
    runtime.bind_lifecycle_projection(None);
    for who in [&alice, &bob] {
        register_live(&runtime, &core.core, spec(who), 0, SessionId::new())
            .await
            .expect("registered live");
    }
    let seen = recording_hook(&runtime);
    runtime.settle_lifecycle_projection().await;

    let pass = runtime
        .declare_roster_pass(
            &[spec(&alice)],
            true,
            true,
            None,
            HostedIdentities::CreateNow,
        )
        .await;
    assert!(
        matches!(
            pass.unlisted.get(&bob),
            Some(crate::identity_first::bridge::UnlistedIdentityAnswer::Retired)
        ),
        "{:?}",
        pass.unlisted
    );
    assert!(
        core.core
            .live_member(&bob)
            .expect("Core's member read")
            .is_none(),
        "the seal retires the unlisted member"
    );
    assert!(
        !runtime.entries.read().await.contains_key(&bob),
        "the unlisted identity leaves the registry"
    );
    runtime.settle_lifecycle_projection().await;
    let heard = lock(&seen).clone();
    assert_eq!(heard.len(), 1, "{heard:?}");
    assert_eq!(heard[0].identity, bob);

    let again = runtime
        .declare_roster_pass(
            &[spec(&alice)],
            true,
            true,
            None,
            HostedIdentities::CreateNow,
        )
        .await;
    assert!(again.unlisted.is_empty(), "{:?}", again.unlisted);
    runtime.settle_lifecycle_projection().await;
    assert_eq!(lock(&seen).len(), 1, "the hook hears the retirement once");
}

/// A customizer that changes no build and records each seat it is told of,
/// once `released` reads true: until then each `after_create` waits.
struct ToldSeats {
    released: tokio::sync::watch::Receiver<bool>,
    told: StdMutex<Vec<(AgentIdentity, SessionId)>>,
}

impl ToldSeats {
    /// A customizer that answers at once.
    fn answering() -> Arc<Self> {
        let (release, released) = tokio::sync::watch::channel(true);
        drop(release);
        Arc::new(Self {
            released,
            told: StdMutex::new(Vec::new()),
        })
    }

    /// A customizer whose calls wait until the returned sender sends true.
    fn held() -> (Arc<Self>, tokio::sync::watch::Sender<bool>) {
        let (release, released) = tokio::sync::watch::channel(false);
        (
            Arc::new(Self {
                released,
                told: StdMutex::new(Vec::new()),
            }),
            release,
        )
    }

    fn told(&self) -> Vec<(AgentIdentity, SessionId)> {
        lock(&self.told).clone()
    }
}

#[async_trait::async_trait]
impl AgentCustomizer for ToldSeats {
    async fn customize_build(
        &self,
        _context: &AgentBuildContext,
        _spec: &DurableAgentSpec,
        _draft: &mut AgentBuildDraft,
    ) -> Result<(), crate::identity_first::CustomizerError> {
        Ok(())
    }

    async fn after_create(
        &self,
        identity: &AgentIdentity,
        session_id: &SessionId,
        _context: &crate::mob_handle_runtime::SessionCreatedContext,
    ) -> Result<(), crate::identity_first::CustomizerError> {
        let mut released = self.released.clone();
        let _ = released.wait_for(|released| *released).await;
        lock(&self.told).push((identity.clone(), session_id.clone()));
        Ok(())
    }
}

/// A transcript source that records each session the distiller reads and
/// answers that the store holds no such session, so no model is asked.
#[derive(Default)]
struct ReadSessions(StdMutex<Vec<String>>);

impl ReadSessions {
    fn reads_of(&self, session: &SessionId) -> usize {
        let session = session.to_string();
        lock(&self.0)
            .iter()
            .filter(|read| **read == session)
            .count()
    }
}

#[async_trait::async_trait]
impl TranscriptSource for ReadSessions {
    async fn read(
        &self,
        session_key: &str,
        _from_index: u64,
    ) -> Result<Option<TranscriptSlice>, DistillerError> {
        lock(&self.0).push(session_key.to_string());
        Ok(None)
    }
}

/// The distiller's model client: never asked, since no session it reads
/// holds anything.
struct UnaskedClient;

#[async_trait::async_trait]
impl DistillerClientHandle for UnaskedClient {
    async fn client(&self) -> Result<Arc<dyn meerkat_client::LlmClient>, DistillerError> {
        Ok(Arc::new(meerkat_client::TestClient::default()))
    }

    fn invalidate(&self) {}
}

/// Agent memory over a store in `dir`, whose distiller reads sessions
/// through `reads`, and the taint tracker that marks its reset boundaries.
fn memory_reading(
    dir: &std::path::Path,
    reads: Arc<ReadSessions>,
) -> (AgentMemoryRuntimeInjector, crate::SessionTaintTracker) {
    let store = Arc::new(crate::SqliteAgentMemoryStore::open(dir).expect("open the memory store"));
    let tracker = crate::SessionTaintTracker::new(crate::ContentTrustConfig::default());
    let distiller = Arc::new(DistillerEngine::new(
        DistillerProfile::embedded_default(),
        DistillerConfig {
            enabled: true,
            ..DistillerConfig::default()
        },
        Arc::new(UnaskedClient),
        store.clone(),
        store.clone(),
        reads,
        None,
        None,
        "default",
    ));
    let injector = AgentMemoryRuntimeInjector::new(store, AgentMemoryConfig::default())
        .with_taint_tracker(tracker.clone())
        .with_distiller(distiller);
    (injector, tracker)
}

#[async_trait::async_trait]
impl StewardClientHandle for UnaskedClient {
    async fn client(&self) -> Result<Arc<dyn meerkat_client::LlmClient>, StewardError> {
        Ok(Arc::new(meerkat_client::TestClient::default()))
    }

    fn invalidate(&self) {}
}

/// A source of compaction discards that holds none, and records each session
/// whose semantic-memory scope the distiller drops.
#[derive(Default)]
struct DroppedScopes(StdMutex<Vec<String>>);

impl DroppedScopes {
    /// The sessions whose scope was dropped, in order.
    fn dropped(&self) -> Vec<String> {
        lock(&self.0).clone()
    }
}

#[async_trait::async_trait]
impl CompactionDiscardSource for DroppedScopes {
    async fn read_discards(
        &self,
        _session_key: &str,
        _limit: usize,
    ) -> Result<Vec<DiscardEntry>, DistillerError> {
        Ok(Vec::new())
    }

    async fn drop_scope(&self, session_key: &str) -> Result<usize, DistillerError> {
        lock(&self.0).push(session_key.to_string());
        Ok(1)
    }
}

/// Agent memory over a store in `dir`, whose distiller reads sessions
/// through `reads` and whose steward keeps the exit-interview queue in the
/// same store, and that store.
fn memory_with_exit_interviews(
    dir: &std::path::Path,
    reads: &Arc<ReadSessions>,
) -> (
    AgentMemoryRuntimeInjector,
    Arc<crate::SqliteAgentMemoryStore>,
) {
    memory_with_exit_interviews_over(dir, reads, None)
}

/// [`memory_with_exit_interviews`], whose distiller drops a retired
/// session's semantic-memory scope through `scopes`, when there is one.
fn memory_with_exit_interviews_over(
    dir: &std::path::Path,
    reads: &Arc<ReadSessions>,
    scopes: Option<Arc<dyn CompactionDiscardSource>>,
) -> (
    AgentMemoryRuntimeInjector,
    Arc<crate::SqliteAgentMemoryStore>,
) {
    let store = Arc::new(crate::SqliteAgentMemoryStore::open(dir).expect("open the memory store"));
    let distiller = Arc::new(DistillerEngine::new(
        DistillerProfile::embedded_default(),
        DistillerConfig {
            enabled: true,
            ..DistillerConfig::default()
        },
        Arc::new(UnaskedClient),
        store.clone(),
        store.clone(),
        Arc::clone(reads) as Arc<dyn TranscriptSource>,
        scopes,
        None,
        "default",
    ));
    let steward = Arc::new(StewardEngine::new(
        StewardProfile::embedded_default(),
        StewardConfig {
            enabled: true,
            ..StewardConfig::default()
        },
        Arc::new(UnaskedClient),
        store.clone(),
        Arc::clone(reads) as Arc<dyn TranscriptSource>,
        "default",
    ));
    let injector = AgentMemoryRuntimeInjector::new(store.clone(), AgentMemoryConfig::default())
        .with_distiller(distiller)
        .with_steward(steward);
    (injector, store)
}

/// The exit interviews `store` holds pending for `identity`, each with its
/// cause.
async fn exit_interviews_of(
    store: &crate::SqliteAgentMemoryStore,
    identity: &AgentIdentity,
) -> Vec<String> {
    use crate::memory::StewardStore as _;
    store
        .pending_harvests("default", 64)
        .await
        .expect("read the exit-interview queue")
        .into_iter()
        .filter(|harvest| harvest.identity == identity.as_str())
        .map(|harvest| harvest.cause)
        .collect()
}

/// A retirement hook on `runtime` that records each incarnation it hears
/// of, and answers for an incarnation of `held` only once `gate` has a
/// permit: the projection that calls it waits inside it until then.
fn hook_held_on(
    runtime: &IdentityRuntime,
    held: &AgentIdentity,
    gate: &Arc<tokio::sync::Semaphore>,
) -> Arc<StdMutex<Vec<RetiredIdentityMember>>> {
    let seen = Arc::new(StdMutex::new(Vec::new()));
    let recorded = Arc::clone(&seen);
    let held = held.clone();
    let gate = Arc::clone(gate);
    let observer: RetiredMemberObserver = Arc::new(move |retired| {
        let holds = retired.identity == held;
        lock(&recorded).push(retired);
        let gate = Arc::clone(&gate);
        Box::pin(async move {
            if holds {
                let _ = gate.acquire().await;
            }
            Ok(())
        })
    });
    runtime.set_retired_member_observer(Some(observer));
    seen
}

/// Wait until the hook `seen` records heard of `identity`.
async fn hook_heard_of(seen: &StdMutex<Vec<RetiredIdentityMember>>, identity: &AgentIdentity) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !lock(seen)
            .iter()
            .any(|retired| retired.identity == *identity)
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the hook hears of the identity");
}

/// The causes the hook `seen` records heard for `identity`, each with its
/// generation, in the order heard.
fn heard_of(
    seen: &StdMutex<Vec<RetiredIdentityMember>>,
    identity: &AgentIdentity,
) -> Vec<(String, u64)> {
    lock(seen)
        .iter()
        .filter(|retired| retired.identity == *identity)
        .map(|retired| (retired.cause.as_str().to_string(), retired.generation))
        .collect()
}

/// A role move on a session that lives in memory only rotates the identity
/// onto a fresh session under the roster profile. The retirement hook hears
/// of the incarnation that ended as a role move (`role_move`), never as a
/// reset. The session the move left is distilled once, as a retirement's
/// is, and no reset boundary quarantines what it held: nobody asked for a
/// clean slate.
///
/// RED before a role move had a cause of its own: the move recorded a
/// reset's intent, so the hook heard `reset`, and the session it left was
/// marked a reset boundary.
#[tokio::test]
async fn a_role_move_reaches_the_retirement_hook_as_a_role_move() {
    let alice = identity("personal:alice");
    let planned = SessionId::new();
    let core = Arc::new(SeatCore {
        sessions_in_memory: true,
        ..SeatCore::default()
    });
    core.core.keep_journal();
    let runtime = runtime_over(None, &core);
    // As the identity context that serves a runtime binds it.
    runtime.bind_lifecycle_projection(None);
    register_live(&runtime, &core.core, spec(&alice), 0, planned.clone())
        .await
        .expect("registered live");
    let seen = recording_hook(&runtime);
    let memory_dir = tempfile::tempdir().expect("memory dir");
    let reads = Arc::new(ReadSessions::default());
    let (memory, tracker) = memory_reading(memory_dir.path(), Arc::clone(&reads));
    runtime.set_agent_memory(Some(memory)).await;
    let mut reviewer = spec(&alice);
    reviewer.profile = meerkat_mob::ProfileName::from("reviewer");

    let outcomes = roster_pass(&runtime, std::slice::from_ref(&reviewer), None).await;
    let (created, _) = pass_binding(&outcomes[&alice])
        .unwrap_or_else(|| panic!("the role move rotates the identity: {:?}", outcomes[&alice]));
    assert!(created, "the move rotated the member");
    assert_ne!(
        core.session_of(&alice),
        Some(planned.clone()),
        "the identity runs on a fresh session"
    );
    runtime.settle_lifecycle_projection().await;

    let calls = lock(&seen).clone();
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(calls[0].identity, alice);
    assert_eq!(calls[0].cause.as_str(), "role_move", "{calls:?}");
    assert_eq!(calls[0].generation, 0);
    assert_eq!(calls[0].role, meerkat_mob::ProfileName::from("personal"));
    assert_eq!(calls[0].session_id.as_ref(), Some(&planned));
    assert_eq!(
        reads.reads_of(&planned),
        1,
        "the session the move left is distilled once"
    );
    assert_eq!(
        tracker.evidence_quarantine_reason(&planned.to_string()),
        None,
        "no reset boundary quarantines the session a role move left"
    );
}

/// The first declaration of a runtime whose lifecycle projection has no
/// cursor yet, and cannot start one because the head of Core's journal does
/// not read, is refused, retryable, with nothing asked of Core: a seat Core
/// committed before the projection's first cursor would lie behind it, and
/// its `after_create` would never be told. The next pass, once the head
/// reads, seats the identity, and the projection tells its seat.
///
/// RED before such a declaration was refused: it seated the identity anyway,
/// and the cursor the next one started at the head lay past the seat.
#[tokio::test]
async fn a_declaration_before_the_projection_s_first_cursor_is_refused_then_told() {
    let alice = identity("personal:alice");
    let core = Arc::new(SeatCore {
        journal_head_failures: AtomicUsize::new(1),
        ..SeatCore::default()
    });
    core.core.keep_journal();
    let runtime = runtime_over(None, &core);
    runtime.bind_lifecycle_projection(None);
    let customizer = ToldSeats::answering();
    runtime
        .set_agent_customizer(Some(Arc::clone(&customizer) as Arc<dyn AgentCustomizer>))
        .await;

    let outcomes = roster_pass(&runtime, &[spec(&alice)], None).await;
    let refused = pass_refusal(&outcomes[&alice]).unwrap_or_else(|| {
        panic!(
            "the pass is refused before the projection's first cursor: {:?}",
            outcomes[&alice]
        )
    });
    // Whether the refusal reads retryable is checked last, once the rest of
    // the row has run.
    let retryable = (refused.clears_on_its_own(), format!("{refused:?}"));
    assert_eq!(
        core.apply_calls.load(Ordering::SeqCst),
        0,
        "nothing was asked of Core"
    );

    let outcomes = roster_pass(&runtime, &[spec(&alice)], None).await;
    assert!(
        pass_refusal(&outcomes[&alice]).is_none(),
        "the next pass seats: {:?}",
        outcomes[&alice]
    );
    runtime.settle_lifecycle_projection().await;
    assert_eq!(
        customizer.told(),
        vec![(
            alice.clone(),
            core.session_of(&alice).expect("Core binds the seat")
        )],
        "the seat is told once"
    );
    assert!(retryable.0, "retryable: {}", retryable.1);
}

/// The roster stops listing an identity, and Core refuses its member's
/// retirement before any of it started; then the roster lists the identity
/// again. The pass that lists it takes back what the unlisting recorded for
/// the incarnation it never ended, so a later retirement of that
/// incarnation no Meta verb asked for (a host's own retire) owes nothing:
/// the hook never hears `unlisted` for an identity the roster lists.
///
/// RED before a pass took a stale unlisting back: its record outlived the
/// relist, and the host's retire of the same incarnation reached the hook
/// as `unlisted`.
#[tokio::test]
async fn a_relisted_identity_s_stale_unlisting_owes_nothing() {
    let alice = identity("personal:alice");
    let core = Arc::new(SeatCore {
        refuse_next_retires: AtomicUsize::new(1),
        ..SeatCore::default()
    });
    core.core.keep_journal();
    let runtime = runtime_over(None, &core);
    let roster = Arc::new(MutableRosterProvider::new(vec![spec(&alice)]));
    let context = IdentityFirstRuntimeContext::new(
        Arc::clone(&runtime),
        Arc::clone(&roster) as Arc<dyn RosterProvider>,
        None,
        None,
        None,
    );
    let live = register_live(&runtime, &core.core, spec(&alice), 0, SessionId::new())
        .await
        .expect("registered live");
    let seen = recording_hook(&runtime);

    assert!(roster.remove(&alice));
    let _ = context.refresh_desired_topology().await;
    assert!(
        core.core
            .live_member(&alice)
            .expect("Core's members read")
            .is_some(),
        "Core refused the retire: the member stays live"
    );
    roster.upsert(spec(&alice));
    let _ = context.refresh_desired_topology().await;

    // A retirement of the same incarnation no Meta verb asked for.
    core.core.retire(&live);
    runtime.settle_lifecycle_projection().await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let calls = lock(&seen).clone();
    assert!(
        calls.is_empty(),
        "a listed identity's stale unlisting reaches no hook: {calls:?}"
    );
}

/// A delete's rotation fails in a way that leaves unknown whether Core
/// started it, so what the delete owes stays recorded; a reset Core then
/// refuses before any of it started takes its own record back and puts the
/// delete's back, as it puts back the delete's rotation record. When Core
/// finishes the delete's rotation from its own record, the hook hears of
/// the incarnation it ended as a delete's.
///
/// RED before the refused step put back what the earlier one recorded: the
/// reset's record replaced the delete's, and taking it back left none, so
/// the rotation Core finished reached the hook as a host's own.
#[tokio::test]
async fn a_refused_step_puts_back_what_an_earlier_step_recorded() {
    let alice = identity("personal:alice");
    let core = Arc::new(SeatCore {
        rotation_failure: StdMutex::new(Some(BridgeError::Mob(
            "the rotation's answer was lost".to_string(),
        ))),
        ..SeatCore::default()
    });
    core.core.keep_journal();
    let runtime = runtime_over(None, &core);
    runtime.bind_lifecycle_projection(None);
    register_live(&runtime, &core.core, spec(&alice), 0, SessionId::new())
        .await
        .expect("registered live");
    let seen = recording_hook(&runtime);

    runtime
        .delete_identity_tracked(&alice)
        .await
        .expect_err("the delete's rotation failed");
    core.refuse_next_rotations.store(1, Ordering::SeqCst);
    runtime
        .reset_tracked(&alice)
        .await
        .expect_err("Core refused the reset before any of it started");
    // Core finishes the rotation it started: the retirement ends, and the
    // next incarnation is seated, naming it.
    rotate_live(
        &core.core,
        &alice,
        &spec(&alice).profile,
        1,
        Some(SessionId::new()),
    )
    .expect("Core finishes the delete's rotation");
    runtime.settle_lifecycle_projection().await;
    let calls = lock(&seen).clone();
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(calls[0].generation, 0);
    assert_eq!(calls[0].cause.as_str(), "delete", "{calls:?}");
}

/// A runtime that starts over the projection a stopped predecessor left
/// behind owes that predecessor's side effects: here the `after_create` of
/// a thousand seats. Its settle (what an identity bootstrap waits for before
/// it answers) waits only for the steps the runtime itself takes, so it
/// answers while the thousand calls are still held; once the customizer
/// answers, they all run in the background, each once.
///
/// RED before a predecessor's backlog drained in the background: the settle
/// ran every owed side effect first, so it waited for all thousand calls.
#[tokio::test(flavor = "multi_thread")]
async fn a_predecessor_s_backlog_drains_without_holding_the_settle() {
    const OWED: usize = 1000;
    let core = Arc::new(SeatCore::default());
    core.core.keep_journal();
    let records: Arc<dyn crate::runtime::PersistentMetadataStore> =
        Arc::new(crate::runtime::InMemoryMetadataStore::new());
    let specs = (0..OWED)
        .map(|index| spec(&identity(&format!("personal:owed-{index:04}"))))
        .collect::<Vec<_>>();
    {
        // The predecessor's roster pass seats every identity, and the
        // predecessor stops before its projection ran.
        let predecessor = runtime_over(None, &core);
        predecessor.set_identity_records(Arc::clone(&records), "backlog".to_string());
        let outcomes = roster_pass(&predecessor, &specs, None).await;
        assert!(
            outcomes
                .values()
                .all(|outcome| pass_binding(outcome).is_some()),
            "the predecessor seats every identity"
        );
    }
    let runtime = runtime_over(None, &core);
    runtime.set_identity_records(Arc::clone(&records), "backlog".to_string());
    let (customizer, release) = ToldSeats::held();
    runtime
        .set_agent_customizer(Some(Arc::clone(&customizer) as Arc<dyn AgentCustomizer>))
        .await;
    runtime.bind_lifecycle_projection(None);
    register_all(&runtime, &specs).await;

    tokio::time::timeout(
        Duration::from_secs(10),
        runtime.settle_lifecycle_projection(),
    )
    .await
    .expect("the settle answers while the predecessor's side effects are still owed");
    assert!(
        customizer.told().is_empty(),
        "the owed calls are still held"
    );
    release.send(true).expect("release the owed calls");
    let deadline = tokio::time::Instant::now() + Duration::from_mins(2);
    while customizer.told().len() < OWED {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the backlog did not drain: {} of {OWED} told",
            customizer.told().len()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    let told = customizer
        .told()
        .into_iter()
        .map(|(identity, _)| identity)
        .collect::<BTreeSet<_>>();
    assert_eq!(told.len(), OWED, "each owed seat is told once");
    assert_eq!(customizer.told().len(), OWED, "and no seat twice");
}

/// A reset of an identity whose live member a host made directly on the mob
/// under the identity's member id (no identity-first spawn: its binding
/// carries neither the durable-identity marker nor the identity's label)
/// records nothing for that member: the reset's rotation retires it, and
/// neither the retirement hook nor the identity's memory hears of it.
///
/// RED before a step recorded what it owes only for a member the identity
/// spawned: the reset recorded its intent for the host's member, so the
/// projection told the hook `reset` with that member's runtime id and
/// distilled its session into the identity's memory.
#[tokio::test]
async fn a_reset_over_a_member_the_host_made_records_nothing_for_it() {
    let alice = identity("personal:alice");
    let hosted = SessionId::new();
    let core = Arc::new(SeatCore::default());
    core.core.keep_journal();
    let runtime = runtime_over(None, &core);
    runtime.bind_lifecycle_projection(None);
    register_live(&runtime, &core.core, spec(&alice), 0, hosted.clone())
        .await
        .expect("registered live");
    core.core.mark_host_made(&alice);
    let seen = recording_hook(&runtime);
    let memory_dir = tempfile::tempdir().expect("memory dir");
    let reads = Arc::new(ReadSessions::default());
    let (memory, _tracker) = memory_reading(memory_dir.path(), Arc::clone(&reads));
    runtime.set_agent_memory(Some(memory)).await;

    runtime
        .reset_tracked(&alice)
        .await
        .expect("the reset rotates the identity");
    runtime.settle_lifecycle_projection().await;
    let calls = lock(&seen).clone();
    assert!(calls.is_empty(), "{calls:?}");
    assert_eq!(
        reads.reads_of(&hosted),
        0,
        "nothing of the host's member is distilled into the identity's memory"
    );
}

/// The retirement hook of an incarnation a Meta step ended is called only
/// once that step answered. The projection takes what the step kept for the
/// hook (the registration it read before Core was asked, settled after Core
/// answered) under the identity's lifecycle lock, which the step holds until
/// it answers: a projection that reaches the retirement while the step sits
/// between Core's answer and its own waits for the step.
///
/// RED before the projection took the step's lock: it took what the step
/// kept as soon as Core journaled the retirement, before the step settled
/// it, and called the hook while the step was still in Core.
#[tokio::test(flavor = "multi_thread")]
async fn a_retirement_reaches_the_hook_only_once_its_step_answered() {
    let alice = identity("personal:alice");
    let (reached_tx, reached_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let core = Arc::new(SeatCore {
        retire_gate: StdMutex::new(Some((reached_tx, release_rx))),
        ..SeatCore::default()
    });
    core.core.keep_journal();
    let runtime = runtime_over(None, &core);
    let roster = Arc::new(MutableRosterProvider::new(vec![spec(&alice)]));
    let context = Arc::new(IdentityFirstRuntimeContext::new(
        Arc::clone(&runtime),
        Arc::clone(&roster) as Arc<dyn RosterProvider>,
        None,
        None,
        None,
    ));
    register_live(&runtime, &core.core, spec(&alice), 0, SessionId::new())
        .await
        .expect("registered live");
    let seen = recording_hook(&runtime);

    assert!(roster.remove(&alice));
    let pass = tokio::spawn({
        let context = Arc::clone(&context);
        async move { context.refresh_desired_topology().await }
    });
    tokio::time::timeout(Duration::from_secs(10), reached_rx)
        .await
        .expect("the unlisting reaches Core's retire")
        .expect("the gate signals");
    // Core journaled the retirement; the step has not answered yet.
    let settle = tokio::spawn({
        let runtime = Arc::clone(&runtime);
        async move { runtime.settle_lifecycle_projection().await }
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        lock(&seen).is_empty(),
        "the hook waits for the step: {:?}",
        lock(&seen)
    );
    release_tx.send(()).expect("release the retire");
    let _ = pass.await.expect("the pass task");
    settle.await.expect("the settle task");
    runtime.settle_lifecycle_projection().await;
    let calls = lock(&seen).clone();
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(calls[0].cause.as_str(), "unlisted");
}

/// A page of Core's journal that seats many identities is checked against
/// one read of Core's member history: Core answers a read of one identity's
/// binding by folding the whole history, so a read per seat would have a
/// first boot of thousands of identities read it thousands of times before
/// its bootstrap answered. Each seat is told once.
///
/// RED before a page's seats were checked against one read of the history:
/// each seat read its identity's binding, so a thousand seats read Core's
/// member history a thousand times.
#[tokio::test(flavor = "multi_thread")]
async fn a_page_of_many_seats_reads_core_s_member_history_once() {
    const SEATS: usize = 1000;
    let core = Arc::new(SeatCore::default());
    core.core.keep_journal();
    let runtime = runtime_over(None, &core);
    let customizer = ToldSeats::answering();
    runtime
        .set_agent_customizer(Some(Arc::clone(&customizer) as Arc<dyn AgentCustomizer>))
        .await;
    let specs = (0..SEATS)
        .map(|index| spec(&identity(&format!("personal:seat-{index:04}"))))
        .collect::<Vec<_>>();
    let outcomes = roster_pass(&runtime, &specs, None).await;
    assert!(
        outcomes
            .values()
            .all(|outcome| pass_binding(outcome).is_some()),
        "every identity is seated"
    );
    // From here on, only what the projection reads counts.
    core.history_reads.store(0, Ordering::SeqCst);
    core.binding_reads.store(0, Ordering::SeqCst);
    runtime.bind_lifecycle_projection(None);

    let started = tokio::time::Instant::now();
    tokio::time::timeout(
        Duration::from_mins(1),
        runtime.settle_lifecycle_projection(),
    )
    .await
    .expect("the settle answers");
    let settled_in = started.elapsed();

    assert_eq!(customizer.told().len(), SEATS, "each seat is told once");
    let pages = SEATS.div_ceil(meerkat_mob::IDENTITY_ENTRY_PAGE_MAX);
    let reads =
        core.history_reads.load(Ordering::SeqCst) + core.binding_reads.load(Ordering::SeqCst);
    assert!(
        reads <= pages,
        "{reads} reads of Core's member history for {pages} pages of seats (settled in \
         {settled_in:?})"
    );
}

/// A customizer that changes no build and records the identity of each seat
/// it is told of. Once [`Self::hold_next_build`] armed it, the next build of
/// the identity it holds says that it got there, then waits for the release:
/// the step that builds holds the identity's lifecycle lock all the while.
struct HeldBuild {
    held: AgentIdentity,
    gate: StdMutex<Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>>,
    told: StdMutex<Vec<AgentIdentity>>,
}

impl HeldBuild {
    fn new(held: &AgentIdentity) -> Arc<Self> {
        Arc::new(Self {
            held: held.clone(),
            gate: StdMutex::new(None),
            told: StdMutex::new(Vec::new()),
        })
    }

    /// Hold the next build of the held identity: the receiver says it got
    /// there, and a send on the sender releases it.
    fn hold_next_build(&self) -> (oneshot::Receiver<()>, oneshot::Sender<()>) {
        let (reached, at_build) = oneshot::channel();
        let (release, released) = oneshot::channel();
        *lock(&self.gate) = Some((reached, released));
        (at_build, release)
    }

    fn told(&self) -> Vec<AgentIdentity> {
        lock(&self.told).clone()
    }
}

#[async_trait::async_trait]
impl AgentCustomizer for HeldBuild {
    async fn customize_build(
        &self,
        _context: &AgentBuildContext,
        spec: &DurableAgentSpec,
        _draft: &mut AgentBuildDraft,
    ) -> Result<(), crate::identity_first::CustomizerError> {
        let gate = if spec.identity == self.held {
            lock(&self.gate).take()
        } else {
            None
        };
        if let Some((reached, released)) = gate {
            let _ = reached.send(());
            let _ = released.await;
        }
        Ok(())
    }

    async fn after_create(
        &self,
        identity: &AgentIdentity,
        _session_id: &SessionId,
        _context: &crate::mob_handle_runtime::SessionCreatedContext,
    ) -> Result<(), crate::identity_first::CustomizerError> {
        lock(&self.told).push(identity.clone());
        Ok(())
    }
}

/// A step that holds an identity's lifecycle lock inside the customizer's
/// build (a reset whose `customize_build` does not answer) holds no other
/// identity's lifecycle side effects back: the projection waits for the lock
/// at most a side effect's deadline, tells the identity's seat with a
/// context derived from its spec, and goes on to the seat Core journaled
/// after it.
///
/// RED before the wait for the lock was bounded: the projection waited for
/// the stuck reset, and every side effect journaled after the identity's
/// seat waited with it.
#[tokio::test(flavor = "multi_thread")]
async fn a_step_stuck_in_its_build_holds_no_other_identity_s_side_effects() {
    let alice = identity("personal:alice");
    let bob = identity("personal:bob");
    let core = Arc::new(SeatCore::default());
    core.core.keep_journal();
    let runtime = runtime_over(None, &core);
    runtime.set_lifecycle_effect_limits(Duration::from_millis(200), 1);
    let customizer = HeldBuild::new(&alice);
    runtime
        .set_agent_customizer(Some(Arc::clone(&customizer) as Arc<dyn AgentCustomizer>))
        .await;
    // Alice's seat is journaled; nothing has projected it yet.
    let outcomes = roster_pass(&runtime, &[spec(&alice)], None).await;
    assert!(pass_binding(&outcomes[&alice]).is_some(), "alice is seated");

    // A reset of alice holds her lifecycle lock inside the customizer's
    // build.
    let (at_build, release) = customizer.hold_next_build();
    let reset = tokio::spawn({
        let runtime = Arc::clone(&runtime);
        let alice = alice.clone();
        async move { runtime.reset_tracked(&alice).await }
    });
    tokio::time::timeout(Duration::from_secs(10), at_build)
        .await
        .expect("the reset reaches the build")
        .expect("the build says so");

    // The projection reaches alice's seat while the reset holds her lock,
    // and Core journals bob's seat after it.
    runtime.bind_lifecycle_projection(None);
    let settle = tokio::spawn({
        let runtime = Arc::clone(&runtime);
        async move { runtime.settle_lifecycle_projection().await }
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    let outcomes = roster_pass(&runtime, &[spec(&bob)], None).await;
    assert!(pass_binding(&outcomes[&bob]).is_some(), "bob is seated");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !customizer.told().contains(&bob) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "bob's seat waited for alice's stuck reset: told {:?}",
            customizer.told()
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        customizer.told().contains(&alice),
        "alice's seat is told too, with a context derived from her spec: {:?}",
        customizer.told()
    );
    assert!(
        !reset.is_finished(),
        "the reset is still inside the customizer's build"
    );

    let _ = release.send(());
    reset
        .await
        .expect("the reset task")
        .expect("the reset rotates alice");
    settle.await.expect("the settle task");
}

/// A customizer that changes no build and notes each `after_create` in
/// `order`, as `told {identity}`.
struct OrderedTold {
    order: Arc<StdMutex<Vec<String>>>,
}

#[async_trait::async_trait]
impl AgentCustomizer for OrderedTold {
    async fn customize_build(
        &self,
        _context: &AgentBuildContext,
        _spec: &DurableAgentSpec,
        _draft: &mut AgentBuildDraft,
    ) -> Result<(), crate::identity_first::CustomizerError> {
        Ok(())
    }

    async fn after_create(
        &self,
        identity: &AgentIdentity,
        _session_id: &SessionId,
        _context: &crate::mob_handle_runtime::SessionCreatedContext,
    ) -> Result<(), crate::identity_first::CustomizerError> {
        lock(&self.order).push(format!("told {identity}"));
        Ok(())
    }
}

/// A retirement hook that says when it is first called, and never answers.
fn never_answering_hook(called: oneshot::Sender<()>) -> RetiredMemberObserver {
    let called = Arc::new(StdMutex::new(Some(called)));
    Arc::new(move |_retired| {
        if let Some(called) = lock(&called).take() {
            let _ = called.send(());
        }
        Box::pin(std::future::pending::<Result<(), String>>())
    })
}

/// A retirement hook that says when it is first called, and answers each
/// call once `released` reads true, noting it in `order` then, as
/// `hook {cause} {generation}`.
fn ordering_hook(
    order: &Arc<StdMutex<Vec<String>>>,
    released: tokio::sync::watch::Receiver<bool>,
    called: oneshot::Sender<()>,
) -> RetiredMemberObserver {
    let order = Arc::clone(order);
    let called = Arc::new(StdMutex::new(Some(called)));
    Arc::new(move |retired| {
        if let Some(called) = lock(&called).take() {
            let _ = called.send(());
        }
        let order = Arc::clone(&order);
        let mut released = released.clone();
        Box::pin(async move {
            let _ = released.wait_for(|released| *released).await;
            lock(&order).push(format!(
                "hook {} {}",
                retired.cause.as_str(),
                retired.generation
            ));
            Ok(())
        })
    })
}

/// A predecessor stopped inside the retirement hook of an identity its
/// roster no longer listed, and this runtime's roster lists the identity
/// again: its boot pass seats the identity's next incarnation. The hook the
/// predecessor still owed (`unlisted`, for the incarnation that ended)
/// reaches the host before the next incarnation's `after_create`: one
/// identity's side effects run in journal order across a predecessor's
/// backlog and the steps this runtime takes, so a host never drops what it
/// kept for the unlisted identity after the identity came back.
///
/// RED before the steps of this runtime were held behind an identity's owed
/// backlog: the next incarnation was told while the owed hook still waited,
/// and the hook came after it.
#[tokio::test(flavor = "multi_thread")]
async fn a_predecessor_s_owed_hook_reaches_the_host_before_the_relisted_identity_is_told() {
    let alice = identity("personal:alice");
    let core = Arc::new(SeatCore::default());
    core.core.keep_journal();
    let records: Arc<dyn crate::runtime::PersistentMetadataStore> =
        Arc::new(crate::runtime::InMemoryMetadataStore::new());
    {
        // The predecessor unlists alice, and stops inside her retirement
        // hook: the hook is still owed.
        let predecessor = runtime_over(None, &core);
        predecessor.set_identity_records(Arc::clone(&records), "ordered".to_string());
        let roster = Arc::new(MutableRosterProvider::new(vec![spec(&alice)]));
        let context = IdentityFirstRuntimeContext::new(
            Arc::clone(&predecessor),
            Arc::clone(&roster) as Arc<dyn RosterProvider>,
            None,
            None,
            None,
        );
        register_live(&predecessor, &core.core, spec(&alice), 0, SessionId::new())
            .await
            .expect("registered live");
        let (called, hook_called) = oneshot::channel();
        predecessor.set_retired_member_observer(Some(never_answering_hook(called)));
        assert!(roster.remove(&alice));
        let _ = context.refresh_desired_topology().await;
        tokio::time::timeout(Duration::from_secs(10), hook_called)
            .await
            .expect("the unlisting reaches the hook")
            .expect("the hook says so");
        predecessor
            .stop_lifecycle_projection(tokio::time::Instant::now())
            .await;
    }

    let order = Arc::new(StdMutex::new(Vec::new()));
    let runtime = runtime_over(None, &core);
    runtime.set_identity_records(Arc::clone(&records), "ordered".to_string());
    let (release, released) = tokio::sync::watch::channel(false);
    let (called, hook_called) = oneshot::channel();
    runtime.set_retired_member_observer(Some(ordering_hook(&order, released, called)));
    let roster = Arc::new(MutableRosterProvider::new(vec![spec(&alice)]));
    let context = IdentityFirstRuntimeContext::new(
        Arc::clone(&runtime),
        Arc::clone(&roster) as Arc<dyn RosterProvider>,
        None,
        Some(Arc::new(OrderedTold {
            order: Arc::clone(&order),
        }) as Arc<dyn AgentCustomizer>),
        None,
    );
    // The boot pass seats alice's next incarnation.
    let _ = context.refresh_desired_topology().await;
    assert!(
        core.core
            .live_member(&alice)
            .expect("Core's members read")
            .is_some(),
        "the pass seats alice again"
    );
    let settle = tokio::spawn({
        let runtime = Arc::clone(&runtime);
        async move { runtime.settle_lifecycle_projection().await }
    });
    tokio::time::timeout(Duration::from_secs(10), hook_called)
        .await
        .expect("the owed hook is called")
        .expect("the hook says so");
    tokio::time::sleep(Duration::from_millis(300)).await;
    release.send(true).expect("release the hook");
    tokio::time::timeout(Duration::from_secs(10), settle)
        .await
        .expect("the settle task ends")
        .expect("the settle task");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while lock(&order).len() < 2 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "only {:?} happened",
            lock(&order)
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        lock(&order).clone(),
        vec!["hook unlisted 0".to_string(), format!("told {alice}")],
        "the owed hook reaches the host first"
    );
}

/// A declaration of a runtime whose stored projection cursor is there
/// already is not refused when the head of Core's journal does not read: the
/// seat lies past the stored cursor, so the projection's next preparation
/// reads it, and the seat is told.
///
/// RED before the stored cursor was read first: the failed head read
/// refused the declaration, though the stored cursor made the seat safe.
#[tokio::test(flavor = "multi_thread")]
async fn a_declaration_past_a_stored_cursor_is_not_refused_for_an_unread_head() {
    let alice = identity("personal:alice");
    let core = Arc::new(SeatCore::default());
    core.core.keep_journal();
    let records: Arc<dyn crate::runtime::PersistentMetadataStore> =
        Arc::new(crate::runtime::InMemoryMetadataStore::new());
    {
        // A predecessor stored the projection's cursor.
        let predecessor = runtime_over(None, &core);
        predecessor.set_identity_records(Arc::clone(&records), "stored".to_string());
        predecessor.settle_lifecycle_projection().await;
    }
    core.journal_head_failures.store(1, Ordering::SeqCst);
    let runtime = runtime_over(None, &core);
    runtime.set_identity_records(Arc::clone(&records), "stored".to_string());
    runtime.bind_lifecycle_projection(None);
    let customizer = ToldSeats::answering();
    runtime
        .set_agent_customizer(Some(Arc::clone(&customizer) as Arc<dyn AgentCustomizer>))
        .await;

    let outcomes = roster_pass(&runtime, &[spec(&alice)], None).await;
    assert!(
        pass_binding(&outcomes[&alice]).is_some(),
        "the pass seats alice past the stored cursor: {:?}",
        outcomes[&alice]
    );
    runtime.settle_lifecycle_projection().await;
    let session = core.session_of(&alice).expect("Core binds the seat");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while customizer.told().is_empty() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the seat past the stored cursor was not told"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        customizer.told(),
        vec![(alice.clone(), session)],
        "the seat is told once"
    );
}

/// A projection whose cursor can be neither read nor started, run after run
/// (the head of Core's journal never reads, and no cursor is stored), reads
/// stalled, with what failed, rather than pending: the declarations it
/// refuses would otherwise be its only trace.
///
/// RED before failed preparations counted toward the stall: the status read
/// pending however often they failed.
#[tokio::test]
async fn a_projection_that_cannot_start_reads_stalled() {
    let alice = identity("personal:alice");
    let core = Arc::new(SeatCore {
        journal_head_failures: AtomicUsize::new(usize::MAX),
        ..SeatCore::default()
    });
    core.core.keep_journal();
    let runtime = runtime_over(None, &core);
    runtime.bind_lifecycle_projection(None);
    for _ in 0..3 {
        let outcomes = roster_pass(&runtime, &[spec(&alice)], None).await;
        assert!(
            pass_refusal(&outcomes[&alice]).is_some(),
            "with no cursor stored, the declaration is refused: {:?}",
            outcomes[&alice]
        );
    }
    assert_eq!(
        core.apply_calls.load(Ordering::SeqCst),
        0,
        "nothing was asked of Core"
    );
    let status = runtime.lifecycle_projection_status();
    assert_eq!(
        status.state,
        crate::identity_first::LifecycleProjectionState::Stalled,
        "{status:?}"
    );
    assert!(status.degraded(), "{status:?}");
    assert!(
        status
            .detail
            .as_deref()
            .is_some_and(|detail| detail.contains("head")),
        "the status says what failed: {status:?}"
    );
}

/// A predecessor's backlog whose journal pages keep failing to read reads
/// stalled, with what failed, while the live lane runs fine: the backlog
/// lane's failures count toward the stall as the live lane's do.
///
/// RED before the backlog lane's failures counted: the status read running
/// while the owed side effects never ran.
#[tokio::test]
async fn a_backlog_that_keeps_failing_reads_stalled() {
    let core = Arc::new(SeatCore::default());
    core.core.keep_journal();
    let records: Arc<dyn crate::runtime::PersistentMetadataStore> =
        Arc::new(crate::runtime::InMemoryMetadataStore::new());
    let specs = ["personal:alice", "personal:bob"]
        .map(|name| spec(&identity(name)))
        .to_vec();
    {
        // The predecessor's roster pass seats both identities, and the
        // predecessor stops before its projection ran.
        let predecessor = runtime_over(None, &core);
        predecessor.set_identity_records(Arc::clone(&records), "stalled".to_string());
        let outcomes = roster_pass(&predecessor, &specs, None).await;
        assert!(
            outcomes
                .values()
                .all(|outcome| pass_binding(outcome).is_some()),
            "{outcomes:?}"
        );
    }
    // The journal pages the backlog reads fail from now on; the live lane
    // reads past them.
    core.journal_pages_fail_before
        .store(specs.len() as u64, Ordering::SeqCst);
    let runtime = runtime_over(None, &core);
    runtime.set_identity_records(Arc::clone(&records), "stalled".to_string());
    runtime.settle_lifecycle_projection().await;
    for _ in 0..3 {
        assert!(
            !runtime.run_lifecycle_backlog_once().await,
            "the backlog lane fails"
        );
    }
    let status = runtime.lifecycle_projection_status();
    assert_eq!(
        status.state,
        crate::identity_first::LifecycleProjectionState::Stalled,
        "{status:?}"
    );
    assert_eq!(status.owed_events, specs.len() as u64, "{status:?}");
    assert!(
        status
            .detail
            .as_deref()
            .is_some_and(|detail| detail.contains("journal")),
        "the status says what failed: {status:?}"
    );
}

/// The lifecycle effect failures `store` holds for `mob_id`, oldest first.
async fn failures_of(
    store: &Arc<dyn crate::runtime::PersistentMetadataStore>,
    mob_id: &str,
) -> Vec<crate::runtime::LifecycleEffectFailure> {
    store
        .lifecycle_effect_failures(mob_id, 0, usize::MAX)
        .await
        .expect("read the failures")
}

/// A projection's start keeps the latest lifecycle effect failures the store
/// holds for the mob, at most a bound's worth, and removes the older ones: a
/// host whose hook fails every retirement leaves a bounded record, however
/// many identities retire.
///
/// RED before a start pruned them: every failure stayed, one durable row per
/// failed side effect, for good.
#[tokio::test]
async fn a_projection_s_start_keeps_only_the_latest_failures() {
    /// How many failures a start keeps per mob.
    const KEPT: usize = 1024;
    const OLDER: usize = 5;
    const FIRST_MS: u64 = 1_700_000_000_000;
    let core = Arc::new(SeatCore::default());
    core.core.keep_journal();
    let records: Arc<dyn crate::runtime::PersistentMetadataStore> =
        Arc::new(crate::runtime::InMemoryMetadataStore::new());
    for index in 0..KEPT + OLDER {
        records
            .record_lifecycle_effect_failure(&crate::runtime::LifecycleEffectFailure::new(
                "kept",
                "an-earlier-start",
                index as u64 + 1,
                format!("personal:failed-{index:04}"),
                0,
                crate::runtime::LifecycleEffectKind::AfterCreate,
                3,
                "timed out",
                FIRST_MS + index as u64,
            ))
            .await
            .expect("record a failure");
    }
    let runtime = runtime_over(None, &core);
    runtime.set_identity_records(Arc::clone(&records), "kept".to_string());
    runtime.settle_lifecycle_projection().await;

    let kept = failures_of(&records, "kept").await;
    assert_eq!(kept.len(), KEPT, "a start keeps at most {KEPT}");
    assert!(
        kept.iter()
            .all(|failure| failure.recorded_at_ms >= FIRST_MS + OLDER as u64),
        "the oldest ones go"
    );
}

/// A declaration whose head read fails is refused, retryable, when the
/// stored projection cursor is of another incarnation of the mob (one
/// recreated since): that cursor's journal is gone, so the next preparation
/// starts at the new head, and a seat Core committed before it would never
/// be told. The next pass, once the head reads, seats the identity, and its
/// seat is told.
///
/// RED before the stored cursor's incarnation was checked: any stored cursor
/// let the declaration go ahead.
#[tokio::test(flavor = "multi_thread")]
async fn a_declaration_past_another_incarnation_s_cursor_is_refused_for_an_unread_head() {
    let alice = identity("personal:alice");
    let core = Arc::new(SeatCore::default());
    core.core.keep_journal();
    let records: Arc<dyn crate::runtime::PersistentMetadataStore> =
        Arc::new(crate::runtime::InMemoryMetadataStore::new());
    // The cursor an earlier incarnation of the mob left.
    records
        .set_lifecycle_projection_cursor(&crate::runtime::LifecycleProjectionCursor::new(
            "recreated",
            "an-earlier-incarnation",
            7,
            1_700_000_000_000,
        ))
        .await
        .expect("store the cursor");
    core.journal_head_failures.store(1, Ordering::SeqCst);
    let runtime = runtime_over(None, &core);
    runtime.set_identity_records(Arc::clone(&records), "recreated".to_string());
    runtime.bind_lifecycle_projection(None);
    let customizer = ToldSeats::answering();
    runtime
        .set_agent_customizer(Some(Arc::clone(&customizer) as Arc<dyn AgentCustomizer>))
        .await;

    let outcomes = roster_pass(&runtime, &[spec(&alice)], None).await;
    let Some(refused) = pass_refusal(&outcomes[&alice]) else {
        panic!(
            "a cursor of another incarnation makes no seat safe: {:?}",
            outcomes[&alice]
        );
    };
    // Whether the refusal reads retryable is checked last, once the rest of
    // the row has run.
    let retryable = (refused.clears_on_its_own(), format!("{refused:?}"));
    assert!(
        lock(&core.seat_calls).is_empty(),
        "nothing was asked of Core"
    );
    assert_eq!(core.apply_calls.load(Ordering::SeqCst), 0);

    let outcomes = roster_pass(&runtime, &[spec(&alice)], None).await;
    assert!(
        pass_binding(&outcomes[&alice]).is_some(),
        "the next pass seats: {:?}",
        outcomes[&alice]
    );
    runtime.settle_lifecycle_projection().await;
    assert_eq!(
        customizer.told(),
        vec![(
            alice.clone(),
            core.session_of(&alice).expect("Core binds the seat")
        )],
        "the seat is told once"
    );
    assert!(retryable.0, "retryable: {}", retryable.1);
}

/// A predecessor over `core` and `records`, for the mob `mob_id`, that
/// unlists `identity` (Core holds it live) and stops inside its retirement
/// hook: the hook stays owed to the next runtime over the same state.
async fn stop_inside_an_owed_unlisted_hook(
    core: &Arc<SeatCore>,
    records: &Arc<dyn crate::runtime::PersistentMetadataStore>,
    mob_id: &str,
    identity: &AgentIdentity,
) {
    let predecessor = runtime_over(None, core);
    predecessor.set_identity_records(Arc::clone(records), mob_id.to_string());
    let roster = Arc::new(MutableRosterProvider::new(vec![spec(identity)]));
    let context = IdentityFirstRuntimeContext::new(
        Arc::clone(&predecessor),
        Arc::clone(&roster) as Arc<dyn RosterProvider>,
        None,
        None,
        None,
    );
    register_live(
        &predecessor,
        &core.core,
        spec(identity),
        0,
        SessionId::new(),
    )
    .await
    .expect("registered live");
    let (called, hook_called) = oneshot::channel();
    predecessor.set_retired_member_observer(Some(never_answering_hook(called)));
    assert!(roster.remove(identity));
    let _ = context.refresh_desired_topology().await;
    tokio::time::timeout(Duration::from_secs(10), hook_called)
        .await
        .expect("the unlisting reaches the hook")
        .expect("the hook says so");
    predecessor
        .stop_lifecycle_projection(tokio::time::Instant::now())
        .await;
}

/// A predecessor stopped inside the hook it owed an identity its roster no
/// longer listed, and this runtime's boot pass lists the identity again.
/// The settle the bootstrap waits for answers while that owed hook is still
/// held by the host: a boot never waits for a predecessor's backlog. The
/// relisted identity's `after_create` follows in the background, after the
/// owed hook, in journal order.
///
/// RED before the settle held such a page for the live task: the settle
/// waited for the backlog to run past the identity, so for the held hook.
#[tokio::test(flavor = "multi_thread")]
async fn a_boot_step_on_an_owed_identity_settles_without_waiting_for_the_backlog() {
    let alice = identity("personal:alice");
    let core = Arc::new(SeatCore::default());
    core.core.keep_journal();
    let records: Arc<dyn crate::runtime::PersistentMetadataStore> =
        Arc::new(crate::runtime::InMemoryMetadataStore::new());
    stop_inside_an_owed_unlisted_hook(&core, &records, "unwaited", &alice).await;

    let order = Arc::new(StdMutex::new(Vec::new()));
    let runtime = runtime_over(None, &core);
    runtime.set_identity_records(Arc::clone(&records), "unwaited".to_string());
    let (release, released) = tokio::sync::watch::channel(false);
    let (called, hook_called) = oneshot::channel();
    runtime.set_retired_member_observer(Some(ordering_hook(&order, released, called)));
    let roster = Arc::new(MutableRosterProvider::new(vec![spec(&alice)]));
    let context = IdentityFirstRuntimeContext::new(
        Arc::clone(&runtime),
        Arc::clone(&roster) as Arc<dyn RosterProvider>,
        None,
        Some(Arc::new(OrderedTold {
            order: Arc::clone(&order),
        }) as Arc<dyn AgentCustomizer>),
        None,
    );
    // The boot pass seats alice's next incarnation.
    let _ = context.refresh_desired_topology().await;
    tokio::time::timeout(Duration::from_secs(10), hook_called)
        .await
        .expect("the owed hook is called")
        .expect("the hook says so");

    tokio::time::timeout(
        Duration::from_secs(5),
        runtime.settle_lifecycle_projection(),
    )
    .await
    .expect("the settle answers while the owed hook is held");
    assert!(
        lock(&order).is_empty(),
        "nothing of alice is told before the owed hook ran: {:?}",
        lock(&order)
    );

    release.send(true).expect("release the hook");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while lock(&order).len() < 2 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "only {:?} happened",
            lock(&order)
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        lock(&order).clone(),
        vec!["hook unlisted 0".to_string(), format!("told {alice}")],
        "the owed hook reaches the host first"
    );
}

/// A customizer that puts a system prompt in each build and records each
/// seat it is told of, with the system prompt the seat's context reports.
#[derive(Default)]
struct PromptedBuild {
    told: StdMutex<Vec<(AgentIdentity, Option<String>)>>,
}

impl PromptedBuild {
    fn told(&self) -> Vec<(AgentIdentity, Option<String>)> {
        lock(&self.told).clone()
    }
}

#[async_trait::async_trait]
impl AgentCustomizer for PromptedBuild {
    async fn customize_build(
        &self,
        _context: &AgentBuildContext,
        spec: &DurableAgentSpec,
        draft: &mut AgentBuildDraft,
    ) -> Result<(), crate::identity_first::CustomizerError> {
        draft.system_prompt = Some(format!("built for {}", spec.identity));
        Ok(())
    }

    async fn after_create(
        &self,
        identity: &AgentIdentity,
        _session_id: &SessionId,
        context: &crate::mob_handle_runtime::SessionCreatedContext,
    ) -> Result<(), crate::identity_first::CustomizerError> {
        lock(&self.told).push((identity.clone(), context.system_prompt.clone()));
        Ok(())
    }
}

/// One roster pass moves one identity onto a fresh session under a new role
/// and seats another. Core's declared apply realizes the move first, which
/// journals the moved identity's new seat, and seats the rest after it;
/// while Core still holds that seat, the pass holds both identities'
/// lifecycle locks. The projection that reaches the moved identity's seat
/// waits for its lock at most its deadline, and tells the seat the build the
/// pass composed for it.
///
/// RED before a seat's build was bound as soon as Core answered it: the
/// step bound every build only once all of it had answered, so the
/// projection told the seat a context derived from its spec, with no system
/// prompt.
#[tokio::test(flavor = "multi_thread")]
async fn a_long_declaration_s_moved_seat_is_told_its_own_build_while_core_seats_the_rest() {
    let alice = identity("personal:alice");
    let bob = identity("personal:bob");
    let (reached, at_seat) = oneshot::channel();
    let (release, released) = oneshot::channel();
    let core = Arc::new(SeatCore {
        sessions_in_memory: true,
        ..SeatCore::default()
    });
    core.core.keep_journal();
    let runtime = runtime_over(None, &core);
    runtime.bind_lifecycle_projection(None);
    runtime.set_lifecycle_effect_limits(Duration::from_millis(200), 1);
    let customizer = Arc::new(PromptedBuild::default());
    runtime
        .set_agent_customizer(Some(Arc::clone(&customizer) as Arc<dyn AgentCustomizer>))
        .await;
    register_live(&runtime, &core.core, spec(&bob), 0, SessionId::new())
        .await
        .expect("registered live");
    // Bob's seat at rest is told before the pass.
    runtime.settle_lifecycle_projection().await;
    let told_before = customizer.told().len();
    *lock(&core.seat_gate) = Some((reached, released));
    let mut reviewer = spec(&bob);
    reviewer.profile = meerkat_mob::ProfileName::from("reviewer");

    let pass = tokio::spawn({
        let runtime = Arc::clone(&runtime);
        let specs = vec![spec(&alice), reviewer];
        async move {
            roster_pass(&runtime, &specs, None)
                .await
                .into_iter()
                .map(|(identity, outcome)| {
                    (identity, pass_refusal(&outcome).map(ToString::to_string))
                })
                .collect::<Vec<_>>()
        }
    });
    tokio::time::timeout(Duration::from_secs(10), at_seat)
        .await
        .expect("Core seats alice once it moved bob")
        .expect("the gate says so");
    // Core journaled bob's move; the pass holds both locks while Core still
    // seats alice.
    tokio::time::timeout(
        Duration::from_secs(10),
        runtime.settle_lifecycle_projection(),
    )
    .await
    .expect("the settle answers");
    let told = customizer.told().split_off(told_before);
    let bob_told = told
        .iter()
        .find(|(identity, _)| *identity == bob)
        .map(|(_, prompt)| prompt.clone());

    release.send(()).expect("release the seat");
    let outcomes = pass.await.expect("the pass task");
    assert!(
        outcomes.iter().all(|(_, refused)| refused.is_none()),
        "the pass seats alice and moves bob: {outcomes:?}"
    );
    assert_eq!(
        bob_told,
        Some(Some(format!("built for {bob}"))),
        "the moved seat is told its own build: {told:?}"
    );
}

/// A reset whose rotation Core carried out, answered with an error the
/// bridge raised after it (a wrapper's `InvalidInput`, here): Core's journal
/// holds the rotation's facts after the head the reset read before it asked
/// Core, so the reset keeps what it recorded, and the retirement hook hears
/// of the incarnation the rotation ended as a reset's.
///
/// RED before a step's outcome was read from Core's journal: an
/// `InvalidInput` answer read as "nothing of the rotation started", so the
/// reset took its record back, and the rotation reached the hook as one no
/// Meta verb asked for (`rotated`).
#[tokio::test]
async fn a_rotation_that_started_before_its_error_keeps_its_intent() {
    let alice = identity("personal:alice");
    let core = Arc::new(SeatCore {
        answers_after_rotating: StdMutex::new(Some(BridgeError::InvalidInput(
            "the wrapper refused Core's answer".to_string(),
        ))),
        ..SeatCore::default()
    });
    core.core.keep_journal();
    let runtime = runtime_over(None, &core);
    runtime.bind_lifecycle_projection(None);
    register_live(&runtime, &core.core, spec(&alice), 0, SessionId::new())
        .await
        .expect("registered live");
    let seen = recording_hook(&runtime);

    runtime
        .reset_tracked(&alice)
        .await
        .expect_err("the bridge answered the rotation with an error");
    assert_eq!(
        core.generation_of(&alice),
        Some(1),
        "Core carried the rotation out"
    );
    runtime.settle_lifecycle_projection().await;
    let calls = lock(&seen).clone();
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(calls[0].generation, 0);
    assert_eq!(calls[0].cause.as_str(), "reset", "{calls:?}");
}

/// A reset the bridge answered with an error before it asked Core (a `Mob`
/// error, which may come before or after Core started): no fact of the
/// identity's member follows the head the reset read before it asked, so
/// nothing of it reached Core, and the reset takes its record back. A host's
/// own rotation of the same incarnation then reaches the retirement hook as
/// what it is (`rotated`), never as a reset.
///
/// RED before a step's outcome was read from Core's journal: a `Mob` error
/// read as "the rotation may have started", so the reset's record stayed,
/// and the host's rotation reached the hook as a reset.
#[tokio::test]
async fn a_rotation_that_never_reached_core_takes_its_intent_back() {
    let alice = identity("personal:alice");
    let core = Arc::new(SeatCore {
        refuse_before_core: StdMutex::new(Some(BridgeError::Mob(
            "the bridge gave up before it asked Core".to_string(),
        ))),
        ..SeatCore::default()
    });
    core.core.keep_journal();
    let runtime = runtime_over(None, &core);
    runtime.bind_lifecycle_projection(None);
    register_live(&runtime, &core.core, spec(&alice), 0, SessionId::new())
        .await
        .expect("registered live");
    let seen = recording_hook(&runtime);

    runtime
        .reset_tracked(&alice)
        .await
        .expect_err("the bridge answered before it asked Core");
    // A host's own rotation of the incarnation the reset named.
    rotate_live(
        &core.core,
        &alice,
        &spec(&alice).profile,
        1,
        Some(SessionId::new()),
    )
    .expect("the host's rotation");
    runtime.settle_lifecycle_projection().await;
    let calls = lock(&seen).clone();
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(calls[0].generation, 0);
    assert_eq!(calls[0].cause.as_str(), "rotated", "{calls:?}");
}

/// A reset whose rotation started in Core and failed before its end keeps
/// what it recorded, and so does its rotation record; a delete of the same
/// incarnation that Core then refuses before any of it started takes its
/// own record back and puts the reset's back. When Core finishes the reset's
/// rotation from its own record, the hook hears of the incarnation it ended
/// as a reset's.
#[tokio::test]
async fn a_delete_refused_behind_a_reset_in_flight_keeps_the_resets_effects() {
    let alice = identity("personal:alice");
    let core = Arc::new(SeatCore {
        rotation_failure: StdMutex::new(Some(BridgeError::Mob(
            "the rotation's answer was lost".to_string(),
        ))),
        ..SeatCore::default()
    });
    core.core.keep_journal();
    let runtime = runtime_over(None, &core);
    runtime.bind_lifecycle_projection(None);
    register_live(&runtime, &core.core, spec(&alice), 0, SessionId::new())
        .await
        .expect("registered live");
    let seen = recording_hook(&runtime);

    runtime
        .reset_tracked(&alice)
        .await
        .expect_err("the reset's rotation failed after it started");
    core.refuse_next_rotations.store(1, Ordering::SeqCst);
    runtime
        .delete_identity_tracked(&alice)
        .await
        .expect_err("Core refused the delete before any of it started");
    // Core finishes the rotation it started: the retirement ends, and the
    // next incarnation is seated, naming it.
    rotate_live(
        &core.core,
        &alice,
        &spec(&alice).profile,
        1,
        Some(SessionId::new()),
    )
    .expect("Core finishes the reset's rotation");
    runtime.settle_lifecycle_projection().await;
    let calls = lock(&seen).clone();
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(calls[0].generation, 0);
    assert_eq!(calls[0].cause.as_str(), "reset", "{calls:?}");
}

/// A reset whose rotation Core started and then failed keeps what it
/// recorded; the reset is asked again and Core re-drives the rotation it
/// started, journaling no new start. The retirement it ends is the
/// reset's.
///
/// RED before a rotation's record that replaces an earlier rotation's
/// record kept the earlier head: the record of the reset asked again kept
/// the head that call read, past the only start Core journaled, so the
/// projection read the retirement as a rotation no Meta verb asked for, and
/// the hook heard `rotated`.
#[tokio::test]
async fn a_retried_reset_core_re_drives_reaches_the_hook_as_reset() {
    let alice = identity("personal:alice");
    let core = Arc::new(SeatCore {
        rotation_failure: StdMutex::new(Some(BridgeError::Mob(
            "the retirement ran past its bound".to_string(),
        ))),
        ..SeatCore::default()
    });
    core.core.keep_journal();
    let runtime = runtime_over(None, &core);
    runtime.bind_lifecycle_projection(None);
    register_live(&runtime, &core.core, spec(&alice), 0, SessionId::new())
        .await
        .expect("registered live");
    let seen = recording_hook(&runtime);
    runtime
        .reset_tracked(&alice)
        .await
        .expect_err("the reset's rotation started and failed");
    runtime
        .reset_tracked(&alice)
        .await
        .expect("the reset asked again re-drives the started rotation");
    runtime.settle_lifecycle_projection().await;
    let calls = lock(&seen).clone();
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(calls[0].generation, 0);
    assert_eq!(calls[0].cause.as_str(), "reset", "{calls:?}");
}

/// The delete twin of [`a_retried_reset_core_re_drives_reaches_the_hook_as_reset`]:
/// the retirement Core ends for the delete asked again owes the delete's
/// side effects, the exit interview (`delete`) and the drop of the retired
/// session's semantic-memory scope, and reaches the hook as a delete.
///
/// RED before a rotation's record that replaces an earlier rotation's
/// record kept the earlier head: the retirement read as a rotation no Meta
/// verb asked for, so neither the exit interview nor the scope drop ran.
#[tokio::test]
async fn a_retried_delete_core_re_drives_owes_the_delete_s_side_effects() {
    let alice = identity("personal:alice");
    let session = SessionId::new();
    let core = Arc::new(SeatCore {
        rotation_failure: StdMutex::new(Some(BridgeError::Mob(
            "the retirement ran past its bound".to_string(),
        ))),
        ..SeatCore::default()
    });
    core.core.keep_journal();
    let runtime = runtime_over(None, &core);
    runtime.bind_lifecycle_projection(None);
    register_live(&runtime, &core.core, spec(&alice), 0, session.clone())
        .await
        .expect("registered live");
    let seen = recording_hook(&runtime);
    let memory_dir = tempfile::tempdir().expect("memory dir");
    let reads = Arc::new(ReadSessions::default());
    let scopes = Arc::new(DroppedScopes::default());
    let (memory, store) = memory_with_exit_interviews_over(
        memory_dir.path(),
        &reads,
        Some(Arc::clone(&scopes) as Arc<dyn CompactionDiscardSource>),
    );
    runtime.set_agent_memory(Some(memory)).await;

    runtime
        .delete_identity_tracked(&alice)
        .await
        .expect_err("the delete's rotation started and failed");
    runtime
        .delete_identity_tracked(&alice)
        .await
        .expect("the delete asked again re-drives the started rotation")
        .expect("Core held an incarnation of alice's");
    runtime.settle_lifecycle_projection().await;
    assert_eq!(
        exit_interviews_of(&store, &alice).await,
        vec!["delete".to_string()],
        "the delete's exit interview is queued"
    );
    assert_eq!(
        scopes.dropped(),
        vec![session.to_string()],
        "the scope of the session the delete ended is dropped"
    );
    assert_eq!(heard_of(&seen, &alice), vec![("delete".to_string(), 0)]);
}

/// The first-boot twin of
/// [`a_retried_reset_core_re_drives_reaches_the_hook_as_reset`]: Core holds
/// alice live on the retired session of her pending first-boot `Fresh` row,
/// and the roster pass that carries the row out asks Core for her fresh
/// life, a reset's rotation, which Core starts and then fails. The row stays
/// pending; the next pass asks for it again, and Core re-drives the rotation
/// it started, journaling no new start. The retirement it ends is the row's
/// reset.
///
/// RED before a rotation's record that replaces an earlier rotation's
/// record kept the earlier head: the next step's record kept the head that
/// step read, past the only start Core journaled, so the hook heard
/// `rotated`.
#[tokio::test]
async fn a_first_boot_fresh_row_core_re_drives_reaches_the_hook_as_reset() {
    let alice = identity("personal:alice");
    let retired_session = SessionId::new();
    let core = Arc::new(SeatCore {
        rotation_failure: StdMutex::new(Some(BridgeError::Mob(
            "the retirement ran past its bound".to_string(),
        ))),
        ..SeatCore::default()
    });
    core.core.keep_journal();
    let state_dir = tempfile::tempdir().expect("state dir");
    FirstBootManifest::write_new(
        state_dir.path(),
        vec![FirstBootRow::Fresh {
            identity: alice.clone(),
            retired_session: retired_session.clone(),
        }],
    )
    .expect("the manifest writes");
    let runtime = runtime_over(Some(state_dir.path()), &core);
    runtime.bind_lifecycle_projection(None);
    bind_live(
        &core.core,
        &alice,
        &spec(&alice).profile,
        0,
        Some(retired_session),
    )
    .expect("Core holds alice live");
    let seen = recording_hook(&runtime);

    let outcomes = roster_pass(&runtime, &[spec(&alice)], None).await;
    assert!(
        pass_refusal(&outcomes[&alice]).is_some(),
        "the row's rotation started and failed: {:?}",
        outcomes[&alice]
    );
    let outcomes = roster_pass(&runtime, &[spec(&alice)], None).await;
    assert!(
        matches!(pass_binding(&outcomes[&alice]), Some((true, _))),
        "the next pass re-drives the row's started rotation: {:?}",
        outcomes[&alice]
    );
    runtime.settle_lifecycle_projection().await;
    let calls = lock(&seen).clone();
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(calls[0].generation, 0);
    assert_eq!(calls[0].cause.as_str(), "reset", "{calls:?}");
}

/// The seat of an incarnation Core still holds is told `after_create`: the
/// projection reads whether the incarnation is Core's from Core's standing
/// of the identity's member, whatever Core's member history reads of it
/// (here a history that reads the member retired).
///
/// RED before a seat's currency was read from Core's standing: it was read
/// from the history's latest binding of the identity, so a history that
/// disagreed with Core's standing kept the seat untold.
#[tokio::test]
async fn a_seat_core_holds_is_told_whatever_core_s_member_history_reads() {
    let alice = identity("personal:alice");
    let core = Arc::new(SeatCore::default());
    core.core.keep_journal();
    core.core.read_retired_in_history(&alice);
    let runtime = runtime_over(None, &core);
    runtime.bind_lifecycle_projection(None);
    let customizer = ToldSeats::answering();
    runtime
        .set_agent_customizer(Some(Arc::clone(&customizer) as Arc<dyn AgentCustomizer>))
        .await;

    let outcomes = roster_pass(&runtime, &[spec(&alice)], None).await;
    assert!(
        pass_refusal(&outcomes[&alice]).is_none(),
        "the pass seats alice: {:?}",
        outcomes[&alice]
    );
    runtime.settle_lifecycle_projection().await;
    assert_eq!(
        customizer.told(),
        vec![(
            alice.clone(),
            core.session_of(&alice).expect("Core binds the seat")
        )],
        "the seat Core holds is told once"
    );
}

/// The roster stops listing an identity, and Core starts its member's
/// retirement, which fails before its end; then the roster lists the
/// identity again. Core's standing reads the incarnation retiring: Core
/// carried the unlisting out, so the pass that lists the identity again
/// keeps what the unlisting recorded, and once Core ends that retirement
/// the hook hears of the incarnation as unlisted.
///
/// RED before the take-back read Core's standing: Core still binding the
/// incarnation read as "Core never carried the unlisting out", so the pass
/// took it back, and the retirement Core then ended reached no hook.
#[tokio::test]
async fn a_relisted_identity_whose_unlisting_core_started_keeps_its_effects() {
    let alice = identity("personal:alice");
    let core = Arc::new(SeatCore {
        retire_starts_then_fails: AtomicUsize::new(1),
        ..SeatCore::default()
    });
    core.core.keep_journal();
    let runtime = runtime_over(None, &core);
    let roster = Arc::new(MutableRosterProvider::new(vec![spec(&alice)]));
    let context = IdentityFirstRuntimeContext::new(
        Arc::clone(&runtime),
        Arc::clone(&roster) as Arc<dyn RosterProvider>,
        None,
        None,
        None,
    );
    let live = register_live(&runtime, &core.core, spec(&alice), 0, SessionId::new())
        .await
        .expect("registered live");
    let seen = recording_hook(&runtime);

    assert!(roster.remove(&alice));
    let _ = context.refresh_desired_topology().await;
    assert!(
        core.core
            .live_member(&alice)
            .expect("Core's members read")
            .is_some(),
        "Core started the member's retirement and has not ended it"
    );
    roster.upsert(spec(&alice));
    let _ = context.refresh_desired_topology().await;

    // Core ends the retirement it started.
    core.core.retire(&live);
    runtime.settle_lifecycle_projection().await;
    let calls = lock(&seen).clone();
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(calls[0].generation, 0);
    assert_eq!(calls[0].cause.as_str(), "unlisted", "{calls:?}");
}

/// The roster's unlisting of an identity records what it owes and Core
/// retires its member, and before the projection reaches that retirement a
/// reset gives the identity, which no member holds now, its fresh life (the
/// roster listed it again). The reset's record for the fresh life after the
/// incarnation Core had retired has a key of its own, apart from the
/// unlisting's, which the projection reads at that retirement: the hook
/// hears of the incarnation as unlisted, its session is distilled as the
/// unlisting's and again as the reset's, and both records are consumed.
/// Had the reset's record replaced the unlisting's, the hook would hear
/// nothing of the incarnation.
#[tokio::test]
async fn a_reset_before_the_projection_reads_an_unlisting_keeps_the_unlisting_s_effects() {
    let alice = identity("personal:alice");
    let session = SessionId::new();
    let core = Arc::new(SeatCore::default());
    core.core.keep_journal();
    let runtime = runtime_over(None, &core);
    let records: Arc<dyn crate::runtime::PersistentMetadataStore> =
        Arc::new(crate::runtime::InMemoryMetadataStore::new());
    runtime.set_identity_records(Arc::clone(&records), "unlisted-then-reset".to_string());
    runtime.bind_lifecycle_projection(None);
    let live = register_live(&runtime, &core.core, spec(&alice), 0, session.clone())
        .await
        .expect("registered live");
    let seen = recording_hook(&runtime);
    let memory_dir = tempfile::tempdir().expect("memory dir");
    let reads = Arc::new(ReadSessions::default());
    let (memory, _tracker) = memory_reading(memory_dir.path(), Arc::clone(&reads));
    runtime.set_agent_memory(Some(memory)).await;

    // The unlisting records what it owes, and Core retires the member; the
    // projection has not read the retirement yet.
    runtime
        .prepare_lifecycle_retirement(&alice, crate::runtime::LifecycleEffectVerb::Unlisted, None)
        .await
        .expect("the unlisting records what it owes")
        .expect("Core holds an incarnation of alice's");
    core.core.retire(&live);
    runtime
        .reset_tracked(&alice)
        .await
        .expect("the reset gives the identity its fresh life");

    runtime.settle_lifecycle_projection().await;
    let calls = lock(&seen).clone();
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(calls[0].generation, 0);
    assert_eq!(calls[0].cause.as_str(), "unlisted", "{calls:?}");
    assert_eq!(
        reads.reads_of(&session),
        2,
        "the session is distilled as the unlisting's and as the reset's"
    );
    assert!(
        records
            .lifecycle_effect_intents("unlisted-then-reset")
            .await
            .expect("the intents read")
            .is_empty(),
        "both records are consumed"
    );
}

/// A host's legacy respawn of an identity's member, which Core journals as a
/// plain retirement and then a seat that names the incarnation it retired,
/// owes what a rotation no Meta verb asked for owes, even when Core's member
/// history keeps no binding of that incarnation any more (its session was
/// named again since, say): the retirement Core journaled before the seat
/// names its session and role, and the hook hears of it as `rotated`, with
/// that session.
///
/// RED before the projection read a respawn's retired incarnation from
/// Core's journal when the history keeps none: the miss read as a plain
/// seat, and the hook heard nothing.
#[tokio::test]
async fn a_respawn_whose_retired_incarnation_the_history_no_longer_keeps_still_owes_its_effects() {
    let alice = identity("personal:alice");
    let first = SessionId::new();
    let core = Arc::new(SeatCore::default());
    core.core.keep_journal();
    let runtime = runtime_over(None, &core);
    runtime.bind_lifecycle_projection(None);
    register_live(&runtime, &core.core, spec(&alice), 0, first.clone())
        .await
        .expect("registered live");
    let seen = recording_hook(&runtime);
    // The projection starts at the journal's head, after alice's seat.
    runtime.settle_lifecycle_projection().await;

    respawn_live(
        &core.core,
        &alice,
        &spec(&alice).profile,
        1,
        Some(SessionId::new()),
    )
    .expect("the host's legacy respawn");
    core.core.forget_in_history(&alice, 0);
    runtime.settle_lifecycle_projection().await;
    let calls = lock(&seen).clone();
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(calls[0].generation, 0);
    assert_eq!(calls[0].cause.as_str(), "rotated", "{calls:?}");
    assert_eq!(calls[0].session_id.as_ref(), Some(&first), "{calls:?}");
}

/// The roster's unlisting of alice and a step of the same incarnation's
/// end, held: the unlisting records what it owes, Core retires her member,
/// and the projection runs the unlisting's side effects, its hook held.
/// Meanwhile `step` (a reset or a delete) gives alice, whom no member holds
/// now, her fresh life, recording what it owes for the incarnation Core
/// retired. Once the hook answers, both run.
async fn unlisting_held_while(
    step: &str,
) -> (
    Arc<IdentityRuntime>,
    AgentIdentity,
    SessionId,
    Arc<StdMutex<Vec<RetiredIdentityMember>>>,
    Arc<ReadSessions>,
    Arc<crate::SqliteAgentMemoryStore>,
    tempfile::TempDir,
) {
    let alice = identity("personal:alice");
    let session = SessionId::new();
    let core = Arc::new(SeatCore::default());
    core.core.keep_journal();
    let runtime = runtime_over(None, &core);
    runtime.bind_lifecycle_projection(None);
    let live = register_live(&runtime, &core.core, spec(&alice), 0, session.clone())
        .await
        .expect("registered live");
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let seen = hook_held_on(&runtime, &alice, &gate);
    let memory_dir = tempfile::tempdir().expect("memory dir");
    let reads = Arc::new(ReadSessions::default());
    let (memory, store) = memory_with_exit_interviews(memory_dir.path(), &reads);
    runtime.set_agent_memory(Some(memory)).await;
    runtime.settle_lifecycle_projection().await;

    runtime
        .prepare_lifecycle_retirement(&alice, crate::runtime::LifecycleEffectVerb::Unlisted, None)
        .await
        .expect("the unlisting records what it owes")
        .expect("Core holds an incarnation of alice's");
    core.core.retire(&live);
    let projecting = tokio::spawn({
        let runtime = Arc::clone(&runtime);
        async move { runtime.settle_lifecycle_projection().await }
    });
    hook_heard_of(&seen, &alice).await;
    // The exit interviews are keyed by the time their step was asked, to
    // the millisecond: the step is asked in a later one than the unlisting.
    tokio::time::sleep(Duration::from_millis(2)).await;
    match step {
        "reset" => {
            runtime
                .reset_tracked(&alice)
                .await
                .expect("the reset gives alice her fresh life");
        }
        _ => {
            runtime
                .delete_identity_tracked(&alice)
                .await
                .expect("the delete gives alice her fresh life")
                .expect("Core held an incarnation of alice's");
        }
    }
    gate.add_permits(64);
    projecting.await.expect("the projection's run");
    runtime.settle_lifecycle_projection().await;
    (runtime, alice, session, seen, reads, store, memory_dir)
}

/// The roster's unlisting of alice and a reset of her, unseated, while the
/// unlisting's side effects run ([`unlisting_held_while`]): both run. The
/// hook hears of the incarnation once, as unlisted, and the session it left
/// is distilled twice, as the unlisting's and, at the fresh life's seat,
/// as the reset's.
///
/// RED before the fresh life's record had a key of its own: the reset's
/// record took the unlisting's key, and the projection's removal of the
/// unlisting's record, once its side effects ran, removed the reset's too,
/// so the fresh life's seat found nothing owed.
#[tokio::test(flavor = "multi_thread")]
async fn a_reset_while_an_unlisting_s_effects_run_keeps_both_effects() {
    let (_runtime, alice, session, seen, reads, _store, _memory_dir) =
        unlisting_held_while("reset").await;
    assert_eq!(
        heard_of(&seen, &alice),
        vec![("unlisted".to_string(), 0)],
        "the hook hears of the incarnation once, as unlisted"
    );
    assert_eq!(
        reads.reads_of(&session),
        2,
        "the session is distilled as the unlisting's and as the reset's"
    );
}

/// The delete twin of [`a_reset_while_an_unlisting_s_effects_run_keeps_both_effects`]:
/// the unlisting's exit interview (`retire`) and the delete's (`delete`)
/// are both queued.
///
/// RED before the fresh life's record had a key of its own: the delete's
/// record was removed with the unlisting's, so its exit interview was never
/// queued.
#[tokio::test(flavor = "multi_thread")]
async fn a_delete_while_an_unlisting_s_effects_run_keeps_both_effects() {
    let (_runtime, alice, _session, seen, _reads, store, _memory_dir) =
        unlisting_held_while("delete").await;
    assert_eq!(
        heard_of(&seen, &alice),
        vec![("unlisted".to_string(), 0)],
        "the hook hears of the incarnation once, as unlisted"
    );
    let mut interviews = exit_interviews_of(&store, &alice).await;
    interviews.sort();
    assert_eq!(
        interviews,
        vec!["delete".to_string(), "retire".to_string()],
        "the unlisting's exit interview and the delete's are both queued"
    );
}

/// A host's rotation of alice's member is in flight when a delete of alice
/// asks Core for its own rotation: Core refuses it, typed, and the host's
/// rotation ends while the delete waits for that answer. Nothing of the
/// delete reached Core, so it keeps nothing: its records are taken back,
/// and the host's rotation reaches the hook as what it is (`rotated`), never
/// as a delete.
///
/// RED before a step's reach counted only the fact the step itself writes
/// first: any fact of the member after the head the delete read (here, the
/// end of the host's rotation) kept the delete's records.
#[tokio::test(flavor = "multi_thread")]
async fn a_delete_core_refuses_while_a_host_rotation_ends_keeps_nothing() {
    let alice = identity("personal:alice");
    let (reached_tx, reached_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let core = Arc::new(SeatCore {
        rotate_gate: StdMutex::new(Some((reached_tx, release_rx))),
        refuse_next_rotations: AtomicUsize::new(1),
        ..SeatCore::default()
    });
    core.core.keep_journal();
    let runtime = runtime_over(None, &core);
    runtime.bind_lifecycle_projection(None);
    let live = register_live(&runtime, &core.core, spec(&alice), 0, SessionId::new())
        .await
        .expect("registered live");
    let seen = recording_hook(&runtime);
    runtime.settle_lifecycle_projection().await;
    // A host's rotation of alice's member starts.
    core.core
        .start_retirement(&live, Some(meerkat_mob::RotationTarget::FreshSession));

    let deleting = tokio::spawn({
        let runtime = Arc::clone(&runtime);
        let alice = alice.clone();
        async move { runtime.delete_identity(&alice).await }
    });
    reached_rx.await.expect("the delete reaches Core");
    // The host's rotation ends while Core answers the delete.
    rotate_live(
        &core.core,
        &alice,
        &spec(&alice).profile,
        1,
        Some(SessionId::new()),
    )
    .expect("the host's rotation ends");
    release_tx.send(()).expect("release the delete's rotation");
    let refused = deleting.await.expect("the delete's task");
    assert!(
        matches!(
            refused,
            Err(IdentityRuntimeError::IdentityWakeRetryable { .. })
        ),
        "{refused:?}"
    );
    assert!(
        !runtime
            .pending_rotation_intents()
            .await
            .expect("the records read")
            .contains_key(&alice),
        "the refused delete keeps no rotation record"
    );
    runtime.settle_lifecycle_projection().await;
    let calls = lock(&seen).clone();
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(calls[0].generation, 0);
    assert_eq!(calls[0].cause.as_str(), "rotated", "{calls:?}");
}

/// What the retirement hook hears of a host's rotation of alice's member
/// that ends while `step` waits for Core's answer to its own rotation,
/// which Core then refuses, when a run of the lifecycle projection reaches
/// the end of the host's rotation in between: while the refused step still
/// holds its record. `step` is a reset, a delete, or a roster pass that
/// moves alice to another role (her session lives in memory only). The run
/// waits for alice's lifecycle lock, which the step holds until Core
/// answers, at most a side effect's deadline, kept short here.
async fn heard_of_a_host_rotation_a_refused_step_raced(step: &str) -> Vec<RetiredIdentityMember> {
    let alice = identity("personal:alice");
    let (reached_tx, reached_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let core = Arc::new(SeatCore {
        rotate_gate: StdMutex::new(Some((reached_tx, release_rx))),
        refuse_next_rotations: AtomicUsize::new(1),
        sessions_in_memory: step == "role_move",
        ..SeatCore::default()
    });
    core.core.keep_journal();
    let runtime = runtime_over(None, &core);
    runtime.bind_lifecycle_projection(None);
    runtime.set_lifecycle_effect_limits(Duration::from_millis(200), 1);
    let live = register_live(&runtime, &core.core, spec(&alice), 0, SessionId::new())
        .await
        .expect("registered live");
    let seen = recording_hook(&runtime);
    runtime.settle_lifecycle_projection().await;
    // A host's rotation of alice's member starts.
    core.core
        .start_retirement(&live, Some(meerkat_mob::RotationTarget::FreshSession));

    let stepping = tokio::spawn({
        let runtime = Arc::clone(&runtime);
        let alice = alice.clone();
        let step = step.to_string();
        async move { step_of_alice(&runtime, &alice, &step).await }
    });
    reached_rx.await.expect("the step reaches Core");
    // The host's rotation ends while Core answers the step.
    rotate_live(
        &core.core,
        &alice,
        &spec(&alice).profile,
        1,
        Some(SessionId::new()),
    )
    .expect("the host's rotation ends");
    // A run of the projection reaches the end of the host's rotation while
    // the step still holds its record.
    tokio::time::timeout(
        Duration::from_secs(10),
        runtime.settle_lifecycle_projection(),
    )
    .await
    .expect("the projection's run answers");
    release_tx.send(()).expect("release the step's rotation");
    let refused = stepping.await.expect("the step's task");
    assert!(
        matches!(
            refused.as_ref().map_err(AsRef::as_ref),
            Err(IdentityRuntimeError::IdentityWakeRetryable { .. })
        ),
        "Core refused the {step}: {refused:?}"
    );
    runtime.settle_lifecycle_projection().await;
    lock(&seen).clone()
}

/// `step` of alice: a reset, a delete, or a roster pass that moves her to
/// the role `reviewer`. Its typed refusal, if Core refused it.
async fn step_of_alice(
    runtime: &IdentityRuntime,
    alice: &AgentIdentity,
    step: &str,
) -> Result<(), Arc<IdentityRuntimeError>> {
    match step {
        "reset" => runtime.reset(alice).await.map(|_| ()).map_err(Arc::new),
        "delete" => runtime
            .delete_identity(alice)
            .await
            .map(|_| ())
            .map_err(Arc::new),
        _ => {
            let mut reviewer = spec(alice);
            reviewer.profile = meerkat_mob::ProfileName::from("reviewer");
            match roster_pass(runtime, std::slice::from_ref(&reviewer), None)
                .await
                .remove(alice)
                .expect("the pass answers alice")
            {
                crate::identity_first::orchestrator::RestoreOutcome::Refused(error) => Err(error),
                _ => Ok(()),
            }
        }
    }
}

/// A host's rotation of alice's member ends while a delete of alice waits
/// for Core's answer to its own rotation, which Core refuses, and a run of
/// the lifecycle projection reaches the end of the host's rotation before
/// the delete takes its record back: the hook hears the host's rotation as
/// what it is (`rotated`), never as a delete.
///
/// RED before a rotation's record claimed only the retirement whose start
/// Core journaled after the head its step read: the record claimed any
/// retirement of its incarnation a run of the projection read while the
/// record stood, so the run heard the host's rotation as a delete.
#[tokio::test(flavor = "multi_thread")]
async fn a_host_rotation_a_refused_delete_raced_reaches_the_hook_as_rotated() {
    let heard = heard_of_a_host_rotation_a_refused_step_raced("delete").await;
    assert_eq!(heard.len(), 1, "{heard:?}");
    assert_eq!(heard[0].generation, 0);
    assert_eq!(heard[0].cause.as_str(), "rotated", "{heard:?}");
}

/// The reset twin of
/// [`a_host_rotation_a_refused_delete_raced_reaches_the_hook_as_rotated`].
///
/// RED before a rotation's record claimed only the retirement whose start
/// Core journaled after the head its step read: the run heard the host's
/// rotation as a reset.
#[tokio::test(flavor = "multi_thread")]
async fn a_host_rotation_a_refused_reset_raced_reaches_the_hook_as_rotated() {
    let heard = heard_of_a_host_rotation_a_refused_step_raced("reset").await;
    assert_eq!(heard.len(), 1, "{heard:?}");
    assert_eq!(heard[0].generation, 0);
    assert_eq!(heard[0].cause.as_str(), "rotated", "{heard:?}");
}

/// The role move twin of
/// [`a_host_rotation_a_refused_delete_raced_reaches_the_hook_as_rotated`]:
/// a roster pass moves alice to another role, and the host's rotation keeps
/// her role.
///
/// RED before a rotation's record claimed only the retirement whose start
/// Core journaled after the head its step read: the run heard the host's
/// rotation as a role move.
#[tokio::test(flavor = "multi_thread")]
async fn a_host_rotation_a_refused_role_move_raced_reaches_the_hook_as_rotated() {
    let heard = heard_of_a_host_rotation_a_refused_step_raced("role_move").await;
    assert_eq!(heard.len(), 1, "{heard:?}");
    assert_eq!(heard[0].generation, 0);
    assert_eq!(heard[0].cause.as_str(), "rotated", "{heard:?}");
}

/// What the retirement hook hears of `step`'s own rotation of alice's member
/// when a run of the lifecycle projection reaches its end while the step
/// still waits for Core's answer: Core carried the rotation out and holds
/// its answer. `step` is a reset, a delete, or a roster pass that moves
/// alice to another role (her session lives in memory only), as in
/// [`heard_of_a_host_rotation_a_refused_step_raced`]; the run waits for
/// alice's lifecycle lock, which the step holds until Core answers, at most
/// a side effect's deadline, kept short here.
async fn heard_of_a_step_s_rotation_read_before_it_answered(
    step: &str,
) -> Vec<RetiredIdentityMember> {
    let alice = identity("personal:alice");
    let (rotated_tx, rotated_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let core = Arc::new(SeatCore {
        rotated_gate: StdMutex::new(Some((rotated_tx, release_rx))),
        sessions_in_memory: step == "role_move",
        ..SeatCore::default()
    });
    core.core.keep_journal();
    let runtime = runtime_over(None, &core);
    runtime.bind_lifecycle_projection(None);
    runtime.set_lifecycle_effect_limits(Duration::from_millis(200), 1);
    register_live(&runtime, &core.core, spec(&alice), 0, SessionId::new())
        .await
        .expect("registered live");
    let seen = recording_hook(&runtime);
    runtime.settle_lifecycle_projection().await;

    let stepping = tokio::spawn({
        let runtime = Arc::clone(&runtime);
        let alice = alice.clone();
        let step = step.to_string();
        async move { step_of_alice(&runtime, &alice, &step).await }
    });
    rotated_rx
        .await
        .expect("Core carries the step's rotation out");
    // A run of the projection reaches the end of the step's rotation while
    // the step still waits for Core's answer.
    tokio::time::timeout(
        Duration::from_secs(10),
        runtime.settle_lifecycle_projection(),
    )
    .await
    .expect("the projection's run answers");
    release_tx.send(()).expect("release Core's answer");
    let answered = stepping.await.expect("the step's task");
    assert!(answered.is_ok(), "the {step} answers: {answered:?}");
    runtime.settle_lifecycle_projection().await;
    lock(&seen).clone()
}

/// A reset, a delete and a role move of alice, each read by a run of the
/// lifecycle projection after Core carried its rotation out and while the
/// step still waits for Core's answer: Core journaled the start of the
/// incarnation's retirement after the head the step read, so the step's
/// record answers the retirement, and the hook hears the step's own cause.
/// The positive twin, in the same harness, of
/// [`a_host_rotation_a_refused_delete_raced_reaches_the_hook_as_rotated`]
/// and its reset and role move twins.
#[tokio::test(flavor = "multi_thread")]
async fn a_step_s_own_rotation_read_before_it_answered_reaches_the_hook_as_the_step_s() {
    // The role_move leg does not discriminate: Core's facts give role_move either way.
    for step in ["reset", "delete", "role_move"] {
        let heard = heard_of_a_step_s_rotation_read_before_it_answered(step).await;
        assert_eq!(heard.len(), 1, "{step}: {heard:?}");
        assert_eq!(heard[0].generation, 0, "{step}");
        assert_eq!(heard[0].cause.as_str(), step, "{heard:?}");
    }
}

/// A host customizer that, while it builds `identity`'s draft, has Core
/// seat the identity's fresh life after the incarnation Core retired, as a
/// host's own step that ran meanwhile would, once.
struct SeatsAFreshLifeWhileBuilding {
    core: Arc<SeatCore>,
    identity: AgentIdentity,
    seated: AtomicBool,
}

#[async_trait::async_trait]
impl AgentCustomizer for SeatsAFreshLifeWhileBuilding {
    async fn customize_build(
        &self,
        context: &AgentBuildContext,
        spec: &DurableAgentSpec,
        _draft: &mut AgentBuildDraft,
    ) -> Result<(), crate::identity_first::CustomizerError> {
        if context.identity == self.identity && !self.seated.swap(true, Ordering::SeqCst) {
            rotate_live(
                &self.core.core,
                &self.identity,
                &spec.profile,
                1,
                Some(SessionId::new()),
            )
            .expect("Core seats the fresh life");
        }
        Ok(())
    }
}

/// Core retired alice's incarnation 0, and a delete of alice reads it
/// retired. While the delete builds her fresh life, a host's own fresh life
/// of alice after that incarnation is seated. The delete reads Core's
/// binding of alice again once it read the head of Core's journal, finds
/// that it changed, and is refused, retryable, with nothing recorded and
/// nothing asked of Core: the host's fresh life owes nothing of a delete.
/// The delete asked again reads the host's fresh life, Core rotates it, and
/// its retirement owes the delete's side effects: the drop of its session's
/// semantic-memory scope, the exit interview (`delete`) and the retirement
/// hook (`delete`).
///
/// RED before the delete read Core's binding again after the head: the
/// delete went ahead with the binding it read before its build, and Core
/// rotated the host's fresh life for it, with no record of the delete keyed
/// by that incarnation.
#[tokio::test]
async fn a_host_fresh_life_a_refused_delete_raced_owes_no_delete() {
    let alice = identity("personal:alice");
    let core = Arc::new(SeatCore::default());
    core.core.keep_journal();
    let runtime = runtime_over(None, &core);
    let records: Arc<dyn crate::runtime::PersistentMetadataStore> =
        Arc::new(crate::runtime::InMemoryMetadataStore::new());
    runtime.set_identity_records(Arc::clone(&records), "fresh-life-raced".to_string());
    runtime.bind_lifecycle_projection(None);
    let live = register_live(&runtime, &core.core, spec(&alice), 0, SessionId::new())
        .await
        .expect("registered live");
    let seen = recording_hook(&runtime);
    let memory_dir = tempfile::tempdir().expect("memory dir");
    let reads = Arc::new(ReadSessions::default());
    let scopes = Arc::new(DroppedScopes::default());
    let (memory, store) = memory_with_exit_interviews_over(
        memory_dir.path(),
        &reads,
        Some(Arc::clone(&scopes) as Arc<dyn CompactionDiscardSource>),
    );
    runtime.set_agent_memory(Some(memory)).await;
    // Core retires alice's incarnation 0 (a host's own retirement).
    core.core.retire(&live);
    runtime.settle_lifecycle_projection().await;
    runtime
        .set_agent_customizer(Some(Arc::new(SeatsAFreshLifeWhileBuilding {
            core: Arc::clone(&core),
            identity: alice.clone(),
            seated: AtomicBool::new(false),
        }) as Arc<dyn AgentCustomizer>))
        .await;

    let refused = runtime.delete_identity(&alice).await;
    assert!(
        matches!(
            refused,
            Err(IdentityRuntimeError::IdentityWakeRetryable { .. })
        ),
        "the delete is refused, retryable: {refused:?}"
    );
    assert_eq!(
        core.rotation_calls.load(Ordering::SeqCst),
        0,
        "Core was asked nothing"
    );
    assert!(
        records
            .lifecycle_effect_intents("fresh-life-raced")
            .await
            .expect("the intents read")
            .is_empty(),
        "the refused delete recorded nothing it owes"
    );
    assert!(
        !runtime
            .pending_rotation_intents()
            .await
            .expect("the records read")
            .contains_key(&alice),
        "the refused delete recorded no rotation"
    );
    let fresh = core
        .session_of(&alice)
        .expect("Core seated the host's fresh life");
    runtime.settle_lifecycle_projection().await;
    assert_eq!(
        exit_interviews_of(&store, &alice).await,
        Vec::<String>::new(),
        "nobody deleted alice"
    );

    runtime
        .delete_identity(&alice)
        .await
        .expect("the delete asked again goes ahead")
        .expect("Core holds an incarnation of alice's");
    runtime.settle_lifecycle_projection().await;
    assert_eq!(
        exit_interviews_of(&store, &alice).await,
        vec!["delete".to_string()],
        "the delete's exit interview is queued"
    );
    assert_eq!(
        scopes.dropped(),
        vec![fresh.to_string()],
        "the scope of the session the delete ended is dropped"
    );
    assert_eq!(heard_of(&seen, &alice), vec![("delete".to_string(), 1)]);
}

/// A host's own rotation of alice's live incarnation 0 ends while a
/// delete of alice builds her fresh life. The delete reads Core's binding
/// of alice again once it read the head of Core's journal, finds
/// incarnation 1 seated, and is refused, retryable, with nothing asked of
/// Core; the host's rotation reaches the hook as `rotated`. The delete
/// asked again rotates incarnation 1 and owes the delete's side effects.
///
/// RED before the delete read Core's binding again after the head: Core
/// rotated incarnation 1 for the delete, and the delete answered. Both reads
/// see a live incarnation, so only the comparison of their generations
/// refuses it.
#[tokio::test]
async fn a_host_rotation_while_a_delete_builds_refuses_the_delete_retryably() {
    let alice = identity("personal:alice");
    let core = Arc::new(SeatCore::default());
    core.core.keep_journal();
    let runtime = runtime_over(None, &core);
    runtime.bind_lifecycle_projection(None);
    register_live(&runtime, &core.core, spec(&alice), 0, SessionId::new())
        .await
        .expect("registered live");
    let seen = recording_hook(&runtime);
    let memory_dir = tempfile::tempdir().expect("memory dir");
    let reads = Arc::new(ReadSessions::default());
    let (memory, store) = memory_with_exit_interviews(memory_dir.path(), &reads);
    runtime.set_agent_memory(Some(memory)).await;
    // The projection starts at the journal's head, after alice's seat, so
    // it reads the host's rotation.
    runtime.settle_lifecycle_projection().await;
    runtime
        .set_agent_customizer(Some(Arc::new(SeatsAFreshLifeWhileBuilding {
            core: Arc::clone(&core),
            identity: alice.clone(),
            seated: AtomicBool::new(false),
        }) as Arc<dyn AgentCustomizer>))
        .await;

    let refused = runtime.delete_identity(&alice).await;
    assert!(
        matches!(
            refused,
            Err(IdentityRuntimeError::IdentityWakeRetryable { .. })
        ),
        "the delete is refused, retryable: {refused:?}"
    );
    assert_eq!(
        core.rotation_calls.load(Ordering::SeqCst),
        0,
        "Core was asked nothing"
    );
    runtime
        .delete_identity(&alice)
        .await
        .expect("the delete asked again goes ahead")
        .expect("Core holds an incarnation of alice's");
    runtime.settle_lifecycle_projection().await;
    assert_eq!(
        heard_of(&seen, &alice),
        vec![("rotated".to_string(), 0), ("delete".to_string(), 1)]
    );
    assert_eq!(
        exit_interviews_of(&store, &alice).await,
        vec!["delete".to_string()]
    );
}

/// Core retired alice's incarnation 0, and a delete read it retired; a
/// host's own fresh life after that incarnation was seated, and the delete
/// recorded what it owes with a head of Core's journal past that seat, as
/// a build without the second read of Core's binding leaves it when it
/// stops before Core answers. The host's seat lies at or before the
/// record's head, so it is not the delete's: the session incarnation 0
/// left owes nothing of a delete.
///
/// RED without the fresh life's gate in the projection, and before the
/// projection read a rotation's record by its head: the record claimed the
/// host's seat, so a delete's exit interview was queued.
#[tokio::test]
async fn a_fresh_life_seated_at_or_before_a_record_s_head_owes_it_nothing() {
    let alice = identity("personal:alice");
    let core = Arc::new(SeatCore::default());
    core.core.keep_journal();
    let runtime = runtime_over(None, &core);
    runtime.bind_lifecycle_projection(None);
    let live = register_live(&runtime, &core.core, spec(&alice), 0, SessionId::new())
        .await
        .expect("registered live");
    let memory_dir = tempfile::tempdir().expect("memory dir");
    let reads = Arc::new(ReadSessions::default());
    let (memory, store) = memory_with_exit_interviews(memory_dir.path(), &reads);
    runtime.set_agent_memory(Some(memory)).await;
    // Core retires alice's incarnation 0 (a host's own retirement).
    core.core.retire(&live);
    runtime.settle_lifecycle_projection().await;
    // The delete reads the retired incarnation 0, a host's own fresh
    // life after it is seated, and the delete reads the head past it.
    let read = core
        .core
        .last_member_binding(&alice)
        .expect("Core's history reads");
    rotate_live(
        &core.core,
        &alice,
        &spec(&alice).profile,
        1,
        Some(SessionId::new()),
    )
    .expect("the host's fresh life");
    let head = core.core.journal_head().expect("the head reads").cursor;
    runtime
        .prepare_lifecycle_retirement_of(
            &alice,
            read,
            crate::runtime::LifecycleEffectVerb::Delete,
            Some(head),
        )
        .await
        .expect("the record writes")
        .expect("incarnation 0 is the identity's");
    runtime.settle_lifecycle_projection().await;
    assert_eq!(
        exit_interviews_of(&store, &alice).await,
        Vec::<String>::new(),
        "the host's fresh life owes nothing of a delete"
    );
}

/// A reset whose read of the head of Core's journal fails, while the
/// projection reads Core's journal, is refused before Core is asked for
/// anything, retryable: with no head, a failure of its rotation could not
/// tell whether any of it reached Core.
///
/// RED before such a step was refused: it asked Core with no head.
#[tokio::test]
async fn a_reset_whose_journal_head_does_not_read_asks_core_nothing() {
    let alice = identity("personal:alice");
    let core = Arc::new(SeatCore::default());
    core.core.keep_journal();
    let runtime = runtime_over(None, &core);
    runtime.bind_lifecycle_projection(None);
    register_live(&runtime, &core.core, spec(&alice), 0, SessionId::new())
        .await
        .expect("registered live");
    runtime.settle_lifecycle_projection().await;
    core.journal_head_failures.store(1, Ordering::SeqCst);

    let refused = runtime.reset(&alice).await;
    assert!(
        matches!(
            refused,
            Err(IdentityRuntimeError::IdentityHistoryUnavailable { .. })
        ),
        "{refused:?}"
    );
    assert_eq!(
        core.rotation_calls.load(Ordering::SeqCst),
        0,
        "Core was asked nothing"
    );
    assert!(
        runtime
            .pending_rotation_intents()
            .await
            .expect("the records read")
            .is_empty(),
        "the refused reset left no record"
    );
}

/// A reset refused for its member route over a bridge whose journal does
/// not read keeps what it recorded (nothing tells it that none of the
/// rotation reached Core), so its answer says so: the rotation may have
/// started, and Core finishes it. It never answers the refusal as final.
///
/// RED before a kept step's answer said what Meta does with the records:
/// the refusal still answered "was not started", final.
#[tokio::test]
async fn a_refusal_whose_records_are_kept_says_the_rotation_may_have_started() {
    let alice = identity("personal:alice");
    let core = Arc::new(SeatCore {
        route_refused: vec![alice.clone()],
        ..SeatCore::default()
    });
    let runtime = runtime_over(None, &core);
    register_all(&runtime, &[kickoff_spec(&alice)]).await;

    let answered = runtime.reset(&alice).await;
    let Err(IdentityRuntimeError::Internal(detail)) = &answered else {
        panic!("the kept reset answers that it may have started: {answered:?}");
    };
    assert!(detail.contains("may have started"), "{detail}");
    assert!(
        runtime
            .pending_rotation_intents()
            .await
            .expect("the records read")
            .contains_key(&alice),
        "the reset's rotation record stays written"
    );
}

/// The roster's unlisting of alice records what it owes, and a host's
/// legacy respawn of the same incarnation wins the race: Core retires the
/// incarnation and seats the next one, naming it. That retirement is the
/// unlisting's: the hook hears of the incarnation once, as unlisted, and
/// its session is distilled once; the respawn's seat owes nothing more.
///
/// RED before the respawn's seat deferred to a Meta record naming the
/// incarnation it follows: it owed a rotation's side effects as well, so
/// the hook heard of the incarnation twice and its session was distilled
/// twice.
#[tokio::test]
async fn a_roster_unlisting_racing_a_host_respawn_owes_one_hook_and_one_distillation() {
    let alice = identity("personal:alice");
    let session = SessionId::new();
    let core = Arc::new(SeatCore::default());
    core.core.keep_journal();
    let runtime = runtime_over(None, &core);
    runtime.bind_lifecycle_projection(None);
    register_live(&runtime, &core.core, spec(&alice), 0, session.clone())
        .await
        .expect("registered live");
    let seen = recording_hook(&runtime);
    let memory_dir = tempfile::tempdir().expect("memory dir");
    let reads = Arc::new(ReadSessions::default());
    let (memory, _tracker) = memory_reading(memory_dir.path(), Arc::clone(&reads));
    runtime.set_agent_memory(Some(memory)).await;
    runtime.settle_lifecycle_projection().await;

    runtime
        .prepare_lifecycle_retirement(&alice, crate::runtime::LifecycleEffectVerb::Unlisted, None)
        .await
        .expect("the unlisting records what it owes")
        .expect("Core holds an incarnation of alice's");
    respawn_live(
        &core.core,
        &alice,
        &spec(&alice).profile,
        1,
        Some(SessionId::new()),
    )
    .expect("the host's legacy respawn");
    runtime.settle_lifecycle_projection().await;
    assert_eq!(
        heard_of(&seen, &alice),
        vec![("unlisted".to_string(), 0)],
        "the hook hears of the incarnation once, as unlisted"
    );
    assert_eq!(
        reads.reads_of(&session),
        1,
        "the session the incarnation left is distilled once"
    );
}

/// The roster's unlisting of alice records what it owes, and a host's
/// legacy respawn of the same incarnation wins the race, its two steps on
/// two pages: Core retires the incarnation, the projection runs the
/// unlisting's side effects and removes its record, and only then does Core
/// seat the next incarnation, naming the retired one. The hook still hears
/// of the incarnation once, as unlisted, and its session is distilled once:
/// the respawn's seat defers to the record its retirement read.
///
/// RED before the respawn's seat deferred to the Meta record its
/// retirement read: the record was gone by the seat's page, so the seat
/// owed a rotation's side effects as well, and the hook heard `rotated`
/// after `unlisted`.
#[tokio::test]
async fn a_roster_unlisting_racing_a_host_respawn_across_pages_owes_one_hook() {
    let alice = identity("personal:alice");
    let session = SessionId::new();
    let core = Arc::new(SeatCore::default());
    core.core.keep_journal();
    let runtime = runtime_over(None, &core);
    runtime.bind_lifecycle_projection(None);
    let live = register_live(&runtime, &core.core, spec(&alice), 0, session.clone())
        .await
        .expect("registered live");
    let seen = recording_hook(&runtime);
    let memory_dir = tempfile::tempdir().expect("memory dir");
    let reads = Arc::new(ReadSessions::default());
    let (memory, _tracker) = memory_reading(memory_dir.path(), Arc::clone(&reads));
    runtime.set_agent_memory(Some(memory)).await;
    runtime.settle_lifecycle_projection().await;

    runtime
        .prepare_lifecycle_retirement(&alice, crate::runtime::LifecycleEffectVerb::Unlisted, None)
        .await
        .expect("the unlisting records what it owes")
        .expect("Core holds an incarnation of alice's");
    // The respawn's first step, and the page that holds it.
    core.core.retire(&live);
    runtime.settle_lifecycle_projection().await;
    assert_eq!(
        heard_of(&seen, &alice),
        vec![("unlisted".to_string(), 0)],
        "the retirement is the unlisting's"
    );
    // The respawn's second step, on a page of its own.
    respawn_seat_live(
        &core.core,
        &alice,
        &spec(&alice).profile,
        1,
        Some(SessionId::new()),
    )
    .expect("the respawn's seat");
    runtime.settle_lifecycle_projection().await;
    assert_eq!(
        heard_of(&seen, &alice),
        vec![("unlisted".to_string(), 0)],
        "the hook hears of the incarnation once, as unlisted"
    );
    assert_eq!(
        reads.reads_of(&session),
        1,
        "the session the incarnation left is distilled once"
    );
}

/// Core seats alice and then retires her member, both on one page of its
/// journal, while Core's standing still shows the seat (the state Core's
/// actor last published trails its journal). The seat is not told
/// `after_create`: the page that holds it holds its end.
///
/// RED before the page's own facts were read for a seat's end: standing
/// showed the incarnation, so the seat was told after its end.
#[tokio::test]
async fn a_seat_whose_end_is_on_its_page_is_not_told_while_standing_trails() {
    let alice = identity("personal:alice");
    let core = Arc::new(SeatCore::default());
    core.core.keep_journal();
    let runtime = runtime_over(None, &core);
    runtime.bind_lifecycle_projection(None);
    let customizer = ToldSeats::answering();
    runtime
        .set_agent_customizer(Some(Arc::clone(&customizer) as Arc<dyn AgentCustomizer>))
        .await;
    runtime.settle_lifecycle_projection().await;

    let live = register_live(&runtime, &core.core, spec(&alice), 0, SessionId::new())
        .await
        .expect("registered live");
    core.core.pin_standing(&alice, MemberStanding::seated(0));
    core.core.retire(&live);
    runtime.settle_lifecycle_projection().await;
    assert!(
        customizer.told().is_empty(),
        "the seat is not told after its end: {:?}",
        customizer.told()
    );
}

/// Core seats alice and bob while its standing still shows neither (its
/// actor has not published the state that shows the seats yet), and
/// retires bob later, past the page that holds the seats. Alice's seat is
/// told `after_create`: Core's journal holds no end of it. Bob's is not:
/// the journal after the page ends it.
#[tokio::test]
async fn a_seat_standing_does_not_show_yet_is_told_unless_the_journal_ends_it() {
    let alice = identity("personal:alice");
    let bob = identity("personal:bob");
    let core = Arc::new(SeatCore::default());
    core.core.keep_journal();
    let runtime = runtime_over(None, &core);
    runtime.bind_lifecycle_projection(None);
    let customizer = ToldSeats::answering();
    runtime
        .set_agent_customizer(Some(Arc::clone(&customizer) as Arc<dyn AgentCustomizer>))
        .await;
    runtime.settle_lifecycle_projection().await;

    let alice_session = SessionId::new();
    register_live(&runtime, &core.core, spec(&alice), 0, alice_session.clone())
        .await
        .expect("alice registered live");
    let bob_live = register_live(&runtime, &core.core, spec(&bob), 0, SessionId::new())
        .await
        .expect("bob registered live");
    // A busy journal: bob's end lies past the page that holds the seats.
    core.core
        .journal_others(meerkat_mob::IDENTITY_ENTRY_PAGE_MAX);
    core.core.retire(&bob_live);
    core.core.pin_standing(&alice, MemberStanding::unseated());
    core.core.pin_standing(&bob, MemberStanding::unseated());
    runtime.settle_lifecycle_projection().await;
    assert_eq!(
        customizer.told(),
        vec![(alice.clone(), alice_session)],
        "the seat whose end Core journaled is not told"
    );
}

/// A reset gives alice, whose incarnation Core had retired, her fresh
/// life, and Core compensates the fresh life's spawn: its seat is
/// journaled, then closed by a retirement that records no facts. The
/// fresh life's seat still owes the reset's memory side effects of the
/// session the retired incarnation left, once; it is told no
/// `after_create`, the compensation owes nothing, and no hook hears of
/// either.
#[tokio::test]
async fn a_compensated_fresh_life_owes_its_predecessor_once_and_tells_nothing() {
    let alice = identity("personal:alice");
    let session = SessionId::new();
    let core = Arc::new(SeatCore {
        compensate_next_rotations: AtomicUsize::new(1),
        ..SeatCore::default()
    });
    core.core.keep_journal();
    let runtime = runtime_over(None, &core);
    runtime.bind_lifecycle_projection(None);
    let live = register_live(&runtime, &core.core, spec(&alice), 0, session.clone())
        .await
        .expect("registered live");
    let seen = recording_hook(&runtime);
    let customizer = ToldSeats::answering();
    runtime
        .set_agent_customizer(Some(Arc::clone(&customizer) as Arc<dyn AgentCustomizer>))
        .await;
    let memory_dir = tempfile::tempdir().expect("memory dir");
    let reads = Arc::new(ReadSessions::default());
    let (memory, _tracker) = memory_reading(memory_dir.path(), Arc::clone(&reads));
    runtime.set_agent_memory(Some(memory)).await;
    runtime.settle_lifecycle_projection().await;

    core.core.retire(&live);
    runtime
        .reset(&alice)
        .await
        .expect_err("Core compensated the fresh life's spawn");
    runtime.settle_lifecycle_projection().await;
    assert_eq!(
        reads.reads_of(&session),
        1,
        "the reset's distillation of the session the retired incarnation left, once"
    );
    assert!(
        customizer.told().is_empty(),
        "the compensated seat is not told: {:?}",
        customizer.told()
    );
    assert!(lock(&seen).is_empty(), "{:?}", lock(&seen));
}

/// A legacy respawn's seat whose retired incarnation Core's member history
/// no longer keeps, in a journal that holds that incarnation's seat but no
/// retirement of it before the respawn's seat (which Core's journal rules
/// out). The projection does not read back past that seat: the page fails
/// with that, so the projection reads stalled with it, and the seat never
/// reads as a plain one.
///
/// RED before the backward read stopped at the incarnation's own seat: it
/// read on to the journal's start, and its failure named no seat.
#[tokio::test]
async fn a_respawn_with_no_retirement_since_its_predecessor_s_seat_stalls_there() {
    let alice = identity("personal:alice");
    let core = Arc::new(SeatCore::default());
    core.core.keep_journal();
    let runtime = runtime_over(None, &core);
    runtime.bind_lifecycle_projection(None);
    let seen = recording_hook(&runtime);
    runtime.settle_lifecycle_projection().await;

    register_live(&runtime, &core.core, spec(&alice), 0, SessionId::new())
        .await
        .expect("registered live");
    respawn_live_unretired(
        &core.core,
        &alice,
        &spec(&alice).profile,
        1,
        Some(SessionId::new()),
    )
    .expect("the respawn's seat");
    core.core.forget_in_history(&alice, 0);
    for _ in 0..3 {
        runtime.settle_lifecycle_projection().await;
    }
    let status = runtime.lifecycle_projection_status();
    assert_eq!(
        status.state,
        crate::identity_first::LifecycleProjectionState::Stalled,
        "{status:?}"
    );
    assert!(
        status
            .detail
            .as_deref()
            .is_some_and(|detail| detail.contains("between its seat")),
        "the failure names the incarnation's own seat: {status:?}"
    );
    assert!(lock(&seen).is_empty(), "{:?}", lock(&seen));
}

/// A roster pass lists alice again while a step of hers holds her
/// lifecycle lock past a side effect's deadline (a reset whose customizer
/// build is stuck, say). The pass's take-back of her unlisting does not
/// wait for the lock: it skips her, and her unlisting stays recorded. Once
/// the step answered, the next pass takes the unlisting back.
///
/// RED before the take-back's wait was bounded: the pass waited for the
/// lock for as long as the step held it.
#[tokio::test]
async fn a_roster_pass_does_not_wait_for_a_relisted_identity_s_held_lock() {
    let alice = identity("personal:alice");
    let core = Arc::new(SeatCore::default());
    core.core.keep_journal();
    let runtime = runtime_over(None, &core);
    let records: Arc<dyn crate::runtime::PersistentMetadataStore> =
        Arc::new(crate::runtime::InMemoryMetadataStore::new());
    runtime.set_identity_records(Arc::clone(&records), "relisted-held".to_string());
    runtime.bind_lifecycle_projection(None);
    register_live(&runtime, &core.core, spec(&alice), 0, SessionId::new())
        .await
        .expect("registered live");
    runtime.settle_lifecycle_projection().await;
    runtime.set_lifecycle_effect_limits(Duration::from_millis(200), 1);
    runtime
        .prepare_lifecycle_retirement(&alice, crate::runtime::LifecycleEffectVerb::Unlisted, None)
        .await
        .expect("the unlisting records what it owes")
        .expect("Core holds an incarnation of alice's");
    let unlisting = crate::runtime::LifecycleIntentKey::new(alice.as_str(), 0);

    let held = runtime.lifecycle_lock_for(&alice).await.lock_owned().await;
    tokio::time::timeout(
        Duration::from_secs(10),
        runtime.take_back_relisted_unlistings(&[spec(&alice)]),
    )
    .await
    .expect("the pass does not wait for a held lock past a side effect's deadline");
    assert!(
        records
            .lifecycle_effect_intent("relisted-held", &unlisting)
            .await
            .expect("the record reads")
            .is_some(),
        "the unlisting stays recorded for a later pass"
    );
    drop(held);
    runtime.take_back_relisted_unlistings(&[spec(&alice)]).await;
    assert!(
        records
            .lifecycle_effect_intent("relisted-held", &unlisting)
            .await
            .expect("the record reads")
            .is_none(),
        "the next pass takes the unlisting back"
    );
}

/// A reset reads alice's incarnation live, and a host's retirement of it
/// ends before the reset's rotation reaches Core: Core seats her fresh life
/// after that incarnation instead of starting its retirement, and the
/// rotation then fails (Core compensates the spawn). The fresh life's seat
/// says the rotation reached Core, so the reset keeps what it recorded and
/// answers that the rotation may have started.
///
/// RED before a step that read the incarnation live took the fresh life's
/// seat after it for its reach: the reset took its records back and
/// answered that nothing was started, although Core had seated.
#[tokio::test]
async fn a_reset_whose_incarnation_a_host_retired_first_keeps_what_it_recorded() {
    let alice = identity("personal:alice");
    let (reached_tx, reached_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let core = Arc::new(SeatCore {
        rotate_gate: StdMutex::new(Some((reached_tx, release_rx))),
        compensate_next_rotations: AtomicUsize::new(1),
        ..SeatCore::default()
    });
    core.core.keep_journal();
    let runtime = runtime_over(None, &core);
    runtime.bind_lifecycle_projection(None);
    let live = register_live(&runtime, &core.core, spec(&alice), 0, SessionId::new())
        .await
        .expect("registered live");
    runtime.settle_lifecycle_projection().await;

    let resetting = tokio::spawn({
        let runtime = Arc::clone(&runtime);
        let alice = alice.clone();
        async move { runtime.reset(&alice).await }
    });
    reached_rx.await.expect("the reset reaches Core");
    // A host's retirement of the incarnation the reset read live ends first.
    core.core.retire(&live);
    release_tx.send(()).expect("release the reset's rotation");
    let answered = resetting.await.expect("the reset's task");
    let Err(IdentityRuntimeError::Internal(detail)) = &answered else {
        panic!("the reset answers that its rotation may have started: {answered:?}");
    };
    assert!(detail.contains("may have started"), "{detail}");
    assert!(
        runtime
            .pending_rotation_intents()
            .await
            .expect("the records read")
            .contains_key(&alice),
        "the reset's rotation record stays written"
    );
}

/// Source guard: the lifecycle projection and the verbs decide no
/// lifecycle fact Core's journal carries. No retirement and seat are paired
/// by their positions in the journal (`rotation_successor`, a retirement
/// held back at the end of a full page with `split_last`), no rotation's
/// cause is read from Core's member history (`RotationFacts`), no step's
/// outcome from the bridge's error (`nothing_of_the_rotation_started`), and
/// no reset or delete runs the side effects of an incarnation Core had
/// already retired in its own process (`spawn_lifecycle_side_task`, an
/// inline scope drop or exit interview): each comes from the journal's facts.
#[test]
fn the_identity_runtime_decides_no_fact_core_s_journal_carries() {
    let sources = [
        (
            "identity_first/runtime/lifecycle_projection.rs",
            include_str!("lifecycle_projection.rs"),
        ),
        ("identity_first/runtime.rs", include_str!("../runtime.rs")),
        (
            "identity_first/runtime/declare.rs",
            include_str!("declare.rs"),
        ),
    ];
    for (file, source) in sources {
        let production = production_code(source);
        assert!(
            !production.trim().is_empty(),
            "{file}: no production code was read"
        );
        for needle in [
            "rotation_successor",
            "split_last",
            "RotationFacts",
            "nothing_of_the_rotation_started",
            "spawn_lifecycle_side_task",
        ] {
            assert!(
                !production.contains(needle),
                "{file} names `{needle}`: the fact it decides is Core's journal's"
            );
        }
    }
    let verbs = production_code(include_str!("../runtime.rs"));
    for needle in ["drop_orphaned_session_scope", "note_identity_retired"] {
        assert!(
            !verbs.contains(needle),
            "identity_first/runtime.rs names `{needle}`: a retirement's memory side effects \
             are the lifecycle projection's"
        );
    }
}

/// Source guard: the label or marker a member's binding carries is read for
/// the identity's membership in an allowlisted set of places only, which
/// empties once membership is read from Core's identity entry fact. The
/// lifecycle projection reads Core's member history once per page, inside
/// its membership helper (`page_members`), and asks
/// `binding_spawned_for_identity` there and in a verb's preparation only.
#[test]
fn membership_is_read_from_a_binding_s_label_only_where_allowed() {
    let lifecycle = production_code(include_str!("lifecycle_projection.rs"));
    assert_eq!(
        lifecycle.matches(".member_binding_history()").count(),
        1,
        "the lifecycle projection reads Core's member history in one place"
    );
    assert!(
        item_body(&lifecycle, "fn page_members(").contains(".member_binding_history()"),
        "the lifecycle projection reads Core's member history in its membership helper"
    );
    // The label and marker reads that membership is made of are asked
    // through `binding_spawned_for_identity` only, never inlined.
    for inlined in [
        "durable_identity_label(",
        "binding_carries_identity_marker(",
    ] {
        assert!(
            !lifecycle.contains(inlined),
            "the lifecycle projection reads membership inline (`{inlined}`)"
        );
    }
    let callers = [
        (
            "identity_first/runtime/lifecycle_projection.rs",
            lifecycle,
            2,
        ),
        (
            "identity_first/runtime.rs",
            production_code(include_str!("../runtime.rs")),
            2,
        ),
        (
            "identity_first/runtime/declare.rs",
            production_code(include_str!("declare.rs")),
            1,
        ),
        (
            "identity_first/first_boot_manifest.rs",
            production_code(include_str!("../first_boot_manifest.rs")),
            1,
        ),
        (
            "identity_first/runtime/records.rs",
            production_code(include_str!("records.rs")),
            0,
        ),
        (
            "identity_first/runtime/build_preparation.rs",
            production_code(include_str!("build_preparation.rs")),
            0,
        ),
        (
            "identity_first/bridge.rs",
            production_code(include_str!("../bridge.rs")),
            0,
        ),
        (
            "identity_first/orchestrator.rs",
            production_code(include_str!("../orchestrator.rs")),
            0,
        ),
        (
            "identity_first/held_bootstrap.rs",
            production_code(include_str!("../held_bootstrap.rs")),
            0,
        ),
        (
            "identity_first/adapters.rs",
            production_code(include_str!("../adapters.rs")),
            0,
        ),
        (
            "unified_runtime/mod.rs",
            production_code(include_str!("../../unified_runtime/mod.rs")),
            0,
        ),
        (
            "rpc_gateway.rs",
            production_code(include_str!("../../rpc_gateway.rs")),
            0,
        ),
    ];
    for (file, production, allowed) in callers {
        let calls = production.matches("binding_spawned_for_identity(").count()
            - production
                .matches("fn binding_spawned_for_identity(")
                .count();
        assert_eq!(
            calls, allowed,
            "{file} reads an identity's membership from a binding's label or marker in {calls} \
             places; {allowed} are allowed until membership is read from Core's identity entry \
             fact"
        );
    }
}

/// Source guard: Meta's session bridge forwards every kind of fact
/// Core's member lifecycle journal holds. Its conversion of a journal record
/// (`member_lifecycle_fact`) names each of Core's six
/// `MemberLifecycleEvent` kinds, so none is dropped at the bridge: a legacy
/// member reset reaches the projection, whose classification owes nothing
/// for it.
#[test]
fn the_bridge_forwards_every_core_lifecycle_kind() {
    let bridge = production_code(include_str!("../bridge.rs"));
    let conversion = item_body(&bridge, "fn member_lifecycle_fact(");
    // Each kind with the fields of Core's that the conversion forwards: an
    // arm that leaves one to `..` drops it.
    for (kind, fields) in [
        (
            "Spawned",
            &[
                "agent_identity",
                "generation",
                "role",
                "identity_entry",
                "history_unavailable",
                "session_id",
                "rotation",
            ][..],
        ),
        (
            "Retired",
            &[
                "agent_identity",
                "generation",
                "role",
                "session_id",
                "identity_entry",
                "cause",
                "rotation",
            ][..],
        ),
        (
            "RetirementStarted",
            &["agent_identity", "generation", "rotation", "cause"][..],
        ),
        (
            "EntryAdopted",
            &["agent_identity", "generation", "origin"][..],
        ),
        (
            "Reset",
            &["agent_identity", "previous_generation", "new_generation"][..],
        ),
        ("MobReset", &[][..]),
    ] {
        let header = format!("MemberLifecycleEvent::{kind} {{");
        let Some(at) = conversion.find(&header) else {
            panic!("the bridge's conversion of Core's journal names no `{kind}`");
        };
        let pattern = &conversion[at..];
        let pattern = &pattern[..pattern.find('}').unwrap_or(pattern.len())];
        for field in fields {
            assert!(
                binds_field(pattern, field),
                "the bridge's conversion of `{kind}` drops its `{field}`"
            );
        }
    }
}

/// Whether the struct pattern `pattern` binds its field `field`, by that
/// exact name, to anything but `_`: `field` alone, or `field: binding`.
/// A longer name that holds `field` (`successor_role` for `role`) is not
/// that field.
fn binds_field(pattern: &str, field: &str) -> bool {
    let is_name = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_';
    let bytes = pattern.as_bytes();
    let mut from = 0;
    while let Some(at) = pattern[from..].find(field).map(|at| from + at) {
        let end = at + field.len();
        from = end;
        if (at > 0 && is_name(bytes[at - 1])) || bytes.get(end).copied().is_some_and(is_name) {
            continue;
        }
        let Some(binding) = pattern[end..].trim_start().strip_prefix(':') else {
            return true;
        };
        let binding = binding.trim_start();
        let name = &binding[..binding.bytes().take_while(|byte| is_name(*byte)).count()];
        if name != "_" {
            return true;
        }
    }
    false
}

/// The kind guard above reads a field by its exact name: a longer name that
/// holds it, or a binding to `_`, does not forward it.
#[test]
fn a_forwarded_field_is_bound_by_its_exact_name() {
    let pattern = "MemberLifecycleEvent::Retired {\n    agent_identity: member,\n    \
                   generation,\n    successor_role,\n    cause: _,\n    rotation,\n    ..";
    for field in ["agent_identity", "generation", "rotation"] {
        assert!(binds_field(pattern, field), "`{field}` is bound");
    }
    for field in ["role", "cause", "session_id"] {
        assert!(!binds_field(pattern, field), "`{field}` is not bound");
    }
}

/// The source guards above read what they should: the body of the item a
/// header opens, braces inside it included, and nothing past it.
#[test]
fn an_item_body_ends_where_its_braces_close() {
    let code = "fn other() { a(); }\nfn page_members(x: u8) -> u8 {\n    if x { b() } else { c() }\n}\nfn after() { d(); }\n";
    let body = item_body(code, "fn page_members(");
    assert!(body.starts_with('{') && body.ends_with('}'), "{body}");
    assert!(body.contains("b()") && body.contains("c()"), "{body}");
    assert!(!body.contains("d()") && !body.contains("a()"), "{body}");
    assert_eq!(item_body(code, "fn missing("), "");
}

/// The body of the first item `header` opens in `code` (code as
/// [`production_code`] answers it, so no literal or comment holds a brace):
/// from its first `{` to the brace that closes it. Empty when `code` holds
/// no such item.
fn item_body<'a>(code: &'a str, header: &str) -> &'a str {
    let Some(start) = code.find(header) else {
        return "";
    };
    let Some(open) = code[start..].find('{').map(|at| start + at) else {
        return "";
    };
    let mut depth = 0_usize;
    for (at, byte) in code.bytes().enumerate().skip(open) {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return &code[open..=at];
                }
            }
            _ => {}
        }
    }
    ""
}
