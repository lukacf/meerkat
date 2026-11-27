//! Stopping a mob that may have active flow runs (MobKit 0.8.46 interim).
//!
//! meerkat 0.8.51's Stop settles in-flight member work itself, but MobMachine
//! still refuses `Stop` with `InvalidTransition { Running -> Stopped }` while a
//! flow run is active (`StopRunning`'s `no_active_runs` guard). A gateway going
//! down mid-flow is normal, so teardown settles the flow runs and stops:
//!
//! 1. Subscribe to the mob event ledger, then send Stop. Most stops end here,
//!    with one call.
//! 2. On `InvalidTransition`, cancel each non-terminal flow run and await each
//!    run's terminal event (`FlowCompleted`, `FlowFailed`, `FlowCanceled`).
//!    No runs to settle means the refusal is another guard's (explicit
//!    resume, adaptive lifecycle), returned as is.
//! 3. Re-send Stop. A run's terminal event is appended before the actor
//!    applies the run's `FinishRun` (which is what admits Stop), and no
//!    public signal follows `FinishRun` except the machine-state wake. So a
//!    refused re-send is re-issued after each machine commit, with the next
//!    wake armed before each Stop, until Stop is admitted. Unrelated commits
//!    (member-turn interrupts, roster changes) can land before `FinishRun`,
//!    so the re-issue is not capped by count: a refusal from another guard
//!    after the settled runs left surfaces at the hang guard, in the typed
//!    error's `last_refusal`.
//!
//! Everything is bounded by [`MOB_STOP_FLOW_SETTLE_BUDGET`]; past it the
//! typed [`MobStopFlowRunsUnsettled`] names the unsettled runs and the last
//! refusal. meerkat#1593 (Stop settling flow runs itself, or a typed
//! post-`FinishRun` signal) removes this module's re-issue and settle steps.

use std::collections::BTreeSet;
use std::time::Duration;

use meerkat_mob::{MobError, MobRunStatus, RunId};

use crate::mob_handle_runtime::MobRuntimeError;

/// Bound on settling a mob's active flow runs and stopping it during
/// teardown. Inside the mob quiesce window the published shutdown horizon
/// counts.
pub const MOB_STOP_FLOW_SETTLE_BUDGET: Duration = Duration::from_secs(10);

/// A teardown stop that did not settle the mob's flow runs within
/// [`MOB_STOP_FLOW_SETTLE_BUDGET`].
#[derive(Debug)]
pub struct MobStopFlowRunsUnsettled {
    /// Flow runs the teardown cancelled whose terminal it had not observed.
    pub runs: Vec<RunId>,
    /// The last Stop refusal, if a Stop was refused.
    pub last_refusal: Option<MobError>,
    pub budget: Duration,
}

impl MobStopFlowRunsUnsettled {
    /// Whether the budget ran out with no flow run being settled and no
    /// refusal recorded: the mob did not answer in time, and no flow run is
    /// the subject. It says nothing of what the mob did with the Stop.
    pub(crate) fn mob_did_not_answer(&self) -> bool {
        self.runs.is_empty() && self.last_refusal.is_none()
    }
}

impl std::fmt::Display for MobStopFlowRunsUnsettled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.mob_did_not_answer() {
            // Only what was observed: no answer inside the budget. Not that
            // the Stop ran, runs or will finish, and not that it was refused.
            return write!(
                f,
                "mob stop did not complete within {:?}: the mob did not answer in time; no flow \
                 run was being settled and no stop refusal was recorded",
                self.budget
            );
        }
        write!(
            f,
            "mob stop did not settle its flow runs within {:?}: unsettled runs [",
            self.budget
        )?;
        for (index, run) in self.runs.iter().enumerate() {
            if index > 0 {
                write!(f, ", ")?;
            }
            write!(f, "{run}")?;
        }
        write!(f, "]")?;
        match &self.last_refusal {
            Some(refusal) => write!(f, "; last stop refusal: {refusal}"),
            None => write!(f, "; no stop refusal recorded"),
        }
    }
}

/// The run whose terminal a ledger event records, if any.
fn flow_terminal_run(kind: &meerkat_mob::MobEventKind) -> Option<&RunId> {
    match kind {
        meerkat_mob::MobEventKind::FlowCompleted { run_id, .. }
        | meerkat_mob::MobEventKind::FlowFailed { run_id, .. }
        | meerkat_mob::MobEventKind::FlowCanceled { run_id, .. } => Some(run_id),
        _ => None,
    }
}

/// Flow-run terminals from the mob event ledger.
pub(crate) trait FlowTerminals {
    /// The next flow run terminal. `None`: the ledger stream ended.
    async fn next_terminal(&mut self) -> Option<RunId>;
    /// A terminal already delivered, without waiting.
    fn try_next_terminal(&mut self) -> Option<RunId>;
}

/// Machine commit wakes.
pub(crate) trait MachineCommits {
    /// Resolves at the next machine commit after this receiver was created.
    /// `Err`: the actor is gone.
    async fn changed(&mut self) -> Result<(), ()>;
}

/// What a teardown stop needs from a mob.
pub(crate) trait MobStopTarget {
    type Terminals: FlowTerminals;
    type Commits: MachineCommits;
    async fn subscribe_flow_terminals(&self) -> Result<Self::Terminals, MobError>;
    /// Flow runs not yet terminal.
    async fn active_flow_runs(&self) -> Result<Vec<RunId>, MobError>;
    async fn cancel_flow(&self, run_id: RunId) -> Result<(), MobError>;
    /// A commit receiver that has seen the current machine state.
    fn machine_commits(&self) -> Self::Commits;
    async fn stop(&self) -> Result<(), MobError>;
}

/// MobMachine's answer to a Stop on a mob that is already stopped.
fn already_stopped(error: &MobError) -> bool {
    matches!(
        error,
        MobError::InvalidTransition {
            from: meerkat_mob::MobState::Stopped,
            to: meerkat_mob::MobState::Stopped,
        }
    )
}

/// What a teardown stop was waiting on when its budget ran out: the one
/// thing the stop sequence had asked of the mob and had no answer to. It is
/// an observation of the wait, not of what the mob did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StopWaitPoint {
    /// The subscription to the mob's event ledger, before any Stop was sent.
    EventSubscription,
    /// The answer to a Stop.
    StopAnswer,
    /// The listing of the mob's flow runs, after a refused Stop.
    FlowRunListing,
    /// The answer to a flow run's cancel.
    FlowRunCancel,
    /// The terminal event of a cancelled flow run.
    FlowRunTerminal,
    /// The machine commit that lets a refused Stop be sent again.
    MachineCommit,
}

impl StopWaitPoint {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::EventSubscription => "event_subscription",
            Self::StopAnswer => "stop_answer",
            Self::FlowRunListing => "flow_run_listing_after_a_refused_stop",
            Self::FlowRunCancel => "flow_run_cancel",
            Self::FlowRunTerminal => "flow_run_terminal",
            Self::MachineCommit => "machine_commit",
        }
    }
}

/// A teardown stop's result, with what it was waiting on if its budget ran
/// out. The second is for the log of a caller that skips work after a stop
/// that did not complete; the result is what every caller returns.
pub(crate) struct StopAttempt {
    pub(crate) result: Result<(), MobRuntimeError>,
    /// `Some` only when the budget ran out.
    pub(crate) cut_while_waiting_on: Option<StopWaitPoint>,
}

impl StopAttempt {
    /// A stop that was answered, or was not needed.
    pub(crate) fn answered(result: Result<(), MobRuntimeError>) -> Self {
        Self {
            result,
            cut_while_waiting_on: None,
        }
    }
}

/// How a teardown stop that did not complete ended, in the terms of the
/// error it returned: a kind, a count and typed states, and none of the
/// error's text. It classifies the stop ROUTINE's end, not the Stop command:
/// the routine also subscribes, lists and cancels, and its bound can run out
/// before any Stop is sent or after one was answered with a refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StopEnd {
    pub(crate) kind: StopEndKind,
    /// Flow runs listed as active and not seen to end.
    pub(crate) unsettled_flow_runs: usize,
    /// The transition the mob refused, where the error holds one: the last
    /// recorded refusal of a bound that ran out, or the refusal the routine
    /// returned as is.
    pub(crate) invalid_transition: Option<(meerkat_mob::MobState, meerkat_mob::MobState)>,
    pub(crate) error_kind: StopErrorKind,
}

impl StopEnd {
    /// The refused transition as one word for a log line:
    /// `Completed_to_Stopped`, or `none`.
    pub(crate) fn invalid_transition_label(&self) -> InvalidTransitionLabel {
        InvalidTransitionLabel(self.invalid_transition)
    }
}

pub(crate) struct InvalidTransitionLabel(Option<(meerkat_mob::MobState, meerkat_mob::MobState)>);

impl std::fmt::Display for InvalidTransitionLabel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            Some((from, to)) => write!(f, "{}_to_{}", from.as_str(), to.as_str()),
            None => write!(f, "none"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StopEndKind {
    /// The bound ran out with no flow run being settled and no refusal
    /// recorded: whatever the routine was waiting on, the mob did not answer
    /// in time.
    MobDidNotAnswerWithinBound,
    /// The bound ran out with flow runs listed as active and not seen to end.
    FlowRunsUnsettledWithinBound,
    /// The bound ran out with every flow run settled and the Stop still
    /// refused.
    StopStillRefusedAtTheBound,
    /// The routine ended before its bound with an error: a refusal returned
    /// as is, or a failure of the subscription, a listing, a cancel or the
    /// mob's channel.
    EndedWithAnError,
}

impl StopEndKind {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::MobDidNotAnswerWithinBound => "mob_did_not_answer_within_bound",
            Self::FlowRunsUnsettledWithinBound => "flow_runs_unsettled_within_bound",
            Self::StopStillRefusedAtTheBound => "stop_still_refused_at_the_bound",
            Self::EndedWithAnError => "ended_with_an_error",
        }
    }
}

/// The kind of the error a stop routine ended with, without its text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StopErrorKind {
    /// The routine's own bound ran out.
    BoundRanOut,
    /// The mob refused a transition.
    InvalidTransition,
    /// The mob's actor could not be reached or did not reply: its channel
    /// was closed.
    ActorChannelClosed,
    /// Any other error of the mob.
    OtherMobError,
    /// An error that is not the mob's.
    NotAMobError,
}

impl StopErrorKind {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::BoundRanOut => "bound_ran_out",
            Self::InvalidTransition => "invalid_transition",
            Self::ActorChannelClosed => "actor_channel_closed",
            Self::OtherMobError => "other_mob_error",
            Self::NotAMobError => "not_a_mob_error",
        }
    }
}

/// Classify the error of a teardown stop that did not complete.
pub(crate) fn stop_end(error: &MobRuntimeError) -> StopEnd {
    fn refused(error: &MobError) -> Option<(meerkat_mob::MobState, meerkat_mob::MobState)> {
        match error {
            MobError::InvalidTransition { from, to } => Some((*from, *to)),
            _ => None,
        }
    }
    match error {
        MobRuntimeError::MobStopFlowRunsUnsettled(unsettled) => {
            let kind = if !unsettled.runs.is_empty() {
                StopEndKind::FlowRunsUnsettledWithinBound
            } else if unsettled.last_refusal.is_some() {
                StopEndKind::StopStillRefusedAtTheBound
            } else {
                StopEndKind::MobDidNotAnswerWithinBound
            };
            StopEnd {
                kind,
                unsettled_flow_runs: unsettled.runs.len(),
                invalid_transition: unsettled.last_refusal.as_ref().and_then(refused),
                error_kind: StopErrorKind::BoundRanOut,
            }
        }
        MobRuntimeError::Mob(error) => StopEnd {
            kind: StopEndKind::EndedWithAnError,
            unsettled_flow_runs: 0,
            invalid_transition: refused(error),
            error_kind: match error {
                MobError::InvalidTransition { .. } => StopErrorKind::InvalidTransition,
                MobError::ActorCommandChannelClosed | MobError::ActorReplyChannelClosed => {
                    StopErrorKind::ActorChannelClosed
                }
                _ => StopErrorKind::OtherMobError,
            },
        },
        _ => StopEnd {
            kind: StopEndKind::EndedWithAnError,
            unsettled_flow_runs: 0,
            invalid_transition: None,
            error_kind: StopErrorKind::NotAMobError,
        },
    }
}

/// Settle the mob's active flow runs as needed and stop it (module docs):
/// the result alone, as the rows of this module read it.
#[cfg(test)]
pub(crate) async fn settle_flow_runs_then_stop<T: MobStopTarget>(
    target: &T,
    budget: Duration,
) -> Result<(), MobRuntimeError> {
    attempt_settle_flow_runs_then_stop(target, budget)
        .await
        .result
}

/// Settle the mob's active flow runs as needed and stop it (module docs),
/// also saying what the sequence was waiting on if the budget ran out.
pub(crate) async fn attempt_settle_flow_runs_then_stop<T: MobStopTarget>(
    target: &T,
    budget: Duration,
) -> StopAttempt {
    let mut unsettled = BTreeSet::new();
    let mut last_refusal = None;
    let mut waiting_on = StopWaitPoint::EventSubscription;
    let settle_and_stop = async {
        // Subscribe before the first Stop: a run that ends after it is in the
        // stream.
        let mut terminals = target.subscribe_flow_terminals().await?;
        let mut commits = target.machine_commits();
        waiting_on = StopWaitPoint::StopAnswer;
        let refusal = match target.stop().await {
            Ok(()) => return Ok(()),
            // A mob that is already stopped needs no Stop: teardown goes on
            // as for a mob it stopped, rather than reporting an unclean stop.
            Err(error) if already_stopped(&error) => return Ok(()),
            Err(refusal @ MobError::InvalidTransition { .. }) => refusal,
            Err(error) => return Err(error),
        };
        // Runs that ended since the subscription held the Stop too; they are
        // terminal in the run store, so the listing below omits them.
        let mut settled = 0usize;
        while terminals.try_next_terminal().is_some() {
            settled += 1;
        }
        waiting_on = StopWaitPoint::FlowRunListing;
        for run_id in target.active_flow_runs().await? {
            unsettled.insert(run_id);
        }
        if unsettled.is_empty() && settled == 0 {
            // No flow run held the Stop: another guard refused it.
            return Err(refusal);
        }
        last_refusal = Some(refusal);
        waiting_on = StopWaitPoint::FlowRunCancel;
        for run_id in unsettled.clone() {
            target.cancel_flow(run_id).await?;
        }
        waiting_on = StopWaitPoint::FlowRunTerminal;
        while !unsettled.is_empty() {
            let Some(run_id) = terminals.next_terminal().await else {
                return Err(MobError::ActorCommandChannelClosed);
            };
            unsettled.remove(&run_id);
        }
        // Re-issue on machine commits until the settled runs' `FinishRun`
        // admits Stop, bounded by the hang guard. meerkat#1593: a public
        // post-`FinishRun` signal (a `FlowRunRetired` event or an active-run
        // projection) replaces this commit wake with an exact wait, and an
        // unrelated refusal then returns at once.
        loop {
            waiting_on = StopWaitPoint::MachineCommit;
            if commits.changed().await.is_err() {
                return Err(MobError::ActorCommandChannelClosed);
            }
            // Arm the next wake before this Stop: a commit after it is seen,
            // and an earlier `FinishRun` is already reflected in its answer.
            commits = target.machine_commits();
            waiting_on = StopWaitPoint::StopAnswer;
            match target.stop().await {
                Ok(()) => return Ok(()),
                Err(error) if already_stopped(&error) => return Ok(()),
                Err(refusal @ MobError::InvalidTransition { .. }) => {
                    last_refusal = Some(refusal);
                }
                Err(error) => return Err(error),
            }
        }
    };
    match tokio::time::timeout(budget, settle_and_stop).await {
        Ok(result) => StopAttempt::answered(result.map_err(MobRuntimeError::from)),
        Err(_) => StopAttempt {
            result: Err(MobRuntimeError::MobStopFlowRunsUnsettled(Box::new(
                MobStopFlowRunsUnsettled {
                    runs: unsettled.into_iter().collect(),
                    last_refusal,
                    budget,
                },
            ))),
            cut_while_waiting_on: Some(waiting_on),
        },
    }
}

/// [`MobStopTarget`] over a live mob handle, stopping it with the given
/// options.
pub(crate) struct HandleStopTarget<'a>(
    pub(crate) &'a meerkat_mob::MobHandle,
    pub(crate) meerkat_mob::StopOptions,
);

pub(crate) struct LedgerFlowTerminals(meerkat_mob::MobEventsSubscription);

impl FlowTerminals for LedgerFlowTerminals {
    async fn next_terminal(&mut self) -> Option<RunId> {
        loop {
            let event = self.0.event_rx.recv().await?;
            if let Some(run_id) = flow_terminal_run(&event.kind) {
                return Some(run_id.clone());
            }
        }
    }

    fn try_next_terminal(&mut self) -> Option<RunId> {
        while let Ok(event) = self.0.event_rx.try_recv() {
            if let Some(run_id) = flow_terminal_run(&event.kind) {
                return Some(run_id.clone());
            }
        }
        None
    }
}

impl MachineCommits for meerkat_mob::MobMachineStateChanges {
    async fn changed(&mut self) -> Result<(), ()> {
        meerkat_mob::MobMachineStateChanges::changed(self)
            .await
            .map_err(|_| ())
    }
}

impl MobStopTarget for HandleStopTarget<'_> {
    type Terminals = LedgerFlowTerminals;
    type Commits = meerkat_mob::MobMachineStateChanges;

    async fn subscribe_flow_terminals(&self) -> Result<Self::Terminals, MobError> {
        Ok(LedgerFlowTerminals(self.0.events().subscribe().await?))
    }

    async fn active_flow_runs(&self) -> Result<Vec<RunId>, MobError> {
        Ok(self
            .0
            .list_runs(None)
            .await?
            .into_iter()
            .filter(|run| match run.status {
                MobRunStatus::Pending | MobRunStatus::Running => true,
                MobRunStatus::Completed | MobRunStatus::Failed | MobRunStatus::Canceled => false,
            })
            .map(|run| run.run_id)
            .collect())
    }

    async fn cancel_flow(&self, run_id: RunId) -> Result<(), MobError> {
        self.0.cancel_flow(run_id).await
    }

    fn machine_commits(&self) -> Self::Commits {
        self.0.machine_state_changes()
    }

    async fn stop(&self) -> Result<(), MobError> {
        // A row's stand-in for a mob whose Stop gets no answer inside the
        // teardown's bound: nothing is sent and the wait never ends.
        #[cfg(test)]
        if unanswered_stop_for_test::armed(self.0.mob_id().as_str()) {
            return std::future::pending().await;
        }
        let report = self.0.stop_with_options(self.1).await?;
        // A member whose run starts could not be held may still start a turn
        // while the mob is Stopped; teardown proceeds, so say so.
        for (identity, reason) in report.not_holdable() {
            tracing::warn!(
                agent_identity = %identity,
                ?reason,
                "mob stop could not hold this member's run starts"
            );
        }
        Ok(())
    }
}

/// The mobs whose teardown Stop a row leaves without an answer: the stand-in
/// [`HandleStopTarget`] consults before it sends a Stop. Rows only.
#[cfg(test)]
pub(crate) mod unanswered_stop_for_test {
    use std::collections::BTreeSet;
    use std::sync::{Mutex, PoisonError};

    static MOBS: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

    /// Leave every teardown Stop of the mob `mob_id` without an answer, for
    /// the rest of the test process. A row arms a mob id of its own.
    pub(crate) fn arm(mob_id: &str) {
        MOBS.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(mob_id.to_string());
    }

    pub(crate) fn armed(mob_id: &str) -> bool {
        MOBS.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(mob_id)
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use meerkat_mob::MobState;
    use tokio::sync::{mpsc, watch};

    use super::*;

    fn refusal() -> MobError {
        MobError::InvalidTransition {
            from: MobState::Running,
            to: MobState::Stopped,
        }
    }

    /// What a call of the mob double does in place of answering.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Fault {
        /// The call never answers.
        NoAnswer,
        /// The call answers with an error that is no refusal of a Stop.
        Fails,
    }

    /// A mob double: scripted Stop answers (a refusal once the script is
    /// spent), listed flow runs, and a machine commit after a refused Stop
    /// while `refusal_commits` lasts (the first refusal's own quiesce commit,
    /// an unrelated commit, or a run's `FinishRun` landing after it). A row
    /// can leave one of its calls without an answer, or make it fail.
    struct FakeTarget {
        runs: Vec<RunId>,
        cancel_ends_run: bool,
        subscription: Option<Fault>,
        listing: Option<Fault>,
        cancel: Option<Fault>,
        /// Every Stop is left without an answer.
        stop_unanswered: bool,
        /// The terminal stream handed out has already ended.
        terminal_stream_ended: bool,
        refusal_commits: AtomicUsize,
        stop_answers: Mutex<VecDeque<Result<(), MobError>>>,
        stop_calls: AtomicUsize,
        cancels: AtomicUsize,
        terminal_tx: mpsc::UnboundedSender<RunId>,
        terminal_rx: Mutex<Option<mpsc::UnboundedReceiver<RunId>>>,
        commits: watch::Sender<u64>,
    }

    impl FakeTarget {
        fn new(
            runs: Vec<RunId>,
            cancel_ends_run: bool,
            stop_answers: Vec<Result<(), MobError>>,
        ) -> Self {
            let (terminal_tx, terminal_rx) = mpsc::unbounded_channel();
            Self {
                runs,
                cancel_ends_run,
                subscription: None,
                listing: None,
                cancel: None,
                stop_unanswered: false,
                terminal_stream_ended: false,
                refusal_commits: AtomicUsize::new(usize::MAX),
                stop_answers: Mutex::new(stop_answers.into()),
                stop_calls: AtomicUsize::new(0),
                cancels: AtomicUsize::new(0),
                terminal_tx,
                terminal_rx: Mutex::new(Some(terminal_rx)),
                commits: watch::channel(0).0,
            }
        }
    }

    struct FakeTerminals(mpsc::UnboundedReceiver<RunId>);

    impl FlowTerminals for FakeTerminals {
        async fn next_terminal(&mut self) -> Option<RunId> {
            self.0.recv().await
        }

        fn try_next_terminal(&mut self) -> Option<RunId> {
            self.0.try_recv().ok()
        }
    }

    struct FakeCommits(watch::Receiver<u64>);

    impl MachineCommits for FakeCommits {
        async fn changed(&mut self) -> Result<(), ()> {
            self.0.changed().await.map_err(|_| ())
        }
    }

    impl MobStopTarget for FakeTarget {
        type Terminals = FakeTerminals;
        type Commits = FakeCommits;

        async fn subscribe_flow_terminals(&self) -> Result<Self::Terminals, MobError> {
            match self.subscription {
                Some(Fault::NoAnswer) => std::future::pending::<()>().await,
                Some(Fault::Fails) => return Err(MobError::ActorCommandChannelClosed),
                None => {}
            }
            if self.terminal_stream_ended {
                let (_ended, terminal_rx) = mpsc::unbounded_channel();
                return Ok(FakeTerminals(terminal_rx));
            }
            Ok(FakeTerminals(
                self.terminal_rx
                    .lock()
                    .unwrap()
                    .take()
                    .expect("one subscription"),
            ))
        }

        async fn active_flow_runs(&self) -> Result<Vec<RunId>, MobError> {
            match self.listing {
                Some(Fault::NoAnswer) => std::future::pending::<()>().await,
                Some(Fault::Fails) => return Err(MobError::ActorReplyChannelClosed),
                None => {}
            }
            Ok(self.runs.clone())
        }

        async fn cancel_flow(&self, run_id: RunId) -> Result<(), MobError> {
            self.cancels.fetch_add(1, Ordering::SeqCst);
            match self.cancel {
                Some(Fault::NoAnswer) => std::future::pending::<()>().await,
                Some(Fault::Fails) => return Err(MobError::RunNotFound(run_id)),
                None => {}
            }
            if self.cancel_ends_run {
                self.terminal_tx.send(run_id).unwrap();
            }
            Ok(())
        }

        fn machine_commits(&self) -> Self::Commits {
            FakeCommits(self.commits.subscribe())
        }

        async fn stop(&self) -> Result<(), MobError> {
            self.stop_calls.fetch_add(1, Ordering::SeqCst);
            if self.stop_unanswered {
                std::future::pending::<()>().await;
            }
            let answer = self
                .stop_answers
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Err(refusal()));
            if answer.is_err()
                && self
                    .refusal_commits
                    .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                        left.checked_sub(1)
                    })
                    .is_ok()
            {
                self.commits.send_modify(|commit| *commit += 1);
            }
            answer
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_mob_that_stops_is_stopped_with_one_call() {
        let target = FakeTarget::new(vec![RunId::new()], true, vec![Ok(())]);
        settle_flow_runs_then_stop(&target, MOB_STOP_FLOW_SETTLE_BUDGET)
            .await
            .expect("stopped");
        assert_eq!(target.stop_calls.load(Ordering::SeqCst), 1);
        assert_eq!(target.cancels.load(Ordering::SeqCst), 0, "nothing settled");
    }

    /// A Stop on a mob that is already stopped is a stopped mob, not an
    /// unclean stop: teardown goes on, with nothing settled.
    #[tokio::test(start_paused = true)]
    async fn a_mob_already_stopped_counts_as_stopped() {
        let target = FakeTarget::new(
            vec![RunId::new()],
            true,
            vec![Err(MobError::InvalidTransition {
                from: MobState::Stopped,
                to: MobState::Stopped,
            })],
        );
        settle_flow_runs_then_stop(&target, MOB_STOP_FLOW_SETTLE_BUDGET)
            .await
            .expect("an already stopped mob is stopped");
        assert_eq!(target.stop_calls.load(Ordering::SeqCst), 1);
        assert_eq!(target.cancels.load(Ordering::SeqCst), 0, "nothing settled");
    }

    /// No flow run held the refused Stop: another guard refused it (an
    /// explicit resume, the adaptive lifecycle), and it is returned as is.
    #[tokio::test(start_paused = true)]
    async fn a_refusal_with_no_flow_runs_is_returned_as_is() {
        let target = FakeTarget::new(Vec::new(), true, vec![Err(refusal())]);
        let error = settle_flow_runs_then_stop(&target, MOB_STOP_FLOW_SETTLE_BUDGET)
            .await
            .expect_err("refused");
        assert!(
            matches!(
                error,
                MobRuntimeError::Mob(MobError::InvalidTransition { .. })
            ),
            "{error:?}"
        );
        assert_eq!(target.stop_calls.load(Ordering::SeqCst), 1);
    }

    /// The race: the run's terminal event is observed before the actor
    /// applies its `FinishRun`, so the Stop sent after it is refused; the
    /// `FinishRun` commit then wakes one re-issue, which stops the mob.
    #[tokio::test(start_paused = true)]
    async fn a_stop_refused_before_the_runs_finish_run_is_reissued_on_the_commit() {
        let run = RunId::new();
        let target = FakeTarget::new(
            vec![run],
            true,
            vec![Err(refusal()), Err(refusal()), Ok(())],
        );
        settle_flow_runs_then_stop(&target, MOB_STOP_FLOW_SETTLE_BUDGET)
            .await
            .expect("stopped once the run left the machine");
        assert_eq!(target.cancels.load(Ordering::SeqCst), 1);
        assert_eq!(
            target.stop_calls.load(Ordering::SeqCst),
            3,
            "the first Stop, the one refused before FinishRun, the one after"
        );
    }

    /// The CI failure of the capped re-issue: unrelated commits (member-turn
    /// interrupts, roster changes) land before the settled run's `FinishRun`.
    /// Each wakes a re-issue that is refused; the Stop is admitted once the
    /// run has left the machine, however many commits came first.
    #[tokio::test(start_paused = true)]
    async fn a_finish_run_after_unrelated_commits_still_admits_the_stop() {
        let target = FakeTarget::new(
            vec![RunId::new()],
            true,
            vec![
                Err(refusal()),
                Err(refusal()),
                Err(refusal()),
                Err(refusal()),
                Err(refusal()),
                Ok(()),
            ],
        );
        settle_flow_runs_then_stop(&target, MOB_STOP_FLOW_SETTLE_BUDGET)
            .await
            .expect("stopped once the run left the machine");
        assert_eq!(target.stop_calls.load(Ordering::SeqCst), 6);
    }

    /// A refusal that settling the flow runs did not clear (another guard)
    /// and that no further commit can lift surfaces at the hang guard, as the
    /// typed error carrying that refusal.
    #[tokio::test(start_paused = true)]
    async fn an_unrelated_refusal_after_the_settled_runs_surfaces_at_the_guard() {
        let target = FakeTarget::new(vec![RunId::new()], true, Vec::new());
        target.refusal_commits.store(3, Ordering::SeqCst);
        let error = settle_flow_runs_then_stop(&target, MOB_STOP_FLOW_SETTLE_BUDGET)
            .await
            .expect_err("still refused");
        let MobRuntimeError::MobStopFlowRunsUnsettled(unsettled) = &error else {
            panic!("the typed hang-guard error: {error:?}");
        };
        assert!(unsettled.runs.is_empty(), "the run itself settled");
        assert!(matches!(
            unsettled.last_refusal,
            Some(MobError::InvalidTransition { .. })
        ));
        assert_eq!(
            target.stop_calls.load(Ordering::SeqCst),
            4,
            "the first Stop and one re-issue per commit, no more"
        );
    }

    /// A flow run that never terminates trips the named hang guard, whose
    /// typed error names the run and the last refusal.
    #[tokio::test(start_paused = true)]
    async fn a_flow_run_that_never_ends_trips_the_hang_guard() {
        let run = RunId::new();
        let target = FakeTarget::new(vec![run.clone()], false, vec![Err(refusal())]);
        let error = settle_flow_runs_then_stop(&target, MOB_STOP_FLOW_SETTLE_BUDGET)
            .await
            .expect_err("unsettled");
        let MobRuntimeError::MobStopFlowRunsUnsettled(unsettled) = &error else {
            panic!("the typed hang-guard error: {error:?}");
        };
        assert_eq!(unsettled.runs, vec![run.clone()]);
        assert!(matches!(
            unsettled.last_refusal,
            Some(MobError::InvalidTransition { .. })
        ));
        assert_eq!(unsettled.budget, MOB_STOP_FLOW_SETTLE_BUDGET);
        let rendered = error.to_string();
        assert!(
            rendered.contains(&run.to_string()) && rendered.contains("last stop refusal"),
            "{rendered}"
        );
    }

    /// The error's text says what was observed. When the bound ran out with
    /// no flow run being settled and no refusal recorded, no flow run is the
    /// subject: the mob did not answer in time. The text does not say the
    /// stop ran, runs or will finish.
    #[test]
    fn a_stop_cut_with_no_run_and_no_refusal_is_not_described_as_unsettled_flow_runs() {
        let rendered = MobStopFlowRunsUnsettled {
            runs: Vec::new(),
            last_refusal: None,
            budget: MOB_STOP_FLOW_SETTLE_BUDGET,
        }
        .to_string();
        assert_eq!(
            rendered,
            "mob stop did not complete within 10s: the mob did not answer in time; no flow run \
             was being settled and no stop refusal was recorded"
        );
        for claim in ["flow runs", "still", "will", "executing", "refused"] {
            assert!(!rendered.contains(claim), "{claim}: {rendered}");
        }
    }

    /// A bound that ran out while flow runs were being settled, or after a
    /// refusal, keeps the text it has: the runs and the refusal are its
    /// subject.
    #[test]
    fn a_stop_cut_while_settling_flow_runs_keeps_its_text() {
        let run = RunId::new();
        let with_run = MobStopFlowRunsUnsettled {
            runs: vec![run.clone()],
            last_refusal: None,
            budget: MOB_STOP_FLOW_SETTLE_BUDGET,
        }
        .to_string();
        assert_eq!(
            with_run,
            format!(
                "mob stop did not settle its flow runs within 10s: unsettled runs [{run}]; no \
                 stop refusal recorded"
            )
        );
        let with_refusal = MobStopFlowRunsUnsettled {
            runs: Vec::new(),
            last_refusal: Some(refusal()),
            budget: MOB_STOP_FLOW_SETTLE_BUDGET,
        }
        .to_string();
        assert_eq!(
            with_refusal,
            format!(
                "mob stop did not settle its flow runs within 10s: unsettled runs []; last stop \
                 refusal: {}",
                refusal()
            )
        );
    }

    /// A Stop that gets no answer inside the bound ends in the typed error
    /// with no run and no refusal, and its text says the mob did not answer.
    #[tokio::test(start_paused = true)]
    async fn a_stop_that_gets_no_answer_is_reported_as_unanswered() {
        struct Unanswered;
        impl MobStopTarget for Unanswered {
            type Terminals = FakeTerminals;
            type Commits = FakeCommits;
            async fn subscribe_flow_terminals(&self) -> Result<Self::Terminals, MobError> {
                let (_terminal_tx, terminal_rx) = mpsc::unbounded_channel();
                Ok(FakeTerminals(terminal_rx))
            }
            async fn active_flow_runs(&self) -> Result<Vec<RunId>, MobError> {
                Ok(Vec::new())
            }
            async fn cancel_flow(&self, _run_id: RunId) -> Result<(), MobError> {
                Ok(())
            }
            fn machine_commits(&self) -> Self::Commits {
                FakeCommits(watch::channel(0_u64).0.subscribe())
            }
            async fn stop(&self) -> Result<(), MobError> {
                std::future::pending().await
            }
        }

        let error = settle_flow_runs_then_stop(&Unanswered, MOB_STOP_FLOW_SETTLE_BUDGET)
            .await
            .expect_err("the bound runs out");
        let MobRuntimeError::MobStopFlowRunsUnsettled(unsettled) = &error else {
            panic!("the typed error of the bound: {error:?}");
        };
        assert!(unsettled.runs.is_empty());
        assert!(unsettled.last_refusal.is_none());
        assert_eq!(unsettled.budget, MOB_STOP_FLOW_SETTLE_BUDGET);
        let rendered = error.to_string();
        assert!(
            rendered.starts_with("mob stop did not complete within 10s: the mob did not answer"),
            "{rendered}"
        );
        assert!(!rendered.contains("flow runs"), "{rendered}");
    }

    /// A stop whose budget ran out says what it was waiting on, beside the
    /// error it returns; a stop that was answered names nothing.
    #[tokio::test(start_paused = true)]
    async fn a_cut_stop_names_what_it_was_waiting_on() {
        // A refused Stop over a flow run whose terminal never comes.
        let held_by_a_run = FakeTarget::new(vec![RunId::new()], false, Vec::new());
        let attempt =
            attempt_settle_flow_runs_then_stop(&held_by_a_run, MOB_STOP_FLOW_SETTLE_BUDGET).await;
        assert!(matches!(
            attempt.result,
            Err(MobRuntimeError::MobStopFlowRunsUnsettled(_))
        ));
        assert_eq!(
            attempt.cut_while_waiting_on,
            Some(StopWaitPoint::FlowRunTerminal)
        );

        // A Stop that is answered was not cut.
        let stopped = FakeTarget::new(Vec::new(), true, vec![Ok(())]);
        let attempt =
            attempt_settle_flow_runs_then_stop(&stopped, MOB_STOP_FLOW_SETTLE_BUDGET).await;
        assert!(attempt.result.is_ok());
        assert_eq!(attempt.cut_while_waiting_on, None);
    }

    /// The text of the error whose bound ran out with no flow run being
    /// settled and no refusal recorded.
    const MOB_DID_NOT_ANSWER: &str = "mob stop did not complete within 10s: the mob did not answer \
                                      in time; no flow run was being settled and no stop refusal \
                                      was recorded";

    /// What a row reads of a stop routine that did not complete: how its
    /// error is classified, what it was waiting on if its bound ran out, and
    /// the error's own text.
    async fn end_of(target: &FakeTarget) -> (StopEnd, Option<StopWaitPoint>, String) {
        let attempt = attempt_settle_flow_runs_then_stop(target, MOB_STOP_FLOW_SETTLE_BUDGET).await;
        let error = attempt
            .result
            .expect_err("the stop routine did not complete");
        (
            stop_end(&error),
            attempt.cut_while_waiting_on,
            error.to_string(),
        )
    }

    const MOB_DID_NOT_ANSWER_END: StopEnd = StopEnd {
        kind: StopEndKind::MobDidNotAnswerWithinBound,
        unsettled_flow_runs: 0,
        invalid_transition: None,
        error_kind: StopErrorKind::BoundRanOut,
    };

    /// The bound runs out at the subscription to the mob's events, before
    /// any Stop is sent: the mob did not answer within the bound. Nothing
    /// says a Stop was unanswered, for none was sent.
    #[tokio::test(start_paused = true)]
    async fn a_bound_that_runs_out_before_any_stop_is_sent_is_a_mob_that_did_not_answer() {
        let mut target = FakeTarget::new(Vec::new(), true, vec![Ok(())]);
        target.subscription = Some(Fault::NoAnswer);
        let (end, waiting_on, text) = end_of(&target).await;
        assert_eq!(end, MOB_DID_NOT_ANSWER_END);
        assert_eq!(end.kind.as_str(), "mob_did_not_answer_within_bound");
        assert_eq!(waiting_on, Some(StopWaitPoint::EventSubscription));
        assert_eq!(text, MOB_DID_NOT_ANSWER);
        assert_eq!(
            target.stop_calls.load(Ordering::SeqCst),
            0,
            "no Stop was sent"
        );
    }

    /// The bound runs out on the first Stop's answer: the same end, and the
    /// wait says it was the Stop's answer.
    #[tokio::test(start_paused = true)]
    async fn a_bound_that_runs_out_on_the_stop_answer_is_a_mob_that_did_not_answer() {
        let mut target = FakeTarget::new(Vec::new(), true, Vec::new());
        target.stop_unanswered = true;
        let (end, waiting_on, text) = end_of(&target).await;
        assert_eq!(end, MOB_DID_NOT_ANSWER_END);
        assert_eq!(waiting_on, Some(StopWaitPoint::StopAnswer));
        assert_eq!(text, MOB_DID_NOT_ANSWER);
        assert_eq!(target.stop_calls.load(Ordering::SeqCst), 1);
    }

    /// The bound runs out at the listing of the flow runs, after a Stop that
    /// was answered with a refusal: the mob did not answer the listing. The
    /// refusal received is recorded by nothing, so the end is not a Stop
    /// still refused; the wait's name says a refused Stop came before it.
    #[tokio::test(start_paused = true)]
    async fn a_bound_that_runs_out_at_the_listing_after_a_refused_stop_is_a_mob_that_did_not_answer()
     {
        let mut target = FakeTarget::new(Vec::new(), true, vec![Err(refusal())]);
        target.listing = Some(Fault::NoAnswer);
        let (end, waiting_on, text) = end_of(&target).await;
        assert_eq!(end, MOB_DID_NOT_ANSWER_END);
        assert_eq!(waiting_on, Some(StopWaitPoint::FlowRunListing));
        assert_eq!(
            StopWaitPoint::FlowRunListing.as_str(),
            "flow_run_listing_after_a_refused_stop"
        );
        assert_eq!(text, MOB_DID_NOT_ANSWER);
        assert_eq!(
            target.stop_calls.load(Ordering::SeqCst),
            1,
            "one Stop, refused"
        );
    }

    /// The bound runs out after the flow runs settled, with a refusal
    /// recorded and no run left: a Stop still refused at the bound, with no
    /// unsettled flow run counted, and never "flow runs unsettled".
    #[tokio::test(start_paused = true)]
    async fn a_bound_that_runs_out_after_the_runs_settled_is_a_stop_still_refused() {
        let target = FakeTarget::new(vec![RunId::new()], true, Vec::new());
        target.refusal_commits.store(3, Ordering::SeqCst);
        let (end, waiting_on, text) = end_of(&target).await;
        assert_eq!(
            end,
            StopEnd {
                kind: StopEndKind::StopStillRefusedAtTheBound,
                unsettled_flow_runs: 0,
                invalid_transition: Some((MobState::Running, MobState::Stopped)),
                error_kind: StopErrorKind::BoundRanOut,
            }
        );
        assert_eq!(end.kind.as_str(), "stop_still_refused_at_the_bound");
        assert_eq!(
            end.invalid_transition_label().to_string(),
            "Running_to_Stopped"
        );
        assert_eq!(waiting_on, Some(StopWaitPoint::MachineCommit));
        // The error's text is the one it had.
        assert_eq!(
            text,
            format!(
                "mob stop did not settle its flow runs within 10s: unsettled runs []; last stop \
                 refusal: {}",
                refusal()
            )
        );
    }

    /// The bound runs out with flow runs listed as active and not seen to
    /// end, on a cancel's answer or on a run's terminal: flow runs
    /// unsettled, with their count, which is above zero.
    #[tokio::test(start_paused = true)]
    async fn a_bound_that_runs_out_with_flow_runs_left_counts_them() {
        let flow_runs_unsettled = StopEnd {
            kind: StopEndKind::FlowRunsUnsettledWithinBound,
            unsettled_flow_runs: 1,
            invalid_transition: Some((MobState::Running, MobState::Stopped)),
            error_kind: StopErrorKind::BoundRanOut,
        };

        let mut cancel_unanswered = FakeTarget::new(vec![RunId::new()], true, Vec::new());
        cancel_unanswered.cancel = Some(Fault::NoAnswer);
        let (end, waiting_on, _text) = end_of(&cancel_unanswered).await;
        assert_eq!(end, flow_runs_unsettled);
        assert_eq!(end.kind.as_str(), "flow_runs_unsettled_within_bound");
        assert_eq!(waiting_on, Some(StopWaitPoint::FlowRunCancel));

        let terminal_never_comes = FakeTarget::new(vec![RunId::new()], false, Vec::new());
        let (end, waiting_on, _text) = end_of(&terminal_never_comes).await;
        assert_eq!(end, flow_runs_unsettled);
        assert_eq!(waiting_on, Some(StopWaitPoint::FlowRunTerminal));
    }

    /// The routine ends before its bound with an error: ended with an error,
    /// whatever the error is an answer to. A failed subscription, a failed
    /// listing, a failed cancel and an ended event stream are no answer to a
    /// Stop; a refusal returned as is is one. None was cut by the bound.
    #[tokio::test(start_paused = true)]
    async fn an_error_before_the_bound_is_an_end_with_an_error() {
        let ended = |error_kind, invalid_transition| StopEnd {
            kind: StopEndKind::EndedWithAnError,
            unsettled_flow_runs: 0,
            invalid_transition,
            error_kind,
        };

        // The subscription fails: no Stop was sent.
        let mut subscription_fails = FakeTarget::new(Vec::new(), true, vec![Ok(())]);
        subscription_fails.subscription = Some(Fault::Fails);
        let (end, waiting_on, _text) = end_of(&subscription_fails).await;
        assert_eq!(end, ended(StopErrorKind::ActorChannelClosed, None));
        assert_eq!(end.kind.as_str(), "ended_with_an_error");
        assert_eq!(waiting_on, None);
        assert_eq!(subscription_fails.stop_calls.load(Ordering::SeqCst), 0);

        // The listing fails after a refused Stop.
        let mut listing_fails = FakeTarget::new(Vec::new(), true, vec![Err(refusal())]);
        listing_fails.listing = Some(Fault::Fails);
        let (end, waiting_on, _text) = end_of(&listing_fails).await;
        assert_eq!(end, ended(StopErrorKind::ActorChannelClosed, None));
        assert_eq!(waiting_on, None);

        // A cancel fails.
        let mut cancel_fails = FakeTarget::new(vec![RunId::new()], true, Vec::new());
        cancel_fails.cancel = Some(Fault::Fails);
        let (end, waiting_on, _text) = end_of(&cancel_fails).await;
        assert_eq!(end, ended(StopErrorKind::OtherMobError, None));
        assert_eq!(waiting_on, None);

        // The event stream ends while a cancelled run's terminal is awaited:
        // the closed channel.
        let mut stream_ended = FakeTarget::new(vec![RunId::new()], false, Vec::new());
        stream_ended.terminal_stream_ended = true;
        let (end, waiting_on, _text) = end_of(&stream_ended).await;
        assert_eq!(end, ended(StopErrorKind::ActorChannelClosed, None));
        assert_eq!(waiting_on, None);

        // A refusal that no flow run explains, returned as is.
        let refused = FakeTarget::new(Vec::new(), true, vec![Err(refusal())]);
        let (end, waiting_on, _text) = end_of(&refused).await;
        assert_eq!(
            end,
            ended(
                StopErrorKind::InvalidTransition,
                Some((MobState::Running, MobState::Stopped))
            )
        );
        assert_eq!(waiting_on, None);
    }

    /// The classification reads the error's kind, its count and its typed
    /// states only: every value it can give is one fixed word, and an error
    /// that is not the mob's is an end with an error too.
    #[test]
    fn the_end_of_a_stop_routine_is_said_in_fixed_words() {
        let not_the_mobs = stop_end(&MobRuntimeError::InvalidInput("not a mob error"));
        assert_eq!(
            not_the_mobs,
            StopEnd {
                kind: StopEndKind::EndedWithAnError,
                unsettled_flow_runs: 0,
                invalid_transition: None,
                error_kind: StopErrorKind::NotAMobError,
            }
        );
        assert_eq!(not_the_mobs.invalid_transition_label().to_string(), "none");

        // Two runs left and no refusal recorded: the runs are the subject.
        let two_runs = stop_end(&MobRuntimeError::MobStopFlowRunsUnsettled(Box::new(
            MobStopFlowRunsUnsettled {
                runs: vec![RunId::new(), RunId::new()],
                last_refusal: None,
                budget: MOB_STOP_FLOW_SETTLE_BUDGET,
            },
        )));
        assert_eq!(
            two_runs,
            StopEnd {
                kind: StopEndKind::FlowRunsUnsettledWithinBound,
                unsettled_flow_runs: 2,
                invalid_transition: None,
                error_kind: StopErrorKind::BoundRanOut,
            }
        );

        assert_eq!(
            [
                StopEndKind::MobDidNotAnswerWithinBound,
                StopEndKind::FlowRunsUnsettledWithinBound,
                StopEndKind::StopStillRefusedAtTheBound,
                StopEndKind::EndedWithAnError,
            ]
            .map(StopEndKind::as_str),
            [
                "mob_did_not_answer_within_bound",
                "flow_runs_unsettled_within_bound",
                "stop_still_refused_at_the_bound",
                "ended_with_an_error",
            ]
        );
        assert_eq!(
            [
                StopErrorKind::BoundRanOut,
                StopErrorKind::InvalidTransition,
                StopErrorKind::ActorChannelClosed,
                StopErrorKind::OtherMobError,
                StopErrorKind::NotAMobError,
            ]
            .map(StopErrorKind::as_str),
            [
                "bound_ran_out",
                "invalid_transition",
                "actor_channel_closed",
                "other_mob_error",
                "not_a_mob_error",
            ]
        );
        assert_eq!(
            [
                StopWaitPoint::EventSubscription,
                StopWaitPoint::StopAnswer,
                StopWaitPoint::FlowRunListing,
                StopWaitPoint::FlowRunCancel,
                StopWaitPoint::FlowRunTerminal,
                StopWaitPoint::MachineCommit,
            ]
            .map(StopWaitPoint::as_str),
            [
                "event_subscription",
                "stop_answer",
                "flow_run_listing_after_a_refused_stop",
                "flow_run_cancel",
                "flow_run_terminal",
                "machine_commit",
            ]
        );
    }
}
