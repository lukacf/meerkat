//! Bounded, typed supervision of the staged -> executing run transition.
//!
//! `StageForRun` binds an input to a run and removes it from its work lane.
//! From that moment the input is owned by exactly one consumer: the executor
//! the runtime loop calls `CoreExecutor::apply` on. Nothing downstream of
//! staging was bounded, so a consumer that never picked the run up left the
//! input `Staged` forever, with no error, no state change, and no log line -
//! a caller could wait indefinitely on work no one was doing.
//!
//! Run establishment proves state authority (the input is queued, lane-bound,
//! sequence-bound, not already run-associated, and the run matches the
//! machine's `current_run_id`). It proves nothing about the consumer's ability
//! to consume. The runtime loop's own liveness is proven by construction - it
//! is the thing that stages and then calls `apply` - but the actor behind
//! `apply` is unverified at that point and cannot be probed non-destructively.
//!
//! What *is* observable is the machine's own turn state, and only when the loop
//! actually signalled this run's turn start. The loop applies
//! `StartConversationRun`/`StartImmediateAppend` in
//! `prepare_turn_state_for_primitive`, which writes
//! `TurnPhase::ApplyingPrimitive`; the agent applies `PrimitiveApplied` from
//! inside the turn - before the first LLM call - moving the phase off
//! `ApplyingPrimitive` on the same shared authority. So "this run began
//! executing" is a machine-owned fact for exactly those runs, and the bound can
//! be armed honestly: a turn that is slow *after* beginning has already left
//! `ApplyingPrimitive` and is never disturbed.
//!
//! Note what that does *not* say. `ApplyingPrimitive` covers everything from
//! the loop's turn-start transition to the agent's `PrimitiveApplied`, and
//! session hydration happens inside that span, so the bound is not incapable of
//! firing on live work - it is incapable of firing on work that has begun its
//! turn. See [`RUN_EXECUTION_START_BOUND`] for what that costs and why the
//! bound is set where it is.
//!
//! `prepare_turn_state_for_primitive` deliberately skips the turn-start
//! transition for two classes (an appends-empty staged primitive, and the
//! retired drain). For those the phase field says nothing about this run, so
//! this module reports [`RunExecutionProgress::ExecutionStartUnobservable`] and
//! refuses to escalate rather than reading the previous turn's leftover phase
//! as a clean bill of health.
//!
//! Supervision is split in two because the two halves have different reach:
//!
//! * [`StagedRunStartWatchdog`] runs in its own task from the durable
//!   `StageForRun` commit. It only reports, never terminalizes, so it can cover
//!   the whole window - including the pre-`apply` segment, which takes a
//!   blocking `std` mutex and therefore cannot be supervised by a `select!` in
//!   the loop's own task.
//! * [`apply_with_execution_start_bound`] owns the escalation. It can only arm
//!   once the `apply` future exists, because escalating means dropping that
//!   future, but its deadline is measured from the staging instant so the
//!   window it bounds is the staged -> executing window and not merely the
//!   apply -> executing one.
//!
//! The shell supplies only the observation (the window elapsed and the run's
//! primitive is still un-applied). The resolution stays machine-owned: the
//! typed error travels the existing failed-apply path, which realizes the
//! machine's run terminal and resolves completion waiters.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use meerkat_core::lifecycle::core_executor::{CoreApplyOutput, CoreExecutorError};
use meerkat_core::lifecycle::run_primitive::RunPrimitive;
use meerkat_core::lifecycle::{CoreExecutor, InputId, RunId};

use crate::meerkat_machine::dsl as mm_dsl;

// Monotonic clock for the staged -> executing window. `tokio_with_wasm`'s time
// alias has no `Instant`, so wasm32 takes the workspace's browser-safe one
// (`performance.now()`); native takes tokio's, which follows the test runtime's
// virtual clock so the window can be exercised without real sleeping.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use crate::tokio::time::Instant;
#[cfg(target_arch = "wasm32")]
pub(crate) use meerkat_core::time_compat::Instant;

/// How long a staged run may sit without visibly beginning execution before
/// the condition is reported, and how often the report repeats while it holds.
///
/// The notice tier never terminalizes anything, so it cannot harm live work:
/// it states a fact ("this run has not begun executing yet") that an operator
/// previously had to reconstruct from state tables.
///
/// "Reported" means a `tracing` line and nothing else. There is no event-stream
/// or wire delivery of this condition; a caller waiting on the run sees no
/// change at this tier.
pub(crate) const RUN_EXECUTION_START_NOTICE: Duration = Duration::from_secs(120);

/// How long a staged run may sit with its primitive provably un-applied before
/// the runtime loop concludes the consumer will never pick it up.
///
/// Legitimate pre-LLM latency is dominated by session hydration, which scales
/// with transcript size: production measured 14MB at ~60s and 94MB at ~180s.
/// That is a curve, not a ceiling, and because a run that reaches this bound
/// is terminalized without re-queuing, a false positive costs the caller its
/// request permanently. The notice tier above is what closes the reported
/// blindness at two minutes, so this hard bound is deliberately set far clear
/// of any plausible extrapolation of that curve rather than close to it.
///
/// Precisely: this bound cannot fire on work that has begun its turn, because
/// `PrimitiveApplied` moves the phase off `ApplyingPrimitive` before the first
/// LLM call. It can in principle fire on a *live* hydration that exceeds an
/// hour - roughly 20x the largest measured. That false positive costs the
/// request.
///
/// It cannot double-execute: the contributor is terminalized in the same
/// realization as the run and is never returned to a work lane, so no
/// successor picks it up. The stronger claim - that it cannot leave any
/// durable residue at all - rests on `PrimitiveUnapplied` meaning the agent
/// loop has not yet applied the primitive, which is true of the CONVERSATION.
/// Whether every session-service path between staging and that transition is
/// likewise free of durable writes is NOT independently verified here, so
/// this comment does not assert it.
pub(crate) const RUN_EXECUTION_START_BOUND: Duration = Duration::from_secs(3_600);

// A notice tier at or past the hard bound would mean the window is escalated
// before it is ever reported, and the emitted `bound_secs` would stop
// describing the deadline actually used. Escalation terminalizes a run and
// abandons a household instruction, so the ordering is a compile-time fact
// rather than a runtime clamp.
const _: () = assert!(
    RUN_EXECUTION_START_NOTICE.as_secs() < RUN_EXECUTION_START_BOUND.as_secs(),
    "the run-execution notice tier must fire strictly before the hard bound"
);

/// Whether the runtime loop actually signalled this run's turn start on the
/// shared machine authority.
///
/// This gates the *interpretation* of `turn_phase`, it does not replace the
/// read: `turn_phase` is a single field shared by every run on the session, so
/// it only describes this run once this run's turn-start transition has been
/// applied against it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TurnStartSignal {
    /// The loop applied the machine's turn-start transition for this run.
    Signalled,
    /// The loop deliberately skipped the turn-start transition (an
    /// appends-empty staged primitive, or the retired drain), so the phase
    /// field carries no information about this run.
    NotSignalled,
}

/// Shared, monotonic record of whether this run's turn start was signalled.
///
/// The watchdog starts at the durable `StageForRun` commit, before the loop
/// reaches `prepare_turn_state_for_primitive`, so the signal has to be
/// observable after the fact rather than captured up front. Until it flips,
/// every observation is honestly unobservable.
#[derive(Clone, Default)]
pub(crate) struct TurnStartSignalCell(Arc<AtomicBool>);

impl TurnStartSignalCell {
    pub(crate) fn mark_signalled(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    fn signal(&self) -> TurnStartSignal {
        if self.0.load(Ordering::SeqCst) {
            TurnStartSignal::Signalled
        } else {
            TurnStartSignal::NotSignalled
        }
    }
}

/// One staged -> executing window, as armed by the runtime loop at the durable
/// `StageForRun` commit.
///
/// Every field is a mechanical shell fact, not a verdict. `run_id` names the
/// run the window belongs to, `staged_at` is the instant the window opened,
/// and `turn_start` is a clone of the same shared signal cell the loop hands
/// its own supervisors - so an out-of-band reader interprets `turn_phase`
/// under exactly the gate the in-band watchdog uses, never more permissively.
#[derive(Clone)]
pub(crate) struct RunStartWindow {
    pub(crate) run_id: RunId,
    pub(crate) staged_at: Instant,
    pub(crate) turn_start: TurnStartSignalCell,
}

/// Session-scoped cell holding the most recently armed staged -> executing
/// window, for out-of-band health reads.
///
/// This cell holds a timestamp and derives nothing. The verdict about the
/// window ([`observe_run_start_window`]) is recomputed from machine truth on
/// every read, which is what makes staleness harmless by construction: the
/// cell is written at arming and overwritten at the next arming, never
/// cleared, and a stale window can only degrade to [`RunStartHealth::Clear`]
/// (its run is no longer current, or its phase moved on) - never to a false
/// [`RunStartHealth::Overdue`]. A latched verdict flag would instead inherit
/// every failure mode of its writer: a dead watchdog task leaves it unset
/// while a caller waits (stale-quiet), and a missed clear leaves it set
/// forever (a muted alarm in the other direction).
///
/// # Void condition
///
/// **The moment any admission, dispatch, backpressure or lifecycle path
/// branches on this cell or on an observation derived from it, that
/// observation becomes a semantic fact, needs a machine owner, and this
/// design is void.** The only permitted consumer is the runtime host health
/// census (`MeerkatMachine::overdue_run_start_session_count`), which is
/// read-only by contract. If the machine should ever *act* on a run that
/// never began executing, that action goes through the failed-apply path
/// that already owns escalation ([`apply_with_execution_start_bound`]) -
/// never through this cell. The
/// `run_start_window_stays_out_of_machine_authority` test pins this with a
/// source grep over the machine-authority files.
#[derive(Clone, Default)]
pub(crate) struct SharedRunStartWindowCell {
    inner: Arc<std::sync::Mutex<Option<RunStartWindow>>>,
}

impl SharedRunStartWindowCell {
    /// Arm the window for a newly staged run, overwriting any previous window.
    ///
    /// Deliberately no `clear`: clearing would create a second writer with an
    /// ordering obligation against the turn path, and a missed clear would
    /// latch a false alarm. Overwrite-on-arm plus recompute-on-read needs
    /// neither.
    pub(crate) fn arm(&self, run_id: RunId, staged_at: Instant, turn_start: TurnStartSignalCell) {
        *self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(RunStartWindow {
            run_id,
            staged_at,
            turn_start,
        });
    }

    /// The most recently armed window, if any run was ever staged.
    ///
    /// Both lock sites on this mutex (here and [`Self::arm`]) are short field
    /// moves with no I/O and no `await` under the guard, so a plain lock
    /// cannot park a health probe behind a wedged session.
    pub(crate) fn snapshot(&self) -> Option<RunStartWindow> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

/// What one out-of-band read established about a session's staged ->
/// executing window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RunStartHealth {
    /// A POSITIVE fact resolved the window: no window was ever armed, the
    /// window is still inside the notice bound, machine truth shows the run
    /// began executing, or machine truth shows the run is no longer current
    /// (it moved on, however it ended). Nothing else folds here: an absence
    /// of observation is not health.
    Clear,
    /// The window has been open past the notice bound and machine authority
    /// positively shows this exact run still current with its primitive
    /// un-applied: the staged -> executing wedge, observed.
    Overdue,
    /// The window is past the notice bound and neither `Clear` nor `Overdue`
    /// was established: the authority could not be read without blocking, or
    /// the window's run is STILL CURRENT but its start cannot be interpreted
    /// (no runtime binding, or a turn start that was never signalled, so the
    /// shared phase describes some other run). Not a rung and never an
    /// escalation - but also not silence: an unread authority's holder is the
    /// prime suspect for the wedge, and a current run nobody can interpret
    /// has not been proven healthy by anyone. Both resolve on their own - the
    /// lock frees, or the run terminalizes and stops being current - so this
    /// is the per-scrape self-clearing class, never a standing amber.
    Unreadable,
}

/// Recompute one session's staged -> executing window health from machine
/// truth.
///
/// The verdict is a pure function of the armed window, the shared machine
/// authority, and `notice`; nothing here is stored, latched, or trusted from
/// an earlier tick. The facts are the same [`RunTurnStateFacts`] the
/// staged-run watchdog classifies, read through the same non-blocking seam,
/// so the wire claim and the existing log line cannot disagree about what
/// "overdue" means. A stale window degrades through the same facts:
/// `run_is_current` fails once another run took over, and `applying_primitive`
/// fails once the run progressed, so neither can produce a false `Overdue`.
///
/// The FOLD deliberately differs from [`classify_execution_start`] in one
/// place, and the difference is the reporting tier's whole job.
/// Classification gates phase interpretation on the turn-start signal BEFORE
/// resolving run currency, because its consumer escalates - dropping the
/// `apply` future on an unproven condition is forbidden, so anything
/// uninterpretable must refuse loudly but harmlessly. Health has no such
/// constraint and the opposite duty: it resolves CURRENCY first, because a
/// window whose run is no longer current is positively resolved (the run
/// moved on, however it ended - including the appends-empty and retired-drain
/// classes that never signal, whose stale windows must not stand amber
/// forever on a healthy idle session); while a window whose run IS still
/// current but cannot be interpreted - unbound runtime, unsignalled turn
/// start - is an ABSENCE of observation, and an absence may not publish as
/// health. Those fold to [`RunStartHealth::Unreadable`], which self-clears
/// the moment the run terminalizes and stops being current. Folding them to
/// `Clear` would render "I cannot see" as "healthy", which is the exact
/// defect this whole projection exists to remove.
pub(crate) fn observe_run_start_window(
    cell: &SharedRunStartWindowCell,
    authority: &crate::driver::ephemeral::SharedIngressDslAuthority,
    notice: Duration,
) -> RunStartHealth {
    let Some(window) = cell.snapshot() else {
        return RunStartHealth::Clear;
    };
    if window.staged_at.elapsed() < notice {
        return RunStartHealth::Clear;
    }
    let Some((facts, signal)) =
        read_run_turn_state_facts(authority, &window.run_id, &window.turn_start)
    else {
        return RunStartHealth::Unreadable;
    };
    // Positive resolution first: the run moved on, so the window is over.
    if !facts.run_is_current {
        return RunStartHealth::Clear;
    }
    // A current run this read cannot interpret is an absence of observation,
    // never a clean bill of health.
    if !facts.runtime_bound || signal == TurnStartSignal::NotSignalled {
        return RunStartHealth::Unreadable;
    }
    if facts.applying_primitive {
        RunStartHealth::Overdue
    } else {
        RunStartHealth::Clear
    }
}

/// How long an admitted input may sit queued while the session has no run in
/// flight before the condition is reported as parked work.
///
/// Deliberately equal to [`RUN_EXECUTION_START_NOTICE`], so "overdue" means
/// one thing across the whole staged-and-earlier pipeline: a run staged this
/// long without beginning, and an input queued this long with nothing running
/// at all, are the same rung of the same alarm. The queued case is if anything
/// the stronger claim - the predicate below only fires when `current_run_id`
/// is `None`, so the loop had nothing else to do and ordinary staging latency
/// is its wake-up latency, not a turn. Two minutes of parked-with-work-while-
/// idle is already the wedge.
///
/// The bound is meerkat-derived on purpose. The household fleet's own
/// input-queue fuse fires at 300s; that number reflects their turn mix and
/// remains their layer of defense in depth. A wire claim published by this
/// runtime should not import another system's tuning.
pub(crate) const QUEUED_INPUT_START_NOTICE: Duration = RUN_EXECUTION_START_NOTICE;

/// How many stage attempts a still-queued input may accumulate before the
/// churn axis of the parked-work census reports it, independent of any clock.
///
/// This axis exists because the queued-age clock is structurally defeated by
/// exactly the state it most needs to see: the rollback path re-stamps
/// `InputState::updated_at` on every Staged -> Queued return (the ledger
/// bookkeeping in `resolve_staged_rollbacks`), so an input flapping through
/// stage -> fail -> rollback faster than the notice bound reads as
/// forever-fresh on the age axis - and the flapping member is the MORE
/// alarming one, not the less. Stage attempts are the machine's own count
/// (`input_attempt_counts`, incremented by `StageForRun`), untouched by the
/// re-stamp: an input that is QUEUED with two or more attempts has been
/// staged and thrown back at least twice without completing.
///
/// Two is the smallest count that is churn rather than incident: one attempt
/// followed by a rollback is an ordinary recovery the machine performed once.
/// The tier above this is the machine's own `max_stage_attempts` abandonment
/// cap, which terminalizes the input - this census only makes the road there
/// visible.
pub(crate) const PARKED_STAGE_CHURN_ATTEMPTS: u64 = 2;

/// Which axis of the parked-work predicate established the verdict, so a
/// consumer (a test, a log line, an operator reading a trace) is told rather
/// than left to guess.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ParkedQueuedWorkAxis {
    /// The input entered its current queued state more than the notice bound
    /// ago and has simply never been staged.
    AgedInQueue,
    /// The input has been staged and rolled back at least
    /// [`PARKED_STAGE_CHURN_ATTEMPTS`] times and is queued again with nothing
    /// running: churning, not progressing. Its age clock is meaningless -
    /// every rollback resets it.
    StageChurn,
}

/// What one out-of-band read established about a session's queued-but-idle
/// work.
///
/// This is the PRE-staging sibling of [`RunStartHealth`], and the exact class
/// the 2026-08-16 field incident occupied for five days: a session that
/// resumed cleanly on every boot, kept its loop task alive and its channels
/// open, and held peer inputs in the machine's queued phase without ever
/// staging them - `queue_mode: fifo` turning one parked input into a total
/// member outage. Every registration-shaped probe read it as healthy, because
/// registration was healthy; the wedge was visible only in lane truth plus
/// time (or, for a stage-churning input whose clock resets on every rollback,
/// lane truth plus the machine's own attempt count).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ParkedQueuedWorkHealth {
    /// Nothing to report: no queued work, a run is in flight (queued work
    /// waiting behind a live turn is a backlog, not a wedge), the executor
    /// registration is not `Active` (nothing could stage), or no queued input
    /// trips either axis.
    Clear,
    /// The session's registration is `Active`, no run is in flight, and
    /// machine lane truth holds at least one queued input that either aged
    /// past the notice bound or churned past the stage-attempt threshold:
    /// work somebody handed this session, which it is not moving, while doing
    /// nothing else. The axis says which.
    Parked(ParkedQueuedWorkAxis),
    /// One of the two structures this predicate reads could not be read
    /// without blocking, so neither `Clear` nor `Parked` was established.
    /// Never a rung and never an escalation - and not silence either, because
    /// a holder wedged on either lock is a prime suspect for the parked state
    /// itself.
    Unreadable,
}

/// Recompute one session's parked-queued-work health from machine truth plus
/// the ledger's own per-input clocks.
///
/// The verdict is a pure function of the shared generated authority, the
/// input ledger, `notice`, and `now`; nothing is stored, latched, or trusted
/// from an earlier tick, so nothing here can go stale in either direction.
/// The predicate has TWO AXES, because either one alone is structurally
/// blind to a real wedge shape:
///
/// - **Aged in queue** ([`ParkedQueuedWorkAxis::AgedInQueue`]): the input's
///   [`InputState::updated_at`] - the instant it entered its CURRENT state -
///   is older than `notice`. This is the cleanly-parked shape: admitted,
///   never staged, nothing running. `updated_at` rather than `created_at` so
///   an input staged once and rolled back does not carry its pre-staging age
///   into the fresh queue entry.
/// - **Stage churn** ([`ParkedQueuedWorkAxis::StageChurn`]): the machine's
///   own `input_attempt_counts` for the input has reached
///   [`PARKED_STAGE_CHURN_ATTEMPTS`] while the input is queued again with
///   nothing running, and the input has existed (`created_at`) for at least
///   `notice`. This axis exists because the age axis CANNOT see a flapping
///   input: the rollback path re-stamps `updated_at` on every Staged ->
///   Queued return, so stage -> fail -> rollback loops faster than `notice`
///   read as forever-fresh - and the flapping member is the more alarming
///   one. The `created_at` floor keeps a young input inside its ordinary
///   first-bounces window from being read as churn; unlike `updated_at`,
///   `created_at` is written once and never re-stamped, and the attempt
///   count - not age - is what carries the churn claim.
///
/// Lane truth (which inputs are queued, whether a run is in flight, whether
/// registration is `Active`, the attempt counts) is read from the generated
/// machine authority - the DSL is the sole writer of `input_phases` and
/// `input_attempt_counts`. The clocks come from the ledger's per-input shell
/// state.
///
/// The two locks are taken strictly in sequence, never nested: the authority
/// read completes and releases before the driver lock is attempted, so this
/// probe can never hold one contended lock while waiting on the other. Both
/// acquisitions are non-blocking tries; either miss reports
/// [`ParkedQueuedWorkHealth::Unreadable`]. In particular the ledger is read
/// through the driver's own `tokio` mutex with `try_lock` - the wedged party
/// may be holding it, and a health probe that can park behind the wedge it
/// reports is the joke version of this fix.
///
/// A queued id with no ledger row is skipped rather than counted: the ledger
/// carries a row for every admitted input by construction, so the miss is a
/// transient mid-mutation window, and counting it would assert an age nobody
/// measured.
///
/// # Void condition
///
/// **The moment any admission, dispatch, backpressure or lifecycle path
/// branches on this observation, it becomes a semantic fact, needs a machine
/// owner, and this design is void.** The only permitted consumer is the
/// runtime host health census
/// (`MeerkatMachine::parked_queued_input_session_count`), which is read-only
/// by contract. The `run_start_window_stays_out_of_machine_authority` test
/// pins this boundary with a source grep over the machine-authority files.
pub(crate) fn observe_parked_queued_work(
    authority: &crate::driver::ephemeral::SharedIngressDslAuthority,
    driver: &crate::meerkat_machine::driver::SharedDriver,
    notice: Duration,
    now: chrono::DateTime<chrono::Utc>,
) -> ParkedQueuedWorkHealth {
    // Stage 1: machine lane truth, released before stage 2 begins.
    //
    // Deliberately a direct authority read, NOT `with_dsl_state` on the
    // driver: that helper takes the dsl mutex BLOCKING from inside the driver
    // lock, which would nest the two locks this probe must never nest. Anyone
    // extending this census with another machine fact must add it to THIS
    // read, not reach through the driver.
    let queued_inputs: Vec<(String, u64)> = {
        let authority = match authority.try_lock() {
            Ok(authority) => authority,
            Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => {
                return ParkedQueuedWorkHealth::Unreadable;
            }
        };
        let state = authority.state();
        if state.registration_phase != mm_dsl::RegistrationPhase::Active {
            return ParkedQueuedWorkHealth::Clear;
        }
        if state.current_run_id.is_some() {
            return ParkedQueuedWorkHealth::Clear;
        }
        state
            .input_phases
            .iter()
            .filter(|(_, phase)| **phase == mm_dsl::InputPhase::Queued)
            .map(|(key, _)| {
                let attempts = state.input_attempt_counts.get(key).copied().unwrap_or(0);
                (key.clone(), attempts)
            })
            .collect()
    };
    if queued_inputs.is_empty() {
        return ParkedQueuedWorkHealth::Clear;
    }

    // Stage 2: the ledger's per-input clocks, under the driver's own mutex.
    let driver = match driver.try_lock() {
        Ok(driver) => driver,
        Err(_) => return ParkedQueuedWorkHealth::Unreadable,
    };
    let ledger = driver.ledger();
    let past = |since: chrono::DateTime<chrono::Utc>| {
        now.signed_duration_since(since)
            .to_std()
            .is_ok_and(|elapsed| elapsed >= notice)
    };
    for (key, attempts) in &queued_inputs {
        // Production writes `input_phases` keys as `InputId::to_string()`, so
        // an unparseable key names nothing the ledger could hold.
        let Some(input_id) = uuid::Uuid::parse_str(key).ok().map(InputId::from_uuid) else {
            continue;
        };
        let Some(state) = ledger.get(&input_id) else {
            continue;
        };
        if past(state.updated_at()) {
            return ParkedQueuedWorkHealth::Parked(ParkedQueuedWorkAxis::AgedInQueue);
        }
        if *attempts >= PARKED_STAGE_CHURN_ATTEMPTS && past(state.created_at) {
            return ParkedQueuedWorkHealth::Parked(ParkedQueuedWorkAxis::StageChurn);
        }
    }
    ParkedQueuedWorkHealth::Clear
}

/// The exact machine facts an execution-start observation is computed from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RunTurnStateFacts {
    /// A runtime binding is recorded, so a session-owned turn-state handle was
    /// minted against this authority.
    pub(crate) runtime_bound: bool,
    /// Machine authority reports this exact run as `current_run_id`.
    pub(crate) run_is_current: bool,
    /// The shared turn phase is `ApplyingPrimitive`.
    pub(crate) applying_primitive: bool,
}

/// Machine-observed answer to "has this exact run begun executing?".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RunExecutionProgress {
    /// Machine authority shows the run's primitive applied (or the turn
    /// already past it). The consumer is alive and working.
    Executing,
    /// Machine authority still shows the run's primitive un-applied. The
    /// consumer accepted ownership of the staged input and did nothing.
    PrimitiveUnapplied,
    /// Machine authority no longer reports this run as current, so the staged
    /// -> executing window is no longer the thing being measured.
    RunNotCurrent,
    /// No runtime binding is recorded, so no session-owned turn-state handle
    /// was minted against this authority and the agent's turn writes land
    /// somewhere this observer cannot see. Unobservable, never escalated.
    RuntimeUnbound,
    /// This run's turn start was never signalled on this authority, so the
    /// shared `turn_phase` describes some other run (or a fresh session's
    /// default) and says nothing about whether this one started. Unobservable,
    /// never escalated.
    ExecutionStartUnobservable,
    /// Machine authority could not be read without blocking. Unprovable,
    /// never escalated - but also not evidence that the window closed, so the
    /// watchdog keeps reporting rather than standing down.
    Unreadable,
}

impl RunExecutionProgress {
    /// Only a positively proven un-applied primitive may terminalize a run.
    /// Every other observation refuses rather than risking live work.
    pub(crate) fn proves_execution_never_started(self) -> bool {
        matches!(self, Self::PrimitiveUnapplied)
    }

    /// Whether this observation is a positive fact that the staged ->
    /// executing window is over, and supervision can stand down silently.
    ///
    /// `Executing` and `RunNotCurrent` are such facts: the turn began, or the
    /// run moved on. Everything else either proves non-progress or is the
    /// absence of a fact, and standing down on absence would be the original
    /// defect in miniature - a consumer wedged while holding the authority
    /// mutex makes every read unreadable, which is precisely the shape this
    /// supervision exists for.
    pub(crate) fn closes_execution_start_window(self) -> bool {
        matches!(self, Self::Executing | Self::RunNotCurrent)
    }

    /// Whether this observation means the escalation bound cannot arm at all
    /// for this run, which an operator needs told: the safety property this
    /// release adds is off for that run.
    pub(crate) fn execution_start_is_unobservable(self) -> bool {
        matches!(
            self,
            Self::RuntimeUnbound | Self::ExecutionStartUnobservable
        )
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Executing => "Executing",
            Self::PrimitiveUnapplied => "PrimitiveUnapplied",
            Self::RunNotCurrent => "RunNotCurrent",
            Self::RuntimeUnbound => "RuntimeUnbound",
            Self::ExecutionStartUnobservable => "ExecutionStartUnobservable",
            Self::Unreadable => "Unreadable",
        }
    }
}

/// Classify a set of machine facts under the turn-start signal that says
/// whether those facts describe this run at all.
pub(crate) fn classify_execution_start(
    facts: RunTurnStateFacts,
    turn_start: TurnStartSignal,
) -> RunExecutionProgress {
    if !facts.runtime_bound {
        return RunExecutionProgress::RuntimeUnbound;
    }
    if turn_start == TurnStartSignal::NotSignalled {
        return RunExecutionProgress::ExecutionStartUnobservable;
    }
    if !facts.run_is_current {
        return RunExecutionProgress::RunNotCurrent;
    }
    if facts.applying_primitive {
        RunExecutionProgress::PrimitiveUnapplied
    } else {
        RunExecutionProgress::Executing
    }
}

/// Read seam for the machine-owned run-execution fact.
///
/// Kept as a trait so the classification and the bound can be exercised
/// without standing up a machine, and so the supervisor never reaches for a
/// driver lock the wedged party may be holding.
pub(crate) trait RunExecutionProgressSource: Send + Sync {
    fn observe(&self, run_id: &RunId) -> RunExecutionProgress;
}

/// Production source: the session's shared generated-machine authority.
pub(crate) struct AuthorityRunExecutionProgress {
    authority: crate::driver::ephemeral::SharedIngressDslAuthority,
    turn_start: TurnStartSignalCell,
}

impl AuthorityRunExecutionProgress {
    pub(crate) fn new(
        authority: crate::driver::ephemeral::SharedIngressDslAuthority,
        turn_start: TurnStartSignalCell,
    ) -> Self {
        Self {
            authority,
            turn_start,
        }
    }
}

impl RunExecutionProgressSource for AuthorityRunExecutionProgress {
    fn observe(&self, run_id: &RunId) -> RunExecutionProgress {
        match read_run_turn_state_facts(&self.authority, run_id, &self.turn_start) {
            None => RunExecutionProgress::Unreadable,
            Some((facts, signal)) => classify_execution_start(facts, signal),
        }
    }
}

/// One non-blocking read of the machine facts an execution-start observation
/// is computed from, plus the turn-start gate. `None` means the authority
/// could not be read without blocking.
///
/// `try_lock` is deliberate: a wedged holder of this authority must not be
/// able to wedge an observer too. An unreadable authority is an unprovable
/// one, which never escalates.
fn read_run_turn_state_facts(
    authority: &crate::driver::ephemeral::SharedIngressDslAuthority,
    run_id: &RunId,
    turn_start: &TurnStartSignalCell,
) -> Option<(RunTurnStateFacts, TurnStartSignal)> {
    let authority = match authority.try_lock() {
        Ok(authority) => authority,
        Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
        Err(std::sync::TryLockError::WouldBlock) => return None,
    };
    let state = authority.state();
    let current = state
        .current_run_id
        .as_ref()
        .and_then(crate::meerkat_machine::dsl_authority::current_run_id_from_dsl);
    let facts = RunTurnStateFacts {
        runtime_bound: state.active_runtime_id.is_some(),
        run_is_current: current.as_ref() == Some(run_id),
        applying_primitive: state.turn_phase == mm_dsl::TurnPhase::ApplyingPrimitive,
    };
    Some((facts, turn_start.signal()))
}

/// Non-escalating supervision of the whole staged -> executing window, loud in
/// the log and nowhere else.
///
/// The escalation bound can only arm once `apply` is entered, because
/// escalating means dropping the `apply` future. Everything between the
/// durable `StageForRun` commit and that call runs in the loop's own task and
/// part of it takes a blocking `std` mutex, so a wedge there cannot be
/// observed by a `select!` in that same task - the thread is not free to poll
/// it. This watchdog therefore lives in its own task, which is what makes the
/// reported field shape (staged, silent, forever) impossible *in the log*, no
/// matter where in the window the loop is stuck.
///
/// The reach of that is narrower than "loud" suggests, so state it plainly:
/// this emits `tracing` records and nothing else. No runtime event, no
/// completion, and no wire delivery carries the condition to a caller or a
/// host. For the classes the escalation bound can never arm on
/// ([`RunExecutionProgress::RuntimeUnbound`],
/// [`RunExecutionProgress::ExecutionStartUnobservable`], and a persistently
/// [`RunExecutionProgress::Unreadable`] authority) that log line is the only
/// signal that exists anywhere, and the caller still waits.
///
/// It never terminalizes anything. Escalation stays with the task that owns
/// the `apply` future; a supervisor that could terminalize a run from outside
/// that task would be a fresh double-execution hazard.
pub(crate) struct StagedRunStartWatchdog {
    handle: crate::tokio::task::JoinHandle<()>,
}

impl StagedRunStartWatchdog {
    pub(crate) fn spawn(
        progress: Arc<dyn RunExecutionProgressSource + 'static>,
        run_id: RunId,
        input_ids: Vec<InputId>,
        staged_at: Instant,
        notice_every: Duration,
    ) -> Self {
        let handle = crate::tokio::spawn(async move {
            loop {
                crate::tokio::time::sleep(notice_every).await;
                let observed = progress.observe(&run_id);
                if observed.closes_execution_start_window() {
                    return;
                }
                let staged_secs = staged_at.elapsed().as_secs();
                let inputs = input_ids
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(",");
                if observed.execution_start_is_unobservable() {
                    tracing::warn!(
                        %run_id,
                        %inputs,
                        observed = observed.as_str(),
                        staged_secs,
                        "staged run has not returned and its execution start is unobservable; \
                         the execution-start bound cannot arm for this run"
                    );
                } else {
                    tracing::error!(
                        %run_id,
                        %inputs,
                        observed = observed.as_str(),
                        staged_secs,
                        bound_secs = RUN_EXECUTION_START_BOUND.as_secs(),
                        "staged run has not begun executing; its consumer accepted the run and \
                         applied nothing"
                    );
                }
            }
        });
        Self { handle }
    }
}

impl Drop for StagedRunStartWatchdog {
    fn drop(&mut self) {
        // RAII so every early return between staging and the end of `apply`
        // retires the watchdog without hand-threading an abort through them.
        self.handle.abort();
    }
}

/// Apply a run primitive under a bounded staged -> executing window.
///
/// The `apply` future is pinned and polled first (`biased`) for the whole
/// call, so a slow turn is never cancelled and can never be double-executed by
/// this path. The single escalating branch requires positive proof that the
/// primitive was never applied - the turn had not begun mutating the
/// conversation - which is what makes dropping the future there containment
/// rather than a lost turn.
///
/// The deadline is measured from `staged_at`, not from entry, so time spent
/// between the `StageForRun` commit and this call counts against the same
/// window rather than extending it.
///
/// No release, requeue or retry happens here: the staged input stays owned by
/// its run and travels the machine's failed-apply terminal instead.
pub(crate) async fn apply_with_execution_start_bound(
    executor: &mut dyn CoreExecutor,
    progress: &dyn RunExecutionProgressSource,
    run_id: RunId,
    primitive: RunPrimitive,
    staged_at: Instant,
    bound: Duration,
) -> Result<CoreApplyOutput, CoreExecutorError> {
    let apply_future = executor.apply(run_id.clone(), primitive);
    let mut apply_future = std::pin::pin!(apply_future);

    let deadline = crate::tokio::time::sleep(bound.saturating_sub(staged_at.elapsed()));
    let mut deadline = std::pin::pin!(deadline);
    crate::tokio::select! {
        biased;
        result = &mut apply_future => return result,
        () = deadline.as_mut() => {}
    }

    let observed = progress.observe(&run_id);
    let staged_secs = staged_at.elapsed().as_secs();
    if !observed.proves_execution_never_started() {
        if observed.execution_start_is_unobservable() {
            tracing::warn!(
                %run_id,
                observed = observed.as_str(),
                staged_secs,
                bound_secs = bound.as_secs(),
                "staged run passed its execution-start bound with its start unobservable; \
                 leaving the run in flight"
            );
        } else {
            tracing::error!(
                %run_id,
                observed = observed.as_str(),
                staged_secs,
                bound_secs = bound.as_secs(),
                "staged run passed its execution-start bound but non-progress is unproven; \
                 leaving the run in flight"
            );
        }
        return apply_future.await;
    }

    tracing::error!(
        %run_id,
        observed = observed.as_str(),
        staged_secs,
        bound_secs = bound.as_secs(),
        "runtime loop concluded its executor never began executing a staged run; terminalizing the run and handing the executor off"
    );
    Err(
        CoreExecutorError::executor_not_progressing_requires_teardown(format!(
            "runtime loop observed run {run_id} with its primitive still un-applied {staged_secs} seconds after staging; the executor never began executing it"
        )),
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use meerkat_core::lifecycle::core_executor::CoreExecutorTeardownReason;
    use std::sync::atomic::{AtomicU8, AtomicUsize};

    struct ScriptedProgress {
        observations: std::sync::Mutex<Vec<RunExecutionProgress>>,
        cursor: AtomicU8,
    }

    impl ScriptedProgress {
        fn new(observations: Vec<RunExecutionProgress>) -> Arc<Self> {
            Arc::new(Self {
                observations: std::sync::Mutex::new(observations),
                cursor: AtomicU8::new(0),
            })
        }

        fn observation_count(&self) -> u8 {
            self.cursor.load(Ordering::SeqCst)
        }
    }

    impl RunExecutionProgressSource for ScriptedProgress {
        fn observe(&self, _run_id: &RunId) -> RunExecutionProgress {
            let index = usize::from(self.cursor.fetch_add(1, Ordering::SeqCst));
            let observations = self
                .observations
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            observations
                .get(index)
                .copied()
                .or_else(|| observations.last().copied())
                .unwrap_or(RunExecutionProgress::Unreadable)
        }
    }

    struct ScriptedExecutor {
        delay: Option<Duration>,
        cancelled: Arc<AtomicBool>,
    }

    impl ScriptedExecutor {
        fn wedged(cancelled: Arc<AtomicBool>) -> Self {
            Self {
                delay: None,
                cancelled,
            }
        }

        fn slow(delay: Duration, cancelled: Arc<AtomicBool>) -> Self {
            Self {
                delay: Some(delay),
                cancelled,
            }
        }
    }

    /// Records whether the `apply` future was dropped before completing, i.e.
    /// whether the supervisor cancelled in-flight work.
    struct CancelWitness {
        cancelled: Arc<AtomicBool>,
        completed: bool,
    }

    impl Drop for CancelWitness {
        fn drop(&mut self) {
            if !self.completed {
                self.cancelled.store(true, Ordering::SeqCst);
            }
        }
    }

    #[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
    #[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
    impl CoreExecutor for ScriptedExecutor {
        async fn apply(
            &mut self,
            _run_id: RunId,
            _primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            let mut witness = CancelWitness {
                cancelled: Arc::clone(&self.cancelled),
                completed: false,
            };
            match self.delay {
                Some(delay) => {
                    crate::tokio::time::sleep(delay).await;
                    witness.completed = true;
                    Err(CoreExecutorError::Internal("scripted completion".into()))
                }
                None => {
                    // The field shape: the consumer owns the run and does
                    // nothing with it, forever.
                    std::future::pending::<()>().await;
                    unreachable!("wedged executor never returns")
                }
            }
        }

        async fn cancel_after_boundary(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }

        async fn stop_runtime_executor(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }
    }

    fn run_id() -> RunId {
        RunId::new()
    }

    fn staged_primitive() -> RunPrimitive {
        RunPrimitive::StagedInput(meerkat_core::lifecycle::run_primitive::StagedRunInput {
            boundary: meerkat_core::lifecycle::run_primitive::RunApplyBoundary::RunStart,
            appends: Vec::new(),
            contributing_input_ids: Vec::new(),
            turn_metadata: None,
        })
    }

    const TEST_BOUND: Duration = Duration::from_secs(900);

    /// RED without the bound: `apply_with_execution_start_bound` degenerates to
    /// a bare `executor.apply(..).await` against a consumer that never returns,
    /// and the outer timeout is what turns that into a reported failure instead
    /// of an unattributable hang in a lane whose subject is mute hangs.
    #[tokio::test(start_paused = true)]
    async fn wedged_consumer_produces_a_typed_bounded_outcome() {
        let cancelled = Arc::new(AtomicBool::new(false));
        let mut executor = ScriptedExecutor::wedged(Arc::clone(&cancelled));
        let progress = ScriptedProgress::new(vec![RunExecutionProgress::PrimitiveUnapplied]);
        let run_id = run_id();

        let error = crate::tokio::time::timeout(
            TEST_BOUND * 4,
            apply_with_execution_start_bound(
                &mut executor,
                progress.as_ref(),
                run_id.clone(),
                staged_primitive(),
                Instant::now(),
                TEST_BOUND,
            ),
        )
        .await
        .expect("a consumer that never began executing must not hang mute")
        .expect_err("a consumer that never began executing must produce a typed outcome");

        assert!(
            matches!(
                error,
                CoreExecutorError::TeardownRequired {
                    reason: CoreExecutorTeardownReason::ExecutorNotProgressing,
                    ..
                }
            ),
            "expected a typed ExecutorNotProgressing teardown, got {error:?}"
        );
        assert!(
            error.requires_runtime_teardown(),
            "a wedged consumer must hand its executor off instead of receiving the next batch"
        );
        assert_eq!(
            progress.observation_count(),
            1,
            "escalation must rest on exactly one observation, taken at the bound"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn slow_but_executing_turn_is_not_disturbed_by_the_bound() {
        let cancelled = Arc::new(AtomicBool::new(false));
        // Ten times the bound: the window measures staged -> executing, not
        // how long a live turn is allowed to take.
        let mut executor = ScriptedExecutor::slow(TEST_BOUND * 10, Arc::clone(&cancelled));
        let progress = ScriptedProgress::new(vec![RunExecutionProgress::Executing]);

        let result = apply_with_execution_start_bound(
            &mut executor,
            progress.as_ref(),
            run_id(),
            staged_primitive(),
            Instant::now(),
            TEST_BOUND,
        )
        .await;

        assert!(
            matches!(result, Err(CoreExecutorError::Internal(message)) if message == "scripted completion"),
            "an executing turn must return its own outcome"
        );
        assert!(
            !cancelled.load(Ordering::SeqCst),
            "a live turn must never be cancelled by the execution-start bound"
        );
    }

    /// Time already spent between the `StageForRun` commit and this call is
    /// part of the same window. A run that was staged a full bound ago must
    /// escalate immediately rather than being granted a fresh bound.
    #[tokio::test(start_paused = true)]
    async fn the_bound_is_measured_from_staging_not_from_apply_entry() {
        let cancelled = Arc::new(AtomicBool::new(false));
        let mut executor = ScriptedExecutor::wedged(Arc::clone(&cancelled));
        let progress = ScriptedProgress::new(vec![RunExecutionProgress::PrimitiveUnapplied]);

        let staged_at = Instant::now();
        crate::tokio::time::sleep(TEST_BOUND).await;
        let elapsed_before = Instant::now();

        let error = crate::tokio::time::timeout(
            TEST_BOUND * 4,
            apply_with_execution_start_bound(
                &mut executor,
                progress.as_ref(),
                run_id(),
                staged_primitive(),
                staged_at,
                TEST_BOUND,
            ),
        )
        .await
        .expect("a run already past its window must not be granted a fresh one")
        .expect_err("a run already past its window must produce a typed outcome");

        assert!(
            matches!(
                error,
                CoreExecutorError::TeardownRequired {
                    reason: CoreExecutorTeardownReason::ExecutorNotProgressing,
                    ..
                }
            ),
            "expected a typed ExecutorNotProgressing teardown, got {error:?}"
        );
        assert!(
            elapsed_before.elapsed() < TEST_BOUND,
            "pre-apply time must count against the window, not extend it"
        );
    }

    /// The deliberately-unbounded case: at the bound, an observation that does
    /// not prove non-progress leaves the run in flight even when the consumer
    /// is in fact wedged. That is the price of refusing to terminalize on
    /// unproven evidence, and it is stated here rather than left implied.
    #[tokio::test(start_paused = true)]
    async fn unprovable_observations_leave_even_a_wedged_run_in_flight() {
        for observed in [
            RunExecutionProgress::Unreadable,
            RunExecutionProgress::RuntimeUnbound,
            RunExecutionProgress::ExecutionStartUnobservable,
            RunExecutionProgress::RunNotCurrent,
            RunExecutionProgress::Executing,
        ] {
            let cancelled = Arc::new(AtomicBool::new(false));
            let mut executor = ScriptedExecutor::wedged(Arc::clone(&cancelled));
            let progress = ScriptedProgress::new(vec![observed]);

            // The outer timeout is the harness, not the code under test: it is
            // what turns "still waiting" into an assertion instead of a hang.
            // It also drops the `apply` future, so `cancelled` says nothing
            // here; the sibling slow-but-live test is what proves the bound
            // itself never cancels.
            let outcome = crate::tokio::time::timeout(
                TEST_BOUND * 4,
                apply_with_execution_start_bound(
                    &mut executor,
                    progress.as_ref(),
                    run_id(),
                    staged_primitive(),
                    Instant::now(),
                    TEST_BOUND,
                ),
            )
            .await;

            assert!(
                outcome.is_err(),
                "{} must refuse to terminalize and keep awaiting its consumer",
                observed.as_str()
            );
        }
    }

    /// A merely-slow consumer must keep its own outcome even when the
    /// observation at the bound is unprovable rather than positively healthy.
    #[tokio::test(start_paused = true)]
    async fn unprovable_observations_do_not_disturb_a_slow_but_live_turn() {
        for observed in [
            RunExecutionProgress::Unreadable,
            RunExecutionProgress::RuntimeUnbound,
            RunExecutionProgress::ExecutionStartUnobservable,
            RunExecutionProgress::RunNotCurrent,
        ] {
            let cancelled = Arc::new(AtomicBool::new(false));
            let mut executor = ScriptedExecutor::slow(TEST_BOUND * 10, Arc::clone(&cancelled));
            let progress = ScriptedProgress::new(vec![observed]);

            let result = apply_with_execution_start_bound(
                &mut executor,
                progress.as_ref(),
                run_id(),
                staged_primitive(),
                Instant::now(),
                TEST_BOUND,
            )
            .await;

            assert!(
                matches!(result, Err(CoreExecutorError::Internal(message)) if message == "scripted completion"),
                "{} must leave the run in flight rather than terminalize it",
                observed.as_str()
            );
            assert!(
                !cancelled.load(Ordering::SeqCst),
                "{} must not cancel an in-flight turn",
                observed.as_str()
            );
        }
    }

    /// The defect this classification closes: with the turn start unsignalled,
    /// `turn_phase` belongs to whatever ran last (or a fresh session's
    /// default), so reading "not ApplyingPrimitive" as `Executing` would hand
    /// a false clean bill of health to exactly the classes that skip the
    /// turn-start transition - the transient-turn-context class and the
    /// retired drain.
    #[test]
    fn an_unsignalled_turn_start_is_unobservable_not_executing() {
        for applying_primitive in [true, false] {
            let facts = RunTurnStateFacts {
                runtime_bound: true,
                run_is_current: true,
                applying_primitive,
            };
            assert_eq!(
                classify_execution_start(facts, TurnStartSignal::NotSignalled),
                RunExecutionProgress::ExecutionStartUnobservable,
                "an unsignalled turn start must never be read as a fact about this run"
            );
        }
    }

    #[test]
    fn a_signalled_turn_start_classifies_the_shared_phase_as_this_run() {
        let base = RunTurnStateFacts {
            runtime_bound: true,
            run_is_current: true,
            applying_primitive: true,
        };
        assert_eq!(
            classify_execution_start(base, TurnStartSignal::Signalled),
            RunExecutionProgress::PrimitiveUnapplied
        );
        assert_eq!(
            classify_execution_start(
                RunTurnStateFacts {
                    applying_primitive: false,
                    ..base
                },
                TurnStartSignal::Signalled
            ),
            RunExecutionProgress::Executing
        );
        assert_eq!(
            classify_execution_start(
                RunTurnStateFacts {
                    run_is_current: false,
                    ..base
                },
                TurnStartSignal::Signalled
            ),
            RunExecutionProgress::RunNotCurrent
        );
        for turn_start in [TurnStartSignal::Signalled, TurnStartSignal::NotSignalled] {
            assert_eq!(
                classify_execution_start(
                    RunTurnStateFacts {
                        runtime_bound: false,
                        ..base
                    },
                    turn_start
                ),
                RunExecutionProgress::RuntimeUnbound,
                "an unbound runtime is unobservable regardless of the turn-start signal"
            );
        }
    }

    #[test]
    fn only_positive_facts_close_the_execution_start_window() {
        for observed in [
            RunExecutionProgress::Executing,
            RunExecutionProgress::RunNotCurrent,
        ] {
            assert!(
                observed.closes_execution_start_window(),
                "{} is a positive fact that closes the window",
                observed.as_str()
            );
        }
        for observed in [
            RunExecutionProgress::PrimitiveUnapplied,
            RunExecutionProgress::Unreadable,
            RunExecutionProgress::RuntimeUnbound,
            RunExecutionProgress::ExecutionStartUnobservable,
        ] {
            assert!(
                !observed.closes_execution_start_window(),
                "{} leaves the staged -> executing window open",
                observed.as_str()
            );
        }
    }

    #[test]
    fn only_a_proven_unapplied_primitive_may_terminalize() {
        assert!(RunExecutionProgress::PrimitiveUnapplied.proves_execution_never_started());
        for observed in [
            RunExecutionProgress::Executing,
            RunExecutionProgress::RunNotCurrent,
            RunExecutionProgress::RuntimeUnbound,
            RunExecutionProgress::ExecutionStartUnobservable,
            RunExecutionProgress::Unreadable,
        ] {
            assert!(
                !observed.proves_execution_never_started(),
                "{} is not proof that execution never started",
                observed.as_str()
            );
        }
    }

    struct CountingProgress {
        observation: RunExecutionProgress,
        observations: AtomicUsize,
    }

    impl RunExecutionProgressSource for CountingProgress {
        fn observe(&self, _run_id: &RunId) -> RunExecutionProgress {
            self.observations.fetch_add(1, Ordering::SeqCst);
            self.observation
        }
    }

    /// The watchdog is the half that covers the pre-`apply` segment, so it must
    /// keep reporting for as long as the window stays open and it must never
    /// terminalize anything. `Unreadable` is the shape a consumer wedged while
    /// holding the authority mutex produces, and
    /// `ExecutionStartUnobservable` is the class the bound cannot arm for at
    /// all - both are exactly when an operator most needs the line.
    #[tokio::test(start_paused = true)]
    async fn the_watchdog_keeps_reporting_an_open_window() {
        for observation in [
            RunExecutionProgress::PrimitiveUnapplied,
            RunExecutionProgress::Unreadable,
            RunExecutionProgress::ExecutionStartUnobservable,
            RunExecutionProgress::RuntimeUnbound,
        ] {
            let progress = Arc::new(CountingProgress {
                observation,
                observations: AtomicUsize::new(0),
            });
            let watchdog = StagedRunStartWatchdog::spawn(
                Arc::clone(&progress) as Arc<dyn RunExecutionProgressSource>,
                run_id(),
                vec![InputId::new()],
                Instant::now(),
                Duration::from_secs(120),
            );

            crate::tokio::time::sleep(Duration::from_secs(500)).await;
            drop(watchdog);
            let reported = progress.observations.load(Ordering::SeqCst);
            assert!(
                reported >= 4,
                "{} leaves the window open and must be re-reported every notice interval, got {reported}",
                observation.as_str()
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn the_watchdog_stands_down_once_the_run_is_executing() {
        let progress = Arc::new(CountingProgress {
            observation: RunExecutionProgress::Executing,
            observations: AtomicUsize::new(0),
        });
        let watchdog = StagedRunStartWatchdog::spawn(
            Arc::clone(&progress) as Arc<dyn RunExecutionProgressSource>,
            run_id(),
            vec![InputId::new()],
            Instant::now(),
            Duration::from_secs(120),
        );

        crate::tokio::time::sleep(Duration::from_secs(500)).await;
        drop(watchdog);
        assert_eq!(
            progress.observations.load(Ordering::SeqCst),
            1,
            "a run that began executing must stop being supervised after one observation"
        );
    }

    // -----------------------------------------------------------------------
    // Staged-run window census (`observe_run_start_window`)
    //
    // These tests go through the REAL classification - a real generated
    // authority, `AuthorityRunExecutionProgress::observe`, and
    // `classify_execution_start` - not a scripted source, so that breaking
    // `run_is_current` or `applying_primitive` in either place turns the
    // corresponding test red. A scripted source would stay green under
    // exactly those mutations, which is a test of nothing.
    // -----------------------------------------------------------------------

    /// A shared generated authority whose turn state is exactly the given
    /// facts, built by recovering a mutated clone of a real registered
    /// authority's state (the same recover path production uses for
    /// projection previews).
    fn shared_authority_with(
        current_run: Option<uuid::Uuid>,
        turn_phase: mm_dsl::TurnPhase,
        runtime_bound: bool,
    ) -> crate::driver::ephemeral::SharedIngressDslAuthority {
        let session_id = meerkat_core::types::SessionId::new();
        let authority =
            crate::meerkat_machine::dsl_authority::new_registered_authority_without_runtime_entry(
                &session_id,
            )
            .expect("census test authority must register");
        let mut state = authority.state().clone();
        state.active_runtime_id =
            runtime_bound.then(|| mm_dsl::AgentRuntimeId("census-test-runtime".to_string()));
        // Recovery enforces `runtime_binding_identity_is_typed`: a bound
        // runtime must carry a typed generation (or epoch) beside its id.
        state.active_runtime_generation = runtime_bound.then_some(mm_dsl::Generation(1));
        state.current_run_id = current_run.map(|run| mm_dsl::RunId(run.to_string()));
        // Recovery enforces `current_run_only_while_running_or_retired` and
        // `current_run_has_pre_run_phase`, so a state carrying a current run
        // must also be Running with a recorded pre-run phase to be a state the
        // machine could actually reach.
        if current_run.is_some() {
            state.lifecycle_phase = mm_dsl::MeerkatPhase::Running;
            state.pre_run_phase = Some(mm_dsl::PreRunPhase::Attached);
        }
        state.turn_phase = turn_phase;
        Arc::new(std::sync::Mutex::new(
            crate::meerkat_machine::recover_projected_authority(
                state,
                "census test state must recover",
            ),
        ))
    }

    fn armed_window(run: uuid::Uuid, signalled: bool) -> SharedRunStartWindowCell {
        let cell = SharedRunStartWindowCell::default();
        let turn_start = TurnStartSignalCell::default();
        if signalled {
            turn_start.mark_signalled();
        }
        cell.arm(RunId::from_uuid(run), Instant::now(), turn_start);
        cell
    }

    /// `notice` values that make the elapsed check deterministic without
    /// touching the clock: zero is always past the bound, max is never.
    const PAST_BOUND: Duration = Duration::ZERO;
    const INSIDE_BOUND: Duration = Duration::MAX;

    #[tokio::test]
    async fn census_reports_nothing_for_a_session_that_never_staged_a_run() {
        let authority = shared_authority_with(
            Some(uuid::Uuid::new_v4()),
            mm_dsl::TurnPhase::ApplyingPrimitive,
            true,
        );
        assert_eq!(
            observe_run_start_window(&SharedRunStartWindowCell::default(), &authority, PAST_BOUND),
            RunStartHealth::Clear,
            "an unarmed window is not a wedge"
        );
    }

    #[tokio::test]
    async fn census_stays_clear_inside_the_notice_bound_even_when_unapplied() {
        let run = uuid::Uuid::new_v4();
        let authority =
            shared_authority_with(Some(run), mm_dsl::TurnPhase::ApplyingPrimitive, true);
        assert_eq!(
            observe_run_start_window(&armed_window(run, true), &authority, INSIDE_BOUND),
            RunStartHealth::Clear,
            "a window inside the notice bound is ordinary staging latency, not a wedge"
        );
    }

    /// The positive pin: the exact field shape - staged, signalled, past the
    /// bound, and machine authority still shows this run current with its
    /// primitive un-applied - is Overdue.
    #[tokio::test]
    async fn census_reports_a_wedged_pre_apply_run_as_overdue() {
        let run = uuid::Uuid::new_v4();
        let authority =
            shared_authority_with(Some(run), mm_dsl::TurnPhase::ApplyingPrimitive, true);
        assert_eq!(
            observe_run_start_window(&armed_window(run, true), &authority, PAST_BOUND),
            RunStartHealth::Overdue,
            "a past-bound staged run whose primitive is provably un-applied is the wedge"
        );
    }

    /// A stale window naming a superseded run must NOT fire: another run took
    /// over `current_run_id`, so `run_is_current` fails and the classification
    /// is `RunNotCurrent`, whatever the shared phase says. Breaking the
    /// `run_is_current` check (in `observe` or `classify_execution_start`)
    /// turns this test red.
    #[tokio::test]
    async fn census_never_fires_on_a_stale_window_naming_a_superseded_run() {
        let stale_run = uuid::Uuid::new_v4();
        let successor_run = uuid::Uuid::new_v4();
        let authority = shared_authority_with(
            Some(successor_run),
            mm_dsl::TurnPhase::ApplyingPrimitive,
            true,
        );
        assert_eq!(
            observe_run_start_window(&armed_window(stale_run, true), &authority, PAST_BOUND),
            RunStartHealth::Clear,
            "a stale window may not convert a successor's fresh staging into a wedge claim"
        );
    }

    /// A window whose run progressed must NOT fire: `PrimitiveApplied` moved
    /// the phase off `ApplyingPrimitive`, so the classification is
    /// `Executing` no matter how long ago staging happened. Breaking the
    /// `applying_primitive` check turns this test red.
    #[tokio::test]
    async fn census_never_fires_on_a_run_that_began_executing() {
        let run = uuid::Uuid::new_v4();
        let authority = shared_authority_with(Some(run), mm_dsl::TurnPhase::CallingLlm, true);
        assert_eq!(
            observe_run_start_window(&armed_window(run, true), &authority, PAST_BOUND),
            RunStartHealth::Clear,
            "a run that began its turn is slow work, not a staged wedge"
        );
    }

    /// The warn-tier carve-out: a window whose turn start was never signalled
    /// says nothing about the shared phase, so it is unobservable - never
    /// Overdue, and not Unreadable either.
    #[tokio::test]
    async fn census_refuses_to_read_leftover_phase_for_an_unsignalled_window() {
        let run = uuid::Uuid::new_v4();
        let authority =
            shared_authority_with(Some(run), mm_dsl::TurnPhase::ApplyingPrimitive, true);
        assert_eq!(
            observe_run_start_window(&armed_window(run, false), &authority, PAST_BOUND),
            RunStartHealth::Unreadable,
            "a CURRENT run whose start this read cannot interpret is an absence \
             of observation; publishing it as Clear would render 'I cannot see' \
             as 'healthy'. Folding this arm to Clear must turn this test red."
        );
    }

    /// The anti-amber twin of the test above: an unsignalled window whose run
    /// is NO LONGER CURRENT is positively resolved - the run moved on, however
    /// it ended. This is the appends-empty / retired-drain stale-window shape:
    /// those runs never signal, and a healthy idle session may hold such a
    /// window for days. Folding it to Unreadable would stand that session
    /// permanently amber, which is the muted-alarm defect from the other side.
    #[tokio::test]
    async fn census_clears_an_unsignalled_window_once_its_run_moved_on() {
        let stale_run = uuid::Uuid::new_v4();
        let authority = shared_authority_with(None, mm_dsl::TurnPhase::Ready, true);
        assert_eq!(
            observe_run_start_window(&armed_window(stale_run, false), &authority, PAST_BOUND),
            RunStartHealth::Clear,
            "a window whose run is no longer current is resolved, whatever its signal says"
        );
    }

    /// No runtime binding on a STALE window: resolved by currency before the
    /// binding is ever consulted. A stopped session (executor detached,
    /// current run cleared) holding an old window must not read as anything
    /// but Clear.
    #[tokio::test]
    async fn census_treats_an_unbound_runtime_as_unobservable_not_overdue() {
        let run = uuid::Uuid::new_v4();
        let authority = shared_authority_with(None, mm_dsl::TurnPhase::ApplyingPrimitive, false);
        assert_eq!(
            observe_run_start_window(&armed_window(run, true), &authority, PAST_BOUND),
            RunStartHealth::Clear,
            "an unbound runtime with the window's run no longer current is a \
             resolved window, not a wedge and not an unreadable"
        );
    }

    /// No runtime binding on a CURRENT run: an absence of observation, so
    /// Unreadable. The recovered state is unusual (Running with no bound
    /// runtime) but no machine invariant forbids it, and the census guards
    /// the branch rather than assuming it away.
    #[tokio::test]
    async fn census_reports_an_unbound_current_run_as_unreadable() {
        let run = uuid::Uuid::new_v4();
        let authority =
            shared_authority_with(Some(run), mm_dsl::TurnPhase::ApplyingPrimitive, false);
        assert_eq!(
            observe_run_start_window(&armed_window(run, true), &authority, PAST_BOUND),
            RunStartHealth::Unreadable,
            "a current run whose writes land where this observer cannot see \
             has not been proven healthy by anyone"
        );
    }

    /// A held authority must surface as Unreadable - not block, not clear.
    /// `std::sync::Mutex::try_lock` fails from the same thread while the
    /// guard is alive, which is exactly the wedged-holder shape in miniature.
    #[tokio::test]
    async fn census_reports_a_held_authority_as_unreadable_without_blocking() {
        let run = uuid::Uuid::new_v4();
        let authority =
            shared_authority_with(Some(run), mm_dsl::TurnPhase::ApplyingPrimitive, true);
        let window = armed_window(run, true);
        let guard = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(
            observe_run_start_window(&window, &authority, PAST_BOUND),
            RunStartHealth::Unreadable,
            "a held authority is unprovable, and the probe must say so rather than wait"
        );
        drop(guard);
        assert_eq!(
            observe_run_start_window(&window, &authority, PAST_BOUND),
            RunStartHealth::Overdue,
            "the same window reads normally once the authority is released"
        );
    }

    /// The void condition as a source gate: the window cell and its census
    /// vocabulary may not appear in machine authority or dispatch code. This
    /// returns zero hits today, so the gate is meaningful from the day it
    /// lands - a hit means someone made the mechanical observation semantic,
    /// which requires a machine owner, not a new caller.
    #[test]
    fn run_start_window_stays_out_of_machine_authority() {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let machine_dir = manifest.join("src/meerkat_machine");
        let mut files = vec![
            machine_dir.join("dsl.rs"),
            machine_dir.join("dsl_authority.rs"),
            machine_dir.join("dsl_effects.rs"),
            machine_dir.join("composition.rs"),
        ];
        let dispatch_entries =
            std::fs::read_dir(&machine_dir).expect("machine authority directory must be readable");
        for entry in dispatch_entries {
            let path = entry
                .expect("machine authority entry must be readable")
                .path();
            if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("dispatch_") && name.ends_with(".rs"))
            {
                files.push(path);
            }
        }
        let generated_dir = manifest.join("src/generated");
        for entry in
            std::fs::read_dir(&generated_dir).expect("generated directory must be readable")
        {
            let path = entry.expect("generated entry must be readable").path();
            if path.extension().is_some_and(|ext| ext == "rs") {
                files.push(path);
            }
        }
        // The kernels codegen output lives in a sibling crate, present in the
        // workspace layout but not in a packaged meerkat-runtime; gate it when
        // it is there rather than failing a crate-local build.
        if let Some(workspace) = manifest.parent() {
            let kernels = workspace.join("meerkat-machine-kernels/src/generated/meerkat.rs");
            if kernels.exists() {
                files.push(kernels);
            }
        }

        let forbidden = [
            "run_start_window",
            "RunStartWindow",
            "RunStartHealth",
            "SharedRunStartWindowCell",
            "observe_run_start_window",
            "overdue_run_start",
            "ParkedQueuedWorkHealth",
            "ParkedQueuedWorkAxis",
            "observe_parked_queued_work",
            "parked_queued_work_health",
            "parked_queued_input_session_count",
            "QUEUED_INPUT_START_NOTICE",
            "PARKED_STAGE_CHURN_ATTEMPTS",
        ];
        for file in files {
            let source = std::fs::read_to_string(&file)
                .unwrap_or_else(|error| panic!("{} must be readable: {error}", file.display()));
            for token in forbidden {
                assert!(
                    !source.contains(token),
                    "{} references `{token}`: the staged-run window is mechanical \
                     observation for the health census only; a machine-authority or \
                     dispatch consumer makes it a semantic fact that needs a machine \
                     owner (see the void condition on SharedRunStartWindowCell)",
                    file.display()
                );
            }
        }
    }

    // -----------------------------------------------------------------------
    // Parked-queued-work census (`observe_parked_queued_work`)
    //
    // Like the staged-run census above, these route through the REAL
    // generated authority and the REAL ledger clock, so voiding the
    // run-in-flight exclusion, the queued-phase filter, or the age comparison
    // each turns exactly one named test red.
    // -----------------------------------------------------------------------

    /// A shared generated authority whose registration, run, and input-phase
    /// facts are exactly the given ones, recovered through the same path
    /// production uses for projection previews.
    fn parked_census_authority(
        registration_active: bool,
        current_run: Option<uuid::Uuid>,
        inputs: &[(InputId, mm_dsl::InputPhase, u64)],
    ) -> crate::driver::ephemeral::SharedIngressDslAuthority {
        let session_id = meerkat_core::types::SessionId::new();
        let authority =
            crate::meerkat_machine::dsl_authority::new_registered_authority_without_runtime_entry(
                &session_id,
            )
            .expect("parked census test authority must register");
        let mut state = authority.state().clone();
        if registration_active {
            state.registration_phase = mm_dsl::RegistrationPhase::Active;
        }
        state.current_run_id = current_run.map(|run| mm_dsl::RunId(run.to_string()));
        if current_run.is_some() {
            state.lifecycle_phase = mm_dsl::MeerkatPhase::Running;
            state.pre_run_phase = Some(mm_dsl::PreRunPhase::Attached);
        }
        for (input_id, phase, attempts) in inputs {
            // Production writes these keys as `InputId::to_string()`
            // (`driver/ephemeral.rs` phase reads use the same form), so the
            // fixture must too or it would test a correspondence nothing uses.
            state.input_phases.insert(input_id.to_string(), *phase);
            state
                .input_attempt_counts
                .insert(input_id.to_string(), *attempts);
            if *phase == mm_dsl::InputPhase::Queued {
                state
                    .input_lane
                    .insert(input_id.to_string(), mm_dsl::InputLane::Queue);
            }
        }
        Arc::new(std::sync::Mutex::new(
            crate::meerkat_machine::recover_projected_authority(
                state,
                "parked census state must recover",
            ),
        ))
    }

    /// A real ephemeral driver entry whose ledger holds exactly the given
    /// rows, each with controlled `updated_at` and `created_at` clocks.
    fn parked_census_driver(
        rows: &[(
            InputId,
            chrono::DateTime<chrono::Utc>,
            chrono::DateTime<chrono::Utc>,
        )],
    ) -> crate::meerkat_machine::driver::SharedDriver {
        let session_id = meerkat_core::types::SessionId::new();
        let mut driver = crate::driver::ephemeral::EphemeralRuntimeDriver::new(
            crate::identifiers::LogicalRuntimeId::for_session(&session_id),
        );
        for (input_id, updated_at, created_at) in rows {
            let mut state = crate::input_state::InputState::new_accepted(input_id.clone());
            state.updated_at = *updated_at;
            state.created_at = *created_at;
            driver.insert_input_state_for_test(state);
        }
        Arc::new(crate::tokio::sync::Mutex::new(
            crate::meerkat_machine::driver::DriverEntry::Ephemeral(driver),
        ))
    }

    const PARKED_NOTICE: Duration = Duration::from_secs(120);

    /// The positive pin: the exact field shape. Registration `Active`, no run
    /// in flight, an input in the machine's queued phase, and its ledger
    /// clock says it entered that state past the notice bound.
    #[test]
    fn parked_census_reports_queued_work_past_the_bound() {
        let now = chrono::Utc::now();
        let input_id = InputId::new();
        let authority = parked_census_authority(
            true,
            None,
            &[(input_id.clone(), mm_dsl::InputPhase::Queued, 0)],
        );
        let driver = parked_census_driver(&[(
            input_id,
            now - chrono::Duration::seconds(600),
            now - chrono::Duration::seconds(600),
        )]);
        assert_eq!(
            observe_parked_queued_work(&authority, &driver, PARKED_NOTICE, now),
            ParkedQueuedWorkHealth::Parked(ParkedQueuedWorkAxis::AgedInQueue),
            "queued work aged past the bound with nothing running is the wedge"
        );
    }

    /// Queued work waiting behind a live turn is a backlog, not a wedge.
    /// Voiding the run-in-flight exclusion turns this test red.
    #[test]
    fn parked_census_stays_clear_while_a_run_is_in_flight() {
        let now = chrono::Utc::now();
        let input_id = InputId::new();
        let authority = parked_census_authority(
            true,
            Some(uuid::Uuid::new_v4()),
            &[(input_id.clone(), mm_dsl::InputPhase::Queued, 0)],
        );
        let driver = parked_census_driver(&[(
            input_id,
            now - chrono::Duration::seconds(600),
            now - chrono::Duration::seconds(600),
        )]);
        assert_eq!(
            observe_parked_queued_work(&authority, &driver, PARKED_NOTICE, now),
            ParkedQueuedWorkHealth::Clear,
            "aged queued work behind a live run must not be read as a wedge"
        );
    }

    /// A session whose executor registration is not `Active` could not stage
    /// anything, so it is not accused of failing to.
    #[test]
    fn parked_census_stays_clear_without_an_active_registration() {
        let now = chrono::Utc::now();
        let input_id = InputId::new();
        let authority = parked_census_authority(
            false,
            None,
            &[(input_id.clone(), mm_dsl::InputPhase::Queued, 0)],
        );
        let driver = parked_census_driver(&[(
            input_id,
            now - chrono::Duration::seconds(600),
            now - chrono::Duration::seconds(600),
        )]);
        assert_eq!(
            observe_parked_queued_work(&authority, &driver, PARKED_NOTICE, now),
            ParkedQueuedWorkHealth::Clear,
            "nothing could stage here, so nothing failed to"
        );
    }

    /// Only the queued phase names selectable parked work. Voiding the phase
    /// filter turns this test red.
    #[test]
    fn parked_census_ignores_inputs_outside_the_queued_phase() {
        let now = chrono::Utc::now();
        let input_id = InputId::new();
        let authority = parked_census_authority(
            true,
            None,
            &[(input_id.clone(), mm_dsl::InputPhase::Consumed, 0)],
        );
        let driver = parked_census_driver(&[(
            input_id,
            now - chrono::Duration::seconds(600),
            now - chrono::Duration::seconds(600),
        )]);
        assert_eq!(
            observe_parked_queued_work(&authority, &driver, PARKED_NOTICE, now),
            ParkedQueuedWorkHealth::Clear,
            "an aged input outside the queued phase is another stage's concern"
        );
    }

    /// Inside the bound the same facts are ordinary admission latency.
    /// Voiding the age comparison turns this test red.
    #[test]
    fn parked_census_stays_clear_inside_the_notice_bound() {
        let now = chrono::Utc::now();
        let input_id = InputId::new();
        let authority = parked_census_authority(
            true,
            None,
            &[(input_id.clone(), mm_dsl::InputPhase::Queued, 0)],
        );
        let driver = parked_census_driver(&[(
            input_id,
            now - chrono::Duration::seconds(30),
            now - chrono::Duration::seconds(30),
        )]);
        assert_eq!(
            observe_parked_queued_work(&authority, &driver, PARKED_NOTICE, now),
            ParkedQueuedWorkHealth::Clear,
            "thirty seconds queued is latency, not a wedge"
        );
    }

    /// With no queued work the driver lock is never attempted: the probe
    /// short-circuits on machine truth alone. Pinned by holding the driver
    /// guard and still reading `Clear` rather than `Unreadable`.
    #[test]
    fn parked_census_never_touches_the_driver_without_queued_work() {
        let now = chrono::Utc::now();
        let authority = parked_census_authority(true, None, &[]);
        let driver = parked_census_driver(&[]);
        let held = driver.try_lock().expect("fixture driver is uncontended");
        assert_eq!(
            observe_parked_queued_work(&authority, &driver, PARKED_NOTICE, now),
            ParkedQueuedWorkHealth::Clear,
            "no queued work means no driver read, held or not"
        );
        drop(held);
    }

    /// A queued id with no ledger row is a transient mid-mutation window; the
    /// probe skips it rather than asserting an age nobody measured.
    #[test]
    fn parked_census_skips_a_queued_id_with_no_ledger_row() {
        let now = chrono::Utc::now();
        let input_id = InputId::new();
        let authority =
            parked_census_authority(true, None, &[(input_id, mm_dsl::InputPhase::Queued, 0)]);
        let driver = parked_census_driver(&[]);
        assert_eq!(
            observe_parked_queued_work(&authority, &driver, PARKED_NOTICE, now),
            ParkedQueuedWorkHealth::Clear,
            "a phase entry with no ledger clock proves nothing about age"
        );
    }

    /// A held authority surfaces as Unreadable without blocking - the wedged
    /// holder shape, on the first of the two locks.
    #[test]
    fn parked_census_reports_a_held_authority_as_unreadable() {
        let now = chrono::Utc::now();
        let input_id = InputId::new();
        let authority = parked_census_authority(
            true,
            None,
            &[(input_id.clone(), mm_dsl::InputPhase::Queued, 0)],
        );
        let driver = parked_census_driver(&[(
            input_id,
            now - chrono::Duration::seconds(600),
            now - chrono::Duration::seconds(600),
        )]);
        let guard = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(
            observe_parked_queued_work(&authority, &driver, PARKED_NOTICE, now),
            ParkedQueuedWorkHealth::Unreadable,
            "a held authority is unprovable, and the probe must say so rather than wait"
        );
        drop(guard);
    }

    /// A held driver surfaces as Unreadable without blocking - the same shape
    /// on the second lock - and clears on the next read once released.
    #[test]
    fn parked_census_reports_a_held_driver_as_unreadable() {
        let now = chrono::Utc::now();
        let input_id = InputId::new();
        let authority = parked_census_authority(
            true,
            None,
            &[(input_id.clone(), mm_dsl::InputPhase::Queued, 0)],
        );
        let driver = parked_census_driver(&[(
            input_id,
            now - chrono::Duration::seconds(600),
            now - chrono::Duration::seconds(600),
        )]);
        let held = driver.try_lock().expect("fixture driver is uncontended");
        assert_eq!(
            observe_parked_queued_work(&authority, &driver, PARKED_NOTICE, now),
            ParkedQueuedWorkHealth::Unreadable,
            "a held driver ledger is unprovable, and the probe must not join the queue behind it"
        );
        drop(held);
        assert_eq!(
            observe_parked_queued_work(&authority, &driver, PARKED_NOTICE, now),
            ParkedQueuedWorkHealth::Parked(ParkedQueuedWorkAxis::AgedInQueue),
            "the same facts read normally once the driver is released"
        );
    }

    /// THE WORST CASE, pinned: a stage-churning input. Every Staged -> Queued
    /// rollback re-stamps `updated_at`, so this input's age clock reads
    /// seconds-old forever - and the age axis alone would read the most
    /// alarming member as the healthiest. The machine's own attempt count is
    /// what survives the re-stamp. Voiding the churn axis turns this test
    /// red.
    #[test]
    fn parked_census_sees_a_stage_churning_input_whose_age_clock_resets() {
        let now = chrono::Utc::now();
        let input_id = InputId::new();
        let authority = parked_census_authority(
            true,
            None,
            &[(
                input_id.clone(),
                mm_dsl::InputPhase::Queued,
                PARKED_STAGE_CHURN_ATTEMPTS,
            )],
        );
        // updated_at is FRESH - the rollback just re-stamped it - while
        // created_at carries the input's true age.
        let driver = parked_census_driver(&[(
            input_id,
            now - chrono::Duration::seconds(5),
            now - chrono::Duration::seconds(600),
        )]);
        assert_eq!(
            observe_parked_queued_work(&authority, &driver, PARKED_NOTICE, now),
            ParkedQueuedWorkHealth::Parked(ParkedQueuedWorkAxis::StageChurn),
            "an input staged and thrown back twice, queued again with nothing \
             running, is churn - and its re-stamped age clock must not hide it"
        );
    }

    /// The churn axis has a floor: a young input inside its ordinary first
    /// bounces is not churn yet, however many attempts it burned quickly.
    #[test]
    fn parked_census_gives_a_young_churning_input_the_notice_floor() {
        let now = chrono::Utc::now();
        let input_id = InputId::new();
        let authority = parked_census_authority(
            true,
            None,
            &[(
                input_id.clone(),
                mm_dsl::InputPhase::Queued,
                PARKED_STAGE_CHURN_ATTEMPTS,
            )],
        );
        let driver = parked_census_driver(&[(
            input_id,
            now - chrono::Duration::seconds(5),
            now - chrono::Duration::seconds(30),
        )]);
        assert_eq!(
            observe_parked_queued_work(&authority, &driver, PARKED_NOTICE, now),
            ParkedQueuedWorkHealth::Clear,
            "thirty seconds of existence is the ordinary bounce window, not churn"
        );
    }

    /// Below the attempt threshold the churn axis stays silent: one staging
    /// followed by one rollback is a recovery the machine performed once, not
    /// a loop.
    #[test]
    fn parked_census_ignores_a_single_recovered_attempt() {
        let now = chrono::Utc::now();
        let input_id = InputId::new();
        let authority = parked_census_authority(
            true,
            None,
            &[(input_id.clone(), mm_dsl::InputPhase::Queued, 1)],
        );
        let driver = parked_census_driver(&[(
            input_id,
            now - chrono::Duration::seconds(5),
            now - chrono::Duration::seconds(600),
        )]);
        assert_eq!(
            observe_parked_queued_work(&authority, &driver, PARKED_NOTICE, now),
            ParkedQueuedWorkHealth::Clear,
            "one attempt and one rollback is an ordinary recovery, not churn"
        );
    }
}
