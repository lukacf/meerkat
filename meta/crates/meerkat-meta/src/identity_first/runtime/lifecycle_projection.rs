//! Lifecycle side effects as projections over Core's member lifecycle
//! journal.
//!
//! Core journals each lifecycle step of an identity's member, with the facts
//! MobMachine decided in it: a seat, with its session and, when a rotation
//! made it, that rotation; a rotation's terminal step, whose retirement and
//! seat name each other; a retirement, with its session and why it ended; a
//! fresh life of an identity no member held, which names the incarnation it
//! follows; a host's legacy respawn, two steps whose seat names the
//! incarnation the first retired. Some of these steps owe side effects that
//! Meta runs:
//!
//! | Journal fact | Side effects |
//! |---|---|
//! | the seat of an identity's member, while Core still holds that incarnation | the customizer's `after_create`, with the session the seat names |
//! | a retirement the roster's unlisting asked for, or Core's seal unlisted | the distillation of the retired session, the exit interview (`retire`), the retirement hook (`unlisted`) |
//! | a reset's rotation onto a fresh session (online, or a first-boot `Fresh` row's) | the distillation of the retired session behind its reset boundary, the retirement hook (`reset`) |
//! | a delete's rotation onto a fresh session | the drop of the retired session's semantic-memory scope, the exit interview (`delete`), the retirement hook (`delete`) |
//! | a role move's rotation onto a fresh session, or one no Meta verb asked for (a host's legacy respawn included) that seated another role | the distillation of the retired session as a retirement's (no reset boundary), the retirement hook (`role_move`) |
//! | a rotation onto a fresh session under the same role no Meta verb asked for (a host's own, or its legacy respawn whose retirement no Meta verb's record answered) | the distillation of the retired session as a retirement's, the retirement hook (`rotated`) |
//! | the fresh life a reset or a delete seats after an incarnation Core had already retired | that verb's memory side effects of the session the retired incarnation left, and no retirement hook |
//! | any other retirement: a reload's, a respawn on the same session, Core's own | none |
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
//! # What the journal says, and what it does not
//!
//! What each step was is read from Core's facts, never from where they sit
//! in the journal: which rotation a retirement ended in and the seat the
//! same step made (and under which role), the session of each incarnation,
//! why Core's seal unlisted an identity, and which incarnation a fresh life
//! or a legacy respawn's seat follows. Which retirement owes what is decided
//! in one place ([`retirement_owes`]), the facts first; a legacy respawn's
//! seat, whose retirement was a step of its own, owes what such a rotation
//! owes, with the session and role of the incarnation it follows, which the
//! seat's fact does not carry, read by that exact generation from Core's
//! member history, else from the retirement Core journaled for it. A Meta
//! verb's record that answered that retirement (the roster's unlisting the
//! respawn raced) decided what it owes, so the seat then owes nothing more;
//! the projection knows that from the retirement's page, in its own process.
//! After a restart between the two pages the seat owes the rotation's side
//! effects as well, which the at-least-once delivery above allows. Whether a
//! seat's incarnation is still Core's is read from its own page first, then
//! from Core's standing of the identity's member
//! (`SessionBridge::member_standing`); that standing is the state Core's
//! actor last published, which can trail the journal by the step it is
//! committing, so a seat whose end its page holds is not told whatever the
//! standing shows, and a seat the standing does not show yet is read against
//! the journal after its page: one whose end Core journaled is no longer
//! Core's.
//!
//! Core's journal does not say which Meta verb asked for a step: a reset and
//! a delete are the same rotation to Core, and the roster's unlisting the
//! same retirement as any other. Each Meta verb records that
//! ([`crate::runtime::LifecycleEffectIntent`]) before it asks Core for the
//! step, keyed by the incarnation the step ends, for an incarnation the
//! identity spawned only; a reset or a delete of an identity whose last
//! incarnation Core had already retired keys it by that incarnation, and the
//! projection reads it at the fresh life's seat, which names it. That record
//! has a key of its own, apart from the record of the step that ended the
//! incarnation, so consuming one never removes the other. A rotation
//! records the head of Core's journal it read before it asked Core: when it
//! fails, the fact it writes first (its retirement's start, or its seat),
//! past that head, says the rotation reached Core, and its absence says
//! nothing of it did, so it takes its record back and puts back the one it
//! replaced (a later verb's record replaces an earlier one's). A head that
//! does not read refuses the step before Core is asked. Once the cursor past
//! the fact that consumed a record is recorded, the record goes. A roster
//! pass that lists an identity again takes back an unlisting whose
//! retirement Core's standing shows never started. Whether a member is the
//! identity's is read from the marker or the label its binding carries, in
//! one place ([`PageMembers`]), until membership is read from Core's
//! identity entry fact.
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
//! one read of Core's published standing per seat it may tell, and runs the
//! side effects of different identities at most [`EFFECT_CONCURRENCY`] at
//! once in each lane (one identity's in journal order). A seat whose end
//! its page holds reads no standing; a seat Core's standing does not show
//! yet reads the journal after its page on to the head, once per page for
//! all such seats. A start that owes a predecessor's backlog reads
//! the backlog's pages once more, for which identities it owes. It wakes no
//! member: an idle journal costs one empty page read per sweep.
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
//! Core's member history, or Core's standing of an identity's member) turns
//! it off: that is reported once on the error hook
//! (`lifecycle_projection_off`), and the runtime's status and health read it
//! off.

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
    MemberStanding, SessionBridge,
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

/// The identity whose member's standing a preparation reads, only to learn
/// whether the session bridge reads Core's standing at all: no member needs
/// to hold it for that.
const STANDING_PROBE: &str = "lifecycle-projection-standing-probe";

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
    /// The latest incarnation of each identity whose retirement a Meta
    /// verb's record answered, as this process's pages read it: a host's
    /// legacy respawn whose seat names that incarnation owes nothing more.
    answered_retirements: StdMutex<BTreeMap<AgentIdentity, u64>>,
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
    /// Side effects of facts past `cursor` that ran in this process: a page
    /// that failed after them runs again without them.
    done: BTreeSet<EffectKey>,
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
            retiring: StdMutex::new(BTreeMap::new()),
            building: StdMutex::new(BTreeMap::new()),
            built: StdMutex::new(BTreeMap::new()),
            answered_retirements: StdMutex::new(BTreeMap::new()),
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

    /// A Meta verb's record answered the retirement of `identity`'s
    /// incarnation `generation`.
    fn note_answered(&self, identity: &AgentIdentity, generation: u64) {
        lock(&self.answered_retirements).insert(identity.clone(), generation);
    }

    /// Whether a Meta verb's record answered the retirement of `identity`'s
    /// incarnation `generation`, as this process's pages read it.
    fn answered(&self, identity: &AgentIdentity, generation: u64) -> bool {
        lock(&self.answered_retirements).get(identity) == Some(&generation)
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

/// A step that ends an identity's live incarnation, or seats the fresh life
/// after one Core had already retired, prepared before Core is asked for it
/// ([`IdentityRuntime::prepare_lifecycle_retirement`]).
pub(super) struct PreparedRetirement {
    identity: AgentIdentity,
    generation: u64,
    /// Core had retired the incarnation before the step was asked: the step
    /// seats the fresh life after it.
    predecessor_retired: bool,
    /// The step's intent was recorded, in place of `Some` intent an earlier
    /// step recorded for the same incarnation.
    recorded: Option<Option<LifecycleEffectIntent>>,
    /// The incarnation as the retirement hook reads it, when a hook is
    /// installed.
    retiring: Option<RetiredIdentityMember>,
}

impl PreparedRetirement {
    /// The generation of the live incarnation the step ends; `None` for a
    /// step that seats the fresh life after an incarnation Core had already
    /// retired.
    pub(super) fn live_generation(&self) -> Option<u64> {
        (!self.predecessor_retired).then_some(self.generation)
    }

    /// The key of the record the step wrote.
    fn key(&self) -> LifecycleIntentKey {
        if self.predecessor_retired {
            LifecycleIntentKey::fresh_life_after(self.identity.as_str(), self.generation)
        } else {
            LifecycleIntentKey::new(self.identity.as_str(), self.generation)
        }
    }
}

/// What a step records for the incarnation it ends, or follows when Core had
/// already retired it ([`LifecycleEffectIntent`]).
struct IntentOf {
    generation: u64,
    verb: LifecycleEffectVerb,
    /// The incarnation's session, from the binding the step read.
    session_id: Option<SessionId>,
    /// The head of Core's journal the step read before it asks Core.
    asked_after: Option<u64>,
    /// Core had retired the incarnation before the step was asked.
    predecessor_retired: bool,
}

/// Whether any of a step that failed reached Core
/// ([`IdentityRuntime::lifecycle_step_reach`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StepReach {
    /// Core journaled the fact the step writes first after the head the
    /// step read before it asked Core: the step reached Core.
    Reached,
    /// Core journaled no such fact after that head: nothing of the step
    /// reached Core.
    NotReached,
    /// The step read no head before it asked Core (a bridge without the
    /// journal reads), or the journal does not read now.
    Unknown,
}

/// The fact a rotation of an identity's member journals first, by what Core
/// held of the member when the step read it ([`StepFirstFact::of`]): the
/// one fact whose presence after the head the step read says the step
/// reached Core. The end of another operation of the same member, which
/// started before that head, is no such fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StepFirstFact {
    /// Core held the live incarnation `generation`: the rotation starts its
    /// retirement first, naming the rotation's target. When a host's
    /// retirement of that incarnation ended before the rotation reached
    /// Core, Core seats the fresh life after it first instead.
    RetirementStart { generation: u64 },
    /// Core had retired the incarnation `predecessor`: the rotation seats
    /// the fresh life after it first.
    FreshLifeAfter { predecessor: u64 },
    /// Core held no incarnation of the member: the rotation seats one first.
    Seat,
}

impl StepFirstFact {
    /// The fact a rotation journals first, by `last`, Core's last binding of
    /// the member as the step read it (whoever made the member).
    pub(super) fn of(last: Option<&meerkat_mob::MemberBinding>) -> Self {
        match last {
            Some(binding) if binding.is_live() => Self::RetirementStart {
                generation: binding.generation().get(),
            },
            Some(binding) => Self::FreshLifeAfter {
                predecessor: binding.generation().get(),
            },
            None => Self::Seat,
        }
    }

    /// Whether `fact` is this fact of `member`.
    fn is(self, fact: &MemberLifecycleFact, member: &meerkat_mob::AgentIdentity) -> bool {
        match (self, fact) {
            (
                Self::RetirementStart { generation },
                MemberLifecycleFact::RetirementStarted {
                    member: started,
                    generation: retiring,
                    rotation: Some(_),
                    ..
                },
            ) => started == member && *retiring == generation,
            (
                Self::RetirementStart {
                    generation: predecessor,
                }
                | Self::FreshLifeAfter { predecessor },
                MemberLifecycleFact::Seated {
                    member: seated,
                    rotation:
                        Some(meerkat_mob::SeatRotation::FreshLifeAfter {
                            predecessor: after, ..
                        }),
                    ..
                },
            ) => seated == member && after.get() == predecessor,
            (Self::Seat, MemberLifecycleFact::Seated { member: seated, .. }) => seated == member,
            _ => false,
        }
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

/// Where a side effect stands in the journal: the event that owes it, and
/// which of that event's side effects it is. A fresh life's seat owes the
/// memory side effects of the incarnation it follows before its
/// `after_create`; any other event owes one side effect at most.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct EffectKey {
    cursor: u64,
    slot: EffectSlot,
}

/// Which of its journal event's side effects one is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum EffectSlot {
    /// The memory side effects a fresh life's seat owes the incarnation it
    /// follows, which Core had retired before the step that asked for the
    /// fresh life.
    Predecessor,
    /// The event's own: a retirement's side effects, a seat's
    /// `after_create`.
    Own,
}

impl EffectKey {
    /// The own side effect of the event at `cursor`.
    fn own(cursor: u64) -> Self {
        Self {
            cursor,
            slot: EffectSlot::Own,
        }
    }

    /// The side effects the fresh life's seat at `cursor` owes the
    /// incarnation it follows.
    fn predecessor(cursor: u64) -> Self {
        Self {
            cursor,
            slot: EffectSlot::Predecessor,
        }
    }
}

/// One side effect a journal fact owes an identity.
enum Effect {
    /// The customizer's `after_create` for the seat of the incarnation
    /// `generation` Core bound to `session` under `role`; `labels` are
    /// Core's binding's, for a context derived without the build's own.
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
    session: Option<SessionId>,
    cause: RetiredIdentityCause,
    /// When its step was asked: the key of the exit interview it queues.
    asked_at_ms: Option<u64>,
    /// The retired incarnation's role, for its retirement hook. `None` for
    /// an incarnation Core had retired before the step that owes its memory
    /// side effects (a reset or a delete of an identity no member held): its
    /// end owed nothing when Core retired it, so no hook hears of it.
    hook_role: Option<meerkat_mob::ProfileName>,
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

/// The cause of a rotation onto a fresh session no Meta verb asked for, read
/// from Core's facts of it: a role move when the step that retired an
/// incarnation running under `role` seated the next one under another role,
/// a rotation when it seated it under the same one.
fn rotation_cause(
    rotation: &meerkat_mob::RetiredRotation,
    role: &meerkat_mob::ProfileName,
) -> RetiredIdentityCause {
    if rotation.successor_role == *role {
        RetiredIdentityCause::Rotated
    } else {
        RetiredIdentityCause::RoleMove
    }
}

/// What a retirement owes ([`retirement_owes`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RetirementOwes {
    /// The side effects of the verb whose intent names the incarnation, and
    /// describes the end Core journaled.
    Verb(RetiredIdentityCause),
    /// The side effects of the cause Core's facts name: Core's seal unlisted
    /// the identity.
    Facts(RetiredIdentityCause),
    /// The side effects of the cause Core's facts name, when the retired
    /// incarnation is the identity's: a rotation onto a fresh session no
    /// Meta verb asked for.
    IdentitysMember(RetiredIdentityCause),
    /// Nothing.
    Nothing,
}

/// What a retirement owes: the one place a retirement's cause is decided,
/// from what Core's journal says of the incarnation's end (`rotation`,
/// `cause`, and the `role` it ran under) and from the verb whose intent
/// names the incarnation, if one does (`verb`). The facts come first:
///
/// - a rotation onto a fresh session owes the reset's, the delete's or the
///   role move's side effects when that verb asked for it; otherwise it owes
///   a rotation's, or a role move's when the same step seated the next
///   incarnation under another role, for the identity's member only;
/// - a rotation on the member's own session (a reload, a respawn), or one
///   whose target this build does not know, owes nothing;
/// - a plain retirement owes the unlisting's side effects when the roster's
///   unlisting asked for it, or when Core's seal unlisted the identity (the
///   retirement's own cause, never its start's); any other owes nothing.
fn retirement_owes(
    rotation: Option<&meerkat_mob::RetiredRotation>,
    role: &meerkat_mob::ProfileName,
    cause: Option<meerkat_mob::MemberRetirementCause>,
    verb: Option<LifecycleEffectVerb>,
) -> RetirementOwes {
    let asked = verb.and_then(intent_cause);
    match rotation {
        Some(rotation) if rotation.target == meerkat_mob::RotationTarget::FreshSession => {
            match asked {
                Some(
                    asked @ (RetiredIdentityCause::Reset
                    | RetiredIdentityCause::Delete
                    | RetiredIdentityCause::RoleMove),
                ) => RetirementOwes::Verb(asked),
                _ => RetirementOwes::IdentitysMember(rotation_cause(rotation, role)),
            }
        }
        Some(_) => RetirementOwes::Nothing,
        None => match (asked, cause) {
            (Some(RetiredIdentityCause::Unlisted), _) => {
                RetirementOwes::Verb(RetiredIdentityCause::Unlisted)
            }
            (_, Some(meerkat_mob::MemberRetirementCause::Unlisted)) => {
                RetirementOwes::Facts(RetiredIdentityCause::Unlisted)
            }
            _ => RetirementOwes::Nothing,
        },
    }
}

/// Whether the intent of `verb` names an end other than the one Core
/// journaled for its incarnation (`owes`): it is consumed all the same, and
/// owes nothing of its own.
fn intent_is_stale(verb: LifecycleEffectVerb, owes: RetirementOwes) -> bool {
    match owes {
        RetirementOwes::Verb(_) => false,
        // A reload asks for a replacement on the same session, or a plain
        // retirement: both owe nothing.
        RetirementOwes::Nothing => verb != LifecycleEffectVerb::Reload,
        RetirementOwes::Facts(_) | RetirementOwes::IdentitysMember(_) => true,
    }
}

/// The cause whose memory side effects a fresh life owes the incarnation it
/// follows, which Core had retired before the verb asked: a reset's or a
/// delete's. No other verb records an intent for such an incarnation.
fn predecessor_cause(verb: LifecycleEffectVerb) -> Option<RetiredIdentityCause> {
    match verb {
        LifecycleEffectVerb::Reset => Some(RetiredIdentityCause::Reset),
        LifecycleEffectVerb::Delete => Some(RetiredIdentityCause::Delete),
        LifecycleEffectVerb::Unlisted
        | LifecycleEffectVerb::RoleMove
        | LifecycleEffectVerb::Reload => None,
    }
}

/// A seat whose `after_create` waits for the page's reads: Core's member
/// history (whether the member is the identity's), and Core's standing of
/// the identity's member (whether the incarnation is still Core's).
struct PendingSeat {
    cursor: u64,
    identity: AgentIdentity,
    member: meerkat_mob::AgentIdentity,
    generation: u64,
    role: meerkat_mob::ProfileName,
    session: SessionId,
}

/// A retirement that owes its side effects only when the retired
/// incarnation is the identity's ([`RetirementOwes::IdentitysMember`]): a
/// rotation onto a fresh session no Meta verb asked for.
struct PendingRotation {
    cursor: u64,
    identity: AgentIdentity,
    member: meerkat_mob::AgentIdentity,
    generation: u64,
    role: meerkat_mob::ProfileName,
    session: Option<SessionId>,
    cause: RetiredIdentityCause,
}

/// The seat of a host's legacy respawn (`SeatRotation::RespawnAfter`):
/// Core retired the incarnation `predecessor` in a step of its own, a plain
/// retirement that no Meta verb's record answered, and seated this one. The
/// respawn owes what a rotation no Meta verb asked for owes, when the
/// retired incarnation is the identity's and the seat runs on another
/// session.
struct PendingRespawn {
    cursor: u64,
    identity: AgentIdentity,
    member: meerkat_mob::AgentIdentity,
    predecessor: u64,
    /// The seat's role and session.
    role: meerkat_mob::ProfileName,
    session: Option<SessionId>,
}

/// What of a page waits for the page's reads of Core's member history and
/// standing.
#[derive(Default)]
struct PendingFacts {
    seats: Vec<PendingSeat>,
    rotations: Vec<PendingRotation>,
    respawns: Vec<PendingRespawn>,
}

impl PendingFacts {
    fn is_empty(&self) -> bool {
        self.seats.is_empty() && self.rotations.is_empty() && self.respawns.is_empty()
    }
}

/// The facts of `page` a lane runs, and the cursor it stands at once they
/// ran. A lane that ends at `until` takes no fact past it, and covers its
/// range once the page read up to its end.
fn processable(
    page: &MemberLifecycleJournalPage,
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
    (&page.records, page.next)
}

/// The page's one read of Core's member history, for whether the members
/// its seats and its rotations no Meta verb asked for name are the
/// identity's: the one place the projection reads membership, from the
/// durable-identity marker or the `agent_identity` label a binding carries
/// ([`binding_spawned_for_identity`]), until membership is read from Core's
/// identity entry fact.
struct PageMembers {
    history: meerkat_mob::MemberBindingHistory,
}

/// Read the page's [`PageMembers`] through `bridge`. A bridge that cannot
/// read Core's member history turns the projection off.
async fn page_members(bridge: &Arc<dyn SessionBridge>) -> Result<PageMembers, PageError> {
    match bridge.member_binding_history().await {
        Ok(history) => Ok(PageMembers { history }),
        Err(BridgeError::Unsupported { .. }) => {
            Err(PageError::Unreadable("read Core's member history"))
        }
        Err(error) => Err(PageError::Failed(format!(
            "reading Core's member history: {error}"
        ))),
    }
}

impl PageMembers {
    /// Core's binding of `member`'s incarnation `generation` as the history
    /// keeps it: the member's latest binding, the latest binding of
    /// `session` (the session the fact names, when it names one), or of
    /// another session since the mob's last reset. `None` when the history
    /// keeps no binding of that incarnation.
    fn incarnation(
        &self,
        member: &meerkat_mob::AgentIdentity,
        generation: u64,
        session: Option<&SessionId>,
    ) -> Option<&meerkat_mob::MemberBinding> {
        let of_generation = |binding: &&meerkat_mob::MemberBinding| {
            binding.identity() == member && binding.generation().get() == generation
        };
        self.history
            .last_binding(member)
            .filter(of_generation)
            .or_else(|| {
                session
                    .and_then(|session| self.history.binding_for_session(session))
                    .filter(of_generation)
            })
            .or_else(|| {
                self.history
                    .session_bindings_since_reset()
                    .find(of_generation)
            })
    }

    /// [`Self::incarnation`], else the member's latest binding of any
    /// generation, whose spawn spec its earlier incarnations carried too.
    fn binding(
        &self,
        member: &meerkat_mob::AgentIdentity,
        generation: u64,
        session: Option<&SessionId>,
    ) -> Option<&meerkat_mob::MemberBinding> {
        self.incarnation(member, generation, session)
            .or_else(|| self.history.last_binding(member))
    }

    /// Whether `member`'s incarnation `generation` (on `session`) is
    /// `identity`'s: an identity-first spawn made it.
    fn is_identitys(
        &self,
        identity: &AgentIdentity,
        member: &meerkat_mob::AgentIdentity,
        generation: u64,
        session: Option<&SessionId>,
    ) -> bool {
        self.binding(member, generation, session)
            .is_some_and(|binding| binding_spawned_for_identity(binding, identity))
    }

    /// The labels of `member`'s incarnation `generation` (on `session`), for
    /// an `after_create` context derived without the build's own.
    fn labels(
        &self,
        member: &meerkat_mob::AgentIdentity,
        generation: u64,
        session: Option<&SessionId>,
    ) -> BTreeMap<String, String> {
        self.binding(member, generation, session)
            .map(|binding| binding.labels().clone())
            .unwrap_or_default()
    }
}

/// Core's standing of `identity`'s member, read through `bridge`. A bridge
/// that cannot read it turns the projection off.
async fn member_standing(
    bridge: &Arc<dyn SessionBridge>,
    identity: &AgentIdentity,
) -> Result<MemberStanding, PageError> {
    match bridge.member_standing(identity).await {
        Ok(standing) => Ok(standing),
        Err(BridgeError::Unsupported { .. }) => Err(PageError::Unreadable(
            "read Core's standing of an identity's member",
        )),
        Err(error) => Err(PageError::Failed(format!(
            "reading Core's standing of {identity}: {error}"
        ))),
    }
}

/// Whether `fact` ends `member`'s incarnation `generation`: its retirement,
/// a later seat of the member (Core seats one only once that incarnation
/// ended), a legacy member reset carrying it on, or a mob reset.
fn ends_incarnation(
    fact: &MemberLifecycleFact,
    member: &meerkat_mob::AgentIdentity,
    generation: u64,
) -> bool {
    match fact {
        MemberLifecycleFact::Retired {
            member: retired,
            generation: ended,
            ..
        } => retired == member && *ended == generation,
        MemberLifecycleFact::Seated {
            member: seated,
            generation: later,
            ..
        } => seated == member && *later > generation,
        MemberLifecycleFact::LegacyReset {
            member: reset,
            previous_generation,
            ..
        } => reset == member && *previous_generation == generation,
        MemberLifecycleFact::MobReset => true,
        MemberLifecycleFact::RetirementStarted { .. }
        | MemberLifecycleFact::EntryAdopted { .. } => false,
    }
}

/// Note in `ended` the seats of `seats` whose incarnation `record` ends
/// after them.
fn note_ended_seats(
    record: &MemberLifecycleFactRecord,
    seats: &[PendingSeat],
    ended: &mut BTreeSet<u64>,
) {
    for seat in seats {
        if record.cursor > seat.cursor
            && ends_incarnation(&record.fact, &seat.member, seat.generation)
        {
            ended.insert(seat.cursor);
        }
    }
}

/// What a fresh life's seat consumes and owes the incarnation it follows
/// ([`IdentityRuntime::fresh_life_after`]).
#[derive(Default)]
struct FreshLifeOwes {
    /// The intents it consumes, removed once the cursor past the seat is
    /// recorded.
    consumed: Vec<LifecycleIntentKey>,
    /// The memory side effects it owes the incarnation it follows.
    retirement: Option<Retirement>,
}

/// Of `seats`, whose incarnations neither their page nor Core's standing
/// shows ended, those whose incarnation Core's journal ends after `upto`
/// (the page's last event), read up to the journal's head
/// ([`ends_incarnation`]). Core's standing is the machine state its actor
/// last published, which can trail the journal by the step the actor is
/// committing, and Core journals an incarnation's end before it publishes
/// the state that shows it: a seat whose end the journal does not hold is
/// the incarnation Core holds.
async fn seats_ended_after(
    bridge: &Arc<dyn SessionBridge>,
    upto: u64,
    seats: &[PendingSeat],
) -> Result<BTreeSet<u64>, PageError> {
    let mut ended = BTreeSet::new();
    let head = match bridge.member_lifecycle_journal_head().await {
        Ok(head) => head.cursor,
        Err(BridgeError::Unsupported { .. }) => {
            return Err(PageError::Unreadable(
                "read the head of Core's member lifecycle journal",
            ));
        }
        Err(error) => {
            return Err(PageError::Failed(format!(
                "reading the head of Core's member lifecycle journal: {error}"
            )));
        }
    };
    let mut after = upto;
    while after < head && ended.len() < seats.len() {
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
                    "reading Core's member lifecycle journal after {after}: {error}"
                )));
            }
        };
        if page.next <= after {
            break;
        }
        for record in &page.records {
            note_ended_seats(record, seats, &mut ended);
        }
        after = page.next;
    }
    Ok(ended)
}

/// What the backward read of [`respawned_incarnation`] met first of the
/// incarnation it reads.
enum RespawnedFact {
    /// Its retirement: the session it names and the role it ran under.
    Retired(Option<SessionId>, meerkat_mob::ProfileName),
    /// Its own seat, at this journal event: no retirement of it lies
    /// between that seat and the respawn's.
    Seated(u64),
}

/// The session and role of `member`'s incarnation `generation`, which the
/// legacy respawn's seat at the journal event `seat` names, read from the
/// retirement Core journaled for that incarnation: the journal is read back
/// from the seat, a page at a time, nearest first, down to the
/// incarnation's own seat at most.
///
/// The read is total. Core journals a legacy respawn as two steps, the
/// retirement of the incarnation (its `Retired` row names the session the
/// incarnation was bound to at its end and the role it ran under) and then
/// the seat, which names the incarnation from the respawn hold the
/// retirement's start installed and the seat's step took; so that
/// retirement lies before the seat, and after the incarnation's own seat.
/// And Core's journal keeps every member lifecycle event its member history
/// folds, retirements included. A journal with no such retirement there
/// breaks that, and fails the page: the respawn stays owed, each later run
/// reads the page again (back to the incarnation's seat, no further), and
/// the projection reads stalled with this failure while it lasts.
async fn respawned_incarnation(
    bridge: &Arc<dyn SessionBridge>,
    member: &meerkat_mob::AgentIdentity,
    generation: u64,
    seat: u64,
) -> Result<(Option<SessionId>, meerkat_mob::ProfileName), PageError> {
    // The window of journal events read next is (after, last].
    let mut last = seat.saturating_sub(1);
    while last > 0 {
        let after = last.saturating_sub(PAGE_LIMIT as u64);
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
                    "reading Core's member lifecycle journal after {after} for the incarnation a \
                     respawn retired: {error}"
                )));
            }
        };
        let met = page
            .records
            .iter()
            .filter(|record| record.cursor <= last)
            .rev()
            .find_map(|record| match &record.fact {
                MemberLifecycleFact::Retired {
                    member: retired,
                    generation: ended,
                    role,
                    session,
                    ..
                } if retired == member && *ended == generation => {
                    Some(RespawnedFact::Retired(session.clone(), role.clone()))
                }
                MemberLifecycleFact::Seated {
                    member: seated,
                    generation: began,
                    ..
                } if seated == member && *began == generation => {
                    Some(RespawnedFact::Seated(record.cursor))
                }
                _ => None,
            });
        match met {
            Some(RespawnedFact::Retired(session, role)) => return Ok((session, role)),
            Some(RespawnedFact::Seated(began)) => {
                return Err(PageError::Failed(format!(
                    "Core's member lifecycle journal holds no retirement of the incarnation \
                     {generation} of {member} between its seat at {began} and the respawn's seat \
                     at {seat}, which names it"
                )));
            }
            None => last = after,
        }
    }
    Err(PageError::Failed(format!(
        "Core's member lifecycle journal holds no retirement of the incarnation {generation} of \
         {member} before the respawn's seat at {seat}, which names it"
    )))
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
    /// Core's journal, or Core's standing of an identity's member, turns the
    /// projection off. A failure says whether a cursor is stored already,
    /// and counts toward the stall the status reports.
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
        // Nor, perhaps, Core's standing of an identity's member: the read of
        // any identity's says whether the bridge reads it. A read that fails
        // otherwise is read again where a page needs it.
        let probe = AgentIdentity::parse(STANDING_PROBE).map_err(|error| {
            unprepared(format!(
                "the identity Core's standing is first read for: {error}"
            ))
        })?;
        if let Err(BridgeError::Unsupported { .. }) = bridge.member_standing(&probe).await {
            self.turn_lifecycle_projection_off("read Core's standing of an identity's member");
            return Ok(());
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
             member_lifecycle_journal_page, the binding reads and member_standing, or forward \
             them to Meta's MobSessionBridge"
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
    /// head once more and ends, and the backlog task ends at its next page,
    /// by `deadline` at the latest. What they did not reach runs at the next
    /// start, from the record the store holds.
    pub(crate) async fn stop_lifecycle_projection(&self, deadline: tokio::time::Instant) {
        let projection = &self.lifecycle_projection;
        projection.stopping.send_replace(true);
        projection.kick.notify_one();
        let mut tasks = Vec::new();
        tasks.extend(lock(&projection.live_task).take());
        tasks.extend(lock(&projection.backlog_task).take());
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

    /// Before Core is asked for a step that ends `identity`'s live
    /// incarnation (`verb`): record what the step owes, keyed by that
    /// incarnation, with `asked_after`, the head of Core's journal the step
    /// read before it asks Core, and, when its retirement owes the retirement
    /// hook ([`intent_cause`] names a cause for `verb`), keep the incarnation
    /// as the hook reads it. A reset or a delete of an identity whose last
    /// incarnation Core had already retired records what it owes for that
    /// incarnation too, read at the fresh life Core seats after it. `None`
    /// when Core holds no incarnation of the identity's that the step ends
    /// or follows, so the step owes the identity nothing. Refused typed, with
    /// nothing asked of Core, when the record cannot be written.
    pub(super) async fn prepare_lifecycle_retirement(
        &self,
        identity: &AgentIdentity,
        verb: LifecycleEffectVerb,
        asked_after: Option<u64>,
    ) -> Result<Option<PreparedRetirement>, IdentityRuntimeError> {
        let binding = self.core_last_binding(identity).await?;
        self.prepare_lifecycle_retirement_of(identity, binding, verb, asked_after)
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
        asked_after: Option<u64>,
    ) -> Result<Option<PreparedRetirement>, IdentityRuntimeError> {
        // A member a host made directly on the mob under the identity's
        // member id is not the identity's: its end owes the identity
        // nothing.
        let Some(binding) =
            binding.filter(|binding| binding_spawned_for_identity(binding, identity))
        else {
            return Ok(None);
        };
        // An incarnation Core had already retired owes only a reset's or a
        // delete's memory side effects, at the fresh life the step seats.
        let predecessor_retired = !binding.is_live();
        if predecessor_retired
            && !matches!(
                verb,
                LifecycleEffectVerb::Reset | LifecycleEffectVerb::Delete
            )
        {
            return Ok(None);
        }
        let generation = binding.generation().get();
        let session_id = binding.session_id().cloned();
        let retiring = match intent_cause(verb) {
            Some(cause) if !predecessor_retired => {
                self.retiring_member(identity, &binding, cause).await
            }
            _ => None,
        };
        let recorded = self
            .record_lifecycle_effect_intent(
                identity,
                IntentOf {
                    generation,
                    verb,
                    session_id,
                    asked_after,
                    predecessor_retired,
                },
            )
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
            predecessor_retired,
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

    /// Nothing of the step `prepared` prepared reached Core: the step was
    /// not asked of Core, or Core's journal holds no fact of the identity's
    /// member after the head the step read before it asked. Its record goes,
    /// the one an earlier step recorded for the same incarnation is put back,
    /// and what this process kept of it goes.
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
                    .clear_lifecycle_effect_intent(&mob_id, &prepared.key())
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

    /// The cursor of the head of Core's member lifecycle journal, read
    /// before a step of `identity` asks Core for anything, so the step can
    /// read after a failure whether any of it reached Core
    /// ([`Self::lifecycle_step_reach`]). `None` without a session bridge, or
    /// over a bridge without the journal reads. A head that does not read
    /// otherwise refuses the step, retryable, before Core is asked: a
    /// failure of the step could not be told apart from one that reached
    /// Core.
    pub(super) async fn lifecycle_journal_head(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<u64>, IdentityRuntimeError> {
        let Some(bridge) = self.bridge.as_ref() else {
            return Ok(None);
        };
        match bridge.member_lifecycle_journal_head().await {
            Ok(head) => Ok(Some(head.cursor)),
            Err(BridgeError::Unsupported { .. }) => Ok(None),
            Err(error) => Err(IdentityRuntimeError::IdentityHistoryUnavailable {
                identity: identity.clone(),
                detail: format!(
                    "nothing was asked of Core: the head of Core's member lifecycle journal did \
                     not read ({error})"
                ),
            }),
        }
    }

    /// Whether any of a step of `identity` that failed reached Core: the
    /// fact the step writes first (`first`), which Core journaled after
    /// `asked_after`, the head the step read before it asked Core. Core
    /// journals a step's facts before it answers the step, so the journal
    /// is read up to its head as it stands now. Another fact of the member,
    /// the end of an operation that started before that head, says nothing
    /// of the step.
    pub(super) async fn lifecycle_step_reach(
        &self,
        identity: &AgentIdentity,
        asked_after: Option<u64>,
        first: StepFirstFact,
    ) -> StepReach {
        let (Some(bridge), Some(mut after)) = (self.bridge.as_ref(), asked_after) else {
            return StepReach::Unknown;
        };
        let unread = |error: BridgeError| {
            tracing::debug!(
                %identity,
                %error,
                "Core's member lifecycle journal did not read after a failed step; the step \
                 keeps what it recorded"
            );
            StepReach::Unknown
        };
        let head = match bridge.member_lifecycle_journal_head().await {
            Ok(head) => head.cursor,
            Err(error) => return unread(error),
        };
        // A head behind the one the step read is another journal's: the mob
        // was created again since.
        if head < after {
            return StepReach::Unknown;
        }
        let member = crate::member_comms_id::roster_member_id_for_identity(identity.as_str());
        while after < head {
            let page = match bridge
                .member_lifecycle_journal_page(after, PAGE_LIMIT)
                .await
            {
                Ok(page) => page,
                Err(error) => return unread(error),
            };
            if page
                .records
                .iter()
                .any(|record| first.is(&record.fact, &member))
            {
                return StepReach::Reached;
            }
            if page.next <= after {
                break;
            }
            after = page.next;
        }
        StepReach::NotReached
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

    /// A roster pass lists `roster`: an unlisting whose retirement Core
    /// never started is taken back, so a later retirement of that
    /// incarnation that no Meta verb asked for owes nothing. Core's standing
    /// of the identity's member says so: it still holds the incarnation the
    /// unlisting named, seated and not retiring. An unlisting whose
    /// retirement Core started (it journaled the start, and the member reads
    /// retiring) stands: the retirement Core ends is the unlisting's. An
    /// identity whose lifecycle lock a step holds past a side effect's
    /// deadline is skipped, and a later pass takes its unlisting back.
    pub(super) async fn take_back_relisted_unlistings(&self, roster: &[DurableAgentSpec]) {
        let Phase::Ready { incarnation } = self.lifecycle_projection.phase() else {
            return;
        };
        let Some(bridge) = self.bridge.as_ref() else {
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
            // Under the identity's lifecycle lock, which a step holds while
            // it records: the record is taken back only while it is still
            // the unlisting's, never one a step recorded since. A step that
            // holds the lock past a side effect's deadline (a build stuck in
            // the customizer) is not waited for: the record stays, and a
            // later pass takes it back.
            let step = self.lifecycle_lock_for(&identity).await;
            let deadline = self.lifecycle_projection.limits().deadline;
            let Ok(_lifecycle_guard) = tokio::time::timeout(deadline, step.lock_owned()).await
            else {
                tracing::warn!(
                    %identity,
                    ?deadline,
                    "a step of a relisted identity held its lifecycle lock past a side effect's \
                     deadline; its unlisting is taken back by a later roster pass"
                );
                continue;
            };
            match store.lifecycle_effect_intent(&mob_id, &intent.key()).await {
                Ok(Some(current)) if current == intent => {}
                Ok(_) => continue,
                Err(error) => {
                    tracing::warn!(
                        %identity,
                        %error,
                        "the unlisting of a relisted identity did not read again; a later pass \
                         takes it back"
                    );
                    continue;
                }
            }
            let standing = match bridge.member_standing(&identity).await {
                Ok(standing) => standing,
                Err(BridgeError::Unsupported { .. }) => {
                    self.turn_lifecycle_projection_off(
                        "read Core's standing of an identity's member",
                    );
                    return;
                }
                Err(error) => {
                    tracing::warn!(
                        %identity,
                        %error,
                        "Core's standing of a relisted identity's member did not read"
                    );
                    continue;
                }
            };
            let never_started = matches!(
                standing,
                MemberStanding::Seated { generation } if generation == intent.generation
            );
            if never_started
                && let Err(error) = store
                    .clear_lifecycle_effect_intent(&mob_id, &intent.key())
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

    /// Record what the step that ends `identity`'s incarnation (or follows
    /// it, when Core had already retired it) owes, durably, before Core is
    /// asked for it, with the intent it replaces: a later step's intent for
    /// an incarnation replaces an earlier one's. `Ok(None)` when nothing is
    /// projected, or nothing would consume the record (no identity context
    /// or public view bound the projection), so nothing is recorded.
    async fn record_lifecycle_effect_intent(
        &self,
        identity: &AgentIdentity,
        intent: IntentOf,
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
        let record = LifecycleEffectIntent::new(
            mob_id.clone(),
            incarnation,
            identity.as_str(),
            intent.generation,
            intent.verb,
            intent.session_id,
            unix_ms_now(),
        )
        .with_asked_after(intent.asked_after)
        .with_predecessor_retired(intent.predecessor_retired);
        // The record it replaces is the one under its own key: a fresh
        // life's record never replaces the record of the step that ended
        // the incarnation it follows.
        let earlier = store
            .lifecycle_effect_intent(&mob_id, &record.key())
            .await
            .map_err(|error| {
                format!("reading the incarnation's lifecycle effect intent: {error}")
            })?;
        store
            .set_lifecycle_effect_intent(&record)
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
                processable(&page, lane.until)
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
                .apply_lifecycle_page(kind, &incarnation, records, upto, &mut lane.done)
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
            lane.done.retain(|key| key.cursor > upto);
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
            match store.clear_lifecycle_effect_intent(&mob_id, &key).await {
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
                .clear_lifecycle_effect_intent(&mob_id, &intent.key())
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

    /// Run the side effects of `records`, `kind`'s lane's, read up to the
    /// journal event `upto`, skipping those `done` names and adding those
    /// that run to it. Answers the intents the page consumed, for removal
    /// once the cursor past them is recorded. What each retirement owes is
    /// decided from Core's facts of it and the intent a Meta verb recorded
    /// for its incarnation ([`retirement_owes`]); whether a seat is told
    /// `after_create`, from Core's standing of the identity's member. A side
    /// effect that fails is recorded and reported, and the page goes on; a
    /// read the page depends on that fails fails the page, which runs again.
    /// The live lane runs an identity's side effects only once the backlog
    /// lane ran those a predecessor still owed it: until then the page is
    /// held, after the side effects of the other identities ran.
    async fn apply_lifecycle_page(
        &self,
        kind: LaneKind,
        incarnation: &str,
        records: &[MemberLifecycleFactRecord],
        upto: u64,
        done: &mut BTreeSet<EffectKey>,
    ) -> Result<Vec<LifecycleIntentKey>, PageError> {
        if records.is_empty() {
            return Ok(Vec::new());
        }
        // Which identities a predecessor's backlog owes is read before
        // either lane runs a side effect.
        self.scan_owed_lifecycle_backlog().await?;
        let customizer = self.after_create_customizer().await;
        let mut effects: Vec<(EffectKey, AgentIdentity, Effect)> = Vec::new();
        let mut pending = PendingFacts::default();
        let mut consumed = Vec::new();
        let (store, mob_id) = self.identity_record_store();
        for record in records {
            match &record.fact {
                MemberLifecycleFact::Seated {
                    member,
                    generation,
                    role,
                    session,
                    rotation,
                    ..
                } => {
                    let Some(identity) = meta_identity(member) else {
                        continue;
                    };
                    match rotation {
                        // A rotation's terminal step seated it: the
                        // retirement the same step journaled names the link
                        // and owes the rotation's side effects.
                        Some(meerkat_mob::SeatRotation::Successor { .. }) => {}
                        // A fresh life of an identity whose last incarnation
                        // Core had retired owes what a reset or a delete
                        // recorded for that incarnation.
                        Some(meerkat_mob::SeatRotation::FreshLifeAfter { predecessor, .. }) => {
                            let owes = self
                                .fresh_life_after(incarnation, &identity, member, predecessor.get())
                                .await?;
                            consumed.extend(owes.consumed);
                            if let Some(retirement) = owes.retirement {
                                effects.push((
                                    EffectKey::predecessor(record.cursor),
                                    identity.clone(),
                                    Effect::Retirement(retirement),
                                ));
                            }
                        }
                        // A host's legacy respawn: Core retired the
                        // incarnation it follows in a step of its own, and
                        // this seat names it. What it owes is read with the
                        // page's member history, unless a Meta verb's record
                        // answered that retirement (a roster's unlisting the
                        // respawn raced): the record decided what the
                        // retirement owes, and the respawn owes nothing more.
                        Some(meerkat_mob::SeatRotation::RespawnAfter { predecessor, .. }) => {
                            if !self
                                .lifecycle_projection
                                .answered(&identity, predecessor.get())
                            {
                                pending.respawns.push(PendingRespawn {
                                    cursor: record.cursor,
                                    identity: identity.clone(),
                                    member: member.clone(),
                                    predecessor: predecessor.get(),
                                    role: role.clone(),
                                    session: session.clone(),
                                });
                            }
                        }
                        // A seat whose rotation this build cannot read reads
                        // as a plain seat, and says so.
                        Some(unknown) => {
                            tracing::warn!(
                                %identity,
                                generation = *generation,
                                rotation = ?unknown,
                                "a seat names a rotation this build cannot read; it reads as a \
                                 plain seat and owes the incarnation it follows nothing"
                            );
                        }
                        // A seat no rotation made owes its predecessor
                        // nothing.
                        None => {}
                    }
                    // Nothing to tell without a customizer, nor for a member
                    // bound to no session of its own (a peer-only member).
                    let (Some(_), Some(session)) = (customizer.as_ref(), session) else {
                        continue;
                    };
                    pending.seats.push(PendingSeat {
                        cursor: record.cursor,
                        identity,
                        member: member.clone(),
                        generation: *generation,
                        role: role.clone(),
                        session: session.clone(),
                    });
                }
                MemberLifecycleFact::Retired {
                    member,
                    generation,
                    role,
                    session,
                    cause,
                    rotation,
                    ..
                } => {
                    let Some(identity) = meta_identity(member) else {
                        continue;
                    };
                    // The record of the step that ended the incarnation;
                    // what a reset or a delete recorded for the fresh life
                    // after it has a key of its own, read at that seat.
                    let intent = store
                        .lifecycle_effect_intent(
                            &mob_id,
                            &LifecycleIntentKey::new(identity.as_str(), *generation),
                        )
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
                    let intent = intent.filter(|intent| intent.incarnation == incarnation);
                    let owes = retirement_owes(
                        rotation.as_ref(),
                        role,
                        *cause,
                        intent.as_ref().map(|intent| intent.verb),
                    );
                    if let Some(intent) = intent.as_ref() {
                        if intent_is_stale(intent.verb, owes) {
                            tracing::info!(
                                %identity,
                                generation = *generation,
                                verb = ?intent.verb,
                                "a lifecycle effect intent names an end Core did not journal for \
                                 its incarnation; it is consumed and owes nothing"
                            );
                        } else {
                            // The record answered this retirement: a host's
                            // legacy respawn whose seat names the
                            // incarnation owes nothing more.
                            self.lifecycle_projection
                                .note_answered(&identity, *generation);
                        }
                    }
                    // The session Core's journal names; a row written
                    // before it named one reads the session the verb read.
                    let session = session
                        .clone()
                        .or_else(|| intent.as_ref().and_then(|intent| intent.session_id.clone()));
                    match owes {
                        RetirementOwes::Verb(cause) | RetirementOwes::Facts(cause) => {
                            effects.push((
                                EffectKey::own(record.cursor),
                                identity,
                                Effect::Retirement(Retirement {
                                    member: member.clone(),
                                    generation: *generation,
                                    session,
                                    cause,
                                    asked_at_ms: intent
                                        .as_ref()
                                        .map(|intent| intent.recorded_at_ms),
                                    hook_role: Some(role.clone()),
                                }),
                            ));
                        }
                        RetirementOwes::IdentitysMember(cause) => {
                            pending.rotations.push(PendingRotation {
                                cursor: record.cursor,
                                identity,
                                member: member.clone(),
                                generation: *generation,
                                role: role.clone(),
                                session,
                                cause,
                            });
                        }
                        RetirementOwes::Nothing => {}
                    }
                }
                // A retirement's start, a legacy adoption, a legacy member
                // reset and a mob reset owe no side effect.
                MemberLifecycleFact::RetirementStarted { .. }
                | MemberLifecycleFact::EntryAdopted { .. }
                | MemberLifecycleFact::LegacyReset { .. }
                | MemberLifecycleFact::MobReset => {}
            }
        }
        if !pending.is_empty() {
            self.resolve_member_facts(records, upto, pending, &mut effects)
                .await?;
        }
        // One identity's side effects in journal order; different
        // identities' at most EFFECT_CONCURRENCY at once.
        effects.sort_by_key(|(key, _, _)| *key);
        let mut jobs: BTreeMap<AgentIdentity, Vec<(EffectKey, Effect)>> = BTreeMap::new();
        for (key, identity, effect) in effects {
            if done.contains(&key) {
                continue;
            }
            jobs.entry(identity).or_default().push((key, effect));
        }
        let outcomes = stream::iter(jobs.into_iter().map(|(identity, effects)| {
            let customizer = customizer.clone();
            async move {
                if kind == LaneKind::Live && self.lifecycle_projection.still_owed(&identity) {
                    return (Vec::new(), Err(JobStop::Held(identity)));
                }
                let mut ran = Vec::with_capacity(effects.len());
                for (key, effect) in effects {
                    let owed = Owed {
                        incarnation,
                        cursor: key.cursor,
                    };
                    if let Err(error) = self
                        .run_lifecycle_effect(&identity, effect, customizer.as_ref(), owed)
                        .await
                    {
                        return (ran, Err(JobStop::Failed(error)));
                    }
                    ran.push(key);
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

    /// What the fresh life's seat of `identity`'s member owes the
    /// incarnation `predecessor` it follows, which Core had retired before a
    /// reset or a delete asked for the fresh life: the verb's memory side
    /// effects on the session the verb read, and no retirement hook (that
    /// end owed none when Core retired the incarnation). The verb's intent is
    /// consumed here, and with it the identity's intents for earlier
    /// incarnations Core had retired, which no fresh life follows any more.
    /// A fresh life Core then closed again (a spawn's compensation) consumes
    /// it the same: the incarnation it follows is the one the verb named,
    /// and no later fresh life names it.
    async fn fresh_life_after(
        &self,
        incarnation: &str,
        identity: &AgentIdentity,
        member: &meerkat_mob::AgentIdentity,
        predecessor: u64,
    ) -> Result<FreshLifeOwes, PageError> {
        let (store, mob_id) = self.identity_record_store();
        let Some(intent) = store
            .lifecycle_effect_intent(
                &mob_id,
                &LifecycleIntentKey::fresh_life_after(identity.as_str(), predecessor),
            )
            .await
            .map_err(|error| {
                format!(
                    "reading the fresh life's lifecycle effect intent of {identity} after \
                     generation {predecessor}: {error}"
                )
            })?
        else {
            return Ok(FreshLifeOwes::default());
        };
        let mut owes = FreshLifeOwes {
            consumed: vec![intent.key()],
            retirement: None,
        };
        match store.lifecycle_effect_intents(&mob_id).await {
            Ok(intents) => {
                for earlier in intents {
                    if earlier.predecessor_retired
                        && earlier.identity == intent.identity
                        && earlier.generation < predecessor
                    {
                        owes.consumed.push(earlier.key());
                    }
                }
            }
            Err(error) => tracing::warn!(
                %identity,
                %error,
                "the lifecycle effect intents could not be listed; those of earlier incarnations \
                 the identity's fresh life follows no more are removed at a later one"
            ),
        }
        // An intent of an earlier incarnation of the mob names another
        // journal's step.
        if intent.incarnation != incarnation {
            return Ok(owes);
        }
        if let Some(cause) = predecessor_cause(intent.verb) {
            owes.retirement = Some(Retirement {
                member: member.clone(),
                generation: predecessor,
                session: intent.session_id,
                cause,
                asked_at_ms: Some(intent.recorded_at_ms),
                hook_role: None,
            });
        }
        Ok(owes)
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

    /// The side effects of the page's seats, of its rotations no Meta verb
    /// asked for and of its legacy respawns, from the page's one read of
    /// Core's member history ([`page_members`]: whether each member is the
    /// identity's, and the session and role of the incarnation a respawn
    /// retired, which its seat's fact does not carry, else read from Core's
    /// journal) and, for each seat whose end the page does not hold, Core's
    /// standing of the identity's member. A seat is told `after_create`
    /// while Core still holds its incarnation, seated or retiring; one whose
    /// incarnation ended since (on its page, in Core's standing, or in the
    /// journal after the page) is told nothing, its retirement standing for
    /// it.
    async fn resolve_member_facts(
        &self,
        records: &[MemberLifecycleFactRecord],
        upto: u64,
        pending: PendingFacts,
        effects: &mut Vec<(EffectKey, AgentIdentity, Effect)>,
    ) -> Result<(), PageError> {
        let Some(bridge) = self.bridge.as_ref() else {
            return Ok(());
        };
        let PendingFacts {
            seats,
            rotations,
            respawns,
        } = pending;
        let members = page_members(bridge).await?;
        for respawn in respawns {
            // The session and role of the incarnation the respawn retired,
            // which the seat's fact does not carry, read by its exact
            // generation: from Core's member history, else from the
            // retirement Core journaled for it, a read that is total
            // ([`respawned_incarnation`]). Reading the history departs from
            // reading a session from its step's own facts only: the
            // retirement may lie pages before the seat, and the history
            // answers without reading back to it.
            let (retired_session, retired_role) =
                match members.incarnation(&respawn.member, respawn.predecessor, None) {
                    Some(retired) => (retired.session_id().cloned(), retired.role().clone()),
                    None => {
                        respawned_incarnation(
                            bridge,
                            &respawn.member,
                            respawn.predecessor,
                            respawn.cursor,
                        )
                        .await?
                    }
                };
            // A member no identity-first step made is a worker's.
            if !members.is_identitys(
                &respawn.identity,
                &respawn.member,
                respawn.predecessor,
                retired_session.as_ref(),
            ) {
                continue;
            }
            // A respawn onto the session the retired incarnation ran on owes
            // nothing, as any replacement on its own session does; nor does
            // one of a member bound to no session of its own. A host's
            // respawn of a worker never lands here: Core launches its
            // successor on a fresh session, and refuses any other launch
            // mode. The replacement an identity reconcile seats for the
            // member it retired does, when it resumes the session its
            // intent names and the retired incarnation ran on.
            let (Some(retired_session), Some(seated)) = (retired_session, respawn.session.as_ref())
            else {
                continue;
            };
            if retired_session == *seated {
                continue;
            }
            let cause = if retired_role == respawn.role {
                RetiredIdentityCause::Rotated
            } else {
                RetiredIdentityCause::RoleMove
            };
            effects.push((
                EffectKey::predecessor(respawn.cursor),
                respawn.identity,
                Effect::Retirement(Retirement {
                    member: respawn.member,
                    generation: respawn.predecessor,
                    session: Some(retired_session),
                    cause,
                    asked_at_ms: None,
                    hook_role: Some(retired_role),
                }),
            ));
        }
        for rotation in rotations {
            // A member no identity-first step made is a worker's.
            if !members.is_identitys(
                &rotation.identity,
                &rotation.member,
                rotation.generation,
                rotation.session.as_ref(),
            ) {
                continue;
            }
            effects.push((
                EffectKey::own(rotation.cursor),
                rotation.identity,
                Effect::Retirement(Retirement {
                    member: rotation.member,
                    generation: rotation.generation,
                    session: rotation.session,
                    cause: rotation.cause,
                    asked_at_ms: None,
                    hook_role: Some(rotation.role),
                }),
            ));
        }
        // A seat whose end its own page holds is not told, whatever Core's
        // standing shows: that standing is the state Core's actor last
        // published, which can trail the journal.
        let mut ended_on_page = BTreeSet::new();
        for record in records {
            note_ended_seats(record, &seats, &mut ended_on_page);
        }
        let mut current = Vec::new();
        let mut unplaced = Vec::new();
        for seat in seats {
            if ended_on_page.contains(&seat.cursor) {
                continue;
            }
            // A seat no identity-first step made is a worker's.
            if !members.is_identitys(
                &seat.identity,
                &seat.member,
                seat.generation,
                Some(&seat.session),
            ) {
                continue;
            }
            match member_standing(bridge, &seat.identity).await?.generation() {
                // Core still holds the incarnation, seated or retiring.
                Some(held) if held == seat.generation => current.push(seat),
                // A later incarnation holds the identity: this one ended.
                Some(held) if held > seat.generation => {}
                // Core's standing does not show the incarnation: the state
                // that shows its seat may not be published yet, or it ended
                // with no later one.
                _ => unplaced.push(seat),
            }
        }
        if !unplaced.is_empty() {
            let ended = seats_ended_after(bridge, upto, &unplaced).await?;
            current.extend(
                unplaced
                    .into_iter()
                    .filter(|seat| !ended.contains(&seat.cursor)),
            );
        }
        for seat in current {
            let labels = members.labels(&seat.member, seat.generation, Some(&seat.session));
            effects.push((
                EffectKey::own(seat.cursor),
                seat.identity,
                Effect::Told {
                    member: seat.member,
                    generation: seat.generation,
                    session: seat.session,
                    role: seat.role,
                    labels,
                },
            ));
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
        // An incarnation Core had retired before the step that owes its
        // memory side effects owes no hook.
        let Some(role) = retirement.hook_role else {
            return Ok(());
        };
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
            role,
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

    fn alpha() -> meerkat_mob::AgentIdentity {
        meerkat_mob::AgentIdentity::from("alpha")
    }

    fn lead() -> meerkat_mob::ProfileName {
        meerkat_mob::ProfileName::from("lead")
    }

    fn retired(cursor: u64) -> MemberLifecycleFactRecord {
        MemberLifecycleFactRecord::new(cursor, MemberLifecycleFact::retired(alpha(), 1, lead()))
    }

    fn seated(cursor: u64, generation: u64) -> MemberLifecycleFactRecord {
        MemberLifecycleFactRecord::new(
            cursor,
            MemberLifecycleFact::seated(alpha(), generation, lead()),
        )
    }

    fn rotation(
        target: meerkat_mob::RotationTarget,
        successor_role: &str,
    ) -> meerkat_mob::RetiredRotation {
        meerkat_mob::RetiredRotation::new(
            target,
            meerkat_mob::Generation::new(2),
            meerkat_mob::ProfileName::from(successor_role),
        )
    }

    /// A page is run whole up to where it read, a retirement at its end
    /// included: which seat follows a retirement is read from Core's facts,
    /// never from where they sit, so nothing waits for the next read.
    #[test]
    fn a_retirement_ending_a_full_page_runs_with_the_page() {
        let page = MemberLifecycleJournalPage::new(
            vec![seated(3, 0), retired(PAGE_LIMIT as u64)],
            PAGE_LIMIT as u64,
        );
        let (records, upto) = processable(&page, None);
        assert_eq!(records, page.records.as_slice());
        assert_eq!(upto, PAGE_LIMIT as u64);
    }

    /// An owed range takes no fact past its end, and a page that read past
    /// the end covers the range.
    #[test]
    fn an_owed_range_ends_where_it_ends() {
        let page = MemberLifecycleJournalPage::new(
            vec![seated(3, 0), retired(5), seated(6, 2), retired(8)],
            8,
        );
        let (records, upto) = processable(&page, Some(5));
        assert_eq!(records, &page.records[..2]);
        assert_eq!(upto, 5);
        let short = MemberLifecycleJournalPage::new(vec![seated(3, 0)], 3);
        let (records, upto) = processable(&short, Some(5));
        assert_eq!(records, short.records.as_slice());
        assert_eq!(upto, 3, "a page short of the range's end reads on");
    }

    /// What a retirement owes is decided from Core's facts of its end first,
    /// and from the intent a Meta verb recorded for its incarnation: a
    /// rotation onto a fresh session owes its verb's side effects, or a
    /// rotation's (a role move's under another role) for the identity's
    /// member when no verb asked for it; a rotation on the same session owes
    /// nothing; a plain retirement owes the unlisting's when the roster or
    /// Core's seal unlisted the identity, and nothing otherwise. An intent
    /// that names another end than Core's is stale.
    #[test]
    fn each_retirement_owes_what_core_s_facts_and_its_intent_say() {
        use LifecycleEffectVerb as Verb;
        use RetiredIdentityCause as Cause;
        let fresh = rotation(meerkat_mob::RotationTarget::FreshSession, "lead");
        let moved = rotation(meerkat_mob::RotationTarget::FreshSession, "reviewer");
        let same = rotation(meerkat_mob::RotationTarget::SameSession, "lead");
        let unlisted = Some(meerkat_mob::MemberRetirementCause::Unlisted);
        let rows = [
            (
                Some(&fresh),
                None,
                Some(Verb::Reset),
                RetirementOwes::Verb(Cause::Reset),
            ),
            (
                Some(&fresh),
                None,
                Some(Verb::Delete),
                RetirementOwes::Verb(Cause::Delete),
            ),
            (
                Some(&moved),
                None,
                Some(Verb::RoleMove),
                RetirementOwes::Verb(Cause::RoleMove),
            ),
            (
                Some(&fresh),
                None,
                None,
                RetirementOwes::IdentitysMember(Cause::Rotated),
            ),
            // A rotation's retirement owes what its rotation owes, whatever
            // cause it names.
            (
                Some(&fresh),
                unlisted,
                None,
                RetirementOwes::IdentitysMember(Cause::Rotated),
            ),
            (
                Some(&moved),
                None,
                None,
                RetirementOwes::IdentitysMember(Cause::RoleMove),
            ),
            (
                Some(&fresh),
                None,
                Some(Verb::Unlisted),
                RetirementOwes::IdentitysMember(Cause::Rotated),
            ),
            (
                Some(&fresh),
                None,
                Some(Verb::Reload),
                RetirementOwes::IdentitysMember(Cause::Rotated),
            ),
            (
                Some(&same),
                None,
                Some(Verb::Reload),
                RetirementOwes::Nothing,
            ),
            (Some(&same), None, None, RetirementOwes::Nothing),
            (
                Some(&same),
                None,
                Some(Verb::Reset),
                RetirementOwes::Nothing,
            ),
            (
                None,
                None,
                Some(Verb::Unlisted),
                RetirementOwes::Verb(Cause::Unlisted),
            ),
            (None, unlisted, None, RetirementOwes::Facts(Cause::Unlisted)),
            (
                None,
                unlisted,
                Some(Verb::Unlisted),
                RetirementOwes::Verb(Cause::Unlisted),
            ),
            (None, None, Some(Verb::Reload), RetirementOwes::Nothing),
            (None, None, Some(Verb::Reset), RetirementOwes::Nothing),
            (None, None, None, RetirementOwes::Nothing),
        ];
        for (rotation, cause, verb, owes) in rows {
            assert_eq!(
                retirement_owes(rotation, &lead(), cause, verb),
                owes,
                "rotation {rotation:?}, cause {cause:?}, verb {verb:?}"
            );
        }
        assert!(!intent_is_stale(
            Verb::Reset,
            RetirementOwes::Verb(Cause::Reset)
        ));
        assert!(!intent_is_stale(Verb::Reload, RetirementOwes::Nothing));
        assert!(intent_is_stale(Verb::Reset, RetirementOwes::Nothing));
        assert!(intent_is_stale(
            Verb::Unlisted,
            RetirementOwes::IdentitysMember(Cause::Rotated)
        ));
        assert!(RetirementMemory::of(Cause::Reset).reset_boundary);
        assert!(!RetirementMemory::of(Cause::RoleMove).reset_boundary);
        assert!(!RetirementMemory::of(Cause::Rotated).reset_boundary);
    }

    /// A fresh life owes the incarnation it follows the memory side effects
    /// of a reset or a delete only: no other verb records an intent for an
    /// incarnation Core had already retired.
    #[test]
    fn a_fresh_life_owes_its_predecessor_a_reset_s_or_a_delete_s_memory_only() {
        assert_eq!(
            predecessor_cause(LifecycleEffectVerb::Reset),
            Some(RetiredIdentityCause::Reset)
        );
        assert_eq!(
            predecessor_cause(LifecycleEffectVerb::Delete),
            Some(RetiredIdentityCause::Delete)
        );
        for verb in [
            LifecycleEffectVerb::Unlisted,
            LifecycleEffectVerb::RoleMove,
            LifecycleEffectVerb::Reload,
        ] {
            assert_eq!(predecessor_cause(verb), None, "{verb:?}");
        }
    }

    /// An incarnation ends with its own retirement, a later seat of the same
    /// member, a legacy member reset that carries it on, or a mob reset;
    /// another member's facts and its own retirement start do not end it.
    #[test]
    fn an_incarnation_ends_where_core_s_journal_ends_it() {
        let other = meerkat_mob::AgentIdentity::from("beta");
        let ends = |fact: MemberLifecycleFact| ends_incarnation(&fact, &alpha(), 1);
        assert!(ends(MemberLifecycleFact::retired(alpha(), 1, lead())));
        assert!(ends(MemberLifecycleFact::seated(alpha(), 2, lead())));
        assert!(ends(MemberLifecycleFact::legacy_reset(alpha(), 1, 2)));
        assert!(ends(MemberLifecycleFact::mob_reset()));
        assert!(!ends(MemberLifecycleFact::retired(alpha(), 0, lead())));
        assert!(!ends(MemberLifecycleFact::seated(alpha(), 1, lead())));
        assert!(!ends(MemberLifecycleFact::retired(
            other.clone(),
            1,
            lead()
        )));
        assert!(!ends(MemberLifecycleFact::seated(other, 2, lead())));
        assert!(!ends(MemberLifecycleFact::retirement_started(
            alpha(),
            1,
            None,
            None
        )));
    }

    /// A failed step reached Core when the fact it journals first follows
    /// the head it read. For an incarnation it read live, that is the start
    /// of the incarnation's retirement naming a rotation or, when a host's
    /// retirement of it ended before the rotation reached Core, the fresh
    /// life Core seated after it; for an incarnation Core had retired, that
    /// fresh life; for a member Core held none of, any seat. Another fact of
    /// the member (a plain retirement's start, the seat of a rotation another
    /// step started, a retirement) is no reach.
    #[test]
    fn a_step_reaches_core_with_the_fact_it_journals_first() {
        let started =
            |rotation| MemberLifecycleFact::retirement_started(alpha(), 0, rotation, None);
        let seated_after = |rotation| {
            MemberLifecycleFact::seated(alpha(), 1, lead()).with_seat_rotation(Some(rotation))
        };
        let fresh_life = || {
            seated_after(meerkat_mob::SeatRotation::fresh_life_after(
                meerkat_mob::Generation::new(0),
            ))
        };
        let live = StepFirstFact::RetirementStart { generation: 0 };
        let retired = StepFirstFact::FreshLifeAfter { predecessor: 0 };
        for (first, fact, reached) in [
            (
                live,
                started(Some(meerkat_mob::RotationTarget::FreshSession)),
                true,
            ),
            (live, started(None), false),
            (live, fresh_life(), true),
            (
                live,
                seated_after(meerkat_mob::SeatRotation::successor(
                    meerkat_mob::Generation::new(0),
                    meerkat_mob::RotationTarget::FreshSession,
                )),
                false,
            ),
            (
                live,
                MemberLifecycleFact::retired(alpha(), 0, lead()),
                false,
            ),
            (retired, fresh_life(), true),
            (
                retired,
                MemberLifecycleFact::seated(alpha(), 1, lead()),
                false,
            ),
            (
                StepFirstFact::Seat,
                MemberLifecycleFact::seated(alpha(), 1, lead()),
                true,
            ),
            (
                StepFirstFact::Seat,
                MemberLifecycleFact::seated(meerkat_mob::AgentIdentity::from("beta"), 0, lead()),
                false,
            ),
        ] {
            assert_eq!(first.is(&fact, &alpha()), reached, "{first:?} at {fact:?}");
        }
    }

    /// The note of a later retirement a Meta verb's record answered keeps
    /// the earlier one's: the live lane may read an identity's later
    /// retirement before the backlog lane reads the respawn's seat that
    /// names the earlier.
    #[test]
    #[allow(clippy::expect_used)]
    fn a_later_answered_retirement_keeps_the_earlier_one() {
        let projection = LifecycleProjection::new();
        let alice = AgentIdentity::parse("personal:alice").expect("identity");
        projection.note_answered(&alice, 3);
        projection.note_answered(&alice, 4);
        assert!(
            projection.answered(&alice, 3),
            "the earlier retirement is still answered"
        );
        assert!(projection.answered(&alice, 4));
        assert!(!projection.answered(&alice, 5));
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
