//! Rows for the seat step and the first-boot carry-out: every roster
//! identity Core does not hold is seated Dormant in one Core call, with
//! nothing materialized; a pending first-boot row is carried out by its
//! identity's seat; and a rotation a crash interrupted is driven to its end
//! by the next compile.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use meerkat_core::types::HandlingMode;
use tokio::sync::oneshot;

use super::*;
use crate::identity_first::bridge::{
    BridgeDelivery, BridgeError, DormantSeatRequest, DormantSeatResult, MemberRuntimeBinding,
    ResumeRejectionKind, ResumeSessionOutcome, SessionBridge,
};
use crate::identity_first::core_test_bridge::{CoreReadsBridge, bind_live, register_live};
use crate::identity_first::core_test_members::CoreTestMembers;
use crate::identity_first::first_boot_manifest::FirstBootManifest;
use crate::identity_first::local_lease::LocalLeaseProvider;
use crate::identity_first::local_store::LocalContinuityStore;
use crate::identity_first::{
    AgentAddressability, AgentRuntimeId, DurabilityPolicy, IdentityRuntimeConfig, SessionSnapshot,
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
    wakes: AtomicUsize,
    /// Core keeps these sessions archived: a seat that names one, for an
    /// identity Core does not hold, answers `SessionArchived` and writes
    /// nothing.
    archived: Vec<SessionId>,
    /// Core's resume binds the identity on the named session at its next
    /// generation (a revival, or a declared role migration's restamp)
    /// instead of refusing it.
    resumes_onto_session: bool,
    /// The role migration this activation declares, by identity.
    migrations: BTreeMap<AgentIdentity, meerkat_mob::ProfileName>,
    /// The identities Core keeps host-pending: a delivery to one is refused
    /// until a seat of it, or its rotation, ends the mark.
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
    /// Core's rotation fails with this error after it may have started (a
    /// failure that is not one of Core's typed refusals before the start).
    rotation_failure: StdMutex<Option<BridgeError>>,
    /// The launch's member auth binding refuses these identities' member
    /// builds wherever Core builds one outside a wake, as the bridge reads
    /// the gateway's build check off Core's typed refusal: the live spawn
    /// of a fresh seat that owes a kickoff, and of the rotation of an
    /// identity Core does not hold that owes one (each refused before
    /// anything is seated), and a resume (an archived session's revival, a
    /// declared role migration). A seat or a rotation that owes no kickoff
    /// builds nothing, so it is not refused here.
    route_refused: Vec<AgentIdentity>,
    /// Every rotation asked of Core, refused or carried out.
    rotation_calls: AtomicUsize,
    /// The sessions Core holds unborn: a seat named them and no wake has
    /// created them (`MemberBindingHistory::session_unborn`).
    unborn: StdMutex<BTreeSet<SessionId>>,
    /// The incarnations Core holds materialized in this process, as Core's
    /// residency does: a revival (a resume) builds its member, a release or a
    /// retirement drops it, and a wake or a delivery that finds its seated
    /// member not materialized builds it.
    materialized: StdMutex<BTreeSet<AgentRuntimeId>>,
    /// Every member build Core made: each revival, and each wake or delivery
    /// that found its member not materialized.
    builds: AtomicUsize,
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
        if !self.resumes_onto_session {
            return Err(BridgeError::Mob(format!(
                "{identity}: a seat resumes no session"
            )));
        }
        // Core's revival or restamp: the identity is bound on the session at
        // its next generation. A spawn ends no host preparation, and Core
        // refuses the plain retirement of an occupant while its host
        // preparation is pending: the retirement never reaches its terminal.
        if let Some(live) = self.core.live_member(identity)? {
            if lock(&self.host_pending).contains(identity) {
                return Err(BridgeError::Mob(format!(
                    "{identity}: Core refuses retiring the occupant while its host preparation \
                     is pending"
                )));
            }
            self.core.retire(&live.agent_runtime_id);
            self.dematerialize(&live.agent_runtime_id);
        }
        let generation = self.next_generation(identity)?;
        let agent_runtime_id = bind_live(
            &self.core,
            identity,
            &spec.profile,
            generation,
            Some(session_id.clone()),
        )?;
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
        if let Some(failure) = lock(&self.rotation_failure).take() {
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
            self.core.retire(&live.agent_runtime_id);
            self.dematerialize(&live.agent_runtime_id);
        }
        let generation = self.next_generation(identity)?;
        let session_id = SessionId::new();
        bind_live(
            &self.core,
            identity,
            &spec.profile,
            generation,
            Some(session_id.clone()),
        )?;
        lock(&self.host_pending).remove(identity);
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
        self.core.retire(runtime_id);
        self.dematerialize(runtime_id);
        Ok(())
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
        runtime.register(spec.clone()).await;
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

/// A hook that holds a seat step right before its manifest rewrite: it says
/// when the step got there, then waits for `released` (forever, when that
/// never resolves).
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

/// A roster compile seats every identity Core does not
/// hold, Dormant, in ONE Core call planned from ONE read of Core's member
/// history. Nothing is created, resumed or woken: each identity is a member
/// bound to a fresh session Core minted, and a delivery materializes it.
#[tokio::test]
async fn x3_r15_a_compile_seats_every_unheld_identity_in_one_core_call_and_wakes_none() {
    let core = Arc::new(SeatCore::default());
    let runtime = runtime_over(None, &core);
    let specs = ["personal:alice", "personal:bob", "personal:carol"]
        .map(|name| spec(&identity(name)))
        .to_vec();
    register_all(&runtime, &specs).await;

    let outcomes = runtime.seat_identities(&specs, None).await;

    for spec in &specs {
        let outcome = outcomes
            .get(&spec.identity)
            .expect("one outcome per identity")
            .as_ref()
            .expect("seated");
        assert!(!outcome.already_seated, "{}", spec.identity);
        assert!(outcome.minted, "{}: a fresh seat", spec.identity);
        assert_eq!(outcome.session_id, core.session_of(&spec.identity));
    }
    let calls = lock(&core.seat_calls).clone();
    assert_eq!(calls.len(), 1, "one Core call seats the whole compile");
    assert_eq!(
        calls[0],
        specs
            .iter()
            .map(|spec| (spec.identity.clone(), None))
            .collect::<Vec<_>>()
    );
    assert_eq!(core.history_reads.load(Ordering::SeqCst), 1);
    assert_eq!(core.creates.load(Ordering::SeqCst), 0);
    assert_eq!(core.resumes.load(Ordering::SeqCst), 0);
    assert_eq!(core.wakes.load(Ordering::SeqCst), 0, "a seat wakes nothing");
}

/// A compile over identities Core already holds calls Core for
/// nothing: one history read plans them all, and every one answers
/// already seated.
#[tokio::test]
async fn r15_a_compile_over_seated_identities_calls_core_for_nothing() {
    let core = Arc::new(SeatCore::default());
    let runtime = runtime_over(None, &core);
    let specs = ["personal:alice", "personal:bob"]
        .map(|name| spec(&identity(name)))
        .to_vec();
    register_all(&runtime, &specs).await;
    runtime.seat_identities(&specs, None).await;

    let outcomes = runtime.seat_identities(&specs, None).await;

    assert!(
        outcomes
            .values()
            .all(|outcome| outcome.as_ref().is_ok_and(|seat| seat.already_seated)),
        "every identity is already seated"
    );
    assert_eq!(lock(&core.seat_calls).len(), 1, "no second Core call");
    assert_eq!(
        core.history_reads.load(Ordering::SeqCst),
        2,
        "one read per compile"
    );
}

/// The first-boot carry-out: each pending `Resume` row is its identity's
/// seat on the row's session, all of them in one Core call, and the
/// manifest is rewritten once for the whole step (here removed, every row
/// being done). Nothing is resumed or woken.
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
    register_all(&runtime, &specs).await;

    let outcomes = runtime.seat_identities(&specs, None).await;

    for row in &rows {
        assert!(outcomes[row.identity()].is_ok(), "{}", row.identity());
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
    register_all(&runtime, &specs).await;

    let outcomes = runtime.seat_identities(&specs, None).await;

    assert!(outcomes[&refused].is_err(), "the refused row fails");
    assert_eq!(
        core.session_of(&refused),
        None,
        "the refused row binds nothing"
    );
    for row in rows.iter().filter(|row| row.identity() != &refused) {
        assert!(outcomes[row.identity()].is_ok(), "{}", row.identity());
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
    register_all(&runtime, &specs).await;
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

    let outcomes = runtime.seat_identities(&specs, None).await;

    assert!(outcomes[&alice].is_ok());
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
/// another identity of the same seat step holds no step up. Every draft is
/// built before any Core step, with no lifecycle lock held: the send seats
/// its own target (carrying out the target's row) and is delivered, and the
/// step completes. Each identity is seated once, on its row's session, and
/// the manifest ends with every row done.
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
    register_all(&runtime, &specs).await;

    let outcomes = tokio::time::timeout(
        Duration::from_secs(30),
        runtime.seat_identities(&specs, None),
    )
    .await
    .expect("the step completes: a send from a build into the step waits for no step");

    for row in &rows {
        assert!(
            outcomes[row.identity()].is_ok(),
            "{}: {:?}",
            row.identity(),
            outcomes[row.identity()].as_ref().err()
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
    assert_eq!(manifest_rows(state_dir.path()), None, "every row is done");
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

/// Two seat steps that planned the same identity Core does not hold before
/// either reached Core (both builds wait until both steps planned) seat it
/// once. Core binds it in the first step and answers the second step's
/// fresh seat as already seated on the session it bound, so both steps
/// succeed, the second reads the identity already seated, and Core holds
/// one binding.
#[tokio::test]
async fn two_seat_steps_planned_together_seat_an_identity_once() {
    let alice = identity("personal:alice");
    let core = Arc::new(SeatCore::default());
    let runtime = runtime_over(None, &core);
    runtime
        .set_agent_customizer(Some(Arc::new(BuildsMeet {
            barrier: tokio::sync::Barrier::new(2),
        })))
        .await;
    let specs = vec![spec(&alice)];
    register_all(&runtime, &specs).await;

    let (first, second) = tokio::time::timeout(Duration::from_secs(30), async {
        tokio::join!(
            runtime.seat_identities(&specs, None),
            runtime.seat_identities(&specs, None)
        )
    })
    .await
    .expect("both steps complete");

    let outcomes = [&first[&alice], &second[&alice]];
    assert!(
        outcomes.iter().all(|outcome| outcome.is_ok()),
        "both steps seat the identity: {:?}",
        outcomes
            .iter()
            .map(|outcome| outcome.as_ref().err())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| outcome.as_ref().is_ok_and(|seat| seat.already_seated))
            .count(),
        1,
        "the later step finds the identity seated"
    );
    assert_eq!(lock(&core.seat_calls).len(), 2, "each step asked Core");
    assert_eq!(core.generation_of(&alice), Some(0), "Core bound it once");
    assert!(core.session_of(&alice).is_some());
}

/// While a seat step waits for its one manifest rewrite (here it is held
/// right before it), the identities whose rows it carried out are free: a
/// reset of one goes ahead at once and writes the manifest down first, so
/// Core rotates the identity only once its row is gone, and a stop then
/// leaves no row to read contradicted. A reset of the identity whose seat
/// Core refused is refused as pending: its row stays. Released, the step
/// finds the manifest already written down to the refused row.
#[tokio::test]
async fn a_reset_while_the_seat_step_waits_for_its_rewrite_writes_the_row_out_first() {
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
    register_all(&runtime, &specs).await;
    let (release, released) = oneshot::channel::<()>();
    let reached_rewrite = hold_at_rewrite(&runtime, async move {
        let _ = released.await;
    });

    let window = async {
        reached_rewrite.await.expect("the step reaches its rewrite");
        let fresh = tokio::time::timeout(Duration::from_secs(30), runtime.reset(&reset))
            .await
            .expect("the reset waits for no seat step")
            .expect("the reset goes ahead");
        let refusal = runtime.reset(&refused).await;
        let listed = manifest_rows(state_dir.path());
        assert!(release.send(()).is_ok(), "the step waits to be released");
        (fresh, refusal, listed)
    };
    let (outcomes, (fresh, refusal, listed)) =
        tokio::join!(runtime.seat_identities(&specs, None), window);

    assert!(
        outcomes[&reset].is_ok(),
        "{:?}",
        outcomes[&reset].as_ref().err()
    );
    assert!(outcomes[rows[1].identity()].is_ok());
    assert!(outcomes[&refused].is_err(), "Core refused this seat");
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

/// A seat step dropped after Core seated its rows and before its one
/// manifest rewrite (the request that carried the roster compile went away)
/// leaves those rows settled in this process and still listed in the
/// manifest. A reset of such an identity writes the manifest first, so Core
/// rotates the identity only once its row is gone. The next boot after a
/// stop reads the rest against Core's member history: the rows no step
/// carried out are seated once, no identity is seated twice, the reset
/// identity keeps its fresh life (no row of it is left to read
/// contradicted), and the manifest is removed.
#[tokio::test]
async fn a_seat_step_dropped_before_its_rewrite_leaves_no_row_a_reset_could_contradict() {
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
        register_all(&runtime, &specs).await;
        // The step seats the first three rows' identities and is dropped
        // at its rewrite; no step carries the other three rows out.
        let reached_rewrite = hold_at_rewrite(&runtime, std::future::pending::<()>());
        tokio::select! {
            _ = runtime.seat_identities(&specs[..3], None) => {
                panic!("the step is held at its rewrite");
            }
            reached = reached_rewrite => reached.expect("the step reaches its rewrite"),
        }
        for row in &rows[..3] {
            assert_eq!(
                core.session_of(row.identity()).as_ref(),
                Some(row.session()),
                "the dropped step seated {}",
                row.identity()
            );
        }
        assert_eq!(
            runtime.first_boot_rewrites.load(Ordering::SeqCst),
            0,
            "the dropped step never rewrote the manifest"
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
            "the reset wrote the carried-out rows out first; the rows no step carried out stay"
        );
    }

    // The process stops; the next boot reads the manifest again.
    let runtime = runtime_over(Some(state_dir.path()), &core);
    register_all(&runtime, &specs).await;
    let outcomes = runtime.seat_identities(&specs, None).await;

    for (identity, outcome) in &outcomes {
        assert!(
            outcome.is_ok(),
            "no row reads contradicted: {identity}: {:?}",
            outcome.as_ref().err()
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

/// The stop of a boot whose seat step seated its rows but had not rewritten
/// the manifest yet (here the step is held right before its rewrite, then
/// dropped with its runtime) leaves every row in the manifest. The next boot
/// reads each row against Core's member history: a seated row reads done,
/// and its seat only ends the identity's host preparation (Core answers it
/// already seated and binds nothing); a row whose seat Core refused is tried
/// again; a row whose session Core holds no durable copy of is refused by
/// Core, typed; and a row Core's history contradicts is refused before any
/// Core step, with Core's binding left as it is. The next boot's one rewrite
/// keeps exactly those three rows.
#[tokio::test]
async fn a_boot_that_stops_before_its_seat_step_rewrite_settles_the_seated_rows_next_boot() {
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
        register_all(&runtime, &specs).await;
        let reached_rewrite = hold_at_rewrite(&runtime, std::future::pending::<()>());
        tokio::select! {
            _ = runtime.seat_identities(&specs, None) => {
                panic!("the step is held at its rewrite");
            }
            reached = reached_rewrite => reached.expect("the step reaches its rewrite"),
        }
        assert_eq!(
            runtime.first_boot_rewrites.load(Ordering::SeqCst),
            0,
            "the boot stops before the step's rewrite"
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
    register_all(&runtime, &specs).await;
    let outcomes = runtime.seat_identities(&specs, None).await;

    for row in &rows[..3] {
        let seat = outcomes[row.identity()]
            .as_ref()
            .expect("a seated row reads done");
        assert!(
            seat.already_seated,
            "{} is not seated again",
            row.identity()
        );
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
        outcomes[&failing].is_err(),
        "the row whose seat Core refused fails again"
    );
    assert!(
        matches!(
            &outcomes[&unheld],
            Err(IdentityRuntimeError::FirstBootRowRefused {
                core_session: None,
                ..
            })
        ),
        "a row whose session Core holds no durable copy of is refused: {:?}",
        outcomes[&unheld].as_ref().err()
    );
    assert!(
        matches!(
            &outcomes[&contradicted],
            Err(IdentityRuntimeError::FirstBootRowRefused {
                core_session: Some(session),
                ..
            }) if *session == elsewhere
        ),
        "a row Core's history contradicts is refused: {:?}",
        outcomes[&contradicted].as_ref().err()
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

/// A pending first-boot `Fresh` row of an identity Core retired is
/// carried out by a Dormant seat on a fresh session in the seat call: no
/// spawn, no retire, no rotation.
#[tokio::test]
async fn r64_a_first_boot_fresh_row_is_a_dormant_seat_on_a_fresh_session() {
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
    register_all(&runtime, &specs).await;

    let outcomes = runtime.seat_identities(&specs, None).await;

    let seat = outcomes[&alice].as_ref().expect("seated");
    assert!(seat.minted, "a fresh session");
    let session = core.session_of(&alice).expect("bound");
    assert_ne!(session, retired_session);
    assert_eq!(
        lock(&core.seat_calls).clone(),
        vec![vec![(alice.clone(), None)]]
    );
    assert!(lock(&core.rotations).is_empty(), "no rotation");
    assert_eq!(core.creates.load(Ordering::SeqCst), 0, "no spawn");
    assert_eq!(
        FirstBootManifest::read(state_dir.path()).expect("reads"),
        None
    );
}

/// A rotation (`reset`, `delete_identity`) a crash interrupted is
/// recorded; the next compile drives it to its end through Core's rotation,
/// and removes the record once Core answered.
#[tokio::test]
async fn r114_an_interrupted_rotation_is_driven_to_its_end_by_the_next_compile() {
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
    register_all(&runtime, &specs).await;
    runtime
        .record_rotation_intent(&alice, "reset", Some(1))
        .await
        .expect("the rotation is recorded");

    let outcomes = runtime.seat_identities(&specs, None).await;

    let seat = outcomes[&alice].as_ref().expect("rotated");
    assert!(seat.minted);
    assert_eq!(lock(&core.rotations).clone(), vec![alice.clone()]);
    assert_eq!(core.generation_of(&alice), Some(2));
    assert_ne!(core.session_of(&alice), Some(old_session));
    assert!(
        runtime
            .pending_rotation_intents()
            .await
            .expect("the records read")
            .is_empty(),
        "the finished rotation's record is removed"
    );
}

/// When Core already finished the recorded rotation (the crash came
/// after its terminal step), the compile settles the record with a seat
/// step that writes nothing, and no second rotation runs.
#[tokio::test]
async fn r114_a_rotation_core_finished_settles_its_record_without_a_second_rotation() {
    let alice = identity("personal:alice");
    let core = Arc::new(SeatCore::default());
    let new_session = SessionId::new();
    bind_live(
        &core.core,
        &alice,
        &meerkat_mob::ProfileName::from("personal"),
        2,
        Some(new_session.clone()),
    )
    .expect("bound");
    let runtime = runtime_over(None, &core);
    let specs = vec![spec(&alice)];
    register_all(&runtime, &specs).await;
    runtime
        .record_rotation_intent(&alice, "delete_identity", Some(1))
        .await
        .expect("the rotation is recorded");

    let outcomes = runtime.seat_identities(&specs, None).await;

    let seat = outcomes[&alice].as_ref().expect("settled");
    assert!(seat.already_seated, "Core holds the rotated member already");
    assert!(lock(&core.rotations).is_empty(), "no second rotation");
    assert_eq!(core.session_of(&alice), Some(new_session));
    assert!(
        runtime
            .pending_rotation_intents()
            .await
            .expect("the records read")
            .is_empty()
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
        ("identity_first/runtime/seat.rs", include_str!("seat.rs")),
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

/// The member id Meta hands the mob as host-pending for `identity`.
fn member_id(identity: &AgentIdentity) -> meerkat_mob::AgentIdentity {
    crate::member_comms_id::roster_member_id_for_identity(identity.as_str())
}

fn text(content: &str) -> meerkat_core::ContentInput {
    meerkat_core::ContentInput::Text(content.to_string())
}

/// A first-boot `Resume` row whose session Core keeps archived (a directory
/// an earlier release wrote, whose members it retired): Core answers the
/// seat `SessionArchived`, the step revives the identity once on that
/// session and releases it, and since neither ends the host preparation
/// Meta asked Core for at start, the step ends it with a settles-only seat.
/// The identity's next delivery goes through.
#[tokio::test]
async fn an_archived_first_boot_session_revived_and_released_ends_its_host_preparation() {
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
    register_all(&runtime, &[spec(&alice)]).await;

    let outcomes = runtime.seat_identities(&[spec(&alice)], None).await;

    assert!(
        outcomes[&alice].is_ok(),
        "{:?}",
        outcomes[&alice].as_ref().err()
    );
    assert_eq!(
        core.session_of(&alice),
        Some(session.clone()),
        "the identity is seated on its own session"
    );
    assert_eq!(
        lock(&core.releases).as_slice(),
        std::slice::from_ref(&alice),
        "the revival is released"
    );
    assert!(
        !lock(&core.host_pending).contains(&alice),
        "the step ended the host preparation Meta asked for"
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
/// member: the identity's next seat step revives it on that session.
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
/// identity whose built member Core retired: the seat step revives the
/// identity once on its archived session, and the revival stays live, since
/// the materialization makes the member live right after the step. Core's
/// wake finds it live: the member is built once, by its revival, and nothing
/// is released.
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
    assert_eq!(core.resumes.load(Ordering::SeqCst), 1, "one revival");
    assert!(
        lock(&core.releases).is_empty(),
        "the revival is not released"
    );
    assert_eq!(
        core.wakes.load(Ordering::SeqCst),
        1,
        "Core's wake is asked once"
    );
    assert_eq!(
        core.builds.load(Ordering::SeqCst),
        1,
        "the member is built once, by its revival: the wake finds it live"
    );
}

/// A respawn of a built member: Core retires it and keeps its session
/// archived, the seat step revives the identity on that session, and the
/// revival stays live for the respawn's wake. The respawn builds the member
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
    assert_eq!(core.resumes.load(Ordering::SeqCst), 1, "one revival");
    assert!(
        lock(&core.releases).is_empty(),
        "the revival is not released"
    );
    assert_eq!(
        core.builds.load(Ordering::SeqCst) - built,
        1,
        "the respawn builds the member once, by its revival: its wake finds it live"
    );
}

/// A delivery to an identity whose built member Core retired: the seat step
/// before its admission revives the identity once on its archived session,
/// and the revival stays live for the delivery Core makes right after it.
/// The member is built once, by its revival, and nothing is released.
#[tokio::test]
async fn a_delivery_over_an_archived_session_builds_the_member_once() {
    let alice = identity("personal:alice");
    let (core, runtime, _session) = retired_on_archived_session(&alice).await;

    runtime
        .send(&alice, &text("hello"))
        .await
        .expect("the delivery goes through");

    assert_eq!(core.resumes.load(Ordering::SeqCst), 1, "one revival");
    assert!(
        lock(&core.releases).is_empty(),
        "the revival is not released"
    );
    assert_eq!(
        lock(&core.delivered).as_slice(),
        [AgentRuntimeId::parse("rt:personal:alice:1").expect("a runtime id")],
        "the delivery goes to the revived member"
    );
    assert_eq!(
        core.builds.load(Ordering::SeqCst),
        1,
        "the member is built once, by its revival: the delivery finds it live"
    );
}

/// The roster pass's seat of an identity whose built member Core retired
/// still revives it once on its archived session and releases it: the pass
/// makes nothing live, so the identity is a Dormant member on its own
/// session, built at its first delivery. A seat step keeps a revival live
/// only for a caller that makes the member live right after it.
#[tokio::test]
async fn the_roster_pass_still_releases_an_archived_sessions_revival() {
    let alice = identity("personal:alice");
    let (core, runtime, session) = retired_on_archived_session(&alice).await;

    let outcomes = runtime.seat_identities(&[spec(&alice)], None).await;

    let outcome = outcomes[&alice].as_ref().expect("alice is seated");
    assert_eq!(
        outcome.session_id,
        Some(session),
        "seated on its own session"
    );
    assert_eq!(core.resumes.load(Ordering::SeqCst), 1, "one revival");
    assert_eq!(
        lock(&core.releases).as_slice(),
        std::slice::from_ref(&alice),
        "the revival is released"
    );
    assert!(
        lock(&core.materialized).is_empty(),
        "the member is not materialized after the pass"
    );
    assert_eq!(
        core.wakes.load(Ordering::SeqCst),
        0,
        "the pass wakes nothing"
    );
}

/// An explicit materialization that applies a declared role migration: the
/// seat step restamps the member onto its own session under the new role,
/// which builds it, and the restamp stays live, since the materialization
/// makes the member live right after the step. Core's wake finds it live:
/// the member is built once, by its restamp, and nothing is released.
#[tokio::test]
async fn a_materialization_that_applies_a_declared_migration_builds_the_member_once() {
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
    runtime.register(spec(&alice)).await;

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
    assert_eq!(core.resumes.load(Ordering::SeqCst), 1, "one restamp");
    assert!(
        lock(&core.releases).is_empty(),
        "the restamp is not released"
    );
    assert_eq!(
        core.builds.load(Ordering::SeqCst),
        1,
        "the member is built once, by its restamp: the wake finds it live"
    );
}

/// A declared role migration of an identity whose first-boot row Core
/// already shows done: the step restamps the member onto its own session
/// under the new role and releases it. The restamp retires the member under
/// its previous role first, which Core refuses while the host preparation
/// Meta asked for at start is pending (the double refuses it as Core does),
/// so the step ends that mark with a settles-only seat before the move, and
/// the identity's next delivery goes through.
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
    register_all(&runtime, &[spec(&alice)]).await;

    let outcomes = runtime.seat_identities(&[spec(&alice)], None).await;

    assert!(
        outcomes[&alice].is_ok(),
        "{:?}",
        outcomes[&alice].as_ref().err()
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
        "the step ended the host preparation Meta asked for"
    );
    runtime
        .send(&alice, &text("hello"))
        .await
        .expect("the identity's next delivery goes through");
}

/// A reset Core refuses before any of it starts (here: the mob is not
/// running) answers the typed retryable refusal and takes its rotation
/// record back, so no later roster compile or restart resets the identity.
/// A record an earlier reset left is put back as it was.
#[tokio::test]
async fn a_reset_core_refuses_before_it_starts_leaves_no_rotation_to_carry_out() {
    let alice = identity("personal:alice");
    let bob = identity("personal:bob");
    let core = Arc::new(SeatCore {
        refuses_rotation: true,
        ..SeatCore::default()
    });
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
    let outcomes = runtime.seat_identities(&[spec(&alice)], None).await;
    assert!(
        outcomes[&alice].is_ok(),
        "{:?}",
        outcomes[&alice].as_ref().err()
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
/// member: its live member is released, never retired, and with no
/// migration declared the seat step holds it typed under its previous role.
/// Its deliveries are refused typed and Core's binding stays as it was.
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

    let mut retired = Vec::new();
    runtime
        .reconcile_roster_members(&[spec(&alice)], 1, &mut retired)
        .await
        .expect("the pass reconciles the roster");
    assert!(retired.is_empty(), "the pass retires nobody");
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

    let outcome = runtime
        .seat_identities(&[spec(&alice)], None)
        .await
        .remove(&alice)
        .expect("the step answers alice");
    assert!(
        matches!(
            outcome,
            Err(IdentityRuntimeError::WakeHeld {
                hold: WakeHold::RoleMigrationNotApplied,
                ..
            })
        ),
        "{:?}",
        outcome.as_ref().err()
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

/// The same move with the migration declared: the seat step restamps the
/// member onto its own session under the new role and releases it, and its
/// next delivery goes through.
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

    let mut retired = Vec::new();
    runtime
        .reconcile_roster_members(&[spec(&alice)], 1, &mut retired)
        .await
        .expect("the pass reconciles the roster");
    let outcome = runtime
        .seat_identities(&[spec(&alice)], None)
        .await
        .remove(&alice)
        .expect("the step answers alice");

    assert!(outcome.is_ok(), "{:?}", outcome.as_ref().err());
    assert!(retired.is_empty(), "the pass retires nobody");
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
        "the roster pass releases the restamp: the member is not materialized after it"
    );
    runtime
        .send(&alice, &text("hello"))
        .await
        .expect("the moved identity's delivery goes through");
}

/// The bootstrap pass's snapshot of one seated identity, `Active` and
/// ready, bound to the runtime's identity entry.
async fn ready_bootstrap_entry(runtime: &IdentityRuntime, identity: &AgentIdentity) {
    let generation = runtime
        .begin_identity_bootstrap_pending(super::super::IdentityBootstrapMode::EagerMaterialize);
    runtime
        .accept_bootstrap_spec_locked(spec(identity), generation)
        .await
        .expect("the entry is bound to the pass");
    runtime.modify_bootstrap_status(Some(generation), |snapshot| {
        snapshot.identities.insert(
            identity.clone(),
            super::super::IdentityBootstrapEntry {
                state: super::super::IdentityBootstrapState::Active,
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
        Some(super::super::IdentityBootstrapState::Active),
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
        Some(super::super::IdentityBootstrapState::Broken),
        "{status:?}"
    );
    assert!(!status.ready, "{status:?}");
}

/// A materialization or warm of a member Core already holds, which Core
/// answers with its Broken verdict carrying a durable resume hold, answers
/// the held refusal on the session Core names, and the identity records it.
/// The seat step names no session for a member Core already holds, so the
/// session is read off Core's refusal; a refusal with no session would read
/// as retryable.
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
/// member it planned for. The plan is made before the drafts are built, with
/// no lifecycle lock held; a reset that moves the identity onto another
/// session meanwhile leaves the move nothing to do: no release, no second
/// rotation, and the identity stays where the reset put it. Its next seat
/// step plans again.
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

    let outcomes = runtime
        .seat_identities(std::slice::from_ref(&reviewer), None)
        .await;

    let outcome = outcomes[&alice]
        .as_ref()
        .expect("the stale move is skipped, not refused");
    assert!(
        outcome.already_seated,
        "nothing was released, rotated or seated"
    );
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

/// A declared role migration of a member whose session no wake has created
/// is planned as Core's rotation, made with no lifecycle lock held. A wake
/// that creates the session while the draft is built (a peer message,
/// outside Meta's lock) leaves the session as it is: the carry-out reads
/// Core's unborn fact again under the lock and refuses retryably, with no
/// release and no rotation. The identity's next seat step plans the
/// declared migration, which restamps the member onto that session under
/// the new role.
#[tokio::test]
async fn a_declared_move_of_an_unborn_seat_woken_while_its_draft_was_built_keeps_its_session() {
    let alice = identity("personal:alice");
    let previous = meerkat_mob::ProfileName::from("previous");
    let session = SessionId::new();
    let core = Arc::new(SeatCore {
        resumes_onto_session: true,
        migrations: BTreeMap::from([(alice.clone(), previous.clone())]),
        ..SeatCore::default()
    });
    lock(&core.unborn).insert(session.clone());
    let runtime = runtime_over(None, &core);
    let mut registered = spec(&alice);
    registered.profile = previous.clone();
    register_live(&runtime, &core.core, registered, 0, session.clone())
        .await
        .expect("registered live");
    runtime
        .set_agent_customizer(Some(Arc::new(WakesWhileBuilding {
            core: Arc::clone(&core),
            identity: alice.clone(),
            session: session.clone(),
        })))
        .await;
    let role = || {
        core.core
            .last_member_binding(&alice)
            .expect("Core's history reads")
            .map(|binding| binding.role().clone())
    };

    let outcome = runtime
        .seat_identities(&[spec(&alice)], None)
        .await
        .remove(&alice)
        .expect("the step answers alice");
    match &outcome {
        Err(IdentityRuntimeError::IdentityWakeRetryable {
            session_id, detail, ..
        }) => {
            assert_eq!(session_id.as_ref(), Some(&session));
            assert!(detail.contains("a wake created session"), "{detail}");
        }
        other => panic!(
            "the rotation of a session a wake created is refused retryably: {:?}",
            other.as_ref().err()
        ),
    }
    assert!(lock(&core.releases).is_empty(), "no release was asked");
    assert_eq!(
        core.rotation_calls.load(Ordering::SeqCst),
        0,
        "the created session is not rotated away"
    );
    assert_eq!(core.session_of(&alice), Some(session.clone()));
    assert_eq!(role(), Some(previous), "nothing moved yet");

    let outcome = runtime
        .seat_identities(&[spec(&alice)], None)
        .await
        .remove(&alice)
        .expect("the next step answers alice");
    assert!(outcome.is_ok(), "{:?}", outcome.as_ref().err());
    assert_eq!(core.rotation_calls.load(Ordering::SeqCst), 0, "no rotation");
    assert_eq!(
        core.resumes.load(Ordering::SeqCst),
        1,
        "the declared migration restamps the member"
    );
    assert_eq!(
        core.session_of(&alice),
        Some(session),
        "the member keeps its session"
    );
    assert_eq!(
        role(),
        Some(meerkat_mob::ProfileName::from("personal")),
        "the member runs the new role"
    );
}

/// A host customizer that, while it builds `identity`'s draft, has Core
/// create `session`, the identity's unborn seat session, as a wake a peer
/// message caused meanwhile would.
struct WakesWhileBuilding {
    core: Arc<SeatCore>,
    identity: AgentIdentity,
    session: SessionId,
}

#[async_trait::async_trait]
impl AgentCustomizer for WakesWhileBuilding {
    async fn customize_build(
        &self,
        context: &AgentBuildContext,
        _spec: &DurableAgentSpec,
        _draft: &mut AgentBuildDraft,
    ) -> Result<(), crate::identity_first::CustomizerError> {
        if context.identity == self.identity {
            lock(&self.core.unborn).remove(&self.session);
        }
        Ok(())
    }
}

/// A seeded identity Core already holds, with nothing owed (its seat step
/// plans no Core call): the step still ends the host preparation Meta asked
/// for at start, with one settles-only seat on the identity's session.
#[tokio::test]
async fn a_seeded_identity_core_already_holds_has_its_host_preparation_ended() {
    let alice = identity("personal:alice");
    let session = SessionId::new();
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
    let runtime = runtime_over(None, &core);
    runtime.seed_host_pending([member_id(&alice)]);
    register_all(&runtime, &[spec(&alice)]).await;

    let outcomes = runtime.seat_identities(&[spec(&alice)], None).await;

    assert!(
        outcomes[&alice].is_ok(),
        "{:?}",
        outcomes[&alice].as_ref().err()
    );
    assert_eq!(
        lock(&core.seat_calls).as_slice(),
        &[vec![(alice.clone(), Some(session.clone()))]],
        "one settles-only seat on the identity's own session"
    );
    assert!(
        !lock(&core.host_pending).contains(&alice),
        "the step ended the host preparation Meta asked for"
    );
    assert_eq!(core.generation_of(&alice), Some(0), "nothing was rotated");
}

/// A delivery to a seeded identity Core holds takes the seat step instead of
/// going straight to Core, so the mark is ended before the delivery, which
/// goes through.
#[tokio::test]
async fn a_delivery_to_a_seeded_identity_ends_its_host_preparation_first() {
    let alice = identity("personal:alice");
    let session = SessionId::new();
    let core = Arc::new(SeatCore {
        admits_deliveries: true,
        ..SeatCore::default()
    });
    lock(&core.host_pending).insert(alice.clone());
    let runtime = runtime_over(None, &core);
    register_live(&runtime, &core.core, spec(&alice), 0, session.clone())
        .await
        .expect("registered live");
    runtime.seed_host_pending([member_id(&alice)]);

    runtime
        .send(&alice, &text("hello"))
        .await
        .expect("the delivery goes through");

    assert!(
        !lock(&core.host_pending).contains(&alice),
        "the delivery's seat step ended the mark"
    );
    assert_eq!(
        lock(&core.seat_calls).as_slice(),
        &[vec![(alice.clone(), Some(session))]],
        "one settles-only seat, before the delivery"
    );
    assert_eq!(lock(&core.delivered).len(), 1, "one delivery");
}

/// The members Meta hands the mob as host-pending before its build name
/// only listed identities Meta seats: a hosted identity (placed, or bound to
/// an external runtime) is never named, since Core seats no such member and
/// no seat step could end its mark.
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

    let pending = super::super::host_pending_members_before_build(
        Some(&location),
        None,
        &[spec(&local), placed(&hosted)],
    )
    .await;

    assert_eq!(pending, vec![member_id(&local)]);
}

/// A reset whose rotation fails after it may have started keeps its
/// rotation record, so the next compile drives the rotation to its end,
/// once.
#[tokio::test]
async fn a_reset_that_may_have_started_keeps_its_record_and_rotates_once_later() {
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
    assert!(
        matches!(failed, Err(IdentityRuntimeError::Internal(_))),
        "{failed:?}"
    );
    assert!(
        runtime
            .pending_rotation_intents()
            .await
            .expect("the records read")
            .contains_key(&alice),
        "the record of a rotation that may have started stays"
    );

    let outcomes = runtime.seat_identities(&[spec(&alice)], None).await;
    assert!(
        outcomes[&alice].is_ok(),
        "{:?}",
        outcomes[&alice].as_ref().err()
    );
    assert_eq!(
        lock(&core.rotations).as_slice(),
        std::slice::from_ref(&alice),
        "the next compile rotates the identity, once"
    );
    assert!(
        runtime
            .pending_rotation_intents()
            .await
            .expect("the records read")
            .is_empty(),
        "the finished rotation's record is gone"
    );
    assert_eq!(core.generation_of(&alice), Some(1));
}

/// A reset whose fresh life spec cannot be lowered is refused before Core
/// is asked: its record is taken back, and no later compile resets the
/// identity.
#[tokio::test]
async fn a_reset_whose_spec_cannot_be_lowered_leaves_no_rotation_to_carry_out() {
    let alice = identity("personal:alice");
    let core = Arc::new(SeatCore::default());
    *lock(&core.rotation_failure) = Some(BridgeError::InvalidInput(
        "lowering the fresh life spec failed".to_string(),
    ));
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
    let outcomes = runtime.seat_identities(&[spec(&alice)], None).await;
    assert!(
        outcomes[&alice].is_ok(),
        "{:?}",
        outcomes[&alice].as_ref().err()
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
/// binding), it is final and typed: the compile and every delivery answer
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
    register_all(&runtime, &[kickoff_spec(&alice)]).await;

    let outcomes = runtime.seat_identities(&[kickoff_spec(&alice)], None).await;
    let refused = outcomes[&alice]
        .as_ref()
        .err()
        .expect("the compile's seat is refused");
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
        "one seat per step (the compile and each delivery), never retried"
    );
    assert!(core.session_of(&alice).is_none(), "nothing was seated");
    assert_eq!(core.wakes.load(Ordering::SeqCst), 0, "nothing was woken");
    assert!(lock(&core.delivered).is_empty(), "nothing was delivered");
}

/// A reset of an identity Core does not hold that owes a kickoff is Core's
/// live spawn of its fresh life. Refused for its member route, it is final
/// and leaves nothing to carry out: none of the rotation started, so its
/// record is taken back, a restart hands the mob no host-pending identity
/// for it, and the next compile rotates nothing (its seat, the same live
/// spawn, is refused final again).
#[tokio::test]
async fn a_reset_refused_for_the_member_route_leaves_no_rotation_to_carry_out() {
    let alice = identity("personal:alice");
    let core = Arc::new(SeatCore {
        route_refused: vec![alice.clone()],
        ..SeatCore::default()
    });
    let runtime = runtime_over(None, &core);
    let records = Arc::new(crate::runtime::InMemoryMetadataStore::new());
    runtime.set_identity_records(records.clone(), "route-refused".to_string());
    register_all(&runtime, &[kickoff_spec(&alice)]).await;

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
    let store: &dyn crate::runtime::PersistentMetadataStore = &*records;
    let host_pending = super::super::host_pending_members_before_build(
        None,
        Some((store, "route-refused")),
        &[kickoff_spec(&alice)],
    )
    .await;
    assert!(
        host_pending.is_empty(),
        "a restart hands the mob no rotation to carry out: {host_pending:?}"
    );

    let outcomes = runtime.seat_identities(&[kickoff_spec(&alice)], None).await;
    assert!(
        matches!(
            outcomes[&alice],
            Err(IdentityRuntimeError::HostRejectedBuild { .. })
        ),
        "the compile's seat is the same refused spawn: {:?}",
        outcomes[&alice].as_ref().err()
    );
    assert_eq!(
        core.rotation_calls.load(Ordering::SeqCst),
        1,
        "the reset asked one rotation, and the compile none"
    );
    assert!(lock(&core.rotations).is_empty(), "nothing was rotated");
    assert!(core.session_of(&alice).is_none(), "nothing was seated");
}

/// A rotation a crash left recorded is driven by the next compile, the seat
/// step's own rotation. For an identity Core does not hold that owes a
/// kickoff, that rotation is Core's live spawn of the fresh life. Refused
/// for its member route, the step answers `HostRejectedBuild`, final, never
/// the retryable wake refusal: each compile asks the rotation once and is
/// answered the same way, and the record stays for a compile whose build
/// passes.
#[tokio::test]
async fn a_rotation_redrive_refused_for_the_member_route_is_final_and_typed() {
    let alice = identity("personal:alice");
    let core = Arc::new(SeatCore {
        route_refused: vec![alice.clone()],
        ..SeatCore::default()
    });
    let runtime = runtime_over(None, &core);
    register_all(&runtime, &[kickoff_spec(&alice)]).await;
    runtime
        .record_rotation_intent(&alice, "reset", None)
        .await
        .expect("record the interrupted rotation");

    for compile in 1..=2_usize {
        let outcomes = runtime.seat_identities(&[kickoff_spec(&alice)], None).await;
        match &outcomes[&alice] {
            Err(refused @ IdentityRuntimeError::HostRejectedBuild { .. }) => {
                assert!(
                    !refused.clears_on_its_own(),
                    "compile {compile}: {refused:?}"
                );
            }
            other => panic!(
                "compile {compile} answers the final refusal: {:?}",
                other.as_ref().err()
            ),
        }
        assert_eq!(
            core.rotation_calls.load(Ordering::SeqCst),
            compile,
            "compile {compile} drives the recorded rotation once"
        );
    }
    assert!(
        lock(&core.seat_calls).is_empty(),
        "the re-drive is a rotation, not a seat"
    );
    assert!(lock(&core.rotations).is_empty(), "nothing was rotated");
    assert!(
        runtime
            .pending_rotation_intents()
            .await
            .expect("the records read")
            .contains_key(&alice),
        "the record stays for a compile whose build passes"
    );
}

/// An archived session's revival is a build Core makes on the seat step
/// (the resume door). Refused for its member route, it is final and typed:
/// the step answers `HostRejectedBuild`, which does not clear on its own,
/// never the retryable resume rejection, and nothing is seated or released.
#[tokio::test]
async fn an_archived_revival_refused_for_the_member_route_is_final_and_typed() {
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
        archived: vec![session],
        resumes_onto_session: true,
        route_refused: vec![alice.clone()],
        ..SeatCore::default()
    });
    let runtime = runtime_over(Some(state_dir.path()), &core);
    register_all(&runtime, &[spec(&alice)]).await;

    let outcomes = runtime.seat_identities(&[spec(&alice)], None).await;

    let refused = outcomes[&alice]
        .as_ref()
        .err()
        .expect("the revival is refused");
    assert!(
        matches!(refused, IdentityRuntimeError::HostRejectedBuild { .. }),
        "{refused:?}"
    );
    assert!(!refused.clears_on_its_own(), "{refused:?}");
    assert_eq!(core.resumes.load(Ordering::SeqCst), 1, "one revival asked");
    assert!(core.session_of(&alice).is_none(), "nothing was seated");
    assert!(lock(&core.releases).is_empty(), "nothing was released");
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
    let mode = super::super::IdentityBootstrapMode::EagerMaterialize;
    let generation = runtime.begin_identity_bootstrap_pending(mode.clone());
    core.history_read_fails.store(true, Ordering::SeqCst);
    let result = crate::identity_first::orchestrator::RestoreFlowResult {
        outcomes: BTreeMap::new(),
        managed_edges: Vec::new(),
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

/// Shutdown joins a delivery whose seat step is inside Core's seat call:
/// the join waits until Core answers.
#[tokio::test]
async fn shutdown_waits_for_a_seat_step_inside_cores_seat_call() {
    let alice = identity("personal:alice");
    let (reached_tx, reached_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let core = Arc::new(SeatCore {
        admits_deliveries: true,
        seat_gate: StdMutex::new(Some((reached_tx, release_rx))),
        ..SeatCore::default()
    });
    let runtime = runtime_over(None, &core);
    register_all(&runtime, &[spec(&alice)]).await;

    let sender = Arc::clone(&runtime);
    let target = alice.clone();
    let send = tokio::spawn(async move {
        sender
            .send_admission_tracked(&target, None, &text("hello"), HandlingMode::Queue, None)
            .await
    });
    tokio::time::timeout(Duration::from_secs(10), reached_rx)
        .await
        .expect("the seat step reaches Core's seat call")
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
