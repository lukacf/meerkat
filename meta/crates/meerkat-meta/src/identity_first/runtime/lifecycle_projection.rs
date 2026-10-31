//! Lifecycle side effects as projections over Core's member lifecycle
//! journal.
//!
//! Core journals each lifecycle step of an identity's member: a seat, a
//! rotation onto a fresh session (one append holds the retirement of the
//! incarnation that ends and the seat of the next one), a retirement. Some
//! of these steps owe side effects that Meta runs:
//!
//! | Journal fact | Side effects |
//! |---|---|
//! | the seat of an identity's member, while Core still binds that incarnation | the customizer's `after_create`, with the session the seat binds |
//! | a retirement the roster's unlisting asked for | the distillation of the retired session, the exit interview (`retire`), the retirement hook (`unlisted`) |
//! | a reset's retirement (online, or a first-boot `Fresh` row's) | the distillation of the retired session behind its reset boundary, the retirement hook (`reset`) |
//! | a delete's retirement | the drop of the retired session's semantic-memory scope, the exit interview (`delete`), the retirement hook (`delete`) |
//! | a role move's retirement onto a fresh session, or a rotation no Meta verb asked for that moved the role | the distillation of the retired session as a retirement's (no reset boundary), the retirement hook (`role_move`) |
//! | a rotation onto a fresh session under the same role no Meta verb asked for (a host's own) | the distillation of the retired session as a retirement's, the retirement hook (`rotated`) |
//! | any other retirement: a reload's, a respawn's, Core's own | none |
//!
//! A release (`retire`, a scoped retire, a reprofile) is no journaled step:
//! its retirement hook stays the verb's own, called before the verb answers.
//! A seat whose incarnation has ended by the time the projection reaches it
//! is told nothing: its retirement's side effects stand for it.
//!
//! # At least once, and bounded
//!
//! The projection reads the journal page by page from a cursor kept in
//! Meta's metadata store ([`crate::runtime::LifecycleProjectionCursor`]) and
//! advances the cursor only once the side effects of the facts it covers
//! were attempted. A process stopped in between, by a kill or a crash,
//! attempts them again at its next start, so no side effect a step owes is
//! lost; the cursor lives in whichever metadata store the runtime was
//! given, so with a store that keeps nothing across a restart (an in-memory
//! one) that holds within the process only. Each side effect tolerates the
//! repeat: the exit interview is keyed by the time its step was asked, so a
//! repeat queues nothing more in a store that keys it so; a repeated
//! distillation reads the session again; the retirement hook and
//! `after_create` may hear of the same incarnation again after a restart,
//! and a host keys what it does by the incarnation (`agent_runtime_id`,
//! generation) or the session. Within one process a page that failed part
//! way runs again without repeating the side effects that ran.
//!
//! Each side effect is bounded: an attempt of the retirement hook or of
//! `after_create` that has not answered within its deadline, or an exit
//! interview write that failed or did not land within it, is attempted
//! again, at most [`EFFECT_ATTEMPTS`] times in all; a distillation is
//! attempted once, within the distiller's own pre-rotation bound. An answer
//! the host gives (an error of the hook or of `after_create`) is final. A
//! side effect that failed is recorded durably
//! ([`crate::runtime::LifecycleEffectFailure`]) and reported on the error
//! hook (`retired_member_release_failed` for the retirement hook,
//! `lifecycle_effect_failed` for the others), and the projection moves on:
//! one side effect that never answers, or a write that always fails, delays
//! the side effects after it by at most its bound.
//!
//! # What the journal does not say
//!
//! Core's journal does not say which Meta verb asked for a retirement: a
//! reset and a delete are the same rotation to Core, and the roster's
//! unlisting the same retirement as any other. Each Meta verb records that
//! ([`crate::runtime::LifecycleEffectIntent`]) before it asks Core for the
//! step, keyed by the incarnation the step ends, for an incarnation the
//! identity spawned only, and the projection reads the record when it
//! reaches the step's retirement; once the cursor past it is recorded, the
//! record goes. A step Core refused before any of it started takes its
//! record back and puts back the one it replaced. A roster pass that lists
//! an identity again takes back an unlisting Core never carried out. Which
//! retirement owes what is decided in one place ([`intent_cause`],
//! [`rotation_cause`]): once Core's journal says why it retired each
//! incarnation, and on which session, those facts come first there, and
//! only a reset's and a delete's records remain.
//!
//! What a step knows that no durable record keeps (the exact registration
//! the retiring incarnation held, read before Core was asked; the build
//! draft an `after_create` reports) is kept in this process, and the
//! projection takes it under the identity's lifecycle lock, which the step
//! holds until it answers. It waits for the lock at most a side effect's
//! deadline: a step that holds it longer (a build stuck in the customizer)
//! holds back no side effect, and the retirement hook then names no
//! registration. After a restart the retirement hook names no registration,
//! and `after_create` reports a context derived from the identity's roster
//! spec (or from Core's binding), with no system prompt.
//!
//! # A predecessor's backlog
//!
//! The side effects of the events a stopped process left behind its cursor
//! when this one starts run in the background, in a lane of their own, and
//! their count stands in the projection's status until they ran: an
//! identity bootstrap never waits for them. One identity's side effects
//! keep journal order across the two lanes: the live lane holds a page with
//! side effects of an identity the backlog still owes some, after running
//! the other identities' of the page, and the live task runs the page again
//! once the backlog lane ran past that identity; the steps journaled after
//! it follow it there, in the background too.
//!
//! # Scale
//!
//! The projection reads one page at a time, at most [`PAGE_LIMIT`] journal
//! events, with one read of Core's member history per page that holds a
//! seat or a rotation no Meta verb asked for (Core answers a read of one
//! identity's binding from the same fold, so a page reads the fold once),
//! and runs the side effects of different identities at most
//! [`EFFECT_CONCURRENCY`] at once in each lane (one identity's in journal
//! order). A start that owes a predecessor's backlog reads the backlog's
//! pages once more, for which identities it owes. It reads nothing per
//! member and wakes no member: an idle journal costs one empty page read per
//! sweep.
//!
//! # Start and stop
//!
//! The identity context that serves the runtime binds the projection
//! ([`IdentityRuntime::bind_lifecycle_projection`]); a runtime that no
//! context serves is bound by the public view an embedder hands the RPC
//! handler, or by its restore, and records nothing for steps while no one
//! binds it. It starts at the end of the context's first roster pass, or at
//! the first step a Meta verb asks Core for after the binding. Each such
//! step kicks it, and a sweep every [`SWEEP_INTERVAL`] reads the steps
//! nothing kicked (Core's own, a host's). The cursor is read, or started at
//! the journal's head when there is none, before the first step Meta asks
//! Core for in this process, so every step Meta asks for is projected; a
//! seat step is refused, retryable, while no cursor is stored and none can
//! be started. With one stored, a step goes ahead whatever else failed: what
//! it journals lies past that cursor, and the live task prepares the
//! projection again at each sweep until a preparation gets through. A
//! unified runtime's identity bootstrap waits for the projection to reach
//! the journal's head before it answers, so the side effects of the
//! bootstrap's own steps run before it answers. A shutdown lets it reach the
//! head within its share of the runtime's event drain window, then stops it;
//! what it did not reach runs at the next start. Preparations, and runs of
//! either lane, that keep failing read stalled in the status.
//!
//! Nothing is projected on a candidate launch, whose verbs change no stored
//! member, or for a runtime without a session bridge. A session bridge that
//! cannot read what the projection reads (Core's member lifecycle journal,
//! or an identity's binding) turns it off: that is reported once on the
//! error hook (`lifecycle_projection_off`), and the runtime's status and
//! health read it off.

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use futures::stream::{self, StreamExt};
use meerkat_core::types::SessionId;
use tokio::sync::{Mutex, Notify, watch};
use tokio::task::JoinHandle;

use super::super::bridge::{
    BridgeError, MemberLifecycleFact, MemberLifecycleFactRecord, MemberLifecycleJournalPage,
};
use super::super::contracts::AgentCustomizer;
use super::super::types::{
    AgentIdentity, AgentRuntimeId, DurableAgentSpec, LifecycleProjectionState,
    LifecycleProjectionStatus, RetiredIdentityCause, RetiredIdentityMember,
};
use super::records::unix_ms_now;
use super::{IdentityRuntime, IdentityRuntimeError, binding_spawned_for_identity};
use crate::memory::distiller::DistillCause;
use crate::mob_handle_runtime::SessionCreatedContext;
use crate::runtime::{
    LifecycleEffectFailure, LifecycleEffectIntent, LifecycleEffectKind, LifecycleEffectVerb,
    LifecycleIntentKey, LifecycleProjectionBacklog, LifecycleProjectionCursor,
};
use crate::unified_runtime::types::ErrorEvent;

/// The most journal events one page reads: Core's own bound.
const PAGE_LIMIT: usize = meerkat_mob::IDENTITY_ENTRY_PAGE_MAX;

/// How many identities' side effects of one page run at once.
const EFFECT_CONCURRENCY: usize = 16;

/// How often the projection reads the journal when nothing kicked it.
const SWEEP_INTERVAL: Duration = Duration::from_secs(5);

/// How long one attempt of a side effect may run: a retirement hook or an
/// `after_create` that has not answered by then, or an exit interview write
/// that has not landed, failed that attempt.
const EFFECT_DEADLINE: Duration = Duration::from_secs(30);

/// How many times a side effect is attempted before its failure is recorded
/// and the projection moves on.
const EFFECT_ATTEMPTS: u32 = 3;

/// The pause between two attempts of one side effect.
const EFFECT_RETRY_PAUSE: Duration = Duration::from_secs(1);

/// How many runs in a row may fail before the projection reads as stalled.
const STALLED_AFTER_FAILED_RUNS: u32 = 3;

/// The most intents of an earlier mob incarnation one start removes.
const STALE_INTENT_SWEEP: usize = 1024;

/// How many lifecycle effect failures of the mob a start of the projection
/// keeps: the latest ones.
const FAILURES_KEPT: usize = 1024;

/// After how many failures recorded in one run the projection prunes them
/// again: a long run keeps at most [`FAILURES_KEPT`] and this many more.
const FAILURES_PRUNED_EVERY: usize = 256;

fn lock<T>(mutex: &StdMutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The bounds of each side effect: a deadline per attempt, the attempts,
/// and the pause between two of them.
#[derive(Debug, Clone, Copy)]
struct EffectLimits {
    deadline: Duration,
    attempts: u32,
    pause: Duration,
}

impl Default for EffectLimits {
    fn default() -> Self {
        Self {
            deadline: EFFECT_DEADLINE,
            attempts: EFFECT_ATTEMPTS,
            pause: EFFECT_RETRY_PAUSE,
        }
    }
}

/// What a bounded side effect came to: its answer, or what its last attempt
/// failed with, and how many attempts were made.
struct Bounded<T> {
    attempts: u32,
    outcome: Result<T, String>,
}

/// Run `attempt` until it answers, at most `limits.attempts` times, each
/// attempt bounded by `limits.deadline`. An attempt that answers `Err`
/// failed, as one that did not answer in time did; a side effect whose
/// answer stands whatever it is answers `Ok` with it.
async fn bounded<T, F, Fut>(limits: EffectLimits, mut attempt: F) -> Bounded<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, String>>,
{
    let allowed = limits.attempts.max(1);
    let mut attempts = 0;
    loop {
        attempts += 1;
        let failure = match tokio::time::timeout(limits.deadline, attempt()).await {
            Ok(Ok(answer)) => {
                return Bounded {
                    attempts,
                    outcome: Ok(answer),
                };
            }
            Ok(Err(error)) => error,
            Err(_) => format!(
                "timed out: no answer within {} ms",
                limits.deadline.as_millis()
            ),
        };
        if attempts >= allowed {
            return Bounded {
                attempts,
                outcome: Err(failure),
            };
        }
        tokio::time::sleep(limits.pause).await;
    }
}

/// The identity runtime's lifecycle projection: where it stands in Core's
/// journal, its tasks, and what this process knows of the steps it asked
/// Core for that no durable record keeps.
pub(super) struct LifecycleProjection {
    phase: StdMutex<Phase>,
    /// Serializes the preparations.
    preparing: Mutex<()>,
    /// The live lane: the journal after where this process started. Held by
    /// the one run of the lane in progress.
    live: Mutex<Lane>,
    /// The backlog lane: what a stopped predecessor still owed. Held by the
    /// one run of the lane in progress.
    backlog: Mutex<Lane>,
    /// What the store holds, as both lanes left it.
    record: StdMutex<Record>,
    /// Serializes the writes of [`Self::record`], so the last one written
    /// is the latest.
    writing: Mutex<()>,
    /// The identities a predecessor's backlog still owes side effects, each
    /// with the last journal event that owes it one: the live lane runs an
    /// identity's side effects only once the backlog lane ran past that
    /// event. `None` until the owed ranges were read.
    owed: watch::Sender<Option<BTreeMap<AgentIdentity, u64>>>,
    /// Serializes the reads of the owed ranges.
    scanning: Mutex<()>,
    /// The runtime the tasks run for.
    owner: StdMutex<Option<Owner>>,
    /// Wakes the live task.
    kick: Arc<Notify>,
    /// Set once the runtime shuts down: the live task finishes the run it is
    /// in, or runs once more, and ends; the backlog task stops at its next
    /// page; no task starts after it.
    stopping: watch::Sender<bool>,
    live_task: StdMutex<Option<JoinHandle<()>>>,
    backlog_task: StdMutex<Option<JoinHandle<()>>>,
    /// Side effects a verb runs beside the projection, which no journal fact
    /// carries (the distillation of a reset's incarnation Core had already
    /// retired); a shutdown joins them with the tasks.
    side_tasks: StdMutex<Vec<JoinHandle<()>>>,
    /// The incarnation a step this process asked Core for is about to end,
    /// as the retirement hook reads it, by identity: its registration was
    /// read before Core was asked.
    retiring: StdMutex<BTreeMap<AgentIdentity, Retiring>>,
    /// What the build of a seat-like step made, by identity, until Core
    /// answers the step with its session.
    building: StdMutex<BTreeMap<AgentIdentity, SessionCreatedContext>>,
    /// What the build of a seat-like step made, by identity and the session
    /// Core bound, for the `after_create` of that seat.
    built: StdMutex<BTreeMap<(AgentIdentity, SessionId), SessionCreatedContext>>,
    limits: StdMutex<EffectLimits>,
    /// What failed in a row: the preparations, and each lane's runs.
    failed_runs: StdMutex<FailedRuns>,
    /// Whether a step was refused its record for want of a binding, said
    /// once.
    unbound_said: AtomicBool,
    /// How many lifecycle effect failures this run recorded.
    failures_recorded: AtomicUsize,
}

#[derive(Clone)]
enum Phase {
    /// Not prepared yet, or the last preparation failed.
    Unprepared,
    /// The cursor is read, over the journal of the mob incarnation
    /// `incarnation`.
    Ready { incarnation: String },
    /// Nothing to project: a candidate launch, or no session bridge.
    Inactive,
    /// The session bridge cannot read what the projection reads.
    Off { detail: String },
}

#[derive(Clone)]
struct Owner {
    runtime: Weak<IdentityRuntime>,
    /// The customizer the binding carries: an identity context's, the one
    /// its roster passes build with.
    customizer: Option<Arc<dyn AgentCustomizer>>,
    /// An identity context bound it; another binding never replaces one.
    by_context: bool,
}

/// An incarnation a step this process asked Core for is about to end, as
/// the retirement hook reads it.
struct Retiring {
    member: RetiredIdentityMember,
    /// The step answered Core's part and settled the registration (a
    /// registration still current after Core's answer was not ended by it):
    /// what the hook names, whether or not the step still holds the
    /// identity's lifecycle lock.
    settled: bool,
}

/// One lane's progress.
#[derive(Default)]
struct Lane {
    /// The cursor of the last journal event whose side effects ran.
    cursor: u64,
    /// The last event the lane reads; `None` reads on to the head.
    until: Option<u64>,
    /// Facts past `cursor` whose side effects ran in this process: a page
    /// that failed after them runs again without them.
    done: BTreeSet<u64>,
}

/// The projection's record, as the store holds it.
#[derive(Clone, Default)]
struct Record {
    cursor: u64,
    backlog: Vec<LifecycleProjectionBacklog>,
    uncleared: BTreeSet<LifecycleIntentKey>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LaneKind {
    Live,
    Backlog,
}

/// What one run of the projection was.
#[derive(Clone, Copy)]
enum Run {
    /// A preparation: the cursor read, or started at the journal's head.
    Prepare,
    /// A run of a lane.
    Lane(LaneKind),
}

/// How many runs of each kind failed in a row, and on what.
#[derive(Default)]
struct FailedRuns {
    prepare: Failures,
    live: Failures,
    backlog: Failures,
}

impl FailedRuns {
    fn of(&mut self, run: Run) -> &mut Failures {
        match run {
            Run::Prepare => &mut self.prepare,
            Run::Lane(LaneKind::Live) => &mut self.live,
            Run::Lane(LaneKind::Backlog) => &mut self.backlog,
        }
    }
}

/// How many runs of one kind failed in a row, and what the last failed on.
#[derive(Default)]
struct Failures {
    in_a_row: u32,
    last: Option<String>,
}

/// Why the lifecycle projection could not be prepared.
pub(super) struct UnpreparedProjection {
    /// What failed.
    detail: String,
    /// The store holds a cursor of the projection over the journal of the
    /// mob's current incarnation already.
    cursor_stored: bool,
}

impl UnpreparedProjection {
    /// Whether the store holds a cursor of the projection over the journal
    /// of the mob's current incarnation already: a step Core journals now
    /// lies past it, so the projection reads that step once a preparation
    /// gets through.
    pub(super) fn cursor_stored(&self) -> bool {
        self.cursor_stored
    }
}

impl std::fmt::Display for UnpreparedProjection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.detail)
    }
}

/// Why a page could not be applied.
enum PageError {
    /// The bridge cannot do this read at all: the projection is off.
    Unreadable(&'static str),
    /// The side effects of these identities on the page follow a
    /// predecessor's backlog, which has not run past them yet: the page
    /// runs again once it has.
    Held(BTreeSet<AgentIdentity>),
    Failed(String),
}

impl From<String> for PageError {
    fn from(error: String) -> Self {
        Self::Failed(error)
    }
}

/// Why one identity's side effects of a page stopped short.
enum JobStop {
    /// They follow a predecessor's backlog, which still owes this identity
    /// side effects.
    Held(AgentIdentity),
    /// A side effect's failure could not be recorded.
    Failed(String),
}

/// How a run of a lane ended, short of a failure.
enum LaneEnd {
    /// It reached its end: the journal's head, or the end of the owed
    /// ranges.
    Reached,
    /// It holds a page whose side effects of these identities follow a
    /// predecessor's backlog.
    Held(BTreeSet<AgentIdentity>),
}

impl LifecycleProjection {
    pub(super) fn new() -> Self {
        Self {
            phase: StdMutex::new(Phase::Unprepared),
            preparing: Mutex::new(()),
            live: Mutex::new(Lane::default()),
            backlog: Mutex::new(Lane::default()),
            record: StdMutex::new(Record::default()),
            writing: Mutex::new(()),
            owed: watch::channel(None).0,
            scanning: Mutex::new(()),
            owner: StdMutex::new(None),
            kick: Arc::new(Notify::new()),
            stopping: watch::channel(false).0,
            live_task: StdMutex::new(None),
            backlog_task: StdMutex::new(None),
            side_tasks: StdMutex::new(Vec::new()),
            retiring: StdMutex::new(BTreeMap::new()),
            building: StdMutex::new(BTreeMap::new()),
            built: StdMutex::new(BTreeMap::new()),
            limits: StdMutex::new(EffectLimits::default()),
            failed_runs: StdMutex::new(FailedRuns::default()),
            unbound_said: AtomicBool::new(false),
            failures_recorded: AtomicUsize::new(0),
        }
    }

    fn phase(&self) -> Phase {
        lock(&self.phase).clone()
    }

    fn set_phase(&self, phase: Phase) {
        *lock(&self.phase) = phase;
    }

    fn limits(&self) -> EffectLimits {
        *lock(&self.limits)
    }

    fn lane(&self, kind: LaneKind) -> &Mutex<Lane> {
        match kind {
            LaneKind::Live => &self.live,
            LaneKind::Backlog => &self.backlog,
        }
    }

    fn stash_retiring(&self, member: RetiredIdentityMember) {
        lock(&self.retiring).insert(
            member.identity.clone(),
            Retiring {
                member,
                settled: false,
            },
        );
    }

    /// Replace the kept incarnation of `member`'s identity with the one its
    /// step settled, while the projection has not taken it yet.
    fn refine_retiring(&self, member: RetiredIdentityMember) {
        let mut retiring = lock(&self.retiring);
        if retiring
            .get(&member.identity)
            .is_some_and(|kept| kept.member.generation == member.generation)
        {
            retiring.insert(
                member.identity.clone(),
                Retiring {
                    member,
                    settled: true,
                },
            );
        }
    }

    fn take_retiring(&self, identity: &AgentIdentity, generation: u64) -> Option<Retiring> {
        let mut retiring = lock(&self.retiring);
        if retiring
            .get(identity)
            .is_some_and(|kept| kept.member.generation == generation)
        {
            return retiring.remove(identity);
        }
        None
    }

    fn take_built(
        &self,
        identity: &AgentIdentity,
        session: &SessionId,
    ) -> Option<SessionCreatedContext> {
        lock(&self.built).remove(&(identity.clone(), session.clone()))
    }

    fn owed_events(&self) -> u64 {
        lock(&self.record)
            .backlog
            .iter()
            .map(LifecycleProjectionBacklog::events)
            .sum()
    }

    /// Count a run toward the stall the status reports: `failed` is what it
    /// failed on, `None` when it got through.
    fn note_run(&self, run: Run, failed: Option<&str>) {
        let mut failed_runs = lock(&self.failed_runs);
        let failures = failed_runs.of(run);
        match failed {
            None => *failures = Failures::default(),
            Some(error) => {
                failures.in_a_row = failures.in_a_row.saturating_add(1);
                failures.last = Some(error.to_string());
            }
        }
    }

    /// What the first of `runs` that failed [`STALLED_AFTER_FAILED_RUNS`]
    /// times in a row failed on last, when one did.
    fn stalled(&self, runs: &[Run]) -> Option<String> {
        let mut failed_runs = lock(&self.failed_runs);
        runs.iter().find_map(|run| {
            let failures = failed_runs.of(*run);
            (failures.in_a_row >= STALLED_AFTER_FAILED_RUNS)
                .then(|| failures.last.clone().unwrap_or_default())
        })
    }

    /// Whether a predecessor's backlog still owes `identity` side effects.
    fn still_owed(&self, identity: &AgentIdentity) -> bool {
        backlog_owes(&self.owed.borrow(), identity)
    }
}

/// A step that ends an identity's live incarnation, prepared before Core is
/// asked for it ([`IdentityRuntime::prepare_lifecycle_retirement`]).
pub(super) struct PreparedRetirement {
    identity: AgentIdentity,
    generation: u64,
    /// The step's intent was recorded, in place of `Some` intent an earlier
    /// step recorded for the same incarnation.
    recorded: Option<Option<LifecycleEffectIntent>>,
    /// The incarnation as the retirement hook reads it, when a hook is
    /// installed.
    retiring: Option<RetiredIdentityMember>,
}

impl PreparedRetirement {
    /// The generation of the incarnation the step ends.
    pub(super) fn generation(&self) -> u64 {
        self.generation
    }
}

/// Where a side effect was owed: the mob incarnation whose journal owed
/// it, and the event that did.
#[derive(Clone, Copy)]
struct Owed<'a> {
    incarnation: &'a str,
    cursor: u64,
}

/// A side effect that failed, before it is recorded.
struct FailedEffect<'a> {
    identity: &'a AgentIdentity,
    member: &'a meerkat_mob::AgentIdentity,
    generation: u64,
    session: Option<&'a SessionId>,
    effect: LifecycleEffectKind,
    attempts: u32,
    error: String,
}

/// One side effect a journal fact owes an identity.
enum Effect {
    /// The customizer's `after_create` for the seat of the incarnation
    /// `generation` Core bound to `session`; `role` and `labels` are Core's
    /// binding's, for a context derived without the build's own.
    Told {
        member: meerkat_mob::AgentIdentity,
        generation: u64,
        session: SessionId,
        role: meerkat_mob::ProfileName,
        labels: BTreeMap<String, String>,
    },
    Retirement(Retirement),
}

/// The retirement of one incarnation, with what it owes.
struct Retirement {
    member: meerkat_mob::AgentIdentity,
    generation: u64,
    role: meerkat_mob::ProfileName,
    session: Option<SessionId>,
    cause: RetiredIdentityCause,
    /// When its step was asked: the key of the exit interview it queues.
    asked_at_ms: Option<u64>,
}

/// The memory side effects a retirement owes beside its hook, by its cause.
#[derive(Default)]
struct RetirementMemory {
    /// Mark the retired session's reset boundary: what is distilled of it
    /// lands quarantined, pending the steward's review.
    reset_boundary: bool,
    /// Bind the retired generation to its session for the distiller's
    /// evidence.
    generation_mark: bool,
    /// Distill the retired session, with this cause.
    distill: Option<DistillCause>,
    /// Drop the retired session's semantic-memory scope: the identity never
    /// resumes it.
    drop_scope: bool,
    /// Queue the identity's exit interview, with this cause.
    exit_interview: Option<&'static str>,
}

impl RetirementMemory {
    fn of(cause: RetiredIdentityCause) -> Self {
        match cause {
            RetiredIdentityCause::Unlisted => Self {
                distill: Some(DistillCause::Retire),
                exit_interview: Some("retire"),
                ..Self::default()
            },
            // The marks come first: no distillate over the retired session
            // lands before its reset boundary.
            RetiredIdentityCause::Reset => Self {
                reset_boundary: true,
                generation_mark: true,
                distill: Some(DistillCause::Reset),
                ..Self::default()
            },
            // The identity goes on, on a fresh session: what the session it
            // left holds is distilled as a retirement's, with no reset
            // boundary, since nobody asked for a clean slate.
            RetiredIdentityCause::RoleMove | RetiredIdentityCause::Rotated => Self {
                generation_mark: true,
                distill: Some(DistillCause::Retire),
                ..Self::default()
            },
            // The delete distilled the session before Core was asked.
            RetiredIdentityCause::Delete => Self {
                drop_scope: true,
                exit_interview: Some("delete"),
                ..Self::default()
            },
            // Releases are not journaled.
            RetiredIdentityCause::Retire | RetiredIdentityCause::Scoped => Self::default(),
        }
    }
}

/// What Core's member history says a rotation no Meta verb asked for did.
struct RotationFacts {
    /// The successor runs on another session than the incarnation it
    /// replaced.
    fresh_session: bool,
    /// The successor runs under another role.
    role_moved: bool,
}

impl RotationFacts {
    fn of(retired: &meerkat_mob::MemberBinding, seated: &meerkat_mob::MemberBinding) -> Self {
        Self {
            fresh_session: retired.session_id().is_some()
                && seated.session_id().is_some()
                && retired.session_id() != seated.session_id(),
            role_moved: retired.role() != seated.role(),
        }
    }
}

/// The cause a retirement a Meta verb recorded its intent for ends its
/// incarnation with: the verb's. A replacement on the same session owes
/// nothing.
fn intent_cause(verb: LifecycleEffectVerb) -> Option<RetiredIdentityCause> {
    match verb {
        LifecycleEffectVerb::Unlisted => Some(RetiredIdentityCause::Unlisted),
        LifecycleEffectVerb::Reset => Some(RetiredIdentityCause::Reset),
        LifecycleEffectVerb::Delete => Some(RetiredIdentityCause::Delete),
        LifecycleEffectVerb::RoleMove => Some(RetiredIdentityCause::RoleMove),
        LifecycleEffectVerb::Reload => None,
    }
}

/// The cause of a retirement no Meta verb recorded an intent for, which
/// Core journaled with its successor's seat: a role move when the successor
/// runs under another role, a rotation when it runs on a fresh session
/// under the same one. A replacement on the same session owes nothing.
fn rotation_cause(rotation: &RotationFacts) -> Option<RetiredIdentityCause> {
    if !rotation.fresh_session {
        return None;
    }
    Some(if rotation.role_moved {
        RetiredIdentityCause::RoleMove
    } else {
        RetiredIdentityCause::Rotated
    })
}

/// A fact whose side effects wait for Core's member history: a seat, or a
/// retirement no intent names that Core journaled with its successor's seat.
enum Pending {
    Seat {
        cursor: u64,
        identity: AgentIdentity,
        member: meerkat_mob::AgentIdentity,
        generation: u64,
    },
    Rotation {
        cursor: u64,
        identity: AgentIdentity,
        member: meerkat_mob::AgentIdentity,
        generation: u64,
        role: meerkat_mob::ProfileName,
        successor: u64,
    },
}

/// The facts of `page`, read after `after`, the projection runs now, and the
/// cursor it stands at once they ran. A lane that ends at `until` takes no
/// fact past it. Core journals a rotation's retirement and its successor's
/// seat in one append, at consecutive cursors: a retirement that ends a page
/// which may have stopped at its limit waits for the next read, which
/// starts right before it, so it is read with the event that follows it.
/// The end of a range is never between the two: it was read as the head of
/// the journal, after a whole append.
fn processable(
    page: &MemberLifecycleJournalPage,
    after: u64,
    until: Option<u64>,
) -> (&[MemberLifecycleFactRecord], u64) {
    if let Some(until) = until {
        let within = page
            .records
            .iter()
            .take_while(|record| record.cursor <= until)
            .count();
        if page.next >= until {
            return (&page.records[..within], until);
        }
    }
    if let Some((last, rest)) = page.records.split_last()
        && matches!(last.fact, MemberLifecycleFact::Retired { .. })
        && last.cursor == page.next
        && page.next.saturating_sub(after) >= PAGE_LIMIT as u64
        && last.cursor > after.saturating_add(1)
    {
        return (rest, last.cursor - 1);
    }
    (&page.records, page.next)
}

/// The durable identity a Core member id names.
fn meta_identity(member: &meerkat_mob::AgentIdentity) -> Option<AgentIdentity> {
    AgentIdentity::parse(&crate::member_comms_id::durable_identity_for_roster_member(
        member.as_str(),
    ))
    .ok()
}

/// Whether the identities a predecessor's backlog owes, as the backlog lane
/// published them, take in `identity`; unread, they may take in any.
fn backlog_owes(
    published: &Option<BTreeMap<AgentIdentity, u64>>,
    identity: &AgentIdentity,
) -> bool {
    published
        .as_ref()
        .is_none_or(|owed| owed.contains_key(identity))
}

/// Wait until the backlog lane ran past every identity of `held`: whether it
/// did (`false` once the projection is gone).
async fn backlog_passed(
    owed: &mut watch::Receiver<Option<BTreeMap<AgentIdentity, u64>>>,
    held: &BTreeSet<AgentIdentity>,
) -> bool {
    loop {
        let pending = {
            let published = owed.borrow_and_update();
            held.iter()
                .any(|identity| backlog_owes(&published, identity))
        };
        if !pending {
            return true;
        }
        if owed.changed().await.is_err() {
            return false;
        }
    }
}

/// The seat of `member` at `generation` follows its retirement at
/// `generation` in the same append: Core rotated it.
fn rotation_successor(
    records: &[MemberLifecycleFactRecord],
    index: usize,
    member: &meerkat_mob::AgentIdentity,
    generation: u64,
) -> Option<u64> {
    let retired = records.get(index)?;
    let next = records.get(index + 1)?;
    if next.cursor != retired.cursor.saturating_add(1) {
        return None;
    }
    match &next.fact {
        MemberLifecycleFact::Seated {
            member: seated,
            generation: successor,
            ..
        } if seated == member && *successor > generation => Some(*successor),
        _ => None,
    }
}

/// The runtime id the retirement hook names for `member`'s incarnation
/// `generation`.
fn incarnation_runtime_id(
    member: &meerkat_mob::AgentIdentity,
    generation: u64,
) -> Option<AgentRuntimeId> {
    AgentRuntimeId::parse(&crate::member_comms_id::public_runtime_alias(
        &meerkat_mob::ids::AgentRuntimeId::new(
            member.clone(),
            meerkat_mob::Generation::new(generation),
        ),
    ))
    .ok()
}

impl IdentityRuntime {
    /// Bind the lifecycle projection to this runtime, with the customizer the
    /// identity context that serves it carries: the context's construction
    /// does. The projection runs its tasks for a bound runtime only.
    pub(crate) fn bind_lifecycle_projection(
        self: &Arc<Self>,
        customizer: Option<Arc<dyn AgentCustomizer>>,
    ) {
        *lock(&self.lifecycle_projection.owner) = Some(Owner {
            runtime: Arc::downgrade(self),
            customizer,
            by_context: true,
        });
    }

    /// Bind the lifecycle projection to this runtime when no identity
    /// context did: the public view an embedder hands the RPC handler, and a
    /// restore without an identity context, do, so such a runtime runs the
    /// side effects of its steps. `customizer` is the one its restore builds
    /// with; without one, `after_create` goes to the runtime's own.
    pub(crate) fn adopt_lifecycle_projection(
        self: &Arc<Self>,
        customizer: Option<Arc<dyn AgentCustomizer>>,
    ) {
        let mut owner = lock(&self.lifecycle_projection.owner);
        if let Some(bound) = owner.as_mut() {
            if !bound.by_context && customizer.is_some() {
                bound.customizer = customizer;
            }
            return;
        }
        *owner = Some(Owner {
            runtime: Arc::downgrade(self),
            customizer,
            by_context: false,
        });
    }

    /// Where the lifecycle projection stands: running, stalled, off and why,
    /// and how many journal events a stopped predecessor's side effects are
    /// still owed for.
    pub fn lifecycle_projection_status(&self) -> LifecycleProjectionStatus {
        let projection = &self.lifecycle_projection;
        let stalled = |detail: String| {
            let mut status = LifecycleProjectionStatus::new(LifecycleProjectionState::Stalled);
            status.detail = Some(detail);
            status
        };
        let mut status = match projection.phase() {
            // A preparation that keeps failing reads stalled: the projection
            // runs nothing until one gets through.
            Phase::Unprepared => match projection.stalled(&[Run::Prepare]) {
                Some(detail) => stalled(detail),
                None => LifecycleProjectionStatus::new(LifecycleProjectionState::Pending),
            },
            Phase::Ready { .. } => match projection
                .stalled(&[Run::Lane(LaneKind::Live), Run::Lane(LaneKind::Backlog)])
            {
                Some(detail) => stalled(detail),
                None => LifecycleProjectionStatus::new(LifecycleProjectionState::Running),
            },
            Phase::Inactive => LifecycleProjectionStatus::new(LifecycleProjectionState::Inactive),
            Phase::Off { detail } => {
                let mut status = LifecycleProjectionStatus::new(LifecycleProjectionState::Off);
                status.detail = Some(detail);
                status
            }
        };
        status.owed_events = projection.owed_events();
        status
    }

    /// One page of the lifecycle side effects the projection recorded as
    /// failed, oldest first: at most `limit` of them, after the first
    /// `skip`. The projection keeps the latest 1024 of the mob's: each start
    /// removes the older ones, and so does a run after every 256 it records.
    pub async fn lifecycle_effect_failures(
        &self,
        skip: usize,
        limit: usize,
    ) -> Result<Vec<LifecycleEffectFailure>, crate::runtime::MetadataStoreError> {
        let (store, mob_id) = self.identity_record_store();
        store.lifecycle_effect_failures(&mob_id, skip, limit).await
    }

    /// Read the projection's cursor, or start it at the journal's head when
    /// the store holds none for the mob's current incarnation; the events a
    /// stopped predecessor left behind its cursor become the backlog. Called
    /// before every step Meta asks Core for, so every such step is
    /// projected; cheap once prepared. A candidate launch and a runtime
    /// without a session bridge project nothing; a bridge that cannot read
    /// Core's journal turns the projection off. A failure says whether a
    /// cursor is stored already, and counts toward the stall the status
    /// reports.
    pub(super) async fn prepare_lifecycle_projection(&self) -> Result<(), UnpreparedProjection> {
        let projection = &self.lifecycle_projection;
        if !matches!(projection.phase(), Phase::Unprepared) {
            return Ok(());
        }
        let _preparing = projection.preparing.lock().await;
        if !matches!(projection.phase(), Phase::Unprepared) {
            return Ok(());
        }
        let prepared = self.prepare_lifecycle_projection_locked().await;
        projection.note_run(
            Run::Prepare,
            prepared
                .as_ref()
                .err()
                .map(|unprepared| unprepared.detail.as_str()),
        );
        prepared
    }

    /// [`Self::prepare_lifecycle_projection`], under its lock.
    async fn prepare_lifecycle_projection_locked(&self) -> Result<(), UnpreparedProjection> {
        let projection = &self.lifecycle_projection;
        if self.is_candidate_launch() {
            projection.set_phase(Phase::Inactive);
            return Ok(());
        }
        let Some(bridge) = self.bridge.as_ref() else {
            projection.set_phase(Phase::Inactive);
            return Ok(());
        };
        // The stored cursor first: once one of the mob's current incarnation
        // is stored, a step Core journals lies past it whatever else fails
        // here, so a seat step goes ahead.
        let (store, mob_id) = self.identity_record_store();
        let stored = store
            .lifecycle_projection_cursor(&mob_id)
            .await
            .map_err(|error| UnpreparedProjection {
                detail: format!("reading the lifecycle projection's cursor: {error}"),
                cursor_stored: false,
            })?;
        let head = match bridge.member_lifecycle_journal_head().await {
            Ok(head) => head,
            Err(BridgeError::Unsupported { .. }) => {
                self.turn_lifecycle_projection_off(
                    "read the head of Core's member lifecycle journal",
                );
                return Ok(());
            }
            Err(error) => {
                // Without the head, the bridge's own answer says which
                // incarnation of the mob runs: a cursor of another one (the
                // mob was recreated since), or one the bridge cannot place,
                // makes no step safe.
                let current = bridge.mob_incarnation();
                return Err(UnpreparedProjection {
                    detail: format!("reading the head of Core's member lifecycle journal: {error}"),
                    cursor_stored: stored.as_ref().is_some_and(|stored| {
                        current.as_deref() == Some(stored.incarnation.as_str())
                    }),
                });
            }
        };
        let cursor_stored = stored
            .as_ref()
            .is_some_and(|stored| stored.incarnation == head.incarnation);
        let unprepared = |detail: String| UnpreparedProjection {
            detail,
            cursor_stored,
        };
        // A bridge that reads the head may still read no page.
        match bridge.member_lifecycle_journal_page(head.cursor, 1).await {
            Ok(_) => {}
            Err(BridgeError::Unsupported { .. }) => {
                self.turn_lifecycle_projection_off(
                    "read the pages of Core's member lifecycle journal",
                );
                return Ok(());
            }
            Err(error) => {
                return Err(unprepared(format!(
                    "reading Core's member lifecycle journal after its head: {error}"
                )));
            }
        }
        // With no cursor yet (a new mob, or one an earlier release ran, which
        // owed its side effects in its own process), or one of an earlier
        // incarnation of the mob, whose journal is gone, the projection
        // starts at the journal's head.
        let mut record = Record {
            cursor: head.cursor,
            ..Record::default()
        };
        if let Some(stored) = stored
            && stored.incarnation == head.incarnation
        {
            record.uncleared = stored.uncleared.into_iter().collect();
            if stored.cursor > head.cursor {
                // A mob store older than the cursor: what the cursor names
                // past the head is gone.
                tracing::warn!(
                    runtime_instance_id = %self.runtime_instance_id,
                    stored = stored.cursor,
                    head = head.cursor,
                    "the lifecycle projection's cursor is past the head of Core's journal; the \
                     projection starts at the head"
                );
            } else {
                record.backlog = stored.backlog;
                if stored.cursor < head.cursor {
                    // What a stopped predecessor still owed: it runs in the
                    // background.
                    record
                        .backlog
                        .push(LifecycleProjectionBacklog::new(stored.cursor, head.cursor));
                }
            }
        }
        // Which identities a backlog owes is read when a lane first needs
        // it; with no backlog, it owes none.
        let nothing_owed = record.backlog.is_empty();
        *lock(&projection.record) = record;
        self.write_lifecycle_record(&head.incarnation)
            .await
            .map_err(unprepared)?;
        *projection.live.lock().await = Lane {
            cursor: head.cursor,
            ..Lane::default()
        };
        projection
            .owed
            .send_replace(nothing_owed.then(BTreeMap::new));
        projection.set_phase(Phase::Ready {
            incarnation: head.incarnation.clone(),
        });
        self.clear_uncleared_lifecycle_intents().await;
        self.sweep_stale_lifecycle_intents(&head.incarnation).await;
        self.prune_lifecycle_effect_failures().await;
        Ok(())
    }

    /// The projection is off: the session bridge cannot `read`. Reported
    /// once.
    fn turn_lifecycle_projection_off(&self, read: &str) {
        let detail = format!(
            "the session bridge cannot {read}, so no lifecycle side effect runs: no customizer \
             after_create, no retirement hook, no distillation or exit interview of a retired \
             incarnation. Implement SessionBridge::member_lifecycle_journal_head and \
             member_lifecycle_journal_page, and the binding reads, or forward them to Meta's \
             MobSessionBridge"
        );
        {
            let mut phase = lock(&self.lifecycle_projection.phase);
            if matches!(*phase, Phase::Off { .. }) {
                return;
            }
            *phase = Phase::Off {
                detail: detail.clone(),
            };
        }
        self.emit_error(ErrorEvent::LifecycleProjectionOff { detail });
    }

    /// Start the projection's tasks for a bound runtime that is not shutting
    /// down: the live one, once, which prepares the projection at each sweep
    /// until a preparation gets through; the backlog one while a
    /// predecessor's side effects are owed.
    pub(crate) fn start_lifecycle_projection(&self) {
        let projection = &self.lifecycle_projection;
        if *projection.stopping.borrow()
            || matches!(projection.phase(), Phase::Inactive | Phase::Off { .. })
        {
            return;
        }
        let Some(owner) = lock(&projection.owner).clone() else {
            return;
        };
        {
            let mut task = lock(&projection.live_task);
            if task.is_none() {
                let kick = Arc::clone(&projection.kick);
                let mut stopping = projection.stopping.subscribe();
                let runtime = owner.runtime.clone();
                *task = Some(tokio::spawn(async move {
                    loop {
                        let Some(identities) = runtime.upgrade() else {
                            return;
                        };
                        // A preparation that failed is tried again here, so a
                        // step that went ahead past a stored cursor is
                        // projected without waiting for another step.
                        if let Err(error) = identities.prepare_lifecycle_projection().await {
                            tracing::warn!(
                                runtime_instance_id = %identities.runtime_instance_id,
                                %error,
                                "the lifecycle projection could not be prepared; it is tried \
                                 again at the next sweep"
                            );
                        }
                        let mut held = None;
                        match identities.lifecycle_projection.phase() {
                            Phase::Ready { .. } => {
                                if let Some(LaneEnd::Held(held_identities)) =
                                    identities.run_logged_lifecycle_lane(LaneKind::Live).await
                                {
                                    held = Some(held_identities);
                                }
                                // The backlog task, for a backlog the
                                // preparation found.
                                identities.start_lifecycle_projection();
                            }
                            Phase::Unprepared => {}
                            Phase::Inactive | Phase::Off { .. } => return,
                        }
                        let mut owed = identities.lifecycle_projection.owed.subscribe();
                        drop(identities);
                        if *stopping.borrow() {
                            return;
                        }
                        match held {
                            // A page held for a predecessor's backlog runs
                            // again once the backlog ran past every identity
                            // it held, or at the next sweep; the steps
                            // journaled after it follow it.
                            Some(held) => tokio::select! {
                                passed = backlog_passed(&mut owed, &held) => {
                                    if !passed {
                                        return;
                                    }
                                }
                                () = tokio::time::sleep(SWEEP_INTERVAL) => {}
                                changed = stopping.changed() => {
                                    if changed.is_err() {
                                        return;
                                    }
                                }
                            },
                            None => tokio::select! {
                                () = kick.notified() => {}
                                () = tokio::time::sleep(SWEEP_INTERVAL) => {}
                                changed = stopping.changed() => {
                                    if changed.is_err() {
                                        return;
                                    }
                                }
                            },
                        }
                    }
                }));
            }
        }
        if !matches!(projection.phase(), Phase::Ready { .. }) || projection.owed_events() == 0 {
            return;
        }
        let mut task = lock(&projection.backlog_task);
        if task.as_ref().is_some_and(|task| !task.is_finished()) {
            return;
        }
        let mut stopping = projection.stopping.subscribe();
        let runtime = owner.runtime;
        *task = Some(tokio::spawn(async move {
            loop {
                let Some(identities) = runtime.upgrade() else {
                    return;
                };
                let drained = matches!(
                    identities
                        .run_logged_lifecycle_lane(LaneKind::Backlog)
                        .await,
                    Some(LaneEnd::Reached)
                ) && identities.lifecycle_projection.owed_events() == 0;
                let off = !matches!(identities.lifecycle_projection.phase(), Phase::Ready { .. });
                drop(identities);
                if drained || off || *stopping.borrow() {
                    return;
                }
                tokio::select! {
                    () = tokio::time::sleep(SWEEP_INTERVAL) => {}
                    changed = stopping.changed() => {
                        if changed.is_err() {
                            return;
                        }
                    }
                }
            }
        }));
    }

    /// A step Meta asked Core for was journaled: wake the projection, and
    /// start it when it is bound and has not started yet.
    pub(super) fn kick_lifecycle_projection(&self) {
        self.lifecycle_projection.kick.notify_one();
        self.start_lifecycle_projection();
    }

    /// Run the side effects of every lifecycle step Core journaled since this
    /// process started, here: the live lane reaches the journal's head
    /// before this answers, or stops at a failure, which is logged and read
    /// again at its next sweep. What a stopped predecessor still owed runs
    /// in the background, and never holds this up: a page with side effects
    /// of an identity that backlog still owes is left to the live task,
    /// which runs it once the backlog ran past that identity, and the steps
    /// journaled after it follow it there. Never call it from a retirement
    /// hook or a customizer's `after_create`: it would wait for the side
    /// effect that is running it.
    pub(crate) async fn settle_lifecycle_projection(&self) {
        if let Err(error) = self.prepare_lifecycle_projection().await {
            tracing::warn!(
                runtime_instance_id = %self.runtime_instance_id,
                %error,
                "the lifecycle projection could not be prepared; it is tried again at its next \
                 sweep or its next step"
            );
            self.start_lifecycle_projection();
            return;
        }
        self.run_logged_lifecycle_lane(LaneKind::Live).await;
        // The live task takes up a page this run held.
        self.kick_lifecycle_projection();
    }

    /// Stop the projection at a shutdown: its live task reaches the journal's
    /// head once more and ends, the backlog task ends at its next page, and
    /// the side tasks end, by `deadline` at the latest. What they did not
    /// reach runs at the next start, from the record the store holds.
    pub(crate) async fn stop_lifecycle_projection(&self, deadline: tokio::time::Instant) {
        let projection = &self.lifecycle_projection;
        projection.stopping.send_replace(true);
        projection.kick.notify_one();
        let mut tasks = Vec::new();
        tasks.extend(lock(&projection.live_task).take());
        tasks.extend(lock(&projection.backlog_task).take());
        tasks.extend(std::mem::take(&mut *lock(&projection.side_tasks)));
        let mut unfinished = 0_usize;
        for mut task in tasks {
            if tokio::time::timeout_at(deadline, &mut task).await.is_err() {
                unfinished += 1;
                task.abort();
                let _ = task.await;
            }
        }
        if unfinished > 0 {
            tracing::warn!(
                runtime_instance_id = %self.runtime_instance_id,
                unfinished,
                "the lifecycle projection did not finish within its share of the shutdown's \
                 window; what it did not reach runs at the next start"
            );
        }
    }

    /// Run `side_effect` beside the projection, joined by a shutdown with the
    /// projection's tasks: a side effect a verb owes that no journal fact
    /// carries.
    pub(super) fn spawn_lifecycle_side_task(
        &self,
        side_effect: impl Future<Output = ()> + Send + 'static,
    ) {
        let projection = &self.lifecycle_projection;
        let mut tasks = lock(&projection.side_tasks);
        tasks.retain(|task| !task.is_finished());
        tasks.push(tokio::spawn(side_effect));
    }

    /// Before Core is asked for a step that ends `identity`'s live
    /// incarnation (`verb`): record what the step owes, keyed by that
    /// incarnation, and, when its retirement owes the retirement hook
    /// ([`intent_cause`] names a cause for `verb`), keep the incarnation as
    /// the hook reads it. `None` when Core holds no live member the identity
    /// spawned, so the step ends nothing the identity owns. Refused typed,
    /// with nothing asked of Core, when the record cannot be written.
    pub(super) async fn prepare_lifecycle_retirement(
        &self,
        identity: &AgentIdentity,
        verb: LifecycleEffectVerb,
    ) -> Result<Option<PreparedRetirement>, IdentityRuntimeError> {
        let binding = self.core_last_binding(identity).await?;
        self.prepare_lifecycle_retirement_of(identity, binding, verb)
            .await
    }

    /// [`Self::prepare_lifecycle_retirement`] of `binding`, the identity's
    /// last binding the step itself read, so the step and its record read
    /// Core's liveness once.
    pub(super) async fn prepare_lifecycle_retirement_of(
        &self,
        identity: &AgentIdentity,
        binding: Option<meerkat_mob::MemberBinding>,
        verb: LifecycleEffectVerb,
    ) -> Result<Option<PreparedRetirement>, IdentityRuntimeError> {
        // A member a host made directly on the mob under the identity's
        // member id is not the identity's: its end owes the identity
        // nothing.
        let Some(binding) = binding
            .filter(meerkat_mob::MemberBinding::is_live)
            .filter(|binding| binding_spawned_for_identity(binding, identity))
        else {
            return Ok(None);
        };
        let generation = binding.generation().get();
        let session_id = binding.session_id().cloned();
        let retiring = match intent_cause(verb) {
            Some(cause) => self.retiring_member(identity, &binding, cause).await,
            None => None,
        };
        let recorded = self
            .record_lifecycle_effect_intent(identity, generation, verb, session_id)
            .await
            .map_err(|detail| IdentityRuntimeError::IdentityHistoryUnavailable {
                identity: identity.clone(),
                detail: format!(
                    "nothing was asked of Core: the side effects the step owes could not be \
                     recorded ({detail})"
                ),
            })?;
        if let Some(member) = retiring.clone() {
            self.lifecycle_projection.stash_retiring(member);
        }
        Ok(Some(PreparedRetirement {
            identity: identity.clone(),
            generation,
            recorded,
            retiring,
        }))
    }

    /// Core answered the step `prepared` prepared: a registration still
    /// current now was not ended by it, so the hook will name none; then wake
    /// the projection.
    pub(super) async fn settle_lifecycle_retirement(&self, prepared: Option<PreparedRetirement>) {
        if let Some(PreparedRetirement {
            retiring: Some(member),
            ..
        }) = prepared
        {
            let mut retiring = Some(member);
            self.settle_retired_registration(&mut retiring).await;
            if let Some(member) = retiring {
                self.lifecycle_projection.refine_retiring(member);
            }
        }
        self.kick_lifecycle_projection();
    }

    /// Core refused the step `prepared` prepared before any of it started:
    /// its record goes, the one an earlier step recorded for the same
    /// incarnation is put back, and what this process kept of it goes.
    pub(super) async fn abandon_lifecycle_retirement(&self, prepared: Option<PreparedRetirement>) {
        let Some(prepared) = prepared else {
            return;
        };
        let _ = self
            .lifecycle_projection
            .take_retiring(&prepared.identity, prepared.generation);
        let Some(earlier) = prepared.recorded else {
            return;
        };
        let (store, mob_id) = self.identity_record_store();
        let restored = match earlier {
            Some(earlier) => store.set_lifecycle_effect_intent(&earlier).await,
            None => {
                store
                    .clear_lifecycle_effect_intent(
                        &mob_id,
                        prepared.identity.as_str(),
                        prepared.generation,
                    )
                    .await
            }
        };
        if let Err(error) = restored {
            tracing::warn!(
                identity = %prepared.identity,
                generation = prepared.generation,
                %error,
                "the lifecycle effect intent of a step Core refused could not be taken back; \
                 the next step that ends the same incarnation records its own in its place"
            );
        }
    }

    /// Keep what the build of a seat-like step of `identity` made, which the
    /// step is about to ask Core for, for the `after_create` the projection
    /// tells once it reaches the seat.
    pub(super) fn note_lifecycle_build(
        &self,
        identity: &AgentIdentity,
        context: SessionCreatedContext,
    ) {
        lock(&self.lifecycle_projection.building).insert(identity.clone(), context);
    }

    /// Core answered the step [`Self::note_lifecycle_build`] kept a build for
    /// with a seat on `session`: the build is the one that seat's
    /// `after_create` reports. Called before the step releases the
    /// identity's lifecycle lock.
    pub(super) fn bind_lifecycle_build(&self, identity: &AgentIdentity, session: &SessionId) {
        let projection = &self.lifecycle_projection;
        if let Some(context) = lock(&projection.building).remove(identity) {
            lock(&projection.built).insert((identity.clone(), session.clone()), context);
        }
    }

    /// The step [`Self::note_lifecycle_build`] kept a build for seated
    /// nothing.
    pub(super) fn forget_lifecycle_build(&self, identity: &AgentIdentity) {
        lock(&self.lifecycle_projection.building).remove(identity);
    }

    /// A roster pass lists `roster`: an unlisting whose retirement Core never
    /// carried out (Core still holds the incarnation it named live) is taken
    /// back, so a later retirement of that incarnation that no Meta verb
    /// asked for owes nothing.
    pub(super) async fn take_back_relisted_unlistings(&self, roster: &[DurableAgentSpec]) {
        let Phase::Ready { incarnation } = self.lifecycle_projection.phase() else {
            return;
        };
        let (store, mob_id) = self.identity_record_store();
        let intents = match store.lifecycle_effect_intents(&mob_id).await {
            Ok(intents) => intents,
            Err(error) => {
                tracing::warn!(
                    %error,
                    "the lifecycle effect intents could not be listed; an unlisting the roster \
                     undid is taken back by a later pass"
                );
                return;
            }
        };
        let listed = roster
            .iter()
            .map(|spec| &spec.identity)
            .collect::<BTreeSet<_>>();
        for intent in intents.into_iter().filter(|intent| {
            intent.verb == LifecycleEffectVerb::Unlisted && intent.incarnation == incarnation
        }) {
            let Ok(identity) = AgentIdentity::parse(&intent.identity) else {
                continue;
            };
            if !listed.contains(&identity) {
                continue;
            }
            let live = match self.core_last_binding(&identity).await {
                Ok(binding) => binding.filter(meerkat_mob::MemberBinding::is_live),
                Err(error) => {
                    tracing::warn!(%identity, %error, "Core's binding of a relisted identity did not read");
                    continue;
                }
            };
            if live.is_some_and(|binding| binding.generation().get() == intent.generation)
                && let Err(error) = store
                    .clear_lifecycle_effect_intent(&mob_id, &intent.identity, intent.generation)
                    .await
            {
                tracing::warn!(
                    %identity,
                    generation = intent.generation,
                    %error,
                    "the unlisting of a relisted identity could not be taken back"
                );
            }
        }
    }

    /// Record what the step that ends `identity`'s incarnation `generation`
    /// owes (`verb`), durably, before Core is asked for it, with the intent
    /// it replaces. `Ok(None)` when nothing is projected, or nothing would
    /// consume the record (no identity context or public view bound the
    /// projection), so nothing is recorded.
    async fn record_lifecycle_effect_intent(
        &self,
        identity: &AgentIdentity,
        generation: u64,
        verb: LifecycleEffectVerb,
        session_id: Option<SessionId>,
    ) -> Result<Option<Option<LifecycleEffectIntent>>, String> {
        self.prepare_lifecycle_projection()
            .await
            .map_err(|unprepared| unprepared.detail)?;
        let Phase::Ready { incarnation } = self.lifecycle_projection.phase() else {
            return Ok(None);
        };
        if lock(&self.lifecycle_projection.owner).is_none() {
            if !self
                .lifecycle_projection
                .unbound_said
                .swap(true, Ordering::SeqCst)
            {
                tracing::warn!(
                    runtime_instance_id = %self.runtime_instance_id,
                    "no identity context or public view serves this identity runtime: the \
                     lifecycle side effects of its steps are not recorded or run"
                );
            }
            return Ok(None);
        }
        let (store, mob_id) = self.identity_record_store();
        let earlier = store
            .lifecycle_effect_intent(&mob_id, identity.as_str(), generation)
            .await
            .map_err(|error| {
                format!("reading the incarnation's lifecycle effect intent: {error}")
            })?;
        store
            .set_lifecycle_effect_intent(&LifecycleEffectIntent::new(
                mob_id,
                incarnation,
                identity.as_str(),
                generation,
                verb,
                session_id,
                unix_ms_now(),
            ))
            .await
            .map_err(|error| format!("recording the step's lifecycle effect intent: {error}"))?;
        Ok(Some(earlier))
    }

    /// Run `kind`'s lane, logging what stopped it short and counting it
    /// toward the stall the status reports: how it ended, `None` when it
    /// failed.
    async fn run_logged_lifecycle_lane(&self, kind: LaneKind) -> Option<LaneEnd> {
        let outcome = self.run_lifecycle_lane(kind).await;
        self.lifecycle_projection
            .note_run(Run::Lane(kind), outcome.as_ref().err().map(String::as_str));
        match outcome {
            Ok(end) => Some(end),
            Err(error) => {
                tracing::warn!(
                    runtime_instance_id = %self.runtime_instance_id,
                    %error,
                    "the lifecycle projection stopped short; it reads from where it stopped at \
                     its next sweep"
                );
                None
            }
        }
    }

    /// Read the journal for `kind`'s lane from its cursor to its end (the
    /// head, for the live lane; the owed ranges, for the backlog lane),
    /// running each page's side effects and then recording where the lane
    /// stands. The live lane holds a page with side effects of an identity a
    /// predecessor's backlog still owes, and ends there. A lane's runs are
    /// serialized.
    async fn run_lifecycle_lane(&self, kind: LaneKind) -> Result<LaneEnd, String> {
        let projection = &self.lifecycle_projection;
        let Phase::Ready { incarnation } = projection.phase() else {
            return Ok(LaneEnd::Reached);
        };
        let Some(bridge) = self.bridge.clone() else {
            return Ok(LaneEnd::Reached);
        };
        let mut lane = projection.lane(kind).lock().await;
        // The live lane moved past pages with no fact since its last write.
        let mut unwritten = false;
        loop {
            if kind == LaneKind::Backlog {
                if *projection.stopping.borrow() {
                    return Ok(LaneEnd::Reached);
                }
                // The first owed range is the one the lane reads.
                let Some(range) = lock(&projection.record).backlog.first().copied() else {
                    return Ok(LaneEnd::Reached);
                };
                if lane.until != Some(range.until) || lane.cursor < range.after {
                    *lane = Lane {
                        cursor: range.after,
                        until: Some(range.until),
                        ..Lane::default()
                    };
                }
            }
            let after = lane.cursor;
            let page = match bridge
                .member_lifecycle_journal_page(after, PAGE_LIMIT)
                .await
            {
                Ok(page) => page,
                Err(BridgeError::Unsupported { .. }) => {
                    self.turn_lifecycle_projection_off(
                        "read the pages of Core's member lifecycle journal",
                    );
                    return Ok(LaneEnd::Reached);
                }
                Err(error) => {
                    return Err(format!(
                        "reading Core's member lifecycle journal after {after}: {error}"
                    ));
                }
            };
            let (records, upto) = if page.next > after {
                processable(&page, after, lane.until)
            } else if let Some(until) = lane.until {
                // An owed range past what the journal holds: nothing of it
                // is left to read.
                tracing::warn!(
                    after,
                    until,
                    "an owed range of the lifecycle projection lies past Core's journal"
                );
                (&page.records[..0], until)
            } else {
                // The live lane is at the journal's head.
                if unwritten {
                    self.write_lifecycle_record(&incarnation).await?;
                }
                return Ok(LaneEnd::Reached);
            };
            let consumed = match self
                .apply_lifecycle_page(kind, &incarnation, records, &mut lane.done)
                .await
            {
                Ok(consumed) => consumed,
                Err(PageError::Unreadable(read)) => {
                    self.turn_lifecycle_projection_off(read);
                    return Ok(LaneEnd::Reached);
                }
                // Side effects on the page follow a predecessor's backlog,
                // which has not run past their identities yet: the page
                // runs again once it has, without the side effects that
                // ran, and the lane ends here.
                Err(PageError::Held(held)) => {
                    if unwritten {
                        self.write_lifecycle_record(&incarnation).await?;
                    }
                    return Ok(LaneEnd::Held(held));
                }
                Err(PageError::Failed(error)) => return Err(error),
            };
            lane.cursor = upto;
            lane.done.retain(|cursor| *cursor > upto);
            {
                let mut record = lock(&projection.record);
                match kind {
                    LaneKind::Live => record.cursor = upto,
                    LaneKind::Backlog => {
                        let finished = lane.until.is_some_and(|until| upto >= until);
                        if finished {
                            if !record.backlog.is_empty() {
                                record.backlog.remove(0);
                            }
                        } else if let Some(first) = record.backlog.first_mut() {
                            first.after = upto;
                        }
                    }
                }
                record.uncleared.extend(consumed);
            }
            if kind == LaneKind::Backlog {
                // The backlog ran every side effect up to `upto`: an
                // identity it owes nothing after that is the live lane's.
                projection.owed.send_modify(|owed| {
                    if let Some(owed) = owed {
                        owed.retain(|_, last| *last > upto);
                    }
                });
            }
            if kind == LaneKind::Live && records.is_empty() {
                // A page with no fact moved the cursor only: it is written
                // with the next page that has one, or when the lane reaches
                // the head.
                unwritten = true;
                continue;
            }
            unwritten = false;
            self.write_lifecycle_record(&incarnation).await?;
            self.clear_uncleared_lifecycle_intents().await;
        }
    }

    /// Write the projection's record as both lanes left it.
    async fn write_lifecycle_record(&self, incarnation: &str) -> Result<(), String> {
        let projection = &self.lifecycle_projection;
        let _writing = projection.writing.lock().await;
        let snapshot = lock(&projection.record).clone();
        let (store, mob_id) = self.identity_record_store();
        let mut cursor =
            LifecycleProjectionCursor::new(mob_id, incarnation, snapshot.cursor, unix_ms_now());
        cursor.backlog = snapshot.backlog;
        cursor.uncleared = snapshot.uncleared.into_iter().collect();
        store
            .set_lifecycle_projection_cursor(&cursor)
            .await
            .map_err(|error| format!("recording the lifecycle projection's cursor: {error}"))
    }

    /// Remove the intents of the retirements whose side effects ran, which
    /// the recorded cursor covers; one that cannot be removed now is tried
    /// again after the next write, and at the next start.
    async fn clear_uncleared_lifecycle_intents(&self) {
        let keys = lock(&self.lifecycle_projection.record)
            .uncleared
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        if keys.is_empty() {
            return;
        }
        let (store, mob_id) = self.identity_record_store();
        for key in keys {
            match store
                .clear_lifecycle_effect_intent(&mob_id, &key.identity, key.generation)
                .await
            {
                Ok(()) => {
                    lock(&self.lifecycle_projection.record)
                        .uncleared
                        .remove(&key);
                }
                Err(error) => tracing::warn!(
                    identity = %key.identity,
                    generation = key.generation,
                    %error,
                    "the lifecycle effect intent of a step whose side effects ran could not be \
                     removed; it is tried again"
                ),
            }
        }
    }

    /// Remove the intents an earlier incarnation of the mob left, at most
    /// [`STALE_INTENT_SWEEP`]: no journal this runtime reads names their
    /// steps.
    async fn sweep_stale_lifecycle_intents(&self, incarnation: &str) {
        let (store, mob_id) = self.identity_record_store();
        let intents = match store.lifecycle_effect_intents(&mob_id).await {
            Ok(intents) => intents,
            Err(error) => {
                tracing::warn!(%error, "the lifecycle effect intents could not be listed for the sweep");
                return;
            }
        };
        for intent in intents
            .into_iter()
            .filter(|intent| intent.incarnation != incarnation)
            .take(STALE_INTENT_SWEEP)
        {
            if let Err(error) = store
                .clear_lifecycle_effect_intent(&mob_id, &intent.identity, intent.generation)
                .await
            {
                tracing::warn!(
                    identity = %intent.identity,
                    generation = intent.generation,
                    %error,
                    "a lifecycle effect intent of an earlier incarnation of the mob could not be \
                     removed"
                );
            }
        }
    }

    /// Keep the latest [`FAILURES_KEPT`] lifecycle effect failures of the
    /// mob and remove the older ones: one is recorded per failed side
    /// effect, so a hook that fails every retirement would otherwise leave
    /// one per retirement for good.
    async fn prune_lifecycle_effect_failures(&self) {
        let (store, mob_id) = self.identity_record_store();
        match store
            .prune_lifecycle_effect_failures(&mob_id, FAILURES_KEPT)
            .await
        {
            Ok(0) => {}
            Ok(removed) => tracing::debug!(
                removed,
                "the oldest lifecycle effect failures of the mob were removed"
            ),
            Err(error) => tracing::warn!(
                %error,
                "the lifecycle effect failures of the mob could not be pruned; the next start \
                 tries again"
            ),
        }
    }

    /// Run the side effects of `records`, `kind`'s lane's, skipping those
    /// `done` names and adding those that run to it. Answers the intents the
    /// page's retirements consumed, for removal once the cursor past them is
    /// recorded. A side effect that fails is recorded and reported, and the
    /// page goes on; a read the page depends on that fails fails the page,
    /// which runs again. The live lane runs an identity's side effects only
    /// once the backlog lane ran those a predecessor still owed it: until
    /// then the page is held, after the side effects of the other
    /// identities ran.
    async fn apply_lifecycle_page(
        &self,
        kind: LaneKind,
        incarnation: &str,
        records: &[MemberLifecycleFactRecord],
        done: &mut BTreeSet<u64>,
    ) -> Result<Vec<LifecycleIntentKey>, PageError> {
        if records.is_empty() {
            return Ok(Vec::new());
        }
        // Which identities a predecessor's backlog owes is read before
        // either lane runs a side effect.
        self.scan_owed_lifecycle_backlog().await?;
        let customizer = self.after_create_customizer().await;
        let mut effects: Vec<(u64, AgentIdentity, Effect)> = Vec::new();
        let mut pending: Vec<Pending> = Vec::new();
        let mut consumed = Vec::new();
        let (store, mob_id) = self.identity_record_store();
        for (index, record) in records.iter().enumerate() {
            match &record.fact {
                MemberLifecycleFact::Seated {
                    member, generation, ..
                } => {
                    // Nothing to tell without a customizer.
                    if customizer.is_none() {
                        continue;
                    }
                    let Some(identity) = meta_identity(member) else {
                        continue;
                    };
                    pending.push(Pending::Seat {
                        cursor: record.cursor,
                        identity,
                        member: member.clone(),
                        generation: *generation,
                    });
                }
                MemberLifecycleFact::Retired {
                    member,
                    generation,
                    role,
                    ..
                } => {
                    let Some(identity) = meta_identity(member) else {
                        continue;
                    };
                    let intent = store
                        .lifecycle_effect_intent(&mob_id, identity.as_str(), *generation)
                        .await
                        .map_err(|error| {
                            format!(
                                "reading the lifecycle effect intent of {identity} at \
                                 generation {generation}: {error}"
                            )
                        })?;
                    if let Some(intent) = intent.as_ref() {
                        consumed.push(intent.key());
                    }
                    // An intent of an earlier incarnation of the mob names
                    // another journal's step.
                    match intent.filter(|intent| intent.incarnation == incarnation) {
                        Some(intent) => {
                            let Some(cause) = intent_cause(intent.verb) else {
                                continue;
                            };
                            effects.push((
                                record.cursor,
                                identity,
                                Effect::Retirement(Retirement {
                                    member: member.clone(),
                                    generation: *generation,
                                    role: role.clone(),
                                    session: intent.session_id,
                                    cause,
                                    asked_at_ms: Some(intent.recorded_at_ms),
                                }),
                            ));
                        }
                        None => {
                            // A retirement no Meta verb asked for owes
                            // nothing, unless Core journaled it with its
                            // successor's seat: a rotation, read from Core's
                            // member history.
                            if let Some(successor) =
                                rotation_successor(records, index, member, *generation)
                            {
                                pending.push(Pending::Rotation {
                                    cursor: record.cursor,
                                    identity,
                                    member: member.clone(),
                                    generation: *generation,
                                    role: role.clone(),
                                    successor,
                                });
                            }
                        }
                    }
                }
                MemberLifecycleFact::RetirementStarted { .. }
                | MemberLifecycleFact::EntryAdopted { .. }
                | MemberLifecycleFact::LegacyReset { .. }
                | MemberLifecycleFact::MobReset => {}
            }
        }
        if !pending.is_empty() {
            self.resolve_pending_lifecycle_facts(pending, &mut effects)
                .await?;
        }
        // One identity's side effects in journal order; different
        // identities' at most EFFECT_CONCURRENCY at once.
        effects.sort_by_key(|(cursor, _, _)| *cursor);
        let mut jobs: BTreeMap<AgentIdentity, Vec<(u64, Effect)>> = BTreeMap::new();
        for (cursor, identity, effect) in effects {
            if done.contains(&cursor) {
                continue;
            }
            jobs.entry(identity).or_default().push((cursor, effect));
        }
        let outcomes = stream::iter(jobs.into_iter().map(|(identity, effects)| {
            let customizer = customizer.clone();
            async move {
                if kind == LaneKind::Live && self.lifecycle_projection.still_owed(&identity) {
                    return (Vec::new(), Err(JobStop::Held(identity)));
                }
                let mut ran = Vec::with_capacity(effects.len());
                for (cursor, effect) in effects {
                    let owed = Owed {
                        incarnation,
                        cursor,
                    };
                    if let Err(error) = self
                        .run_lifecycle_effect(&identity, effect, customizer.as_ref(), owed)
                        .await
                    {
                        return (ran, Err(JobStop::Failed(error)));
                    }
                    ran.push(cursor);
                }
                (ran, Ok(()))
            }
        }))
        .buffer_unordered(EFFECT_CONCURRENCY)
        .collect::<Vec<_>>()
        .await;
        let mut failure = None;
        let mut held = BTreeSet::new();
        for (ran, outcome) in outcomes {
            done.extend(ran);
            match outcome {
                Ok(()) => {}
                Err(JobStop::Held(identity)) => {
                    held.insert(identity);
                }
                Err(JobStop::Failed(error)) => {
                    failure.get_or_insert(error);
                }
            }
        }
        if let Some(error) = failure {
            return Err(PageError::Failed(error));
        }
        if !held.is_empty() {
            return Err(PageError::Held(held));
        }
        Ok(consumed)
    }

    /// Read, once per start, which identities a predecessor's backlog owes
    /// side effects, and the last journal event that owes each one: a read
    /// of the owed ranges' pages, nothing more. What the backlog lane ran
    /// meanwhile is not owed.
    async fn scan_owed_lifecycle_backlog(&self) -> Result<(), PageError> {
        let projection = &self.lifecycle_projection;
        if projection.owed.borrow().is_some() {
            return Ok(());
        }
        let _scanning = projection.scanning.lock().await;
        if projection.owed.borrow().is_some() {
            return Ok(());
        }
        let Some(bridge) = self.bridge.as_ref() else {
            return Ok(());
        };
        let ranges = lock(&projection.record).backlog.clone();
        let mut owed = BTreeMap::new();
        for range in ranges {
            let mut after = range.after;
            while after < range.until {
                let page = match bridge
                    .member_lifecycle_journal_page(after, PAGE_LIMIT)
                    .await
                {
                    Ok(page) => page,
                    Err(BridgeError::Unsupported { .. }) => {
                        return Err(PageError::Unreadable(
                            "read the pages of Core's member lifecycle journal",
                        ));
                    }
                    Err(error) => {
                        return Err(PageError::Failed(format!(
                            "reading which identities the backlog owes, in Core's member \
                             lifecycle journal after {after}: {error}"
                        )));
                    }
                };
                // A range past what the journal holds has nothing left.
                if page.next <= after {
                    break;
                }
                for record in page
                    .records
                    .iter()
                    .take_while(|record| record.cursor <= range.until)
                {
                    let member = match &record.fact {
                        MemberLifecycleFact::Seated { member, .. }
                        | MemberLifecycleFact::Retired { member, .. } => member,
                        MemberLifecycleFact::RetirementStarted { .. }
                        | MemberLifecycleFact::EntryAdopted { .. }
                        | MemberLifecycleFact::LegacyReset { .. }
                        | MemberLifecycleFact::MobReset => continue,
                    };
                    if let Some(identity) = meta_identity(member) {
                        owed.insert(identity, record.cursor);
                    }
                }
                after = page.next;
            }
        }
        projection.owed.send_modify(|published| {
            if published.is_none() {
                // The backlog lane ran every event before the first owed
                // range's start, and the backlog is done once no range is
                // left.
                let ran_until = lock(&projection.record)
                    .backlog
                    .first()
                    .map(|range| range.after);
                owed.retain(|_, last| ran_until.is_some_and(|after| *last > after));
                *published = Some(std::mem::take(&mut owed));
            }
        });
        Ok(())
    }

    /// The side effects of the seats and the unrecorded rotations of a page,
    /// from one read of Core's member history: Core answers a read of one
    /// identity's binding by folding the whole history, so the page reads
    /// the fold once and looks each seat's member up in it.
    async fn resolve_pending_lifecycle_facts(
        &self,
        pending: Vec<Pending>,
        effects: &mut Vec<(u64, AgentIdentity, Effect)>,
    ) -> Result<(), PageError> {
        let Some(bridge) = self.bridge.as_ref() else {
            return Ok(());
        };
        let history = match bridge.member_binding_history().await {
            Ok(history) => history,
            Err(BridgeError::Unsupported { .. }) => {
                return Err(PageError::Unreadable("read Core's member history"));
            }
            Err(error) => {
                return Err(PageError::Failed(format!(
                    "reading Core's member history: {error}"
                )));
            }
        };
        // The bindings of the rotations' members by generation: the latest
        // of each session (a session a later incarnation resumed names that
        // one), and each member's latest.
        let mut bindings = BTreeMap::new();
        let rotated = pending
            .iter()
            .filter_map(|pending| match pending {
                Pending::Rotation { member, .. } => Some(member.clone()),
                Pending::Seat { .. } => None,
            })
            .collect::<BTreeSet<_>>();
        if !rotated.is_empty() {
            for binding in history
                .session_bindings_since_reset()
                .filter(|binding| rotated.contains(binding.identity()))
            {
                bindings.insert(
                    (binding.identity().clone(), binding.generation().get()),
                    binding.clone(),
                );
            }
            for member in &rotated {
                if let Some(binding) = history.last_binding(member) {
                    bindings.insert(
                        (member.clone(), binding.generation().get()),
                        binding.clone(),
                    );
                }
            }
        }
        for pending in pending {
            match pending {
                Pending::Seat {
                    cursor,
                    identity,
                    member,
                    generation,
                } => {
                    // Only the incarnation Core still binds is told: one
                    // that ended since (retired, or replaced on its
                    // session) is not, and its retirement's side effects
                    // stand for it. A seat no identity-first step made is a
                    // worker's.
                    let Some(binding) = history.last_binding(&member).filter(|binding| {
                        binding.is_live()
                            && binding.identity() == &member
                            && binding.generation().get() == generation
                            && binding_spawned_for_identity(binding, &identity)
                    }) else {
                        continue;
                    };
                    // A peer-only member has no session to tell.
                    let Some(session) = binding.session_id().cloned() else {
                        continue;
                    };
                    effects.push((
                        cursor,
                        identity,
                        Effect::Told {
                            member,
                            generation,
                            session,
                            role: binding.role().clone(),
                            labels: binding.labels().clone(),
                        },
                    ));
                }
                Pending::Rotation {
                    cursor,
                    identity,
                    member,
                    generation,
                    role,
                    successor,
                } => {
                    let retired = bindings.get(&(member.clone(), generation));
                    let seated = bindings.get(&(member.clone(), successor));
                    let (Some(retired), Some(seated)) = (retired, seated) else {
                        continue;
                    };
                    // A member no identity-first step made is a worker's.
                    if !binding_spawned_for_identity(retired, &identity) {
                        continue;
                    }
                    let Some(cause) = rotation_cause(&RotationFacts::of(retired, seated)) else {
                        continue;
                    };
                    effects.push((
                        cursor,
                        identity,
                        Effect::Retirement(Retirement {
                            member,
                            generation,
                            role,
                            session: retired.session_id().cloned(),
                            cause,
                            asked_at_ms: None,
                        }),
                    ));
                }
            }
        }
        Ok(())
    }

    /// The customizer `after_create` goes to: the one the projection's
    /// binding carries, else the runtime's own.
    async fn after_create_customizer(&self) -> Option<Arc<dyn AgentCustomizer>> {
        let bound = lock(&self.lifecycle_projection.owner)
            .as_ref()
            .and_then(|owner| owner.customizer.clone());
        match bound {
            Some(customizer) => Some(customizer),
            None => self.customizer.read().await.clone(),
        }
    }

    /// The lock of `identity`'s lifecycle steps, once the step that holds it
    /// answered: `None` when one holds it past `limits.deadline` (a build
    /// stuck in the customizer), which a side effect does not wait for.
    async fn answered_step(
        &self,
        identity: &AgentIdentity,
        limits: EffectLimits,
    ) -> Option<tokio::sync::OwnedMutexGuard<()>> {
        let step = self.lifecycle_lock_for(identity).await;
        match tokio::time::timeout(limits.deadline, step.lock_owned()).await {
            Ok(answered) => Some(answered),
            Err(_) => {
                tracing::warn!(
                    %identity,
                    deadline = ?limits.deadline,
                    "a step of the identity held its lifecycle lock past a side effect's \
                     deadline; the side effect runs without what the step kept for it"
                );
                None
            }
        }
    }

    /// Run one side effect, bounded. Only a failure to record a failed side
    /// effect fails the page.
    async fn run_lifecycle_effect(
        &self,
        identity: &AgentIdentity,
        effect: Effect,
        customizer: Option<&Arc<dyn AgentCustomizer>>,
        owed: Owed<'_>,
    ) -> Result<(), String> {
        match effect {
            Effect::Told {
                member,
                generation,
                session,
                role,
                labels,
            } => {
                let Some(customizer) = customizer else {
                    return Ok(());
                };
                let limits = self.lifecycle_projection.limits();
                // What the step that made the seat kept of its build, taken
                // once the step answered: it holds the identity's lifecycle
                // lock until then. A step that holds the lock past the
                // deadline is not waited for; a build it bound, keyed by
                // the seat's own session, is the seat's all the same.
                let built = {
                    let _answered = self.answered_step(identity, limits).await;
                    self.lifecycle_projection.take_built(identity, &session)
                };
                let context = match built {
                    Some(context) => context,
                    None => self.derived_created_context(identity, &role, labels).await,
                };
                let customizer: &dyn AgentCustomizer = &**customizer;
                let (session_ref, context_ref) = (&session, &context);
                let told = bounded(limits, move || async move {
                    Ok::<_, String>(
                        customizer
                            .after_create(identity, session_ref, context_ref)
                            .await,
                    )
                })
                .await;
                let error = match told.outcome {
                    Ok(Ok(())) => return Ok(()),
                    Ok(Err(error)) => format!("after_create answered an error: {error}"),
                    Err(error) => format!("after_create {error}"),
                };
                let failed = FailedEffect {
                    identity,
                    member: &member,
                    generation,
                    session: Some(&session),
                    effect: LifecycleEffectKind::AfterCreate,
                    attempts: told.attempts,
                    error,
                };
                self.record_lifecycle_effect_failure(owed, &failed).await?;
                self.report_lifecycle_effect_failure(&failed);
                Ok(())
            }
            Effect::Retirement(retirement) => {
                self.run_lifecycle_retirement(identity, retirement, owed)
                    .await
            }
        }
    }

    /// The memory side effects of one retirement, then its hook.
    async fn run_lifecycle_retirement(
        &self,
        identity: &AgentIdentity,
        retirement: Retirement,
        owed: Owed<'_>,
    ) -> Result<(), String> {
        let limits = self.lifecycle_projection.limits();
        let memory = self.agent_memory.read().await.clone();
        let session_key = retirement.session.as_ref().map(ToString::to_string);
        let asked_at_ms = retirement.asked_at_ms.unwrap_or_else(unix_ms_now);
        let owes = RetirementMemory::of(retirement.cause);
        if let Some(memory) = memory.as_ref() {
            if let Some(session_key) = session_key.as_deref() {
                if owes.reset_boundary {
                    memory.note_reset_boundary(session_key);
                }
                if owes.generation_mark {
                    memory.note_session_generation(identity, session_key, retirement.generation);
                }
                if let Some(cause) = owes.distill
                    && let Err(bound) = memory
                        .distill_before_rotation_within_bound(identity, session_key, cause)
                        .await
                {
                    let failed = FailedEffect {
                        identity,
                        member: &retirement.member,
                        generation: retirement.generation,
                        session: retirement.session.as_ref(),
                        effect: LifecycleEffectKind::Distillation,
                        attempts: 1,
                        error: format!(
                            "timed out: the distillation did not finish within {} ms",
                            bound.as_millis()
                        ),
                    };
                    self.record_lifecycle_effect_failure(owed, &failed).await?;
                    self.report_lifecycle_effect_failure(&failed);
                }
                if owes.drop_scope {
                    let dropped = bounded(limits, move || async move {
                        memory
                            .drop_orphaned_session_scope(session_key, DistillCause::Delete)
                            .await;
                        Ok::<_, String>(())
                    })
                    .await;
                    if let Err(error) = dropped.outcome {
                        tracing::warn!(
                            %identity,
                            session_key,
                            %error,
                            "the retired session's semantic-memory scope was not dropped; its \
                             rows stay"
                        );
                    }
                }
            }
            if let Some(harvest) = owes.exit_interview {
                let key = session_key.as_deref();
                let queued = bounded(limits, move || async move {
                    memory
                        .record_identity_retired_at(identity, key, harvest, asked_at_ms)
                        .await
                        .map_err(|error| error.to_string())
                })
                .await;
                if let Err(error) = queued.outcome {
                    let failed = FailedEffect {
                        identity,
                        member: &retirement.member,
                        generation: retirement.generation,
                        session: retirement.session.as_ref(),
                        effect: LifecycleEffectKind::ExitInterview,
                        attempts: queued.attempts,
                        error,
                    };
                    self.record_lifecycle_effect_failure(owed, &failed).await?;
                    self.report_lifecycle_effect_failure(&failed);
                }
            }
        }
        let Some(observer) = self.retired_member_observer() else {
            return Ok(());
        };
        let Some(agent_runtime_id) =
            incarnation_runtime_id(&retirement.member, retirement.generation)
        else {
            tracing::warn!(
                %identity,
                member = %retirement.member,
                generation = retirement.generation,
                "the retired incarnation has no runtime id; its retirement hook is not called"
            );
            return Ok(());
        };
        // What the step that ended the incarnation kept for the hook, taken
        // once the step answered: it holds the identity's lifecycle lock
        // until then. When a step holds the lock past the deadline, what was
        // kept counts only if its step settled it already (another step may
        // hold the lock); otherwise the hook names no registration.
        let registration = {
            let answered = self.answered_step(identity, limits).await;
            self.lifecycle_projection
                .take_retiring(identity, retirement.generation)
                .filter(|kept| answered.is_some() || kept.settled)
                .and_then(|kept| kept.member.registration)
        };
        let member = RetiredIdentityMember {
            identity: identity.clone(),
            member: retirement.member.as_str().to_string(),
            role: retirement.role.clone(),
            agent_runtime_id,
            generation: retirement.generation,
            session_id: retirement.session.clone(),
            registration,
            cause: retirement.cause,
        };
        let (observer, retired) = (&observer, &member);
        let hooked = bounded(limits, move || {
            let call = observer(retired.clone());
            async move { Ok::<_, String>(call.await) }
        })
        .await;
        let error = match hooked.outcome {
            Ok(Ok(())) => return Ok(()),
            // The host's answer stands: not tried again.
            Ok(Err(error)) => error,
            Err(error) => format!("the retirement hook {error}, {} attempts", hooked.attempts),
        };
        let failed = FailedEffect {
            identity,
            member: &retirement.member,
            generation: retirement.generation,
            session: retirement.session.as_ref(),
            effect: LifecycleEffectKind::RetirementHook,
            attempts: hooked.attempts,
            error: error.clone(),
        };
        self.record_lifecycle_effect_failure(owed, &failed).await?;
        self.emit_error(super::retired_member_release_failed(&member, error));
        Ok(())
    }

    /// Record `failed`, owed by the journal event `owed` names, durably,
    /// before the cursor moves past that event. An error leaves it
    /// unrecorded: the page that owes it runs again.
    async fn record_lifecycle_effect_failure(
        &self,
        owed: Owed<'_>,
        failed: &FailedEffect<'_>,
    ) -> Result<(), String> {
        tracing::warn!(
            identity = %failed.identity,
            generation = failed.generation,
            effect = failed.effect.as_str(),
            attempts = failed.attempts,
            error = %failed.error,
            "a lifecycle side effect failed; it is recorded, and the projection goes on"
        );
        let (store, mob_id) = self.identity_record_store();
        store
            .record_lifecycle_effect_failure(
                &LifecycleEffectFailure::new(
                    mob_id,
                    owed.incarnation,
                    owed.cursor,
                    failed.identity.as_str(),
                    failed.generation,
                    failed.effect,
                    failed.attempts,
                    failed.error.clone(),
                    unix_ms_now(),
                )
                .with_session(failed.session.cloned()),
            )
            .await
            .map_err(|error| {
                format!(
                    "recording the failure of the {} of {}: {error}",
                    failed.effect.as_str(),
                    failed.identity
                )
            })?;
        // A long run prunes as it records, not only at its start.
        let recorded = self
            .lifecycle_projection
            .failures_recorded
            .fetch_add(1, Ordering::SeqCst)
            .wrapping_add(1);
        if recorded.is_multiple_of(FAILURES_PRUNED_EVERY) {
            self.prune_lifecycle_effect_failures().await;
        }
        Ok(())
    }

    /// Report `failed` on the error hook (`lifecycle_effect_failed`).
    fn report_lifecycle_effect_failure(&self, failed: &FailedEffect<'_>) {
        self.emit_error(ErrorEvent::LifecycleEffectFailed {
            identity: failed.identity.to_string(),
            agent_runtime_id: incarnation_runtime_id(failed.member, failed.generation).map_or_else(
                || format!("{}:{}", failed.member, failed.generation),
                |runtime_id| runtime_id.to_string(),
            ),
            session_id: failed.session.map(ToString::to_string),
            effect: failed.effect.as_str().to_string(),
            attempts: failed.attempts,
            error: failed.error.clone(),
        });
    }

    /// What a seat's `after_create` reports when this process did not build
    /// it (a restart came between the seat and the call): the identity's
    /// roster spec, or Core's binding of an identity this runtime does not
    /// list, and no system prompt.
    async fn derived_created_context(
        &self,
        identity: &AgentIdentity,
        role: &meerkat_mob::ProfileName,
        labels: BTreeMap<String, String>,
    ) -> SessionCreatedContext {
        let spec = self
            .entries
            .read()
            .await
            .get(identity)
            .map(|entry| entry.spec.clone());
        match spec {
            Some(spec) => self.session_created_context(&spec, &Self::base_build_draft(&spec)),
            None => SessionCreatedContext {
                model: self.profile_model(role),
                labels,
                system_prompt: None,
            },
        }
    }
}

#[cfg(test)]
impl IdentityRuntime {
    /// Bound each attempt of a lifecycle side effect by `deadline`, and try
    /// one at most `attempts` times: rows that wait for a bound use short
    /// ones.
    pub(crate) fn set_lifecycle_effect_limits(&self, deadline: Duration, attempts: u32) {
        *lock(&self.lifecycle_projection.limits) = EffectLimits {
            deadline,
            attempts,
            pause: Duration::from_millis(10),
        };
    }

    /// Run the backlog lane once, as its task does at each sweep: whether it
    /// reached its end. Rows that need its runs now run them so.
    pub(crate) async fn run_lifecycle_backlog_once(&self) -> bool {
        matches!(
            self.run_logged_lifecycle_lane(LaneKind::Backlog).await,
            Some(LaneEnd::Reached)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn retired(cursor: u64) -> MemberLifecycleFactRecord {
        MemberLifecycleFactRecord::new(
            cursor,
            MemberLifecycleFact::retired(
                meerkat_mob::AgentIdentity::from("alpha"),
                1,
                meerkat_mob::ProfileName::from("lead"),
            ),
        )
    }

    fn seated(cursor: u64, generation: u64) -> MemberLifecycleFactRecord {
        MemberLifecycleFactRecord::new(
            cursor,
            MemberLifecycleFact::seated(
                meerkat_mob::AgentIdentity::from("alpha"),
                generation,
                meerkat_mob::ProfileName::from("lead"),
            ),
        )
    }

    /// A retirement that ends a page which read as many events as it could
    /// waits for the next read, which starts right before it: the seat of
    /// its successor may follow in the same append.
    #[test]
    fn a_retirement_ending_a_full_page_waits_for_the_next_read() {
        let page = MemberLifecycleJournalPage::new(
            vec![seated(3, 0), retired(PAGE_LIMIT as u64)],
            PAGE_LIMIT as u64,
        );
        let (records, upto) = processable(&page, 0, None);
        assert_eq!(records, &page.records[..1]);
        assert_eq!(upto, PAGE_LIMIT as u64 - 1);
    }

    /// A page that read fewer events than its limit reached the journal's
    /// head: a retirement that ends it has no successor in the same append,
    /// and runs now.
    #[test]
    fn a_retirement_ending_the_journal_runs_now() {
        let page = MemberLifecycleJournalPage::new(vec![seated(3, 0), retired(9)], 9);
        let (records, upto) = processable(&page, 0, None);
        assert_eq!(records, page.records.as_slice());
        assert_eq!(upto, 9);
    }

    /// An owed range takes no fact past its end, and a page that read past
    /// the end covers the range, a retirement at its end included: the end
    /// was read as the head of the journal, after a whole append.
    #[test]
    fn an_owed_range_ends_where_it_ends() {
        let page = MemberLifecycleJournalPage::new(
            vec![seated(3, 0), retired(5), seated(6, 2), retired(8)],
            8,
        );
        let (records, upto) = processable(&page, 0, Some(5));
        assert_eq!(records, &page.records[..2]);
        assert_eq!(upto, 5);
        let short = MemberLifecycleJournalPage::new(vec![seated(3, 0)], 3);
        let (records, upto) = processable(&short, 0, Some(5));
        assert_eq!(records, short.records.as_slice());
        assert_eq!(upto, 3, "a page short of the range's end reads on");
    }

    /// A retirement is a rotation's only when its successor's seat is the
    /// very next event.
    #[test]
    fn only_the_next_event_s_seat_is_a_successor() {
        let member = meerkat_mob::AgentIdentity::from("alpha");
        let rotated = [retired(4), seated(5, 2)];
        assert_eq!(rotation_successor(&rotated, 0, &member, 1), Some(2));
        let later = [retired(4), seated(6, 2)];
        assert_eq!(rotation_successor(&later, 0, &member, 1), None);
        let older = [retired(4), seated(5, 1)];
        assert_eq!(rotation_successor(&older, 0, &member, 1), None);
        let last = [retired(4)];
        assert_eq!(rotation_successor(&last, 0, &member, 1), None);
    }

    /// A rotation no Meta verb asked for says what it did: under another
    /// role, a role move; on a fresh session under the same role, a
    /// rotation; on the same session, nothing owed. A verb's intent names
    /// its own cause, and a replacement's none.
    #[test]
    fn each_retirement_is_named_for_what_happened() {
        let moved = RotationFacts {
            fresh_session: true,
            role_moved: true,
        };
        assert_eq!(rotation_cause(&moved), Some(RetiredIdentityCause::RoleMove));
        let rotated = RotationFacts {
            fresh_session: true,
            role_moved: false,
        };
        assert_eq!(
            rotation_cause(&rotated),
            Some(RetiredIdentityCause::Rotated)
        );
        let replaced = RotationFacts {
            fresh_session: false,
            role_moved: true,
        };
        assert_eq!(rotation_cause(&replaced), None);
        assert_eq!(
            intent_cause(LifecycleEffectVerb::RoleMove),
            Some(RetiredIdentityCause::RoleMove)
        );
        assert_eq!(
            intent_cause(LifecycleEffectVerb::Reset),
            Some(RetiredIdentityCause::Reset)
        );
        assert_eq!(intent_cause(LifecycleEffectVerb::Reload), None);
        assert!(RetirementMemory::of(RetiredIdentityCause::Reset).reset_boundary);
        assert!(!RetirementMemory::of(RetiredIdentityCause::RoleMove).reset_boundary);
        assert!(!RetirementMemory::of(RetiredIdentityCause::Rotated).reset_boundary);
    }

    /// A bounded side effect that never answers is attempted as often as
    /// its bound allows, and fails timed out; one that answers is attempted
    /// once; a failing write is attempted again, and stops at its first
    /// success.
    #[tokio::test]
    async fn a_bounded_side_effect_stops_at_its_attempts() {
        let limits = EffectLimits {
            deadline: Duration::from_millis(20),
            attempts: 3,
            pause: Duration::from_millis(1),
        };
        let calls = std::sync::atomic::AtomicU32::new(0);
        let calls = &calls;
        let never = bounded(limits, move || async move {
            calls.fetch_add(1, Ordering::SeqCst);
            std::future::pending::<Result<(), String>>().await
        })
        .await;
        assert_eq!(never.attempts, 3);
        assert!(
            never
                .outcome
                .as_ref()
                .is_err_and(|error| error.starts_with("timed out")),
            "{:?}",
            never.outcome
        );
        assert_eq!(calls.load(Ordering::SeqCst), 3);

        let answered = bounded(limits, || async { Ok::<_, String>(Err::<(), _>("no")) }).await;
        assert_eq!(answered.attempts, 1, "an answer stands");
        assert!(matches!(answered.outcome, Ok(Err("no"))));

        let tries = std::sync::atomic::AtomicU32::new(0);
        let tries = &tries;
        let written = bounded(limits, move || async move {
            if tries.fetch_add(1, Ordering::SeqCst) == 0 {
                Err("busy".to_string())
            } else {
                Ok(())
            }
        })
        .await;
        assert_eq!(written.attempts, 2);
        assert!(written.outcome.is_ok());
    }

    /// A long run prunes the failures it records as it goes, not only at a
    /// start: the store never holds more than the kept failures and one
    /// prune's worth, however many side effects the run gives up on.
    #[tokio::test]
    #[allow(clippy::expect_used)]
    async fn a_long_run_prunes_the_failures_it_records() {
        const RECORDED: u64 = 2000;
        /// The most a run leaves: the kept failures and one prune's worth.
        const MOST: usize = 1024 + 256;
        let records: Arc<dyn crate::runtime::PersistentMetadataStore> =
            Arc::new(crate::runtime::InMemoryMetadataStore::new());
        let runtime = IdentityRuntime::new(crate::identity_first::IdentityRuntimeConfig {
            continuity_store: Arc::new(
                crate::identity_first::LocalContinuityStore::in_memory().expect("store"),
            ),
            lease_provider: Arc::new(crate::identity_first::LocalLeaseProvider::new()),
            runtime_instance_id: "long-run".to_string(),
            has_runtime_store: true,
            durability_policy: crate::identity_first::DurabilityPolicy::SyncWriteThrough,
            bridge: None,
            default_timeout: None,
        });
        runtime.set_identity_records(Arc::clone(&records), "long-run".to_string());
        let member = meerkat_mob::AgentIdentity::from("alpha");
        for index in 0..RECORDED {
            let identity =
                AgentIdentity::parse(&format!("personal:failed-{index:04}")).expect("identity");
            let failed = FailedEffect {
                identity: &identity,
                member: &member,
                generation: 0,
                session: None,
                effect: LifecycleEffectKind::AfterCreate,
                attempts: 3,
                error: "timed out".to_string(),
            };
            runtime
                .record_lifecycle_effect_failure(
                    Owed {
                        incarnation: "long-run",
                        cursor: index + 1,
                    },
                    &failed,
                )
                .await
                .expect("record the failure");
        }
        let kept = records
            .lifecycle_effect_failures("long-run", 0, usize::MAX)
            .await
            .expect("read the failures")
            .len();
        assert!(kept <= MOST, "the run kept {kept} failures");
    }
}
