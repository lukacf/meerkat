//! A SQLite-backed intent-managed identity converges while convergence-status
//! commits hold the mob database's write lock, as commits through a slow
//! fsync do. The runtime store's fenced writes wait for the lock inside the
//! fence (on a blocking thread), and the session publication retries the
//! fence with the session map released.

use std::fmt;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use super::*;
use crate::store::IDENTITY_RUNTIME_WRITE_FENCE_WAIT;
use crate::store::identity_fence_test_hooks::{self as hooks, FenceAttempt};

/// How long each status commit holds mob.db's write lock.
const STATUS_COMMIT_HOLD: Duration = Duration::from_millis(300);

/// How long the status written right before an identity's fenced
/// registration holds mob.db's write lock, eight identities at once. Eight
/// such commits stay inside member retirement's owner deadline (2 s in test
/// builds), which runs in the same passes.
const REGISTRATION_STATUS_HOLD: Duration = Duration::from_millis(200);

/// A mob on SQLite mob storage whose sessions live in a whole-blob SQLite
/// realm (the session and runtime stores share one file).
struct SqliteMob {
    _dir: tempfile::TempDir,
    mob_db: PathBuf,
    runtime_store: Arc<meerkat_runtime::SqliteRuntimeStore>,
    handle: MobHandle,
}

async fn sqlite_mob(label: &str) -> SqliteMob {
    let dir = tempfile::tempdir().expect("tempdir");
    let realm_db = dir.path().join("realm.db");
    let session_store: Arc<dyn SessionStore> =
        Arc::new(meerkat_store::SqliteSessionStore::open(&realm_db).expect("session store"));
    let runtime_store = Arc::new(
        meerkat_runtime::SqliteRuntimeStore::new_whole_blob(&realm_db)
            .expect("whole-blob runtime store"),
    );
    let service = Arc::new(meerkat_session::PersistentSessionService::new(
        PersistentMockBuilder,
        16,
        session_store,
        runtime_store.clone(),
        Arc::new(meerkat_store::MemoryBlobStore::new()),
    ));
    let mob_db = dir.path().join("mob.db");
    let storage = MobStorage::persistent(&mob_db).expect("persistent mob storage");
    let handle = MobBuilder::new(with_unique_mob_id(sample_definition(), label), storage)
        .with_session_service(service)
        .with_spawn_base_prompt_source(Arc::new(crate::StaticSpawnBasePromptSource(
            "fence contention test base prompt".to_string(),
        )))
        .create()
        .await
        .expect("create the mob");
    SqliteMob {
        _dir: dir,
        mob_db,
        runtime_store,
        handle,
    }
}

/// Spawns `member` as a turn-driven worker and returns its session.
async fn spawn_member(mob: &SqliteMob, member: &AgentIdentity) -> SessionId {
    let mut spawn = SpawnMemberSpec::new(ProfileName::from("worker"), member.clone());
    spawn.runtime_mode = Some(crate::MobRuntimeMode::TurnDriven);
    mob.handle
        .spawn_spec(spawn)
        .await
        .expect("spawn the member");
    mob.handle
        .resolve_bridge_session_id(member)
        .await
        .expect("a session-backed member")
}

/// Adopts the existing `member` and its session as an intent-managed
/// identity.
async fn adopt(mob: &SqliteMob, member: &AgentIdentity, session_id: &SessionId) {
    let adoption = crate::identity::AdoptMemberIdentityDeclaration {
        mob_id: mob.handle.mob_id().clone(),
        agent_identity: member.clone(),
        request_id: crate::identity::IdentityAdoptionId::new(format!("adopt-{member}"))
            .expect("valid adoption id"),
        precondition: crate::identity::IdentityAdoptionPrecondition::ExpectedAbsent,
        declaration_scope: crate::identity::IdentityDeclarationScopeId::new("fence-contention")
            .expect("valid declaration scope"),
        declaration_revision: 1,
        session: crate::identity::DesiredSessionTarget {
            session_id: session_id.clone(),
            lineage_id: meerkat_core::SessionLineageId::for_session(session_id),
            lineage_generation: meerkat_core::SessionGeneration::INITIAL,
            authority_policy: crate::identity::DesiredSessionAuthorityPolicy::RequireExisting,
        },
        member: crate::identity::IdentityProfileMemberDeclaration {
            profile_name: ProfileName::from("worker"),
            profile_override: None,
            model_override: None,
            external_addressable_override: None,
            context: None,
            labels: None,
            additional_instructions: None,
            system_prompt_override: None,
            tool_access_policy: None,
            auth_binding: None,
            budget_limits: None,
            runtime_mode: Some(meerkat_contracts::wire::WireMobRuntimeMode::TurnDriven),
            required_env_keys: Vec::new(),
            required_local_callback_tools: Vec::new(),
            execution: crate::identity::DesiredExecution::ControllingSession,
        },
        wiring_custody: crate::identity::IdentityWiringCustody::ExternalManaged,
        owned_wiring: BTreeSet::new(),
        convergence: crate::identity::IdentityConvergenceMode::Drain { max_wait_ms: 5_000 },
    };
    let outcome = mob
        .handle
        .adopt_member_identity_declaration(adoption)
        .await
        .expect("adopt the member");
    assert!(
        matches!(
            outcome.adoption,
            crate::identity::IdentityAdoptionOutcome::Adopted {
                desired_revision: 1
            }
        ),
        "{:?}",
        outcome.adoption
    );
}

async fn convergence(
    mob: &SqliteMob,
    member: &AgentIdentity,
) -> crate::identity::IdentityStoredObservation<crate::identity::IdentityConvergenceStatus> {
    mob.handle
        .identity_convergence_status(member)
        .await
        .expect("read the convergence status")
}

fn is_converged(
    observation: &crate::identity::IdentityStoredObservation<
        crate::identity::IdentityConvergenceStatus,
    >,
) -> bool {
    matches!(
        observation,
        crate::identity::IdentityStoredObservation::Valid(
            crate::identity::IdentityConvergenceStatus {
                intent_revision: Some(1),
                active_intent_revision: Some(1),
                decision: Some(crate::identity::IdentityReconcileDecision::Converged),
                ..
            }
        )
    )
}

/// The latest convergence status of every member that did not converge,
/// one per line.
async fn unconverged_statuses(
    mob: &SqliteMob,
    members: &[AgentIdentity],
    converged: &[Option<Duration>],
) -> String {
    let mut lines = Vec::new();
    for (member, converged_after) in members.iter().zip(converged) {
        if converged_after.is_none() {
            lines.push(format!("{member}: {:?}", convergence(mob, member).await));
        }
    }
    lines.join("\n")
}

/// Polls every member's convergence until all converged or `bound` passed
/// since `started`; returns when each converged, measured from `started`.
async fn wait_converged(
    mob: &SqliteMob,
    members: &[AgentIdentity],
    started: Instant,
    bound: Duration,
) -> Vec<Option<Duration>> {
    let mut converged = vec![None; members.len()];
    loop {
        for (slot, member) in converged.iter_mut().zip(members) {
            if slot.is_none() && is_converged(&convergence(mob, member).await) {
                *slot = Some(started.elapsed());
            }
        }
        if converged.iter().all(Option::is_some) || started.elapsed() >= bound {
            return converged;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// A plain read of the runtime store every 10 ms until `stop` turns true:
/// when each read started and how long it took.
async fn probe_runtime_reads(
    store: Arc<meerkat_runtime::SqliteRuntimeStore>,
    mut stop: tokio::sync::watch::Receiver<bool>,
) -> Vec<(Instant, Duration)> {
    let probe = meerkat_runtime::LogicalRuntimeId::new("fence-contention-probe");
    let mut reads = Vec::new();
    while !*stop.borrow() {
        let started = Instant::now();
        meerkat_runtime::RuntimeStore::load_machine_lifecycle_record(store.as_ref(), &probe)
            .await
            .expect("probe read");
        reads.push((started, started.elapsed()));
        tokio::select! {
            _ = stop.changed() => {}
            () = tokio::time::sleep(Duration::from_millis(10)) => {}
        }
    }
    reads
}

/// One identity's fence attempts while it converged.
struct IdentityRow {
    identity: AgentIdentity,
    attempts: usize,
    /// Attempts through the bounded-wait entry.
    waiting_attempts: usize,
    /// Time spent waiting for mob.db's write lock, over all attempts.
    total_wait: Duration,
    longest_wait: Duration,
    /// Attempts the lock was refused to.
    refused: usize,
    /// The longest run of refused immediate attempts (the publication's
    /// retries), from its first attempt to the end of its last.
    longest_refused_retry: Duration,
    /// A bounded-wait attempt (the runtime store's CAS) took the lock.
    admitted_waiting: bool,
    /// An immediate attempt (the session publication) took the lock.
    admitted_immediate: bool,
    converged_after: Option<Duration>,
}

/// What the fences and the probe saw while the identities converged.
struct ContentionReport {
    identities: Vec<IdentityRow>,
    /// Fence attempts that waited 20 ms or more for mob.db.
    long_waits: usize,
    probe_reads: usize,
    /// Probe reads that ran inside such a wait.
    probe_reads_during_waits: usize,
    probe_max_read: Duration,
}

/// The longest run of consecutive refused immediate attempts, from the
/// first attempt's start to the last attempt's end.
fn longest_refused_run(attempts: &[&FenceAttempt]) -> Duration {
    let mut longest = Duration::ZERO;
    let mut run_start = None;
    for attempt in attempts.iter().filter(|attempt| !attempt.blocking) {
        if attempt.took_lock {
            run_start = None;
        } else {
            let start = *run_start.get_or_insert(attempt.started);
            longest = longest.max(attempt.started + attempt.waited - start);
        }
    }
    longest
}

impl ContentionReport {
    fn new(
        members: &[AgentIdentity],
        attempts: &[FenceAttempt],
        converged: &[Option<Duration>],
        reads: &[(Instant, Duration)],
    ) -> Self {
        let identities = members
            .iter()
            .zip(converged)
            .map(|(member, converged_after)| {
                let own: Vec<&FenceAttempt> = attempts
                    .iter()
                    .filter(|attempt| attempt.identity == *member)
                    .collect();
                IdentityRow {
                    identity: member.clone(),
                    attempts: own.len(),
                    waiting_attempts: own.iter().filter(|attempt| attempt.blocking).count(),
                    total_wait: own.iter().map(|attempt| attempt.waited).sum(),
                    longest_wait: own
                        .iter()
                        .map(|attempt| attempt.waited)
                        .max()
                        .unwrap_or_default(),
                    refused: own.iter().filter(|attempt| !attempt.took_lock).count(),
                    longest_refused_retry: longest_refused_run(&own),
                    admitted_waiting: own
                        .iter()
                        .any(|attempt| attempt.blocking && attempt.took_lock),
                    admitted_immediate: own
                        .iter()
                        .any(|attempt| !attempt.blocking && attempt.took_lock),
                    converged_after: *converged_after,
                }
            })
            .collect();
        let waits: Vec<(Instant, Instant)> = attempts
            .iter()
            .filter(|attempt| attempt.waited >= Duration::from_millis(20))
            .map(|attempt| (attempt.started, attempt.started + attempt.waited))
            .collect();
        let probe_reads_during_waits = reads
            .iter()
            .filter(|(started, took)| {
                let ended = *started + *took;
                waits
                    .iter()
                    .any(|(from, to)| *started >= *from && ended <= *to)
            })
            .count();
        Self {
            identities,
            long_waits: waits.len(),
            probe_reads: reads.len(),
            probe_reads_during_waits,
            probe_max_read: reads
                .iter()
                .map(|(_, took)| *took)
                .max()
                .unwrap_or_default(),
        }
    }

    fn max_attempts(&self) -> usize {
        self.identities
            .iter()
            .map(|row| row.attempts)
            .max()
            .unwrap_or(0)
    }

    fn max_total_wait(&self) -> Duration {
        self.identities
            .iter()
            .map(|row| row.total_wait)
            .max()
            .unwrap_or_default()
    }

    fn max_refused_retry(&self) -> Duration {
        self.identities
            .iter()
            .map(|row| row.longest_refused_retry)
            .max()
            .unwrap_or_default()
    }

    fn max_single_wait(&self) -> Duration {
        self.identities
            .iter()
            .map(|row| row.longest_wait)
            .max()
            .unwrap_or_default()
    }
}

impl fmt::Display for ContentionReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "identity | attempts | waiting entry | total wait | longest wait | refused | longest refused retry | admitted (wait, immediate) | converged after"
        )?;
        for row in &self.identities {
            writeln!(
                f,
                "{} | {} | {} | {:?} | {:?} | {} | {:?} | {}, {} | {:?}",
                row.identity,
                row.attempts,
                row.waiting_attempts,
                row.total_wait,
                row.longest_wait,
                row.refused,
                row.longest_refused_retry,
                row.admitted_waiting,
                row.admitted_immediate,
                row.converged_after
            )?;
        }
        writeln!(
            f,
            "max attempts {} | max total wait {:?} | max single wait {:?} (bound {:?}) | longest \
             refused retry {:?} (bound 2s)",
            self.max_attempts(),
            self.max_total_wait(),
            self.max_single_wait(),
            IDENTITY_RUNTIME_WRITE_FENCE_WAIT,
            self.max_refused_retry()
        )?;
        write!(
            f,
            "probe: {} plain runtime-store reads, {} inside one of the {} fence waits of 20 ms or \
             more, slowest {:?}",
            self.probe_reads, self.probe_reads_during_waits, self.long_waits, self.probe_max_read
        )
    }
}

/// C3: an adopted identity whose every convergence-status commit holds
/// mob.db's write lock for 300 ms converges. A control: with one identity,
/// a registration that finds the fence held is retried on a later pass, so
/// it converges with or without the bounded wait. RED-1 and RED-5 prove the
/// wait itself.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_adopted_identity_converges_while_status_commits_hold_the_mob_database() {
    let mob = sqlite_mob("fence-contention-one").await;
    let member = AgentIdentity::from("worker-1");
    let session_id = spawn_member(&mob, &member).await;
    hooks::hold_status_commits(&mob.mob_db, STATUS_COMMIT_HOLD, None);

    let started = Instant::now();
    adopt(&mob, &member, &session_id).await;
    let converged = wait_converged(
        &mob,
        std::slice::from_ref(&member),
        started,
        Duration::from_secs(15),
    )
    .await;
    let latest = convergence(&mob, &member).await;
    hooks::release(&mob.mob_db);
    assert!(
        converged[0].is_some(),
        "the adoption converges within 15 s while status commits hold mob.db; latest status: \
         {latest:?}"
    );
    mob.handle.shutdown().await.expect("shut the mob down");
}

/// RED-5: eight adopted identities on one mob reconcile at once while every
/// convergence-status commit of the runtime-registration step (the status
/// written right before the fenced registration) holds mob.db's write lock
/// for 200 ms. Every identity converges within 30 s, every identity that
/// reaches the fence gets its fenced registration admitted, no fence attempt
/// waits out the whole bound, no refused retry runs out its own, and a
/// probe's plain reads of the runtime store keep completing while fences
/// wait. Prints the per-identity numbers.
///
/// Whether a status commit holds mob.db when an identity's bounded-wait
/// attempt starts is up to the scheduler: each identity's own status commit
/// ends before its attempt, and in some runs every attempt took the lock
/// between two commits. So the first bounded-wait attempt starts while a
/// writer holds mob.db's write lock and keeps it 200 ms more, as a held
/// status commit does: every run contains a bounded wait of about 200 ms,
/// and the probe reads during it. The other attempts meet the status
/// commits as they come.
///
/// The statuses of the other steps commit at full speed: adopting a member
/// retires its materialization first, and the load of every status held
/// belongs to the retirement test below.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn eight_adopted_identities_converge_together_while_status_commits_hold_the_mob_database() {
    const IDENTITIES: usize = 8;
    const BOUND: Duration = Duration::from_secs(30);
    let mob = sqlite_mob("fence-contention-eight").await;
    let members: Vec<AgentIdentity> = (1..=IDENTITIES)
        .map(|index| AgentIdentity::from(format!("worker-{index}")))
        .collect();
    let mut sessions = Vec::with_capacity(IDENTITIES);
    for member in &members {
        sessions.push(spawn_member(&mob, member).await);
    }
    hooks::hold_status_commits(
        &mob.mob_db,
        REGISTRATION_STATUS_HOLD,
        Some(crate::identity::IdentityReconcileDecision::EnsureRuntimeRegistration),
    );
    hooks::record_fence_attempts(&mob.mob_db);
    hooks::contend_next_waiting_attempt(&mob.mob_db, REGISTRATION_STATUS_HOLD);

    let (stop, stopped) = tokio::sync::watch::channel(false);
    let probe = tokio::spawn(probe_runtime_reads(mob.runtime_store.clone(), stopped));
    let started = Instant::now();
    futures::future::join_all(
        members
            .iter()
            .zip(&sessions)
            .map(|(member, session_id)| adopt(&mob, member, session_id)),
    )
    .await;
    let converged = wait_converged(&mob, &members, started, BOUND).await;
    stop.send_replace(true);
    let reads = probe.await.expect("the probe task");
    let attempts = hooks::fence_attempts(&mob.mob_db);
    let contending_writer = hooks::contending_writer(&mob.mob_db);
    hooks::release(&mob.mob_db);

    let report = ContentionReport::new(&members, &attempts, &converged, &reads);
    println!("{report}");
    let unconverged = unconverged_statuses(&mob, &members, &converged).await;
    assert!(
        converged.iter().all(Option::is_some),
        "every identity converges within {BOUND:?}:\n{report}\nthe others:\n{unconverged}"
    );
    assert!(
        report
            .identities
            .iter()
            .all(|row| row.attempts == 0 || (row.admitted_waiting && row.admitted_immediate)),
        "every identity that reached the fence ends with its fenced registration admitted:\n{report}"
    );
    assert!(
        attempts
            .iter()
            .any(|attempt| attempt.blocking && attempt.waited >= Duration::from_millis(100)),
        "the run shows the contention it measures: a bounded-wait attempt waited 100 ms or \
         more (the writer across the first one's start: {contending_writer:?}):\n{report}"
    );
    assert!(
        report.max_single_wait() < IDENTITY_RUNTIME_WRITE_FENCE_WAIT,
        "no fence attempt waits out the whole bound:\n{report}"
    );
    assert!(
        report.max_refused_retry() < Duration::from_secs(2),
        "no retry of a refused immediate attempt runs out its 2-second bound:\n{report}"
    );
    assert!(
        report.long_waits == 0 || report.probe_reads_during_waits > 0,
        "plain runtime-store reads complete while fences wait:\n{report}"
    );
    assert!(
        report.probe_max_read < Duration::from_secs(1),
        "no plain runtime-store read waits behind a fence:\n{report}"
    );
    mob.handle.shutdown().await.expect("shut the mob down");
}

/// Eight adopted identities reconcile at once while every convergence-status
/// commit holds mob.db's write lock for 300 ms, for 15 s; then the hold
/// ends. Under that load a member retirement runs past the 2 s budget its
/// reconcile pass gave it (test builds) after the retirement durably
/// started. Identity reconciliation owns such a retirement until it
/// settles, so every identity converges once the load ends, within 60 s of
/// the adoptions.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn identities_whose_retirement_outruns_its_budget_converge_once_the_load_ends() {
    const IDENTITIES: usize = 8;
    const HOLD_FOR: Duration = Duration::from_secs(15);
    const BOUND: Duration = Duration::from_mins(1);
    let mob = sqlite_mob("retirement-outruns-budget").await;
    let members: Vec<AgentIdentity> = (1..=IDENTITIES)
        .map(|index| AgentIdentity::from(format!("worker-{index}")))
        .collect();
    let mut sessions = Vec::with_capacity(IDENTITIES);
    for member in &members {
        sessions.push(spawn_member(&mob, member).await);
    }
    hooks::hold_status_commits(&mob.mob_db, STATUS_COMMIT_HOLD, None);

    let started = Instant::now();
    futures::future::join_all(
        members
            .iter()
            .zip(&sessions)
            .map(|(member, session_id)| adopt(&mob, member, session_id)),
    )
    .await;
    tokio::time::sleep(HOLD_FOR.saturating_sub(started.elapsed())).await;
    hooks::release(&mob.mob_db);
    let converged = wait_converged(&mob, &members, started, BOUND).await;
    let unconverged = unconverged_statuses(&mob, &members, &converged).await;
    println!("converged after (from the adoptions): {converged:?}");
    assert!(
        converged.iter().all(Option::is_some),
        "every identity converges within {BOUND:?} of the adoptions, the hold ending at \
         {HOLD_FOR:?}:\n{unconverged}"
    );
    mob.handle.shutdown().await.expect("shut the mob down");
}

/// An adopted identity whose member retirement settles stuck after its
/// durable start (its session archive stage fails once) converges without
/// a resume: identity reconciliation re-drives the stuck retirement on its
/// backoff passes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_identity_whose_member_retirement_settles_stuck_converges_on_a_redrive() {
    use crate::runtime::actor::retirement_stage_test_hooks as stages;

    const BOUND: Duration = Duration::from_secs(20);
    let mob = sqlite_mob("stuck-retirement-redrive").await;
    let member = AgentIdentity::from("worker-1");
    let session_id = spawn_member(&mob, &member).await;
    stages::fail_next_session_archive(&session_id);

    let started = Instant::now();
    adopt(&mob, &member, &session_id).await;
    let converged = wait_converged(&mob, std::slice::from_ref(&member), started, BOUND).await;
    let latest = convergence(&mob, &member).await;
    assert!(
        !stages::session_archive_failure_armed(&session_id),
        "the adoption's member retirement reached its session archive stage, which failed once"
    );
    assert!(
        converged[0].is_some(),
        "the identity converges within {BOUND:?} once its stuck retirement is re-driven; latest \
         status: {latest:?}"
    );
    mob.handle.shutdown().await.expect("shut the mob down");
}
