//! A cold resume restores its members up to its restore concurrency at a
//! time, and the mob it leaves is the one a restore of one member at a time
//! leaves.
//!
//! Most fixtures spawn their members on a persistent session service, shut
//! the mob down and resume it on the same service and stores. The agent
//! builder holds the build of each restored member (its provision) until the
//! test releases it, so the store round trips of a restore are counted in
//! release rounds: a resume of N members at concurrency C takes ceil(N / C)
//! rounds. The restore pass probe (`builder::restore_pass_probe`) records
//! when each member's restore and turn start and finish, and holds or fails
//! a member's turn. The rows on a service that keeps no durable sessions,
//! and the cold successor row, run on the runtime tests' mock session
//! service.

use super::*;
use crate::runtime::builder::restore_pass_probe::{self, RestorePassEvent};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Bound for one step; it only turns a hang into a failure.
const STEP: Duration = Duration::from_secs(60);

const MEMBERS: usize = 8;

/// The member whose restore fails in the failure rows.
const FAILING: usize = 3;

/// Holds the agent build of each armed session (a restore's provision) until
/// the test releases that session.
struct RestoreGate {
    /// Sessions whose next build waits at the gate.
    armed: std::sync::Mutex<HashSet<SessionId>>,
    /// Each armed session's release.
    releases: std::sync::Mutex<HashMap<SessionId, Arc<tokio::sync::Semaphore>>>,
    /// The sessions whose build arrived since the last arming, in order.
    arrived: std::sync::Mutex<Vec<SessionId>>,
    arrival: tokio::sync::Notify,
    /// Builds waiting at the gate now.
    held: AtomicUsize,
    /// The most builds that waited at the gate at once since the last arming.
    max_held: AtomicUsize,
}

impl RestoreGate {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            armed: std::sync::Mutex::new(HashSet::new()),
            releases: std::sync::Mutex::new(HashMap::new()),
            arrived: std::sync::Mutex::new(Vec::new()),
            arrival: tokio::sync::Notify::new(),
            held: AtomicUsize::new(0),
            max_held: AtomicUsize::new(0),
        })
    }

    /// Hold the next build of each of `sessions`.
    fn arm(&self, sessions: impl IntoIterator<Item = SessionId>) {
        assert_eq!(
            self.held.load(Ordering::SeqCst),
            0,
            "no build waits at the gate"
        );
        self.arrived.lock().expect("arrived builds").clear();
        self.max_held.store(0, Ordering::SeqCst);
        let mut armed = self.armed.lock().expect("armed sessions");
        let mut releases = self.releases.lock().expect("build releases");
        for session in sessions {
            armed.insert(session.clone());
            releases.insert(session, Arc::new(tokio::sync::Semaphore::new(0)));
        }
    }

    async fn hold_if_armed(&self, session_id: &SessionId) {
        if !self
            .armed
            .lock()
            .expect("armed sessions")
            .remove(session_id)
        {
            return;
        }
        let release = self
            .releases
            .lock()
            .expect("build releases")
            .get(session_id)
            .cloned();
        let held = self.held.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_held.fetch_max(held, Ordering::SeqCst);
        self.arrived
            .lock()
            .expect("arrived builds")
            .push(session_id.clone());
        self.arrival.notify_one();
        if let Some(release) = release {
            let _released = release.acquire().await;
        }
        self.held.fetch_sub(1, Ordering::SeqCst);
    }

    /// Let `session`'s held build go on.
    fn release(&self, session: &SessionId) {
        if let Some(release) = self.releases.lock().expect("build releases").get(session) {
            release.add_permits(1);
        }
    }

    fn arrived(&self) -> Vec<SessionId> {
        self.arrived.lock().expect("arrived builds").clone()
    }

    /// Wait until `count` builds arrived since the last arming.
    async fn wait_for_arrivals(&self, count: usize) {
        let waited = tokio::time::timeout(STEP, async {
            loop {
                let arrival = self.arrival.notified();
                if self.arrived().len() >= count {
                    return;
                }
                arrival.await;
            }
        })
        .await;
        assert!(
            waited.is_ok(),
            "{count} builds arrive at the gate ({} arrived)",
            self.arrived().len()
        );
    }

    /// Release the held builds in rounds of `concurrency`, each round once
    /// all of its builds wait at the gate. Returns the number of rounds.
    async fn release_in_rounds(&self, members: usize, concurrency: usize) -> usize {
        let mut released = 0;
        let mut rounds = 0;
        while released < members {
            let round = concurrency.min(members - released);
            self.wait_for_arrivals(released + round).await;
            let arrived = self.arrived();
            assert_eq!(
                arrived.len(),
                released + round,
                "round {rounds}: no more member restores run at once than the bound"
            );
            for session in &arrived[released..] {
                self.release(session);
            }
            released += round;
            rounds += 1;
        }
        rounds
    }
}

/// [`PersistentMockBuilder`], holding each armed session's build at the
/// [`RestoreGate`].
struct HeldRestoreBuilder {
    gate: Arc<RestoreGate>,
}

#[async_trait]
impl SessionAgentBuilder for HeldRestoreBuilder {
    type Agent = PersistentMockAgent;

    async fn build_agent(
        &self,
        req: &CreateSessionRequest,
        event_tx: tokio::sync::mpsc::Sender<AgentEvent>,
    ) -> Result<Self::Agent, SessionError> {
        self.gate
            .hold_if_armed(&requested_test_session_id(req))
            .await;
        PersistentMockBuilder.build_agent(req, event_tx).await
    }
}

/// The host's customizer: it records which members' restores asked it, and
/// fails one member's restore request, alone (`refused`) or with the whole
/// resume (`renamed`, which a restore refuses as a changed identity).
struct RestoreCustomizer {
    refused: Option<AgentIdentity>,
    renamed: Option<AgentIdentity>,
    asked: std::sync::Mutex<Vec<AgentIdentity>>,
}

impl RestoreCustomizer {
    fn new(refused: Option<AgentIdentity>, renamed: Option<AgentIdentity>) -> Arc<Self> {
        Arc::new(Self {
            refused,
            renamed,
            asked: std::sync::Mutex::new(Vec::new()),
        })
    }

    fn asked(&self) -> Vec<AgentIdentity> {
        self.asked.lock().expect("asked members").clone()
    }
}

impl SpawnMemberCustomizer for RestoreCustomizer {
    fn customize_spawn(
        &self,
        ctx: &SpawnCustomizationContext,
        spec: &mut SpawnMemberSpec,
    ) -> Result<(), MobError> {
        if ctx.spawn_source != SpawnSource::Resume {
            return Ok(());
        }
        self.asked
            .lock()
            .expect("asked members")
            .push(spec.identity.clone());
        if self.refused.as_ref() == Some(&spec.identity) {
            return Err(MobError::Internal(format!(
                "the host does not restore {} yet",
                spec.identity
            )));
        }
        if self.renamed.as_ref() == Some(&spec.identity) {
            spec.identity = AgentIdentity::from("impostor");
        }
        Ok(())
    }
}

fn worker(index: usize) -> AgentIdentity {
    AgentIdentity::from(format!("worker-{index:02}"))
}

/// A mob whose members were spawned on a persistent session service and
/// shut down, ready to resume on the same service and stores.
///
/// A row resumes a `SpawnedMob` once, as a restart does (a failed resume may
/// be followed by the one that brings the mob back). Rows that compare two
/// resumes spawn two of them: a second resume of one mob after its first
/// resume shut down runs over what that incarnation left in this process (a
/// handle that is shut down but still held, and the service's one runtime
/// adapter); a restart sees neither.
struct SpawnedMob {
    mob_id: MobId,
    service: Arc<meerkat_session::PersistentSessionService<HeldRestoreBuilder>>,
    storage: MobStorage,
    gate: Arc<RestoreGate>,
    /// Each member and the session it was spawned on, in roster order.
    sessions: Vec<(AgentIdentity, SessionId)>,
}

impl SpawnedMob {
    async fn spawn(members: usize) -> Self {
        let gate = RestoreGate::new();
        let session_store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
        let blob_store: Arc<dyn meerkat_core::BlobStore> =
            Arc::new(meerkat_store::MemoryBlobStore::new());
        let service = Arc::new(meerkat_session::PersistentSessionService::new(
            HeldRestoreBuilder { gate: gate.clone() },
            64,
            session_store,
            runtime_store,
            blob_store,
        ));
        let mut definition = with_unique_mob_id(sample_definition(), "parallel-restore");
        definition.orchestrator = None;
        let mob_id = definition.id.clone();
        let storage = MobStorage::in_memory();
        let handle = MobBuilder::new(definition, storage.clone())
            .with_session_service(service.clone())
            .create()
            .await
            .expect("create the mob");
        let mut sessions = Vec::with_capacity(members);
        for index in 0..members {
            let identity = worker(index);
            let mut spec = SpawnMemberSpec::new("worker", identity.as_str());
            spec.runtime_mode = Some(crate::MobRuntimeMode::TurnDriven);
            handle.spawn_spec(spec).await.expect("spawn a worker");
            let session = handle
                .resolve_bridge_session_id(&identity)
                .await
                .expect("session-backed worker");
            sessions.push((identity, session));
        }
        handle
            .shutdown()
            .await
            .expect("shut the first runtime down");
        Self {
            mob_id,
            service,
            storage,
            gate,
            sessions,
        }
    }

    /// A restart finds no live session: every member is restored through
    /// its provision.
    async fn drop_live_sessions(&self) {
        for (identity, session) in &self.sessions {
            if self.service.live_session_actor_registered(session).await {
                MobSessionService::discard_live_session(self.service.as_ref(), session)
                    .await
                    .unwrap_or_else(|error| {
                        panic!("{identity}: discard the live session the shutdown kept: {error}")
                    });
            }
        }
    }

    fn resume_builder(&self, concurrency: usize) -> MobBuilder {
        MobBuilder::for_resume(self.storage.clone())
            .with_session_service(self.service.clone())
            .notify_orchestrator_on_resume(false)
            .with_resume_restore_concurrency(concurrency)
            // Every member is restored at start: the parallel restore stream is
            // what these rows measure.
            .with_member_restore_policy(crate::runtime::MemberRestorePolicy::Eager)
    }

    /// Start a resume at `concurrency` in a task of its own.
    async fn spawn_resume(
        &self,
        concurrency: usize,
    ) -> tokio::task::JoinHandle<Result<MobHandle, MobError>> {
        self.drop_live_sessions().await;
        tokio::spawn(self.resume_builder(concurrency).resume())
    }

    /// Resume at `concurrency`, holding every member's provision at the
    /// gate and releasing them in rounds.
    async fn resume_held(&self, concurrency: usize) -> HeldResume {
        self.gate
            .arm(self.sessions.iter().map(|(_, session)| session.clone()));
        let journal = self.journal().await;
        let resume = self.spawn_resume(concurrency).await;
        let rounds = self
            .gate
            .release_in_rounds(self.sessions.len(), concurrency)
            .await;
        let handle = joined(resume).await.expect("the resume succeeds");
        let appended = self.journal().await.split_off(journal.len());
        HeldResume {
            rounds,
            max_held: self.gate.max_held.load(Ordering::SeqCst),
            mob: self.restored(&handle, appended).await,
            handle,
        }
    }

    /// Resume at `concurrency`, with `customizer` when given, nothing held.
    /// Returns the resumed mob with what it left, or the resume's error.
    async fn resume(
        &self,
        concurrency: usize,
        customizer: Option<Arc<RestoreCustomizer>>,
    ) -> Result<(MobHandle, RestoredMob), MobError> {
        self.drop_live_sessions().await;
        let journal = self.journal().await;
        let mut builder = self.resume_builder(concurrency);
        if let Some(customizer) = customizer {
            builder = builder.with_spawn_member_customizer(customizer);
        }
        let handle = tokio::time::timeout(STEP, Box::pin(builder.resume()))
            .await
            .expect("the resume answers")?;
        let appended = self.journal().await.split_off(journal.len());
        let restored = self.restored(&handle, appended).await;
        Ok((handle, restored))
    }

    /// The kind of every event in the mob's journal, in order.
    async fn journal(&self) -> Vec<String> {
        self.storage
            .events
            .replay_all()
            .await
            .expect("read the mob journal")
            .iter()
            .map(|event| format!("{:?}", std::mem::discriminant(&event.kind)))
            .collect()
    }

    /// What `handle`'s resume left, each member's session named by whether
    /// it is the session the member was spawned on. `appended` are the kinds
    /// of the events the resume appended to the journal.
    async fn restored(&self, handle: &MobHandle, appended: Vec<String>) -> RestoredMob {
        let listed = handle.list_members().await;
        let state = handle.machine_state_watch_rx.borrow().clone();
        let mut members = Vec::with_capacity(self.sessions.len());
        for (identity, spawned) in &self.sessions {
            let status = listed
                .iter()
                .find(|entry| &entry.agent_identity == identity)
                .map(|entry| format!("{:?}", entry.status));
            let dsl_identity = crate::machines::mob_machine::AgentIdentity::from_domain(identity);
            members.push(RestoredMember {
                identity: identity.clone(),
                status,
                on_its_own_session: handle.resolve_bridge_session_id(identity).await.as_ref()
                    == Some(spawned),
                machine: format!(
                    "{:?}",
                    (
                        state
                            .member_session_bindings
                            .get(&dsl_identity)
                            .map(|session| session.0 == spawned.to_string()),
                        state.member_profile_names.get(&dsl_identity),
                        state.member_runtime_modes.get(&dsl_identity),
                        crate::runtime::boot_plan::member_materialization_phase(
                            &state,
                            &dsl_identity,
                        ),
                        state.member_restore_failures.get(&dsl_identity),
                        state
                            .spawn_profile_authority_profile_names
                            .get(&dsl_identity),
                        state
                            .spawn_profile_authority_material_digests
                            .get(&dsl_identity),
                    )
                ),
            });
        }
        let restore_failures = handle
            .restore_diagnostics
            .read()
            .await
            .iter()
            .map(|(identity, diagnostic)| (identity.clone(), diagnostic.reason.clone()))
            .collect();
        RestoredMob {
            members,
            restore_failures,
            appended,
        }
    }
}

/// The resume `task` answers, within the step bound, without a panic.
async fn joined(
    task: tokio::task::JoinHandle<Result<MobHandle, MobError>>,
) -> Result<MobHandle, MobError> {
    tokio::time::timeout(STEP, task)
        .await
        .expect("the resume answers")
        .expect("the resume task does not panic")
}

struct HeldResume {
    rounds: usize,
    max_held: usize,
    mob: RestoredMob,
    handle: MobHandle,
}

#[derive(Debug, PartialEq)]
struct RestoredMob {
    members: Vec<RestoredMember>,
    restore_failures: Vec<(AgentIdentity, String)>,
    /// The kinds of the events the resume appended to the journal, in order.
    appended: Vec<String>,
}

#[derive(Debug, PartialEq)]
struct RestoredMember {
    identity: AgentIdentity,
    status: Option<String>,
    on_its_own_session: bool,
    /// The member's MobMachine entries the restore writes.
    machine: String,
}

fn every_member_back(mob: &RestoredMob) {
    assert!(
        mob.restore_failures.is_empty(),
        "{:?}",
        mob.restore_failures
    );
    for member in &mob.members {
        assert!(member.on_its_own_session, "{member:?}");
        assert_eq!(member.status.as_deref(), Some("Active"), "{member:?}");
    }
}

/// A resume of 8 members at concurrency 1 takes 8 provision rounds, one
/// member at a time; at concurrency 4 it takes 2, with 4 members' provisions
/// waiting at once. The two resumes run on two mobs seeded the same way, each
/// the first resume of its mob in this process ([`SpawnedMob`]), and leave
/// the same mob: every member back on its own session, the same MobMachine
/// entries, no restore failure, and the same events appended.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_resume_restores_members_up_to_its_concurrency_and_leaves_the_serial_mob() {
    let serial_mob = SpawnedMob::spawn(MEMBERS).await;
    let serial = serial_mob.resume_held(1).await;
    assert_eq!(serial.rounds, MEMBERS, "one member at a time");
    assert_eq!(serial.max_held, 1);
    every_member_back(&serial.mob);
    serial
        .handle
        .shutdown()
        .await
        .expect("shut the serial resume down");

    let concurrent_mob = SpawnedMob::spawn(MEMBERS).await;
    let concurrent = concurrent_mob.resume_held(4).await;
    assert_eq!(
        concurrent.rounds,
        MEMBERS.div_ceil(4),
        "four members at a time"
    );
    assert_eq!(concurrent.max_held, 4);
    assert_eq!(
        concurrent.mob, serial.mob,
        "the concurrent resume leaves the mob the serial resume left"
    );
    concurrent
        .handle
        .shutdown()
        .await
        .expect("shut the concurrent resume down");
}

// Unrun until the candidate's validation.
//
// A restore concurrency above the semaphore maximum (`usize::MAX` for "no
// limit") counts as that maximum: the resume restores every member as at
// any other bound.
//
// RED on the base (e294b75f29, with the row applied): the resume panicked
// where it sized its permits (`Semaphore::new` refuses more than
// `Semaphore::MAX_PERMITS`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_resume_at_a_restore_concurrency_above_the_maximum_restores_every_member() {
    let mob = SpawnedMob::spawn(MEMBERS).await;
    let (handle, restored) = mob
        .resume(usize::MAX, None)
        .await
        .expect("the resume succeeds");
    every_member_back(&restored);
    handle.shutdown().await.expect("shut the resume down");
}

/// At concurrency 1 a member's restore starts only once the turn of the
/// member before it returned: while worker-00's turn is held, worker-01's
/// restore has asked for its permit and has not started. The whole pass is
/// one member after another: restore, turn, then the next member.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn at_concurrency_one_each_member_is_restored_and_applied_before_the_next_restore_starts() {
    let mob = SpawnedMob::spawn(3).await;
    let probe = restore_pass_probe::install(&mob.mob_id);
    probe.hold_turn(&worker(0));
    let resume = mob.spawn_resume(1).await;

    let waited = tokio::time::timeout(
        STEP,
        probe.wait_for(|events| {
            events.contains(&RestorePassEvent::TurnStarted(worker(0)))
                && events.contains(&RestorePassEvent::PermitRequested(worker(1)))
        }),
    )
    .await;
    assert!(
        waited.is_ok(),
        "worker-00's turn starts and worker-01's restore asks for its permit: {:?}",
        probe.events()
    );
    assert!(
        !probe
            .events()
            .contains(&RestorePassEvent::RestoreStarted(worker(1))),
        "worker-01's restore waits for worker-00's turn: {:?}",
        probe.events()
    );
    probe.release_turn(&worker(0));
    let handle = joined(resume).await.expect("the resume succeeds");

    let pass = probe
        .events()
        .into_iter()
        .filter(|event| !matches!(event, RestorePassEvent::PermitRequested(_)))
        .collect::<Vec<_>>();
    let one_after_another = (0..3)
        .flat_map(|index| {
            [
                RestorePassEvent::RestoreStarted(worker(index)),
                RestorePassEvent::RestoreFinished(worker(index)),
                RestorePassEvent::TurnStarted(worker(index)),
                RestorePassEvent::TurnFinished(worker(index)),
            ]
        })
        .collect::<Vec<_>>();
    assert_eq!(pass, one_after_another);
    handle.shutdown().await.expect("shut the resume down");
}

/// The members' turns apply in roster order however their restores finish:
/// with the four provisions released last member first, the restores finish
/// worker-03 first, and the turns still run worker-00 first. A pass that
/// applied turns in completion order fails this row.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn turns_apply_in_roster_order_however_the_restores_finish() {
    let mob = SpawnedMob::spawn(4).await;
    let probe = restore_pass_probe::install(&mob.mob_id);
    mob.gate
        .arm(mob.sessions.iter().map(|(_, session)| session.clone()));
    let resume = mob.spawn_resume(4).await;
    mob.gate.wait_for_arrivals(4).await;
    for (identity, session) in mob.sessions.iter().rev() {
        mob.gate.release(session);
        let finished = tokio::time::timeout(
            STEP,
            probe.wait_for(|events| {
                events.contains(&RestorePassEvent::RestoreFinished(identity.clone()))
            }),
        )
        .await;
        assert!(finished.is_ok(), "{identity}'s restore finishes");
    }
    let handle = joined(resume).await.expect("the resume succeeds");

    let events = probe.events();
    let finished = events
        .iter()
        .filter_map(|event| match event {
            RestorePassEvent::RestoreFinished(identity) => Some(identity.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(finished, (0..4).rev().map(worker).collect::<Vec<_>>());
    let turns = events
        .iter()
        .filter_map(|event| match event {
            RestorePassEvent::TurnStarted(identity) => Some(identity.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(turns, (0..4).map(worker).collect::<Vec<_>>());
    handle.shutdown().await.expect("shut the resume down");
}

/// The member whose turn fails the resume in the failed-turn rows.
const FAILING_TURN: usize = 2;

/// The members after [`FAILING_TURN`], of `members`, that hold a permit while
/// its turn runs at `concurrency`: the restores a failure of that turn leaves
/// restored and not applied.
fn restored_alongside_the_failing_turn(members: usize, concurrency: usize) -> Vec<AgentIdentity> {
    (FAILING_TURN + 1..members.min(FAILING_TURN + concurrency))
        .map(worker)
        .collect()
}

/// Make [`FAILING_TURN`]'s next turn wait at its start, then fail the resume.
fn arm_the_failing_turn(probe: &restore_pass_probe::RestorePassProbe) {
    probe.hold_turn(&worker(FAILING_TURN));
    probe.fail_turn(&worker(FAILING_TURN));
}

/// Let the armed turn fail the resume once it has started and every member
/// in `restored` has finished its restore, so that the failure leaves exactly
/// those members restored and not applied.
async fn fail_the_turn_once_restored(
    probe: &restore_pass_probe::RestorePassProbe,
    restored: &[AgentIdentity],
) {
    let waited = tokio::time::timeout(
        STEP,
        probe.wait_for(|events| {
            events.contains(&RestorePassEvent::TurnStarted(worker(FAILING_TURN)))
                && restored.iter().all(|identity| {
                    events.contains(&RestorePassEvent::RestoreFinished(identity.clone()))
                })
        }),
    )
    .await;
    assert!(
        waited.is_ok(),
        "worker-02's turn starts and {restored:?} finish their restores: {:?}",
        probe.events()
    );
    probe.release_turn(&worker(FAILING_TURN));
}

/// The members after [`FAILING_TURN`], of `members`, whose restore started,
/// in roster order.
fn restores_started_after_the_failing_turn(
    events: &[RestorePassEvent],
    members: usize,
) -> Vec<AgentIdentity> {
    let mut started = events
        .iter()
        .filter_map(|event| match event {
            RestorePassEvent::RestoreStarted(identity)
                if (FAILING_TURN + 1..members).any(|index| &worker(index) == identity) =>
            {
                Some(identity.clone())
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    started.sort();
    started
}

/// What a resume whose turn of worker-02 failed left.
struct FailedTurnResume {
    error: String,
    /// Members after worker-02 whose restore started.
    started_after: Vec<AgentIdentity>,
    /// Members after worker-02 with a live session when the resume returned
    /// its error.
    left_live: Vec<AgentIdentity>,
}

/// Resume `members` members at `concurrency` with worker-02's turn failing
/// the resume once every member holding a permit alongside it is restored,
/// then resume again without the failure, which brings every member back.
async fn resume_failing_a_turn(members: usize, concurrency: usize) -> FailedTurnResume {
    let mob = SpawnedMob::spawn(members).await;
    let probe = restore_pass_probe::install(&mob.mob_id);
    arm_the_failing_turn(&probe);
    let resume = mob.spawn_resume(concurrency).await;
    fail_the_turn_once_restored(
        &probe,
        &restored_alongside_the_failing_turn(members, concurrency),
    )
    .await;
    let Err(error) = joined(resume).await else {
        panic!("the failed turn fails the resume");
    };
    let started_after = restores_started_after_the_failing_turn(&probe.events(), members);
    // The resume returns its error only after its releases, so a released
    // member's live session is already gone: nothing to wait for.
    let mut left_live = Vec::new();
    for (identity, session) in &mob.sessions[FAILING_TURN + 1..] {
        if mob.service.live_session_actor_registered(session).await {
            left_live.push(identity.clone());
        }
    }

    // The next resume, in the same process, finds the failed resume's live
    // sessions as it left them and brings every member back.
    probe.stop_failing_turns();
    let handle = tokio::time::timeout(STEP, Box::pin(mob.resume_builder(concurrency).resume()))
        .await
        .expect("the resume answers")
        .expect("a resume after the failed one succeeds");
    every_member_back(&mob.restored(&handle, Vec::new()).await);
    handle.shutdown().await.expect("shut the resume down");
    FailedTurnResume {
        error: error.to_string(),
        started_after,
        left_live,
    }
}

/// A turn that fails the resume (worker-02's, failed by the probe once the
/// members holding a permit alongside it are restored) fails it with the
/// same error at concurrency 1 and 4. At 1 no member after it started; at 4
/// exactly the three holding a permit did. When the resume returns its error
/// none of them has a live session: the resume released every provision it
/// did not apply before it returned. The next resume brings every member
/// back.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_turn_that_fails_the_resume_fails_it_as_at_concurrency_one_and_leaves_nothing_unapplied()
{
    const CONCURRENCY: usize = 4;
    let serial = resume_failing_a_turn(MEMBERS, 1).await;
    let concurrent = resume_failing_a_turn(MEMBERS, CONCURRENCY).await;
    assert!(
        serial
            .error
            .contains("injected restore turn failure for 'worker-02'"),
        "{}",
        serial.error
    );
    assert_eq!(concurrent.error, serial.error, "the same error");
    assert_eq!(serial.started_after, Vec::<AgentIdentity>::new());
    assert_eq!(
        concurrent.started_after,
        restored_alongside_the_failing_turn(MEMBERS, CONCURRENCY),
        "the members holding a permit alongside worker-02 started, and no other"
    );
    assert_eq!(serial.left_live, Vec::<AgentIdentity>::new());
    assert_eq!(
        concurrent.left_live,
        Vec::<AgentIdentity>::new(),
        "every member restored but not applied was released"
    );
}

/// A member whose restore fails alone (its host customizer refuses it) is
/// restored by neither resume, with the same diagnostic, while every other
/// member comes back: the concurrent resume leaves the mob the serial one
/// does. The two resumes run on two mobs seeded the same way, each the first
/// resume of its mob in this process.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_member_restore_that_fails_alone_fails_alone_as_in_a_serial_resume() {
    let (serial, serial_mob) = SpawnedMob::spawn(MEMBERS)
        .await
        .resume(1, Some(RestoreCustomizer::new(Some(worker(FAILING)), None)))
        .await
        .expect("the serial resume succeeds");
    assert_eq!(
        serial_mob
            .restore_failures
            .iter()
            .map(|(identity, _)| identity.clone())
            .collect::<Vec<_>>(),
        vec![worker(FAILING)],
        "{serial_mob:?}"
    );
    assert!(
        serial_mob.restore_failures[0]
            .1
            .contains("the host does not restore worker-03 yet"),
        "{serial_mob:?}"
    );
    for member in &serial_mob.members {
        if member.identity != worker(FAILING) {
            assert!(member.on_its_own_session, "{member:?}");
            assert_eq!(member.status.as_deref(), Some("Active"), "{member:?}");
        }
    }
    serial
        .shutdown()
        .await
        .expect("shut the serial resume down");

    let (concurrent, concurrent_mob) = SpawnedMob::spawn(MEMBERS)
        .await
        .resume(4, Some(RestoreCustomizer::new(Some(worker(FAILING)), None)))
        .await
        .expect("the concurrent resume succeeds");
    assert_eq!(
        concurrent_mob, serial_mob,
        "the concurrent resume leaves the mob the serial resume left"
    );
    concurrent
        .shutdown()
        .await
        .expect("shut the concurrent resume down");
}

/// A member restore that fails the resume (its customizer changes its
/// identity) fails both resumes with the same error. The serial resume asks
/// the customizer for no member after the failing one. The concurrent resume
/// starts no member restore once that one has failed: it asked for every
/// member up to the failing one, and for none beyond the members that could
/// already be running alongside it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_member_restore_that_fails_the_resume_fails_it_as_a_serial_resume_does() {
    const CONCURRENCY: usize = 4;

    let serial_mob = SpawnedMob::spawn(MEMBERS).await;
    let serial_customizer = RestoreCustomizer::new(None, Some(worker(FAILING)));
    let Err(serial_error) = serial_mob.resume(1, Some(serial_customizer.clone())).await else {
        panic!("the serial resume fails");
    };
    assert_eq!(
        serial_customizer.asked(),
        (0..=FAILING).map(worker).collect::<Vec<_>>(),
        "the serial resume restores the members in roster order and stops at the failure"
    );

    let concurrent_mob = SpawnedMob::spawn(MEMBERS).await;
    let concurrent_customizer = RestoreCustomizer::new(None, Some(worker(FAILING)));
    let Err(concurrent_error) = concurrent_mob
        .resume(CONCURRENCY, Some(concurrent_customizer.clone()))
        .await
    else {
        panic!("the concurrent resume fails");
    };
    assert_eq!(
        concurrent_error.to_string(),
        serial_error.to_string(),
        "the same error"
    );
    assert!(
        serial_error
            .to_string()
            .contains("cannot change resume restore identity from 'worker-03'"),
        "{serial_error}"
    );
    let asked = concurrent_customizer.asked();
    for index in 0..=FAILING {
        assert!(
            asked.contains(&worker(index)),
            "{} is restored before the failure: {asked:?}",
            worker(index)
        );
    }
    for identity in &asked {
        assert!(
            (0..FAILING + CONCURRENCY).any(|index| &worker(index) == identity),
            "{identity} started no later than the bound allows: {asked:?}"
        );
    }
}

/// A fork child of an in-mob source is restored in its own turn, after its
/// source, with the overlay its source was restored with, at concurrency 1
/// and at the default 16 alike.
#[tokio::test]
async fn a_fork_child_restored_after_a_crash_is_built_as_its_source_at_any_restore_concurrency() {
    for concurrency in [1, 16] {
        let (service, storage, source_identity, child_identity, first, expected_source) =
            seat_fork_child_then_crash(false).await;
        let resumed = MobBuilder::for_resume(storage)
            .with_session_service(service.clone())
            .with_spawn_member_customizer(Arc::new(IdentityOverlayCustomizer {
                source: source_identity.clone(),
            }))
            .with_resume_restore_concurrency(concurrency)
            // Every member is restored at start: the parallel restore stream is
            // what these rows measure.
            .with_member_restore_policy(crate::runtime::MemberRestorePolicy::Eager)
            .resume()
            .await
            .unwrap_or_else(|error| panic!("resume at concurrency {concurrency}: {error}"));
        assert_restored_child_built_as_its_source(
            &service,
            &resumed,
            &source_identity,
            &child_identity,
            &first,
            &expected_source,
        )
        .await;
        crash_stop_and_release_routes(resumed).await;
    }
}

/// A mob spawned on a service that keeps no durable sessions and shut down,
/// with its members' live sessions discarded: a resume recreates each member
/// on a fresh session.
struct RecreatableMob {
    mob_id: MobId,
    service: Arc<MockSessionService>,
    storage: MobStorage,
    /// Each member and the session it was spawned on, in roster order.
    sessions: Vec<(AgentIdentity, SessionId)>,
}

impl RecreatableMob {
    async fn spawn(members: usize) -> Self {
        let service = Arc::new(MockSessionService::new());
        service.keep_no_durable_sessions();
        let _ = service.enable_runtime_adapter();
        let mut definition = with_unique_mob_id(sample_definition(), "parallel-recreate");
        definition.orchestrator = None;
        let mob_id = definition.id.clone();
        let storage = MobStorage::in_memory();
        let handle = MobBuilder::new(definition, storage.clone())
            .with_session_service(service.clone())
            .allow_ephemeral_sessions(true)
            .create()
            .await
            .expect("create a mob without durable sessions");
        let mut sessions = Vec::with_capacity(members);
        for index in 0..members {
            let identity = worker(index);
            let mut spec = SpawnMemberSpec::new("worker", identity.as_str());
            spec.runtime_mode = Some(crate::MobRuntimeMode::TurnDriven);
            handle.spawn_spec(spec).await.expect("spawn a worker");
            let session = handle
                .resolve_bridge_session_id(&identity)
                .await
                .expect("session-backed worker");
            sessions.push((identity, session));
        }
        handle
            .shutdown()
            .await
            .expect("shut the first runtime down");
        let mob = Self {
            mob_id,
            service,
            storage,
            sessions,
        };
        for (identity, session) in &mob.sessions {
            if mob.live(session).await {
                MobSessionService::discard_live_session(mob.service.as_ref(), session)
                    .await
                    .unwrap_or_else(|error| {
                        panic!("{identity}: discard the live session: {error}")
                    });
            }
        }
        mob
    }

    fn resume_builder(&self, concurrency: usize) -> MobBuilder {
        MobBuilder::for_resume(self.storage.clone())
            .with_session_service(self.service.clone())
            .allow_ephemeral_sessions(true)
            .notify_orchestrator_on_resume(false)
            .with_resume_restore_concurrency(concurrency)
            // Every member is restored at start: the parallel restore stream is
            // what these rows measure.
            .with_member_restore_policy(crate::runtime::MemberRestorePolicy::Eager)
    }

    /// Resume at `concurrency`; the resume succeeds.
    async fn resume(&self, concurrency: usize) -> MobHandle {
        tokio::time::timeout(STEP, Box::pin(self.resume_builder(concurrency).resume()))
            .await
            .expect("the resume answers")
            .expect("the resume succeeds")
    }

    async fn live(&self, session: &SessionId) -> bool {
        MobSessionService::live_session_actor_registered(self.service.as_ref(), session)
            .await
            .expect("read the live session registry")
    }
}

/// What a resume on a service that keeps no durable sessions left: each
/// member recreated on a fresh session.
async fn recreated_members(concurrency: usize) -> Vec<(AgentIdentity, String, bool, bool)> {
    let mob = RecreatableMob::spawn(6).await;
    let resumed = mob.resume(concurrency).await;
    assert!(
        resumed.restore_diagnostics.read().await.is_empty(),
        "no member restore failed"
    );
    let state = resumed.machine_state_watch_rx.borrow().clone();
    let listed = resumed.list_members().await;
    let mut recreated = Vec::new();
    for (identity, spawned) in &mob.sessions {
        let status = listed
            .iter()
            .find(|entry| &entry.agent_identity == identity)
            .map(|entry| format!("{:?}", entry.status))
            .unwrap_or_default();
        let current = resumed.resolve_bridge_session_id(identity).await;
        let bound = state
            .member_session_bindings
            .get(&crate::machines::mob_machine::AgentIdentity::from_domain(
                identity,
            ))
            .map(|session| session.0.clone());
        recreated.push((
            identity.clone(),
            status,
            current.as_ref().map(ToString::to_string) == bound,
            current.as_ref() != Some(spawned),
        ));
    }
    resumed.shutdown().await.expect("shut the resume down");
    recreated
}

/// A service that keeps no durable sessions recreates every member on a
/// fresh session, bound in MobMachine, at concurrency 4 as at concurrency 1.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_service_without_durable_sessions_recreates_members_as_it_does_one_at_a_time() {
    let serial = recreated_members(1).await;
    for (identity, status, bound, fresh) in &serial {
        assert_eq!(status, "Active", "{identity}");
        assert!(*bound && *fresh, "{identity}: bound to its fresh session");
    }
    assert_eq!(recreated_members(4).await, serial);
}

/// On a service that keeps no durable sessions, a turn that fails the resume
/// (worker-02's, failed by the probe once the members holding a permit
/// alongside it are recreated) leaves none of the members after it with a
/// live session. At concurrency 4 the resume recreated each of the three
/// holding a permit on a fresh session and retired that session before it
/// returned its error; at 1 it recreated none of them.
///
/// No resume follows in this process. worker-02 keeps the fresh session the
/// failed resume provisioned for it (a restore of one member at a time
/// leaves it too); its runtime registration on this mock's one runtime
/// adapter names the failed resume's mob, whose signal queue is gone, so the
/// next resume's orphan sweep cannot retire it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_resume_retires_the_fresh_sessions_it_created_but_did_not_apply() {
    for concurrency in [1, 4] {
        let mob = RecreatableMob::spawn(6).await;
        let after = &mob.sessions[FAILING_TURN + 1..];
        let restored = restored_alongside_the_failing_turn(mob.sessions.len(), concurrency);
        let mut builds_before = Vec::with_capacity(after.len());
        for (identity, _) in after {
            builds_before.push(member_build_count(&mob.service, identity).await);
        }
        let probe = restore_pass_probe::install(&mob.mob_id);
        arm_the_failing_turn(&probe);
        let resume = tokio::spawn(mob.resume_builder(concurrency).resume());
        fail_the_turn_once_restored(&probe, &restored).await;
        let Err(error) = joined(resume).await else {
            panic!("the failed turn fails the resume at concurrency {concurrency}");
        };
        assert!(
            error
                .to_string()
                .contains("injected restore turn failure for 'worker-02'"),
            "{error}"
        );
        assert_eq!(
            restores_started_after_the_failing_turn(&probe.events(), mob.sessions.len()),
            restored,
            "at concurrency {concurrency}, the members holding a permit alongside worker-02 \
             started, and no other"
        );
        for ((identity, spawned), before) in after.iter().zip(builds_before) {
            let builds = member_build_count(&mob.service, identity).await;
            if restored.contains(identity) {
                assert_eq!(builds, before + 1, "{identity} was recreated once");
                let fresh = last_member_build(&mob.service, identity).await.session_id;
                assert!(
                    !mob.live(&fresh).await,
                    "{identity}'s fresh session was retired before the resume returned its error"
                );
            } else {
                assert_eq!(builds, before, "{identity} was not recreated");
            }
            assert!(!mob.live(spawned).await, "{identity}'s spawned session");
        }
    }
}

/// A mob of workers seated with [`seated_provider_params`] whose process
/// restarted after its first worker lost its session. A successor session
/// persisted under that worker's comms name holds
/// [`successor_provider_params`]; the restart left it listed and not live.
struct SuccessorMob {
    mob_id: MobId,
    service: Arc<MockSessionService>,
    storage: MobStorage,
    /// The worker whose session was lost, first in roster order.
    member: AgentIdentity,
    successor: SessionId,
    /// The other workers and their sessions, in roster order.
    peers: Vec<(AgentIdentity, SessionId)>,
}

impl SuccessorMob {
    async fn seat(peers: usize) -> Self {
        let service = Arc::new(MockSessionService::new());
        let _ = service.enable_runtime_adapter();
        let storage = MobStorage::in_memory();
        let restart_storage = MobStorage::with_events_and_runtime_metadata(
            storage.events.clone(),
            storage.runtime_metadata.clone(),
        );
        let mut definition = with_unique_mob_id(provider_params_definition(), "parallel-successor");
        definition.orchestrator = None;
        let mob_id = definition.id.clone();
        let handle = MobBuilder::new(definition, storage)
            .with_session_service(service.clone())
            .create()
            .await
            .expect("create the mob");
        // The roster is in identity order: the migrated worker comes first.
        let member = AgentIdentity::from("migrated-worker");
        let identities = std::iter::once(member.clone())
            .chain((0..peers).map(|index| AgentIdentity::from(format!("peer-{index:02}"))))
            .collect::<Vec<_>>();
        let mut seated = Vec::with_capacity(identities.len());
        for identity in identities {
            let mut spec = SpawnMemberSpec::new("worker", identity.as_str());
            spec.runtime_mode = Some(crate::MobRuntimeMode::TurnDriven);
            handle.spawn_spec(spec).await.expect("spawn a worker");
            let session = handle
                .resolve_bridge_session_id(&identity)
                .await
                .expect("session-backed worker");
            seated.push((identity, session));
        }
        for (_, session) in &seated {
            MobSessionService::discard_live_session(service.as_ref(), session)
                .await
                .expect("discard the live session");
        }
        crash_stop_and_release_routes(handle).await;

        let (_, lost) = seated.remove(0);
        service.delete_persisted_session(&lost).await;
        let successor = service
            .create_session(CreateSessionRequest {
                injected_context: Vec::new(),
                model: "claude-sonnet-4-5".to_string(),
                prompt: "successor".to_string().into(),
                system_prompt: meerkat_core::SystemPromptOverride::Inherit,
                max_tokens: None,
                event_tx: None,
                build: Some(meerkat_core::service::SessionBuildOptions {
                    comms_name: Some(
                        crate::runtime::actor::render_member_comms_name(
                            mob_id.as_str(),
                            "worker",
                            member.as_str(),
                        )
                        .expect("comms name"),
                    ),
                    provider_params: Some(successor_provider_params()),
                    mob_member_binding: None,
                    ..Default::default()
                }),
                initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
                deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
                labels: None,
            })
            .await
            .expect("persist a successor session")
            .session_id;
        // The restart leaves the successor persisted and not live. This mock
        // lists the sessions it holds data for, so the data stays: the
        // successor search lists the successor, and the resume finds it
        // neither live nor an orphan to archive.
        let listed = service
            .live_session_data
            .read()
            .await
            .get(&successor)
            .cloned()
            .expect("the successor's session data");
        MobSessionService::discard_live_session(service.as_ref(), &successor)
            .await
            .expect("discard the successor's live session");
        service
            .live_session_data
            .write()
            .await
            .insert(successor.clone(), listed);
        Self {
            mob_id,
            service,
            storage: restart_storage,
            member,
            successor,
            peers: seated,
        }
    }
}

/// A cold resume at concurrency 4 restores a member whose bound session is
/// absent on its persisted successor. The member's turn runs while the three
/// other members hold their permits (restored, not yet applied): in it the
/// resume finds the successor, repoints the member's binding in the journal,
/// asks the customizer again with the successor's view and rebuilds the
/// member on the successor with the successor's params migrated. The other
/// members come back on their own sessions.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cold_resume_repoints_a_member_to_its_successor_while_other_members_hold_permits() {
    const CONCURRENCY: usize = 4;
    let mob = SuccessorMob::seat(CONCURRENCY - 1).await;
    let probe = restore_pass_probe::install(&mob.mob_id);
    probe.hold_turn(&mob.member);
    let migration = Arc::new(PromptCacheMigration::default());
    let resume = tokio::spawn(
        MobBuilder::for_resume(mob.storage.clone())
            .with_session_service(mob.service.clone())
            .with_spawn_member_customizer(migration.clone())
            .notify_orchestrator_on_resume(false)
            .with_resume_restore_concurrency(CONCURRENCY)
            // Every member is restored at start: the parallel restore stream is
            // what these rows measure.
            .with_member_restore_policy(crate::runtime::MemberRestorePolicy::Eager)
            .resume(),
    );
    let restored = tokio::time::timeout(
        STEP,
        probe.wait_for(|events| {
            events.contains(&RestorePassEvent::TurnStarted(mob.member.clone()))
                && mob.peers.iter().all(|(peer, _)| {
                    events.contains(&RestorePassEvent::RestoreFinished(peer.clone()))
                })
        }),
    )
    .await;
    assert!(
        restored.is_ok(),
        "the member's turn starts and the other members finish their restores: {:?}",
        probe.events()
    );
    probe.release_turn(&mob.member);
    let resumed = joined(resume).await.expect("the resume succeeds");

    // The other members held their permits across the member's turn: each
    // was restored before the turn ended and applied after it.
    let events = probe.events();
    let position = |event: RestorePassEvent| {
        events
            .iter()
            .position(|recorded| recorded == &event)
            .unwrap_or_else(|| panic!("{event:?} is recorded: {events:?}"))
    };
    let member_turn_finished = position(RestorePassEvent::TurnFinished(mob.member.clone()));
    for (peer, _) in &mob.peers {
        assert!(
            position(RestorePassEvent::RestoreFinished(peer.clone())) < member_turn_finished,
            "{peer} is restored before the member's turn ends: {events:?}"
        );
        assert!(
            position(RestorePassEvent::TurnStarted(peer.clone())) > member_turn_finished,
            "{peer} is applied after the member's turn: {events:?}"
        );
    }

    assert!(
        resumed.restore_diagnostics.read().await.is_empty(),
        "no member restore failed"
    );
    assert_eq!(
        resumed.resolve_bridge_session_id(&mob.member).await,
        Some(mob.successor.clone()),
        "the member is repointed to its successor"
    );
    // Two appends, as a restore of one member at a time makes them: the
    // member's turn repoints its binding to the successor (no endpoint is
    // recovered yet), and after the pass the resume upgrades that binding
    // with the endpoint the rebuilt member exposes.
    let repointed = mob
        .storage
        .events
        .replay_all()
        .await
        .expect("read the mob journal")
        .iter()
        .filter_map(|event| event.kind.member_session_binding_recovered())
        .filter(|recovered| recovered.agent_identity == mob.member)
        .map(|recovered| {
            (
                recovered.bridge_session_id().cloned(),
                recovered.member_peer_endpoint().is_some(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        repointed,
        vec![
            (Some(mob.successor.clone()), false),
            (Some(mob.successor.clone()), true),
        ],
        "the member's turn repoints its binding to the successor, then the resume records \
         the successor's endpoint"
    );
    let views = migration.views();
    assert!(
        views.iter().any(|(identity, view)| identity == &mob.member
            && view.session_id == mob.successor
            && view
                .llm_identity
                .as_ref()
                .and_then(|identity| identity.provider_params.clone())
                == Some(successor_provider_params())),
        "the customizer is asked again with the successor's view: {views:?}"
    );
    let rebuilt = last_member_build(&mob.service, &mob.member).await;
    assert_eq!(rebuilt.resume_session_id, Some(mob.successor.clone()));
    assert_eq!(
        rebuilt.provider_params,
        Some(prompt_cache_enabled(successor_provider_params())),
        "the rebuild migrates the successor's params"
    );
    assert!(rebuilt.provider_params_masked);
    for (peer, session) in &mob.peers {
        assert_eq!(
            resumed.resolve_bridge_session_id(peer).await.as_ref(),
            Some(session),
            "{peer} is back on its own session"
        );
    }
    resumed.shutdown().await.expect("shut the resume down");
}
