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

impl std::fmt::Display for MobStopFlowRunsUnsettled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
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

/// Settle the mob's active flow runs as needed and stop it (module docs).
pub(crate) async fn settle_flow_runs_then_stop<T: MobStopTarget>(
    target: &T,
    budget: Duration,
) -> Result<(), MobRuntimeError> {
    let mut unsettled = BTreeSet::new();
    let mut last_refusal = None;
    let settle_and_stop = async {
        // Subscribe before the first Stop: a run that ends after it is in the
        // stream.
        let mut terminals = target.subscribe_flow_terminals().await?;
        let mut commits = target.machine_commits();
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
        for run_id in target.active_flow_runs().await? {
            unsettled.insert(run_id);
        }
        if unsettled.is_empty() && settled == 0 {
            // No flow run held the Stop: another guard refused it.
            return Err(refusal);
        }
        last_refusal = Some(refusal);
        for run_id in unsettled.clone() {
            target.cancel_flow(run_id).await?;
        }
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
            if commits.changed().await.is_err() {
                return Err(MobError::ActorCommandChannelClosed);
            }
            // Arm the next wake before this Stop: a commit after it is seen,
            // and an earlier `FinishRun` is already reflected in its answer.
            commits = target.machine_commits();
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
        Ok(result) => result.map_err(MobRuntimeError::from),
        Err(_) => Err(MobRuntimeError::MobStopFlowRunsUnsettled(Box::new(
            MobStopFlowRunsUnsettled {
                runs: unsettled.into_iter().collect(),
                last_refusal,
                budget,
            },
        ))),
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

    /// A mob double: scripted Stop answers (a refusal once the script is
    /// spent), listed flow runs, and a machine commit after a refused Stop
    /// while `refusal_commits` lasts (the first refusal's own quiesce commit,
    /// an unrelated commit, or a run's `FinishRun` landing after it).
    struct FakeTarget {
        runs: Vec<RunId>,
        cancel_ends_run: bool,
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
            Ok(FakeTerminals(
                self.terminal_rx
                    .lock()
                    .unwrap()
                    .take()
                    .expect("one subscription"),
            ))
        }

        async fn active_flow_runs(&self) -> Result<Vec<RunId>, MobError> {
            Ok(self.runs.clone())
        }

        async fn cancel_flow(&self, run_id: RunId) -> Result<(), MobError> {
            self.cancels.fetch_add(1, Ordering::SeqCst);
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
}
