//! Host admission of a resumed run.
//!
//! A durable session that stopped inside a run keeps that run open: on the
//! next open its runtime resumes the run from the session's run cursor
//! instead of running the run's inputs again. Before it does, the runtime
//! asks the admission bound to the session's registration, once per resume,
//! whether the run's work is still wanted. A host that tracks work outside
//! the session (a work item claim, for example) answers that a run whose work
//! moved on is superseded, and the runtime then settles the run the way a
//! stop would, without running it. An admission that cannot decide leaves
//! the run marked, and the runtime asks it again later.
//!
//! The admission is optional. A registration without one resumes its run.

use std::sync::Arc;
use std::time::Duration;

use meerkat_core::lifecycle::{InputId, RunId};
use meerkat_core::types::SessionId;

use crate::identifiers::LogicalRuntimeId;

/// Attempts the runtime makes, each time it asks, while an admission answers
/// [`ResumedRunAdmissionDecision::Unavailable`] or does not answer in time.
pub const RESUMED_RUN_ADMISSION_ATTEMPTS: u32 = 3;

/// How long one attempt may take. An attempt that takes longer counts as
/// [`ResumedRunAdmissionDecision::Unavailable`].
pub const RESUMED_RUN_ADMISSION_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(8);

/// The pause between two attempts. With the attempt timeout, the runtime
/// stops asking an admission that stays unavailable after about 30 seconds,
/// and asks it again at the next wake of the session's runtime loop or at
/// the session's next registration.
pub const RESUMED_RUN_ADMISSION_RETRY_DELAY: Duration = Duration::from_secs(3);

/// What the runtime asks before it resumes a run.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ResumedRunAdmissionRequest {
    /// The runtime that holds the run.
    pub runtime_id: LogicalRuntimeId,
    /// The session the run belongs to.
    pub session_id: SessionId,
    /// The run the runtime would resume, under its original id.
    pub run_id: RunId,
    /// Every input bound to the run as of its last committed round.
    pub bound_input_ids: Vec<InputId>,
    /// The turn tool overlay the resumed run executes under: the one its
    /// bound inputs carry, merged as their run merges it. `None` when they
    /// carry none.
    pub turn_tool_overlay: Option<meerkat_core::TurnToolOverlay>,
}

impl ResumedRunAdmissionRequest {
    #[must_use]
    pub fn new(
        runtime_id: LogicalRuntimeId,
        session_id: SessionId,
        run_id: RunId,
        bound_input_ids: Vec<InputId>,
    ) -> Self {
        Self {
            runtime_id,
            session_id,
            run_id,
            bound_input_ids,
            turn_tool_overlay: None,
        }
    }

    /// The same request with the overlay the resumed run executes under.
    #[must_use]
    pub fn with_turn_tool_overlay(
        mut self,
        overlay: Option<meerkat_core::TurnToolOverlay>,
    ) -> Self {
        self.turn_tool_overlay = overlay;
        self
    }
}

/// The admission's answer.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ResumedRunAdmissionDecision {
    /// Resume the run.
    Continue,
    /// The run's work moved on. The runtime settles the run at once, like a
    /// stop: its bound inputs are abandoned as refused resumes and the run
    /// ends failed. It is never resumed.
    Superseded {
        /// Why, for the run's terminal record.
        detail: String,
    },
    /// The admission cannot decide now, for example because the record it
    /// reads is unreachable. The runtime asks again, up to
    /// [`RESUMED_RUN_ADMISSION_ATTEMPTS`] times; when every attempt answers
    /// so (or does not answer in time), the run is neither resumed nor
    /// settled: it stays marked, its inputs bound and none ended, and the
    /// runtime asks again at the next wake of the session's runtime loop or
    /// at the session's next registration, never on a timer. An admission
    /// that cannot decide never lets the run resume, and never ends it.
    Unavailable {
        /// Why: for the runtime's log, and for a host that reads the held
        /// run ([`ResumedRunAdmissionUndecided::reason`], cut at
        /// [`RESUMED_RUN_ADMISSION_REASON_MAX_BYTES`]). Write nothing here
        /// that a reader of either must not see.
        detail: String,
    },
}

/// Host admission of a resumed run, bound to one session registration with
/// [`crate::MeerkatMachine::bind_resumed_run_admission`].
///
/// For each resume the runtime asks once per attempt, up to
/// [`RESUMED_RUN_ADMISSION_ATTEMPTS`] attempts, after the session's executor
/// is attached and before the run is authorized again, never while it holds
/// the session's queue; a run whose attempts all went unanswered is asked
/// about again later, the same way. An attempt that runs past
/// [`RESUMED_RUN_ADMISSION_ATTEMPT_TIMEOUT`] is cancelled (its future is
/// dropped) and counts as unavailable, so an implementation must be
/// cancellation-safe, and idempotent across the attempts for one resume. It
/// answers from its own records and maps its own failures to
/// [`ResumedRunAdmissionDecision::Unavailable`].
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
pub trait ResumedRunAdmission: Send + Sync {
    async fn admit_resumed_run(
        &self,
        request: &ResumedRunAdmissionRequest,
    ) -> ResumedRunAdmissionDecision;
}

/// The most bytes of an undecided round's reason text the runtime keeps
/// ([`ResumedRunAdmissionUndecided::reason`]). A longer text is cut at a
/// character boundary.
pub const RESUMED_RUN_ADMISSION_REASON_MAX_BYTES: usize = 512;

/// How long a read of a session's marked run
/// ([`crate::MeerkatMachine::pending_resumed_run`]) waits for that session's
/// own state before it gives up and answers
/// [`PendingResumedRunRead::NotReadableNow`].
///
/// It is the length of that one wait and no promise of how long the call
/// takes: the lookup of the session in the machine's session map comes
/// before the wait, and the caller's task is scheduled like any other.
pub const PENDING_RESUMED_RUN_READ_BOUND: Duration = Duration::from_millis(100);

/// What [`crate::MeerkatMachine::pending_resumed_run`] answers for a session
/// whose registration marks a run.
///
/// Matching it from another crate: the enum and its
/// [`NotReadableNow`](Self::NotReadableNow) variant are both non-exhaustive,
/// so the match keeps a wildcard arm and names that variant's field with
/// `..`:
///
/// ```
/// use meerkat_runtime::PendingResumedRunRead;
///
/// fn describe(read: &PendingResumedRunRead) -> String {
///     match read {
///         PendingResumedRunRead::Pending(run) => format!("run {} waits", run.run_id),
///         PendingResumedRunRead::NotReadableNow { session_id, .. } => {
///             format!("session {session_id} cannot be read now")
///         }
///         _ => "an answer this caller does not know".to_string(),
///     }
/// }
/// ```
///
/// A pattern of that variant without `..` does not compile outside this
/// crate:
///
/// ```compile_fail,E0638
/// use meerkat_runtime::PendingResumedRunRead;
///
/// fn not_readable(read: &PendingResumedRunRead) -> bool {
///     matches!(read, PendingResumedRunRead::NotReadableNow { session_id: _ })
/// }
/// ```
///
/// Neither does building the variant: the runtime is its only producer.
///
/// ```compile_fail,E0639
/// use meerkat_runtime::PendingResumedRunRead;
///
/// fn build(session_id: meerkat_core::SessionId) -> PendingResumedRunRead {
///     PendingResumedRunRead::NotReadableNow { session_id }
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PendingResumedRunRead {
    /// The run the registration marked resumable and has neither resumed nor
    /// settled.
    Pending(PendingResumedRun),
    /// The session's state cannot be read now: something held it for longer
    /// than the read waits for it ([`PENDING_RESUMED_RUN_READ_BOUND`]; a
    /// store commit in flight, for example), or the session's registration
    /// needs a reload, whose in-memory state is not answered. Ask again.
    /// Whether the session marks a run is not known from this answer.
    ///
    /// The variant is non-exhaustive: another crate matches it with `..`
    /// and cannot build it, which keeps its field list open.
    #[non_exhaustive]
    NotReadableNow {
        /// The session that could not be read.
        session_id: SessionId,
    },
}

/// A run its session's registration marked resumable and has neither resumed
/// nor settled.
///
/// The machine says which run is marked, whether it is held for its
/// admission and whether its stop was asked. Reading it asks nothing of the
/// admission or the store and changes nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct PendingResumedRun {
    /// The session the run belongs to.
    pub session_id: SessionId,
    /// The marked run, under its original id.
    pub run_id: RunId,
    /// How long this registration has marked the run, by this process's
    /// monotonic clock. It starts when the registration marks the run and
    /// starts over at every registration. It is an observation of when: no
    /// decision reads it. `None` when this registration holds no stamp of
    /// the mark of this run (the machine marks a run the registration did
    /// not stamp): the time is then not known, which a zero duration could
    /// not say apart from a run marked this instant.
    pub marked_for: Option<Duration>,
    /// A stop of the run is recorded and its settlement has not committed:
    /// the run never resumes, and it is answered until the runtime loop has
    /// settled it, which waits for the session's store to take writes. A
    /// process that ends first loses the recorded stop: the next
    /// registration marks the run again.
    pub stop_requested: bool,
    /// Where the run stands with its admission in this registration.
    pub admission: ResumedRunAdmissionState,
}

/// Where a run marked resumable stands with its admission, in the
/// registration that marks it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ResumedRunAdmissionState {
    /// The run is marked and the machine does not hold it for its admission:
    /// it was not asked about yet (no executor is attached, the session's
    /// runtime loop has not reached it, or it waits for something else
    /// first: its controller, its destination, a host's hold of the
    /// session's run starts), its first round is under way, or its
    /// admission let it continue and it is about to resume. A registration
    /// without an admission reads so until its run resumes.
    MarkedNotHeld,
    /// The machine holds the run for its admission: a round ended undecided,
    /// and the run is neither resumed nor ended.
    Held(ResumedRunAdmissionHeld),
}

/// A run the machine holds for its admission.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ResumedRunAdmissionHeld {
    /// What this registration noted of the run's undecided rounds. `None`
    /// when it noted none of this run: the machine's hold is what says the
    /// run is held, and the rounds are diagnostics beside it.
    pub undecided: Option<ResumedRunAdmissionUndecided>,
}

/// The undecided rounds of a held run that this registration noted. A new
/// registration starts over.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ResumedRunAdmissionUndecided {
    /// The rounds this registration asked about the run that ended
    /// undecided. Only a round the runtime asks moves it (a wake of the
    /// session's loop asks one); a read never does.
    pub rounds: u64,
    /// How long ago the first of those rounds ended, by this process's
    /// monotonic clock.
    pub undecided_for: Duration,
    /// How the last of those rounds ended.
    pub cause: ResumedRunAdmissionUndecidedCause,
    /// The last round's reason: the detail of the admission's `Unavailable`
    /// answer, or that it did not answer in time. Diagnostic text for a
    /// person: it is the admission's own wording, it may change between
    /// releases, and it is cut at
    /// [`RESUMED_RUN_ADMISSION_REASON_MAX_BYTES`]. Decide from
    /// [`Self::cause`], never by matching this text.
    pub reason: String,
}

/// How an undecided round of a resumed-run admission ended: what its last
/// attempt did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ResumedRunAdmissionUndecidedCause {
    /// The admission did not answer within
    /// [`RESUMED_RUN_ADMISSION_ATTEMPT_TIMEOUT`].
    TimedOut,
    /// The admission answered [`ResumedRunAdmissionDecision::Unavailable`].
    AnsweredUnavailable,
}

/// `reason`, cut to [`RESUMED_RUN_ADMISSION_REASON_MAX_BYTES`] at a character
/// boundary.
fn bounded_undecided_reason(reason: &str) -> String {
    if reason.len() <= RESUMED_RUN_ADMISSION_REASON_MAX_BYTES {
        return reason.to_string();
    }
    let mut end = RESUMED_RUN_ADMISSION_REASON_MAX_BYTES;
    while !reason.is_char_boundary(end) {
        end -= 1;
    }
    reason[..end].to_string()
}

/// What one registration noted of the undecided rounds it asked about one
/// run: diagnostics beside the machine's hold. It names its run, so it is
/// never shown for another; the machine alone says whether a run is held.
#[derive(Debug, Clone)]
pub(crate) struct UndecidedRoundsNote {
    run_id: RunId,
    rounds: u64,
    first_undecided_at: meerkat_core::time_compat::Instant,
    cause: ResumedRunAdmissionUndecidedCause,
    reason: String,
}

impl UndecidedRoundsNote {
    /// One more round asked about `run_id` ended undecided at `now`, by
    /// `cause` and with `reason`. A note of the same run counts on and keeps
    /// the time of its first round; a note of another run, or none, starts
    /// over. The caller notes only a round whose hold the machine accepted.
    /// The note keeps `reason` cut at
    /// [`RESUMED_RUN_ADMISSION_REASON_MAX_BYTES`]: the bound is applied
    /// here, where the reason is stored, so it holds for every caller.
    pub(crate) fn note_round(
        note: &mut Option<Self>,
        run_id: &RunId,
        cause: ResumedRunAdmissionUndecidedCause,
        reason: &str,
        now: meerkat_core::time_compat::Instant,
    ) {
        let reason = bounded_undecided_reason(reason);
        match note.as_mut() {
            Some(noted) if &noted.run_id == run_id => {
                noted.rounds = noted.rounds.saturating_add(1);
                noted.cause = cause;
                noted.reason = reason;
            }
            _ => {
                *note = Some(Self {
                    run_id: run_id.clone(),
                    rounds: 1,
                    first_undecided_at: now,
                    cause,
                    reason,
                });
            }
        }
    }

    fn observed(&self, now: meerkat_core::time_compat::Instant) -> ResumedRunAdmissionUndecided {
        ResumedRunAdmissionUndecided {
            rounds: self.rounds,
            undecided_for: now.saturating_duration_since(self.first_undecided_at),
            cause: self.cause,
            reason: self.reason.clone(),
        }
    }
}

/// Where the run a registration marks stands with its admission, from the
/// machine's mark, the machine's hold and the registration's note, read
/// together: `None` with no mark; marked and not held unless the machine's
/// hold names the marked run; held then, with the note's rounds only when
/// the note names that same run.
pub(crate) fn resumed_run_admission_state(
    mark: Option<&RunId>,
    hold: Option<&RunId>,
    note: Option<&UndecidedRoundsNote>,
    now: meerkat_core::time_compat::Instant,
) -> Option<ResumedRunAdmissionState> {
    let marked = mark?;
    if hold != Some(marked) {
        return Some(ResumedRunAdmissionState::MarkedNotHeld);
    }
    Some(ResumedRunAdmissionState::Held(ResumedRunAdmissionHeld {
        undecided: note
            .filter(|noted| &noted.run_id == marked)
            .map(|noted| noted.observed(now)),
    }))
}

/// The runtime's final verdict for one resume after it asked the admission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ResumedRunAdmissionVerdict {
    Continue,
    /// Settle the run as a refused resume, with this detail.
    Refused {
        detail: String,
    },
    /// Every attempt went unanswered: leave the run marked, held, and ask
    /// again later. `cause` is how the last attempt ended and `detail` the
    /// last reason the admission gave.
    Undecided {
        cause: ResumedRunAdmissionUndecidedCause,
        detail: String,
    },
}

/// Ask `admission` about `request` within the bounded attempts.
///
/// No admission resumes the run. `Superseded` settles it at once. An
/// admission that stays unavailable, or does not answer in time, through
/// every attempt leaves it undecided, with the last reason it gave.
pub(crate) async fn ask_resumed_run_admission(
    admission: Option<&Arc<dyn ResumedRunAdmission>>,
    request: &ResumedRunAdmissionRequest,
) -> ResumedRunAdmissionVerdict {
    let Some(admission) = admission else {
        return ResumedRunAdmissionVerdict::Continue;
    };
    let mut last_detail = String::new();
    let mut last_cause = ResumedRunAdmissionUndecidedCause::AnsweredUnavailable;
    for attempt in 1..=RESUMED_RUN_ADMISSION_ATTEMPTS {
        let answer = crate::tokio::time::timeout(
            RESUMED_RUN_ADMISSION_ATTEMPT_TIMEOUT,
            admission.admit_resumed_run(request),
        )
        .await;
        match answer {
            Ok(ResumedRunAdmissionDecision::Continue) => {
                return ResumedRunAdmissionVerdict::Continue;
            }
            Ok(ResumedRunAdmissionDecision::Superseded { detail }) => {
                return ResumedRunAdmissionVerdict::Refused {
                    detail: format!("the resumed run was superseded: {detail}"),
                };
            }
            Ok(ResumedRunAdmissionDecision::Unavailable { detail }) => {
                last_detail = detail;
                last_cause = ResumedRunAdmissionUndecidedCause::AnsweredUnavailable;
            }
            Err(_elapsed) => {
                last_detail = format!(
                    "the admission did not answer within {RESUMED_RUN_ADMISSION_ATTEMPT_TIMEOUT:?}"
                );
                last_cause = ResumedRunAdmissionUndecidedCause::TimedOut;
            }
        }
        tracing::warn!(
            session_id = %request.session_id,
            run_id = %request.run_id,
            attempt,
            detail = %last_detail,
            "resumed run admission is unavailable"
        );
        if attempt < RESUMED_RUN_ADMISSION_ATTEMPTS {
            crate::tokio::time::sleep(RESUMED_RUN_ADMISSION_RETRY_DELAY).await;
        }
    }
    ResumedRunAdmissionVerdict::Undecided {
        cause: last_cause,
        detail: last_detail,
    }
}

/// One registration's admission slot. Every registration owns a fresh, empty
/// slot; a replaced or reopened registration therefore starts without one.
#[derive(Clone, Default)]
pub(crate) struct ResumedRunAdmissionSlot(Arc<std::sync::OnceLock<Arc<dyn ResumedRunAdmission>>>);

impl std::fmt::Debug for ResumedRunAdmissionSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResumedRunAdmissionSlot")
            .field("bound", &self.0.get().is_some())
            .finish()
    }
}

impl ResumedRunAdmissionSlot {
    /// Bind `admission`. Binding the same admission again is accepted;
    /// another one is refused, because the slot is fixed for its
    /// registration's lifetime.
    pub(crate) fn bind(
        &self,
        admission: Arc<dyn ResumedRunAdmission>,
    ) -> Result<(), crate::traits::RuntimeDriverError> {
        let bound = self.0.get_or_init(|| Arc::clone(&admission));
        if Arc::ptr_eq(bound, &admission) {
            Ok(())
        } else {
            Err(crate::traits::RuntimeDriverError::ValidationFailed {
                reason:
                    "the session registration is already bound to another resumed run admission"
                        .to_string(),
            })
        }
    }

    pub(crate) fn current(&self) -> Option<Arc<dyn ResumedRunAdmission>> {
        self.0.get().cloned()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct Scripted {
        answers: Mutex<Vec<ResumedRunAdmissionDecision>>,
        calls: Mutex<u32>,
    }

    impl Scripted {
        fn new(mut answers: Vec<ResumedRunAdmissionDecision>) -> Arc<Self> {
            answers.reverse();
            Arc::new(Self {
                answers: Mutex::new(answers),
                calls: Mutex::new(0),
            })
        }

        fn calls(&self) -> u32 {
            *self.calls.lock().unwrap()
        }
    }

    #[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
    #[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
    impl ResumedRunAdmission for Scripted {
        async fn admit_resumed_run(
            &self,
            _request: &ResumedRunAdmissionRequest,
        ) -> ResumedRunAdmissionDecision {
            *self.calls.lock().unwrap() += 1;
            self.answers
                .lock()
                .unwrap()
                .pop()
                .expect("the runtime asked more often than scripted")
        }
    }

    struct NeverAnswers;

    #[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
    #[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
    impl ResumedRunAdmission for NeverAnswers {
        async fn admit_resumed_run(
            &self,
            _request: &ResumedRunAdmissionRequest,
        ) -> ResumedRunAdmissionDecision {
            std::future::pending().await
        }
    }

    fn request() -> ResumedRunAdmissionRequest {
        let session_id = SessionId::new();
        ResumedRunAdmissionRequest::new(
            LogicalRuntimeId::for_session(&session_id),
            session_id,
            RunId::new(),
            vec![InputId::new()],
        )
    }

    #[tokio::test]
    async fn no_admission_resumes_the_run() {
        assert_eq!(
            ask_resumed_run_admission(None, &request()).await,
            ResumedRunAdmissionVerdict::Continue
        );
    }

    #[tokio::test]
    async fn superseded_settles_at_once_without_asking_again() {
        let scripted = Scripted::new(vec![ResumedRunAdmissionDecision::Superseded {
            detail: "claim moved".into(),
        }]);
        let admission: Arc<dyn ResumedRunAdmission> = scripted.clone();
        let verdict = ask_resumed_run_admission(Some(&admission), &request()).await;
        assert!(
            matches!(&verdict, ResumedRunAdmissionVerdict::Refused { detail } if detail.contains("claim moved")),
            "{verdict:?}"
        );
        assert_eq!(scripted.calls(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn unavailable_is_asked_again_and_continue_on_a_later_attempt_resumes() {
        let scripted = Scripted::new(vec![
            ResumedRunAdmissionDecision::Unavailable {
                detail: "store unreachable".into(),
            },
            ResumedRunAdmissionDecision::Continue,
        ]);
        let admission: Arc<dyn ResumedRunAdmission> = scripted.clone();
        assert_eq!(
            ask_resumed_run_admission(Some(&admission), &request()).await,
            ResumedRunAdmissionVerdict::Continue
        );
        assert_eq!(scripted.calls(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn unavailable_through_every_attempt_is_undecided_with_the_last_reason() {
        let scripted = Scripted::new(vec![
            ResumedRunAdmissionDecision::Unavailable {
                detail: "first".into(),
            },
            ResumedRunAdmissionDecision::Unavailable {
                detail: "second".into(),
            },
            ResumedRunAdmissionDecision::Unavailable {
                detail: "third".into(),
            },
        ]);
        let admission: Arc<dyn ResumedRunAdmission> = scripted.clone();
        let verdict = ask_resumed_run_admission(Some(&admission), &request()).await;
        assert!(
            matches!(&verdict, ResumedRunAdmissionVerdict::Undecided { cause: ResumedRunAdmissionUndecidedCause::AnsweredUnavailable, detail } if detail == "third"),
            "{verdict:?}"
        );
        assert_eq!(scripted.calls(), RESUMED_RUN_ADMISSION_ATTEMPTS);
    }

    #[tokio::test(start_paused = true)]
    async fn an_admission_that_never_answers_is_undecided_after_the_bounded_attempts() {
        let admission: Arc<dyn ResumedRunAdmission> = Arc::new(NeverAnswers);
        let started = crate::tokio::time::Instant::now();
        let verdict = ask_resumed_run_admission(Some(&admission), &request()).await;
        assert!(
            matches!(&verdict, ResumedRunAdmissionVerdict::Undecided { cause: ResumedRunAdmissionUndecidedCause::TimedOut, detail } if detail.contains("did not answer")),
            "{verdict:?}"
        );
        let bound = RESUMED_RUN_ADMISSION_ATTEMPT_TIMEOUT * RESUMED_RUN_ADMISSION_ATTEMPTS
            + RESUMED_RUN_ADMISSION_RETRY_DELAY * (RESUMED_RUN_ADMISSION_ATTEMPTS - 1);
        assert_eq!(started.elapsed(), bound);
    }

    #[test]
    fn a_slot_keeps_its_first_admission_and_refuses_another() {
        let slot = ResumedRunAdmissionSlot::default();
        assert!(slot.current().is_none());
        let first: Arc<dyn ResumedRunAdmission> = Arc::new(NeverAnswers);
        slot.bind(Arc::clone(&first)).expect("first bind");
        slot.bind(Arc::clone(&first))
            .expect("binding the same admission again is accepted");
        let other: Arc<dyn ResumedRunAdmission> = Arc::new(NeverAnswers);
        assert!(slot.bind(other).is_err());
        assert!(Arc::ptr_eq(&slot.current().expect("bound"), &first));
    }

    fn a_note_of(
        run_id: &RunId,
        rounds: u64,
        at: meerkat_core::time_compat::Instant,
    ) -> UndecidedRoundsNote {
        let mut note = None;
        for round in 1..=rounds {
            UndecidedRoundsNote::note_round(
                &mut note,
                run_id,
                ResumedRunAdmissionUndecidedCause::AnsweredUnavailable,
                &format!("round {round}"),
                at,
            );
        }
        note.expect("a noted round")
    }

    /// The answer over the machine's mark, the machine's hold and the
    /// registration's note, every combination: no mark answers nothing,
    /// whatever else is there; a mark the hold does not name is marked and
    /// not held, whatever the note says; a mark the hold names is held, and
    /// only a note of that same run gives it rounds.
    #[test]
    fn the_admission_state_is_decided_by_the_machines_mark_and_hold_and_the_note_only_adds_rounds()
    {
        let run = RunId::new();
        let other = RunId::new();
        let noted_at = meerkat_core::time_compat::Instant::now();
        let now = noted_at + Duration::from_secs(90);
        let own = a_note_of(&run, 2, noted_at);
        let foreign = a_note_of(&other, 5, noted_at);
        let rounds_of = |state: Option<ResumedRunAdmissionState>| match state {
            Some(ResumedRunAdmissionState::Held(held)) => {
                Some(held.undecided.map(|undecided| undecided.rounds))
            }
            _ => None,
        };

        // No mark: nothing, for each hold and each note.
        for hold in [None, Some(&run), Some(&other)] {
            for note in [None, Some(&own), Some(&foreign)] {
                assert_eq!(
                    resumed_run_admission_state(None, hold, note, now),
                    None,
                    "no mark answers nothing (hold {hold:?})"
                );
            }
        }
        // A mark the machine does not hold: marked, not held, whatever the
        // note says, also under a hold that names another run.
        for hold in [None, Some(&other)] {
            for note in [None, Some(&own), Some(&foreign)] {
                assert_eq!(
                    resumed_run_admission_state(Some(&run), hold, note, now),
                    Some(ResumedRunAdmissionState::MarkedNotHeld),
                    "a note never makes a run held (hold {hold:?})"
                );
            }
        }
        // A mark the machine holds: held. No note, or another run's note:
        // held with no rounds. Its own note: its rounds.
        assert_eq!(
            rounds_of(resumed_run_admission_state(
                Some(&run),
                Some(&run),
                None,
                now
            )),
            Some(None),
            "held with no note is held, its rounds absent"
        );
        assert_eq!(
            rounds_of(resumed_run_admission_state(
                Some(&run),
                Some(&run),
                Some(&foreign),
                now
            )),
            Some(None),
            "another run's note is never shown for the held run"
        );
        let Some(ResumedRunAdmissionState::Held(held)) =
            resumed_run_admission_state(Some(&run), Some(&run), Some(&own), now)
        else {
            panic!("the held run reads held");
        };
        let undecided = held.undecided.expect("its own note gives its rounds");
        assert_eq!(undecided.rounds, 2);
        assert_eq!(undecided.reason, "round 2", "the last round's reason");
        assert_eq!(
            undecided.cause,
            ResumedRunAdmissionUndecidedCause::AnsweredUnavailable
        );
        assert_eq!(
            undecided.undecided_for,
            Duration::from_secs(90),
            "since the first of its rounds"
        );
    }

    /// A note of another run starts over: its count, its time and its
    /// reason are the new run's own.
    #[test]
    fn a_note_starts_over_for_another_run() {
        let first = RunId::new();
        let second = RunId::new();
        let start = meerkat_core::time_compat::Instant::now();
        let mut note = Some(a_note_of(&first, 3, start));
        let later = start + Duration::from_secs(40);
        UndecidedRoundsNote::note_round(
            &mut note,
            &second,
            ResumedRunAdmissionUndecidedCause::TimedOut,
            "the second run's round",
            later,
        );
        let now = later + Duration::from_secs(5);
        assert_eq!(
            resumed_run_admission_state(Some(&first), Some(&first), note.as_ref(), now),
            Some(ResumedRunAdmissionState::Held(ResumedRunAdmissionHeld {
                undecided: None
            })),
            "the first run's rounds are gone with its note"
        );
        let Some(ResumedRunAdmissionState::Held(held)) =
            resumed_run_admission_state(Some(&second), Some(&second), note.as_ref(), now)
        else {
            panic!("the second run reads held");
        };
        let undecided = held.undecided.expect("the second run's own note");
        assert_eq!(
            (
                undecided.rounds,
                undecided.cause,
                undecided.reason.as_str(),
                undecided.undecided_for
            ),
            (
                1,
                ResumedRunAdmissionUndecidedCause::TimedOut,
                "the second run's round",
                Duration::from_secs(5)
            )
        );
    }

    #[test]
    fn a_reason_is_cut_at_the_bound_on_a_character_boundary() {
        let short = "the store did not answer";
        assert_eq!(bounded_undecided_reason(short), short);
        let exact = "x".repeat(RESUMED_RUN_ADMISSION_REASON_MAX_BYTES);
        assert_eq!(bounded_undecided_reason(&exact), exact);
        // Two-byte characters with one byte of offset: the bound falls
        // inside a character, and the cut steps back to its start.
        let wide = "x".to_string() + &"\u{e9}".repeat(RESUMED_RUN_ADMISSION_REASON_MAX_BYTES);
        let cut = bounded_undecided_reason(&wide);
        assert_eq!(cut.len(), RESUMED_RUN_ADMISSION_REASON_MAX_BYTES - 1);
        assert!(wide.starts_with(&cut));
    }

    /// The bound is applied where the reason is stored: a note written with
    /// a reason past the bound keeps the cut reason, at its first round and
    /// at a later one, whoever the caller is.
    #[test]
    fn a_note_keeps_its_reason_cut_at_the_bound_at_every_round() {
        let run_id = RunId::new();
        let at = meerkat_core::time_compat::Instant::now();
        let wide = "r".repeat(RESUMED_RUN_ADMISSION_REASON_MAX_BYTES * 3);
        let mut note = None;
        for round in 1..=2_u64 {
            UndecidedRoundsNote::note_round(
                &mut note,
                &run_id,
                ResumedRunAdmissionUndecidedCause::AnsweredUnavailable,
                &wide,
                at,
            );
            let observed = note.as_ref().expect("a noted round").observed(at);
            assert_eq!(observed.rounds, round);
            assert_eq!(
                observed.reason.len(),
                RESUMED_RUN_ADMISSION_REASON_MAX_BYTES,
                "round {round} keeps the reason cut at the bound"
            );
            assert!(wide.starts_with(&observed.reason));
        }
    }
}
